//! Running the app's own Extract command with a rendered buffer.
//!
//! The app's Extract reads the selected sample, writes a WAV and assigns a pad.
//! A mod that rendered its own audio arms a substitute buffer: the writer hook
//! swaps each block's pointer for the rendered one, and the command's range is
//! patched for the duration of the call.  The app still owns the pad, the WAV
//! and the command, so this is the supported way to create a pad.
//!
//! The SDK owns the two command hooks (constructor and execute), the writer, and
//! the command lifetime; a mod only describes the request.

use core::ffi::CStr;
use core::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};

use crate::image;
use crate::platform::pads;
use crate::platform::writer;

/// Where the command's settings object keeps the range start.
const RANGE_START: usize = 0x78;
/// Where it keeps the range end.
const RANGE_END: usize = 0xd0;
/// The app's Extract handler.
const EXTRACT_HANDLER: usize = crate::firmware::EXTRACT_HANDLER.offset;
/// `juce::String(const char *)`.
const STRING_FROM_C: usize = crate::firmware::STRING_FROM_C.offset;

/// One extraction.
pub struct Request<'a> {
    /// The Sample page.
    pub screen: usize,
    /// The page state the selection came from.
    pub state: usize,
    /// The name the new pad gets, e.g. `c"HPSS Harmonic"`.
    pub name: &'static CStr,
    /// The rendered audio, interleaved; `None` copies the app's own range.
    pub buffer: Option<&'a [f32]>,
    /// The source sample's length in frames.
    pub source_frames: u32,
    /// The rendered audio's channel count. It is converted to the source sample's channels.
    pub channels: u32,
    /// First source frame.
    pub start: u32,
    /// One past the last source frame.
    pub end: u32,
    /// Write to this pad instead of the app's next free pad.
    ///
    /// The native command is patched at `+0x58` before execute, so an empty or
    /// populated destination is overwritten without a second assignment command.
    pub destination: Option<i32>,
}

/// What an Extract copies: a sample on the Sample page, or audio the mod made.
pub enum Source<'a> {
    /// The page's selected source, with the range to extract.
    Selection(Request<'a>),
    /// A generated buffer, written to a destination pad.
    Generated(Generated<'a>),
}

/// Generated audio to write to an explicit pad. Existing pad contents are overwritten.
pub struct Generated<'a> {
    /// Live Sample page on its UI thread.
    pub screen: usize,
    /// Native name for the generated sample.
    pub name: &'static CStr,
    /// Interleaved floating-point audio.
    pub audio: &'a [f32],
    /// Number of interleaved channels (one or two); converted to the seed's channels.
    pub channels: u32,
    /// Destination pad index, from 0 through 127.
    pub destination: i32,
}

/// The pad the app assigned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Extracted {
    /// The pad the app assigned.
    pub pad: i32,
}

/// Why an extraction did not produce a pad.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExtractError {
    /// The request's range or channels are unusable.
    Invalid,
    /// The page has no program or the state is stale.
    NoProgram,
    /// The native Extract handler needs at least one free pad to create a command.
    NoFreePad,
    /// The app did not assign a pad, or its writer did not consume the buffer.
    NotApplied,
    /// The temporary source was used but its original layer could not be restored.
    RestoreFailed,
}

impl core::fmt::Display for ExtractError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Invalid => f.write_str("invalid extract range"),
            Self::NoProgram => f.write_str("no program on this page"),
            Self::NoFreePad => f.write_str("no free pad for the native Extract command"),
            Self::NotApplied => f.write_str("the app did not assign a pad"),
            Self::RestoreFailed => f.write_str("the original sample layer could not be restored"),
        }
    }
}

impl std::error::Error for ExtractError {}

/// One armed extraction, as the hooks see it.
#[derive(Clone, Debug, Default)]
struct Arm {
    buffer: Option<Arc<[f32]>>,
    channels: u32,
    start: u32,
    end: u32,
    name: usize,
    destination: i32,
}

struct Operation {
    arm: Arm,
    source: usize,
    commands: Mutex<Vec<usize>>,
    executing: AtomicBool,
    finished: AtomicBool,
    wrote: AtomicBool,
    /// Guards the `finished` transition so a waiter cannot miss its wake-up.
    done: Mutex<()>,
    settled: Condvar,
}

impl Operation {
    /// Mark the native command as returned and wake anyone waiting for it.
    fn complete(&self) {
        let _guard = lock(&self.done);
        self.finished.store(true, Ordering::Release);
        self.settled.notify_all();
    }
}
static OPERATION: Mutex<Option<Arc<Operation>>> = Mutex::new(None);
/// Whether an extraction is in flight: set from the moment a request is accepted
/// until it returns, on any thread.
static IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// Holds [`IN_FLIGHT`] for one request.
struct Flight;

impl Flight {
    fn enter() -> Result<Self, ExtractError> {
        IN_FLIGHT
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| Self)
            .map_err(|_| ExtractError::NotApplied)
    }
}

impl Drop for Flight {
    fn drop(&mut self) {
        IN_FLIGHT.store(false, Ordering::Release);
    }
}

/// Whether an extraction is running on any thread.
#[must_use]
pub fn in_flight() -> bool {
    IN_FLIGHT.load(Ordering::Acquire)
}
static EXECUTION: Mutex<()> = Mutex::new(());

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The app's own command constructor.
pub(crate) type Constructor =
    unsafe extern "C" fn(usize, usize, i32, usize, usize, usize, usize, usize, usize);
/// The app's own command method.
pub(crate) type Command = unsafe extern "C" fn(usize) -> i32;

/// The app's own command constructor.
static mut ORIGINAL_CTOR: Option<Constructor> = None;
/// The app's own command method.
static mut ORIGINAL_EXEC: Option<Command> = None;
/// The app's first-free-pad finder, temporarily redirected for a seeded Extract.
static mut ORIGINAL_FREE_PAD: Option<unsafe extern "C" fn(usize) -> u64> = None;
// Native names are process-lived because the firmware may retain them. Key by
// content rather than address, and do not impose an arbitrary lifetime limit.
static NAMES: Mutex<Vec<(&'static CStr, Box<u64>)>> = Mutex::new(Vec::new());

/// Record the displaced command hooks, at install time.
///
/// # Safety
///
/// Called once, before the hooks can run.
pub(crate) unsafe fn record(constructor: Option<Constructor>, execute: Option<Command>) {
    // SAFETY: the caller's contract.
    unsafe {
        ORIGINAL_CTOR = constructor;
        ORIGINAL_EXEC = execute;
    }
}

/// Record the displaced first-free-pad finder, at install time.
///
/// # Safety
///
/// Called once, before the hook can run.
pub(crate) unsafe fn record_free_pad(free_pad: Option<unsafe extern "C" fn(usize) -> u64>) {
    // SAFETY: the caller's contract.
    unsafe { ORIGINAL_FREE_PAD = free_pad };
}

/// The native first-free-pad finder. A seeded Generated Extract temporarily
/// occupies its requested output pad with the silent source; route the native
/// handler to that same pad so it does not leave a second source copy behind.
///
/// # Safety
///
/// The app calls this with its own live program.
pub(crate) unsafe extern "C" fn free_pad_entry(program: usize) -> u64 {
    if let Some(destination) = super::seed::first_free_override(program)
        && let Ok(index) = u32::try_from(destination)
    {
        #[cfg(feature = "fw-1-4")]
        return u64::from(index) | (1_u64 << 32);
        #[cfg(feature = "fw-1-3")]
        return u64::from(index);
    }
    // SAFETY: recorded before the firmware hook was committed.
    match unsafe { *core::ptr::addr_of!(ORIGINAL_FREE_PAD) } {
        // SAFETY: the app supplied its live program object.
        Some(find) => unsafe { find(program) },
        None => 0,
    }
}

/// The name object for `text`, building it once.
unsafe fn name_object(text: &'static CStr) -> usize {
    let mut names = lock(&NAMES);
    if let Some((_, object)) = names.iter().find(|(name, _)| *name == text) {
        return std::ptr::from_ref(object.as_ref()) as usize;
    }
    let mut object = Box::new(0_u64);
    // SAFETY: run's caller guarantees a validated firmware and UI-thread access.
    unsafe {
        let construct: unsafe extern "C" fn(usize, *const u8) =
            core::mem::transmute(image::base() + STRING_FROM_C);
        construct(
            std::ptr::from_mut(object.as_mut()) as usize,
            text.as_ptr().cast(),
        );
    }
    let pointer = std::ptr::from_ref(object.as_ref()) as usize;
    names.push((text, object));
    pointer
}

fn snapshot() -> Option<Arc<Operation>> {
    lock(&OPERATION).clone()
}

/// The slice command's constructor: rename it and remember the command.
///
/// # Safety
///
/// The app calls this with its own arguments.
pub(crate) unsafe extern "C" fn ctor_entry(
    command: usize,
    name: usize,
    destination: usize,
    settings: usize,
    pad_state: usize,
    project: usize,
    extra1: usize,
    extra2: usize,
    extra3: usize,
) {
    let operation = snapshot();
    let mut chosen = name;
    if let Some(operation) = operation
        && pad_state == operation.source
    {
        chosen = operation.arm.name;
        lock(&operation.commands).push(command);
    }
    // SAFETY: the static is the app's own constructor.
    if let Some(forward) = unsafe { *core::ptr::addr_of!(ORIGINAL_CTOR) } {
        // SAFETY: the app's own call.
        unsafe {
            forward(
                command,
                chosen,
                i32::try_from(destination).unwrap_or(0),
                settings,
                pad_state,
                project,
                extra1,
                extra2,
                extra3,
            );
        }
    }
}

/// The slice command's execute: run it with the armed buffer, if any.
///
/// # Safety
///
/// The app calls this with its own command.
#[must_use]
pub(crate) unsafe extern "C" fn exec_entry(command: usize) -> i32 {
    let operation = snapshot().filter(|operation| lock(&operation.commands).contains(&command));
    // SAFETY: recorded before the hooks are installed.
    let execute = unsafe { *core::ptr::addr_of!(ORIGINAL_EXEC) };
    let Some(operation) = operation else {
        return forward_execute(command, execute);
    };
    if operation.executing.swap(true, Ordering::AcqRel) {
        // The first invocation owns completion; a nested invocation must not
        // release resources while that invocation is still live.
        return forward_execute(command, execute);
    }
    let Ok(_guard) = EXECUTION.try_lock() else {
        // A different operation owns the writer. Forward this command unchanged,
        // and report completion only after its native execution returns.
        let status = forward_execute(command, execute);
        operation.complete();
        return status;
    };
    // SAFETY: command and method belong to this native hook invocation.
    let status = unsafe { run_command(command, &operation, execute) };
    operation.complete();
    status
}

/// Forward one command to the app's own execute method unchanged.
fn forward_execute(command: usize, execute: Option<unsafe extern "C" fn(usize) -> i32>) -> i32 {
    match execute {
        // SAFETY: the caller supplies the app's own command and method.
        Some(execute) => unsafe { execute(command) },
        None => -1,
    }
}

/// Run one armed command, patching its range for the duration of the call.
///
/// # Safety
///
/// `command` must be the app's command and `execute` its displaced method.
unsafe fn run_command(
    command: usize,
    operation: &Operation,
    execute: Option<unsafe extern "C" fn(usize) -> i32>,
) -> i32 {
    let arm = &operation.arm;
    // Arm one operation-scoped plan.  Re-arming replaces the slot atomically, so
    // a stale plan cannot leak into this command.
    let arm_id = if let Some(audio) = &arm.buffer {
        writer::arm_owned(
            Arc::clone(audio),
            (arm.end - arm.start) as usize,
            arm.channels as usize,
        )
    } else {
        writer::clear();
        None
    };
    // The constructor stores the settings object by pointer; the execute method
    // is what copies its range into the slice command.
    let Some(settings) = read_u64(command + 0x60)
        .and_then(|value| usize::try_from(value).ok())
        .filter(|value| *value != 0)
    else {
        if let Some(id) = arm_id {
            writer::clear_if(id);
        }
        return -1;
    };
    let mut patches = vec![
        (settings + RANGE_START, u64::from(arm.start)),
        (settings + RANGE_END, u64::from(arm.end)),
    ];
    if arm.destination >= 0 {
        patches.push((command + 0x58, u64::try_from(arm.destination).unwrap_or(0)));
    }
    let Some(originals) = patches
        .iter()
        .map(|(address, _)| read_u64(*address))
        .collect::<Option<Vec<_>>>()
    else {
        if let Some(id) = arm_id {
            writer::clear_if(id);
        }
        return -1;
    };
    for (index, &(address, value)) in patches.iter().enumerate() {
        if !write_u64(address, value) {
            for ((address, _), original) in patches[..=index].iter().zip(&originals) {
                let _ = write_u64(*address, *original);
            }
            if let Some(id) = arm_id {
                writer::clear_if(id);
            }
            return -1;
        }
    }
    let status = match execute {
        // SAFETY: the app's own method.
        Some(execute) => unsafe { execute(command) },
        None => -1,
    };
    let restored = patches
        .iter()
        .zip(originals)
        .fold(true, |ok, ((address, _), original)| {
            write_u64(*address, original) && ok
        });
    if !restored {
        if let Some(id) = arm_id {
            writer::clear_if(id);
        }
        return -1;
    }
    // The native writer consumes each descriptor synchronously on this thread.
    // Once execute returns, sleeping here cannot advance its substitution plan.
    let wrote = match arm_id {
        None => arm.buffer.is_none(),
        Some(id) => writer::finish(id),
    };
    operation.wrote.store(wrote, Ordering::Release);
    status
}

/// Read eight bytes.
fn read_u64(address: usize) -> Option<u64> {
    let mut buffer = [0_u8; 8];
    image::read(address, &mut buffer).then(|| u64::from_le_bytes(buffer))
}

/// Write eight bytes.
fn write_u64(address: usize, value: u64) -> bool {
    // SAFETY: callers only patch fields of their live native command.
    unsafe { image::write(address, &value.to_le_bytes()) }
}

/// Match the rendered audio to the channel count the native writer expects.
fn writer_audio(audio: &[f32], input_channels: u32, output_channels: u16) -> Arc<[f32]> {
    match (input_channels, output_channels) {
        (1, 2) => audio.iter().flat_map(|sample| [*sample, *sample]).collect(),
        (2, 1) => audio
            .chunks_exact(2)
            .map(|pair| (pair[0] + pair[1]) * 0.5)
            .collect(),
        _ => Arc::from(audio),
    }
}

/// Run one native Extract.
///
/// # Safety
/// Call on the UI thread with live matching Sample-page and state objects in
/// the validated firmware. No other code may mutate their sample/program until
/// the native operation completes. Native writer callbacks must run synchronously
/// on the command executor thread and consume each block before returning. For
/// generated audio, the most recently observed sample service must also remain
/// live on that thread.
///
/// With a buffer the writer substitutes it; without one the app copies its own
/// range. The tested firmware executes the native command synchronously. The
/// handler itself can block; this API cannot interrupt a native call.
///
/// # Errors
///
/// Returns [`ExtractError`] when the request is unusable, the page has no
/// program, or the app did not assign a pad.
pub unsafe fn run(source: Source<'_>) -> Result<Extracted, ExtractError> {
    match source {
        Source::Selection(request) => {
            let _flight = Flight::enter()?;
            // SAFETY: forwarded from this function's caller.
            unsafe { run_inner(&request, false) }
        }
        // SAFETY: forwarded from this function's caller.
        Source::Generated(generated) => unsafe { run_generated(&generated) },
    }
}

/// Keep borrowed native state alive until a constructed command returns.
fn wait_for_completion(operation: &Operation) {
    if lock(&operation.commands).is_empty() {
        return;
    }
    let mut guard = lock(&operation.done);
    while !operation.finished.load(Ordering::Acquire) {
        guard = operation
            .settled
            .wait(guard)
            .unwrap_or_else(PoisonError::into_inner);
    }
}

unsafe fn run_inner(request: &Request<'_>, borrowed: bool) -> Result<Extracted, ExtractError> {
    if request.screen == 0
        || request.start >= request.end
        || request.end > request.source_frames
        || !(1..=2).contains(&request.channels)
        || request.buffer.is_some_and(|audio| {
            audio.is_empty() || !audio.len().is_multiple_of(request.channels as usize)
        })
        || request
            .destination
            .is_some_and(|pad| !(0..128).contains(&pad))
    {
        return Err(ExtractError::Invalid);
    }
    if lock(&OPERATION)
        .as_ref()
        .is_some_and(|operation| !operation.finished.load(Ordering::Acquire))
    {
        return Err(ExtractError::NotApplied);
    }
    let Some(program) = super::descent::State::at(request.state).program() else {
        return Err(ExtractError::NoProgram);
    };
    let native_channels = if request.buffer.is_some() {
        super::descent::State::at(request.state)
            .layers()
            .and_then(super::descent::Layers::sample)
            .and_then(super::descent::geometry)
            .map(|geometry| u32::from(geometry.channels))
            .ok_or(ExtractError::NotApplied)?
    } else {
        request.channels
    };
    // The stock handler calls its free-pad finder before constructing Extract,
    // even when the SDK will redirect the command to an explicit destination.
    // SAFETY: the request contract supplies the live program.
    let first_free = unsafe { pads::first_free_pad(program) }.ok_or(ExtractError::NoFreePad)?;
    let surface = crate::platform::control::surface();
    let before_target = request
        .destination
        .and_then(|pad| pads::sample_for_pad(surface, pad));
    let before_slot = request
        .destination
        .and_then(|pad| pads::pad_slot(program, pad));
    let before_sample = request
        .destination
        .and_then(|pad| pads::program_sample(program, pad));
    // SAFETY: native string construction follows run's firmware/thread contract.
    let name = unsafe { name_object(request.name) };
    let operation = Arc::new(Operation {
        arm: Arm {
            buffer: request
                .buffer
                .map(|audio| writer_audio(audio, request.channels, native_channels as u16)),
            channels: native_channels,
            start: request.start,
            end: request.end,
            name,
            destination: request.destination.unwrap_or(-1),
        },
        source: request.state,
        commands: Mutex::new(Vec::new()),
        executing: AtomicBool::new(false),
        finished: AtomicBool::new(false),
        wrote: AtomicBool::new(false),
        done: Mutex::new(()),
        settled: Condvar::new(),
    });
    *lock(&OPERATION) = Some(Arc::clone(&operation));
    // SAFETY: the caller guarantees the live native page and validated firmware.
    unsafe {
        let handler: unsafe extern "C" fn(usize) =
            core::mem::transmute(image::base() + EXTRACT_HANDLER);
        handler(request.screen);
    }
    // The tested firmware constructs and executes the command inside the
    // handler. Retain the seed if a constructed command is still pending;
    // older device observations did not establish the same execution order.
    if borrowed {
        wait_for_completion(&operation);
    }
    let assigned = match request.destination {
        // The surface is absent on a host without control-surface events, so
        // also compare the destination slot in the program's own pad table.
        Some(pad) => {
            pads::sample_for_pad(surface, pad) != before_target
                || pads::pad_slot(program, pad) != before_slot
                || pads::program_sample(program, pad)
                    .is_some_and(|sample| Some(sample) != before_sample)
        }
        // SAFETY: only a request without a destination needs the live program.
        None => {
            // SAFETY: the caller still holds the live program on the UI thread.
            (unsafe { pads::first_free_pad(program) }) != Some(first_free)
        }
    };
    let finished = operation.finished.load(Ordering::Acquire);
    // A constructed command that is still running retains its audio and blocks
    // another operation until it reports completion.
    if finished || lock(&operation.commands).is_empty() {
        *lock(&OPERATION) = None;
    }
    let wrote = operation.wrote.load(Ordering::Acquire);
    if assigned && wrote && finished {
        Ok(Extracted {
            pad: request.destination.unwrap_or(first_free),
        })
    } else {
        Err(ExtractError::NotApplied)
    }
}

/// Write generated audio using a native seed and restore the source afterward.
///
/// # Safety
/// `screen` and the recorded control surface must be live on the owning UI
/// thread throughout the call, with exclusive access to the page and program.
/// The native writer must satisfy [`run`]'s synchronous callback contract.
///
/// # Errors
/// Returns an error for invalid audio, an unavailable page or seed, or a failed
/// extraction. This can block indefinitely if the native call hangs; see
/// `run_seeded`. The destination is overwritten on success.
unsafe fn run_generated(request: &Generated<'_>) -> Result<Extracted, ExtractError> {
    let channels = usize::try_from(request.channels).map_err(|_| ExtractError::Invalid)?;
    if !(1..=2).contains(&channels)
        || request.audio.is_empty()
        || !request.audio.len().is_multiple_of(channels)
        || !(0..128).contains(&request.destination)
    {
        return Err(ExtractError::Invalid);
    }
    let frames =
        u32::try_from(request.audio.len() / channels).map_err(|_| ExtractError::Invalid)?;
    let state = super::selection::page_state(request.screen).ok_or(ExtractError::NoProgram)?;
    let seeded = Request {
        screen: request.screen,
        state,
        name: request.name,
        buffer: Some(request.audio),
        source_frames: frames,
        channels: request.channels,
        start: 0,
        end: frames,
        destination: Some(request.destination),
    };
    // SAFETY: the caller supplies the live objects and writer contract.
    unsafe { run_seeded(&seeded) }
}

/// Extract generated audio using a temporary native seed, restoring the original
/// layer and removing any extra seed copies before returning.
///
/// # Safety
/// The page, state and control surface must remain live and exclusive on the UI
/// thread, and the native writer must satisfy [`run`]'s callback requirements.
/// If a constructed command is still pending after the handler returns, this
/// call waits for it before restoring the seed. A stuck native handler or pending
/// command can therefore block this UI-thread call indefinitely.
///
/// # Errors
/// Returns an error if the seed cannot be borrowed or the extraction fails.
unsafe fn run_seeded(request: &Request<'_>) -> Result<Extracted, ExtractError> {
    let destination = request.destination.ok_or(ExtractError::Invalid)?;
    let _flight = Flight::enter()?;
    if lock(&OPERATION)
        .as_ref()
        .is_some_and(|operation| !operation.finished.load(Ordering::Acquire))
    {
        return Err(ExtractError::NotApplied);
    }
    let Some(program) = super::descent::State::at(request.state).program() else {
        return Err(ExtractError::NoProgram);
    };
    // Check before borrowing a seed, since the native handler requires a free
    // pad even when the final destination is explicit.
    // SAFETY: the caller guarantees the live program on its UI thread.
    if unsafe { pads::first_free_pad(program) }.is_none() {
        return Err(ExtractError::NoFreePad);
    }
    // SAFETY: the caller supplies the live page and exclusive UI access.
    let (state, frames) = unsafe { super::seed::seed_provider(request.screen, destination) }
        .ok_or(ExtractError::NotApplied)?;
    let result = if state == request.state {
        let seeded = Request {
            screen: request.screen,
            state,
            name: request.name,
            buffer: request.buffer,
            source_frames: frames,
            channels: request.channels,
            start: request.start,
            end: request.end,
            destination: request.destination,
        };
        // SAFETY: the same live page and writer contract apply to the seed.
        unsafe { run_inner(&seeded, true) }
    } else {
        Err(ExtractError::NoProgram)
    };
    // SAFETY: cleanup runs on the same UI thread before the page can change.
    if !unsafe { super::seed::finish(destination) } {
        return Err(ExtractError::RestoreFailed);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{
        Arm, ExtractError, Generated, Operation, RANGE_END, RANGE_START, Request, Source, run,
        wait_for_completion, writer_audio,
    };
    use std::sync::{Arc, Mutex, atomic::AtomicBool, mpsc};

    #[test]
    fn rendered_audio_matches_the_native_sample_channels() {
        assert_eq!(
            &*writer_audio(&[0.25, -0.5], 1, 2),
            &[0.25, 0.25, -0.5, -0.5]
        );
        assert_eq!(&*writer_audio(&[1.0, 0.0, -0.5, 0.5], 2, 1), &[0.5, 0.0]);
        assert_eq!(&*writer_audio(&[0.25, -0.5], 2, 2), &[0.25, -0.5]);
    }

    #[test]
    fn generated_audio_rejects_invalid_geometry_before_touching_the_firmware() {
        let audio = [0.0; 3];
        let request = Generated {
            screen: 0,
            name: c"test",
            audio: &audio,
            channels: 2,
            destination: 0,
        };
        // SAFETY: invalid geometry is rejected without accessing firmware.
        assert_eq!(
            // SAFETY: invalid geometry is rejected without accessing firmware.
            unsafe { run(Source::Generated(request)) },
            Err(ExtractError::Invalid)
        );
    }

    #[test]
    fn borrowed_source_stays_live_until_queued_command_finishes() {
        let operation = Arc::new(Operation {
            arm: Arm::default(),
            source: 0,
            commands: Mutex::new(vec![1]),
            executing: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            wrote: AtomicBool::new(false),
            done: Mutex::new(()),
            settled: std::sync::Condvar::new(),
        });
        let (started_tx, started_rx) = mpsc::channel();
        let (returned_tx, returned_rx) = mpsc::channel();
        let worker_op = Arc::clone(&operation);
        let worker = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            wait_for_completion(&worker_op);
            returned_tx.send(()).unwrap();
        });
        started_rx.recv().unwrap();
        assert!(
            returned_rx
                .recv_timeout(std::time::Duration::from_millis(25))
                .is_err()
        );
        operation.complete();
        returned_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn the_range_slots_are_the_apps() {
        assert_eq!(RANGE_START, 0x78);
        assert_eq!(RANGE_END, 0xd0);
    }

    fn request(start: u32, end: u32, channels: u32, buffer: Option<&[f32]>) -> Request<'_> {
        Request {
            screen: 0x1000,
            state: 0x2000,
            name: c"test",
            buffer,
            source_frames: 1_000,
            channels,
            start,
            end,
            destination: None,
        }
    }

    #[test]
    fn every_unusable_request_is_rejected_before_native_access() {
        let audio = [0.0_f32; 8];
        for request in [
            request(10, 10, 1, None),
            request(10, 5, 1, None),
            request(0, 1_001, 1, None),
            request(0, 10, 0, None),
            request(0, 10, 3, None),
            request(0, 10, 1, Some(&[])),
            request(0, 10, 2, Some(&[0.0; 3])),
        ] {
            // SAFETY: every case is rejected before any native access.
            let error = unsafe { run(Source::Selection(request)) }.unwrap_err();
            assert_eq!(error, ExtractError::Invalid);
        }
        let request = Request {
            destination: Some(128),
            ..request(0, 10, 1, Some(&audio))
        };
        // SAFETY: the out-of-range destination is rejected before native access.
        let error = unsafe { run(Source::Selection(request)) }.unwrap_err();
        assert_eq!(error, ExtractError::Invalid);
    }

    #[test]
    fn an_empty_range_is_refused_before_touching_the_app() {
        let request = Request {
            screen: 0x1000,
            state: 0x2000,
            name: c"test",
            buffer: None,
            source_frames: 0,
            channels: 1,
            start: 10,
            end: 10,
            destination: None,
        };
        // SAFETY: invalid geometry is rejected before any native access.
        let error = unsafe { run(Source::Selection(request)) }.unwrap_err();
        assert_eq!(error, ExtractError::Invalid);
    }
}
