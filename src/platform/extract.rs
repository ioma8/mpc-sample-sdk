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
use std::sync::{Arc, Mutex, PoisonError};

use crate::image;
use crate::platform::pads;
use crate::platform::writer;

/// Where the command's settings object keeps the range start.
const RANGE_START: usize = 0x78;
/// Where it keeps the range end.
const RANGE_END: usize = 0xd0;
/// The app's Extract handler.
const EXTRACT_HANDLER: usize = 0x0165_80d0;
/// `[state + 0x2310]` is the app's program object.
const STATE_PROGRAM: usize = 0x2310;
/// `juce::String(const char *)`.
const STRING_FROM_C: usize = 0x00f5_35c0;

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
    /// The rendered audio's channel count.
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

/// Generated audio to write to an explicit pad. Existing pad contents are overwritten.
pub struct Generated<'a> {
    /// Live Sample page on its UI thread.
    pub screen: usize,
    /// Native name for the generated sample.
    pub name: &'static CStr,
    /// Interleaved floating-point audio.
    pub audio: &'a [f32],
    /// Number of interleaved channels (one or two).
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
    source_frames: usize,
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
}
static OPERATION: Mutex<Option<Arc<Operation>>> = Mutex::new(None);
static RUN: Mutex<()> = Mutex::new(());
static EXECUTION: Mutex<()> = Mutex::new(());

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The app's own command constructor.
static mut ORIGINAL_CTOR: Option<
    unsafe extern "C" fn(usize, usize, i32, usize, usize, usize, usize, usize, usize),
> = None;
/// The app's own command method.
static mut ORIGINAL_EXEC: Option<unsafe extern "C" fn(usize) -> i32> = None;
// Native names are process-lived because the firmware may retain them. Key by
// content rather than address, and do not impose an arbitrary lifetime limit.
static NAMES: Mutex<Vec<(&'static CStr, Box<u64>)>> = Mutex::new(Vec::new());

/// Record the displaced command hooks, at install time.
///
/// # Safety
///
/// Called once, before the hooks can run.
pub unsafe fn record(constructor: usize, execute: usize) {
    // SAFETY: the caller's contract.
    unsafe {
        ORIGINAL_CTOR = if constructor == 0 {
            None
        } else {
            Some(core::mem::transmute_copy::<usize, _>(&constructor))
        };
        ORIGINAL_EXEC = if execute == 0 {
            None
        } else {
            Some(core::mem::transmute_copy::<usize, _>(&execute))
        };
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
pub unsafe extern "C" fn ctor_entry(
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
pub unsafe extern "C" fn exec_entry(command: usize) -> i32 {
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
        operation.finished.store(true, Ordering::Release);
        return status;
    };
    // SAFETY: command and method belong to this native hook invocation.
    let status = unsafe { run_command(command, &operation, execute) };
    operation.finished.store(true, Ordering::Release);
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
        writer::arm_owned(Arc::clone(audio), arm.source_frames, arm.channels as usize)
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

/// Run one native Extract.
///
/// # Safety
/// Call on the UI thread with live matching Sample-page and state objects in
/// the validated firmware. No other code may mutate their sample/program until
/// the native operation completes. Native writer callbacks must run synchronously
/// on the command executor thread and consume each block before returning.
///
/// With a buffer the writer substitutes it; without one the app copies its own
/// range. The tested firmware executes the native command synchronously. The
/// handler itself can block; this API cannot interrupt a native call.
///
/// # Errors
///
/// Returns [`ExtractError`] when the request is unusable, the page has no
/// program, or the app did not assign a pad.
pub unsafe fn run(request: &Request<'_>) -> Result<Extracted, ExtractError> {
    let _run = RUN.try_lock().map_err(|_| ExtractError::NotApplied)?;
    // SAFETY: forwarded from this function's caller.
    unsafe { run_inner(request, false) }
}

/// Keep borrowed native state alive until a constructed command returns.
fn wait_for_completion(operation: &Operation) {
    if !lock(&operation.commands).is_empty() {
        while !operation.finished.load(Ordering::Acquire) {
            std::thread::sleep(core::time::Duration::from_millis(1));
        }
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
    let Some(program) = image::read_u64(
        request
            .state
            .checked_add(STATE_PROGRAM)
            .ok_or(ExtractError::Invalid)?,
    ) else {
        return Err(ExtractError::NoProgram);
    };
    let program = usize::try_from(program).unwrap_or(0);
    if program == 0 {
        return Err(ExtractError::NoProgram);
    }
    // SAFETY: the request contract supplies the live program.
    let before = unsafe { pads::free_count(program) };
    if before < 0 {
        return Err(ExtractError::NoProgram);
    }
    let before_target = request
        .destination
        .and_then(|pad| pads::sample_for_pad(program, pad));
    // SAFETY: native string construction follows run's firmware/thread contract.
    let name = unsafe { name_object(request.name) };
    let operation = Arc::new(Operation {
        arm: Arm {
            buffer: request.buffer.map(Arc::from),
            source_frames: request.source_frames as usize,
            channels: request.channels,
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
        Some(pad) => pads::sample_for_pad(program, pad) != before_target,
        // SAFETY: the caller still holds the live program on the UI thread.
        None => (unsafe { pads::free_count(program) }) != before,
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
            pad: request.destination.unwrap_or(before),
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
/// [`run_seeded`]. The destination is overwritten on success.
pub unsafe fn generated(request: &Generated<'_>) -> Result<Extracted, ExtractError> {
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
pub unsafe fn run_seeded(request: &Request<'_>) -> Result<Extracted, ExtractError> {
    let destination = request.destination.ok_or(ExtractError::Invalid)?;
    let _run = RUN.try_lock().map_err(|_| ExtractError::NotApplied)?;
    if lock(&OPERATION)
        .as_ref()
        .is_some_and(|operation| !operation.finished.load(Ordering::Acquire))
    {
        return Err(ExtractError::NotApplied);
    }
    // SAFETY: the caller supplies the live page and exclusive UI access.
    let (state, frames) =
        unsafe { super::seed::seed_provider(request.screen) }.ok_or(ExtractError::NotApplied)?;
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
        Arm, ExtractError, Generated, Operation, RANGE_END, RANGE_START, Request, generated,
        wait_for_completion,
    };
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    };

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
        assert_eq!(unsafe { generated(&request) }, Err(ExtractError::Invalid));
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
        operation.finished.store(true, Ordering::Release);
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
            let error = unsafe { super::run(&request) }.unwrap_err();
            assert_eq!(error, ExtractError::Invalid);
        }
        let request = Request {
            destination: Some(128),
            ..request(0, 10, 1, Some(&audio))
        };
        // SAFETY: the out-of-range destination is rejected before native access.
        let error = unsafe { super::run(&request) }.unwrap_err();
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
        let error = unsafe { super::run(&request) }.unwrap_err();
        assert_eq!(error, ExtractError::Invalid);
    }
}
