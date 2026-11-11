//! Owned audio substitution for one native Extract writer.
//!
//! A plan is bound to the operation that armed it, the thread that runs that
//! operation, and the first writer closure on that thread.  Other operations
//! cannot read, advance or clear it, and callbacks that do not belong to it are
//! forwarded unchanged.  Each callback retains an owned reference to the audio
//! until its native write has returned, so clearing a plan can never free a
//! buffer a callback is still reading.

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

/// Identifies one armed substitution.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ArmId(u64);

#[derive(Debug)]
struct Plan {
    id: ArmId,
    audio: Arc<[f32]>,
    source_frames: usize,
    channels: usize,
    position: usize,
    thread: std::thread::ThreadId,
    closure: Option<usize>,
}

static PLAN: Mutex<Option<Plan>> = Mutex::new(None);
static NEXT_ARM: AtomicU64 = AtomicU64::new(1);
static BUSY: AtomicU32 = AtomicU32::new(0);

fn lock() -> std::sync::MutexGuard<'static, Option<Plan>> {
    PLAN.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Copy a raw interleaved buffer and arm substitution on the current thread.
///
/// # Safety
/// `buffer` must reference `frames * channels` initialized, readable `f32`
/// values for this call. Call only immediately around the intended native
/// writer command, with no nested unrelated writes before its first callback.
/// Prefer [`super::extract::run`], which owns and scopes the operation.
pub unsafe fn arm(buffer: usize, frames: usize, source_frames: usize, channels: usize) {
    let Some(cells) = frames.checked_mul(channels) else {
        return;
    };
    if buffer == 0 || cells == 0 || cells > isize::MAX as usize / size_of::<f32>() {
        return;
    }
    // SAFETY: the caller supplies initialized audio of the validated length.
    let audio: Arc<[f32]> =
        unsafe { std::slice::from_raw_parts(buffer as *const f32, cells) }.into();
    let _ = arm_owned(audio, source_frames, channels);
}

/// Arm an owned buffer for the operation running on this thread.
///
/// Returns the operation's identity, or `None` when the geometry is unusable.
pub(crate) fn arm_owned(audio: Arc<[f32]>, source_frames: usize, channels: usize) -> Option<ArmId> {
    let mut slot = lock();
    *slot = None;
    if channels == 0
        || source_frames == 0
        || audio.is_empty()
        || !audio.len().is_multiple_of(channels)
    {
        return None;
    }
    let id = ArmId(NEXT_ARM.fetch_add(1, Ordering::AcqRel));
    *slot = Some(Plan {
        id,
        audio,
        source_frames,
        channels,
        position: 0,
        thread: std::thread::current().id(),
        closure: None,
    });
    Some(id)
}

/// Stop substituting unconditionally. In-flight callbacks retain their audio.
pub fn clear() {
    *lock() = None;
}

/// Stop substituting only when the armed plan belongs to `id`.
pub(crate) fn clear_if(id: ArmId) {
    let mut slot = lock();
    if slot.as_ref().is_some_and(|plan| plan.id == id) {
        *slot = None;
    }
}

/// The app's writer signature: closure plus the block descriptor.
pub type Writer = unsafe extern "C" fn(usize, usize);

/// Substitute the armed audio only for the owning thread and writer closure.
///
/// The first callback after arming binds the closure; later callbacks through a
/// different closure are forwarded unchanged, so an unrelated save cannot
/// consume this operation's audio.
///
/// # Safety
/// `block` must be a writable native descriptor (`u32` frames at `+0`, pointer
/// at `+8`). `writer` must be its native writer and must consume the samples
/// synchronously without retaining the descriptor or its audio pointer.
#[allow(
    clippy::cast_ptr_alignment,
    reason = "the native descriptor's pointer slot is read and written unaligned"
)]
pub unsafe fn write(closure: usize, block: *mut u8, writer: Option<Writer>) {
    let Some(writer) = writer else { return };
    if block.is_null() {
        // SAFETY: the caller supplies the original native call's arguments.
        unsafe { writer(closure, block as usize) };
        return;
    }
    // SAFETY: descriptor layout and readability are the caller's contract.
    let frames = unsafe { core::ptr::read_unaligned(block.cast::<u32>()) };
    let substitution = {
        let mut slot = lock();
        // One callback at a time: a nested writer callback must be forwarded
        // rather than advancing the same position twice.
        if BUSY.load(Ordering::Acquire) != 0 {
            None
        } else {
            slot.as_mut().and_then(|plan| {
                if plan.thread != std::thread::current().id()
                    || plan.closure.is_some_and(|bound| bound != closure)
                {
                    return None;
                }
                let start = plan.position;
                let end = start.checked_add(frames as usize)?;
                if frames == 0 || end > plan.source_frames {
                    return None;
                }
                let total = plan.audio.len() / plan.channels;
                let finish = if end == plan.source_frames {
                    total
                } else {
                    end
                };
                if finish <= start || finish > total {
                    return None;
                }
                let count = u32::try_from(finish - start).ok()?;
                plan.closure = Some(closure);
                plan.position = finish;
                // Publish "in flight" while the plan lock is still held, so a
                // waiter can never observe the advanced position as idle before
                // the native write has even started.
                BUSY.fetch_add(1, Ordering::AcqRel);
                Some((Arc::clone(&plan.audio), start * plan.channels, count))
            })
        }
    };
    let Some((audio, start, count)) = substitution else {
        // SAFETY: forwarding the native call unchanged.
        unsafe { writer(closure, block as usize) };
        return;
    };
    // SAFETY: descriptor is writable, and `audio` remains alive through the call.
    unsafe {
        let pointer_slot = block.add(8).cast::<usize>();
        let original = core::ptr::read_unaligned(pointer_slot);
        core::ptr::write_unaligned(block.cast::<u32>(), count);
        core::ptr::write_unaligned(pointer_slot, audio.as_ptr().add(start) as usize);
        writer(closure, block as usize);
        core::ptr::write_unaligned(block.cast::<u32>(), frames);
        core::ptr::write_unaligned(pointer_slot, original);
    }
    BUSY.fetch_sub(1, Ordering::AcqRel);
}

/// Wait at most `timeout_ms` for callbacks; return whether all completed.
#[must_use]
pub fn wait_idle(timeout_ms: u32) -> bool {
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_millis(u64::from(timeout_ms));
    while BUSY.load(Ordering::Acquire) != 0 {
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    true
}

/// The C ABI hook's substitution implementation.
///
/// # Safety
/// The contract of [`write()`] applies.
pub unsafe fn substitute(closure: usize, block: *mut u8, writer: Option<Writer>) {
    // SAFETY: forwarded caller contract.
    unsafe { write(closure, block, writer) };
}

/// Frames reserved for the plan armed as `id`, if it is still current.
#[must_use]
pub(crate) fn position(id: ArmId) -> Option<usize> {
    lock()
        .as_ref()
        .filter(|plan| plan.id == id)
        .map(|plan| plan.position)
}

/// Total frames in the plan armed as `id`, if it is still current.
#[must_use]
pub(crate) fn total_frames(id: ArmId) -> Option<usize> {
    lock()
        .as_ref()
        .filter(|plan| plan.id == id)
        .map(|plan| plan.audio.len() / plan.channels)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use super::{ArmId, arm_owned, clear, clear_if, position, total_frames, wait_idle, write};

    /// The plan is process-wide, so the tests that arm it serialise.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[repr(C)]
    struct Block {
        frames: u32,
        padding: u32,
        pointer: usize,
    }

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn block(frames: u32, pointer: usize) -> Block {
        Block {
            frames,
            padding: 0,
            pointer,
        }
    }

    #[test]
    fn callbacks_keep_owned_audio_and_restore_the_descriptor() {
        static CONSUMED: AtomicBool = AtomicBool::new(false);
        unsafe extern "C" fn consume(_closure: usize, block: usize) {
            // SAFETY: this test supplies Block and the writer supplies four cells.
            let block = unsafe { &*(block as *const Block) };
            assert_eq!(block.frames, 4);
            // Clearing the published plan must not free the callback's audio.
            clear();
            // SAFETY: the callback retains the owned audio while native code runs.
            let observed = unsafe { std::slice::from_raw_parts(block.pointer as *const f32, 4) };
            assert_eq!(observed, &[1., 2., 3., 4.]);
            CONSUMED.store(true, Ordering::Release);
        }
        let _serial = serial();
        let id = arm_owned(std::sync::Arc::from([1., 2., 3., 4.]), 2, 1).expect("armed");
        assert_eq!(position(id), Some(0));
        assert_eq!(total_frames(id), Some(4));
        let mut block = block(2, 123);
        // SAFETY: test descriptor and callback satisfy the writer contract.
        unsafe { write(7, std::ptr::from_mut(&mut block).cast(), Some(consume)) };
        assert!(CONSUMED.load(Ordering::Acquire));
        assert_eq!((block.frames, block.pointer), (2, 123));
        assert!(wait_idle(0));
    }

    #[test]
    fn a_plan_only_serves_its_arming_thread() {
        static CALLED: AtomicBool = AtomicBool::new(false);
        unsafe extern "C" fn consume(_closure: usize, block: usize) {
            CALLED.store(true, Ordering::Release);
            // SAFETY: this test supplies the descriptor and the audio.
            let block = unsafe { &*(block as *const Block) };
            assert_eq!(block.pointer, 0x1234, "another thread must not substitute");
        }
        let _serial = serial();
        let id = arm_owned(std::sync::Arc::from([1.0_f32; 4]), 2, 2).expect("armed");
        // The operation belongs to this thread; a callback on a worker thread is
        // forwarded unchanged.
        std::thread::spawn(move || {
            let mut block = block(2, 0x1234);
            // SAFETY: the descriptor and callback satisfy the writer contract.
            unsafe { write(7, std::ptr::from_mut(&mut block).cast(), Some(consume)) };
            assert_eq!((block.frames, block.pointer), (2, 0x1234));
            assert_eq!(position(id), Some(0), "the other thread cannot advance it");
        })
        .join()
        .expect("writer thread");
        assert!(CALLED.load(Ordering::Acquire));
    }

    #[test]
    fn a_plan_binds_its_first_writer_closure() {
        static SEEN: AtomicUsize = AtomicUsize::new(0);
        unsafe extern "C" fn consume(_closure: usize, block: usize) {
            // SAFETY: this test supplies the descriptor.
            let block = unsafe { &*(block as *const Block) };
            SEEN.store(block.pointer, Ordering::Release);
        }
        let _serial = serial();
        let audio: std::sync::Arc<[f32]> = std::sync::Arc::from([1., 2., 3., 4.]);
        let audio_pointer = audio.as_ptr() as usize;
        let id = arm_owned(audio, 4, 1).expect("armed");
        let mut first = block(2, 0x1111);
        // SAFETY: the descriptor and callback satisfy the writer contract.
        unsafe { write(7, std::ptr::from_mut(&mut first).cast(), Some(consume)) };
        assert_eq!(SEEN.load(Ordering::Acquire), audio_pointer);
        assert_eq!(position(id), Some(2));
        // A different writer closure is an unrelated operation: forward it.
        let mut second = block(2, 0x2222);
        // SAFETY: as above.
        unsafe { write(8, std::ptr::from_mut(&mut second).cast(), Some(consume)) };
        assert_eq!(SEEN.load(Ordering::Acquire), 0x2222);
        assert_eq!(
            position(id),
            Some(2),
            "the unrelated write must not advance"
        );
        assert_eq!((second.frames, second.pointer), (2, 0x2222));
    }

    #[test]
    fn clearing_one_operation_leaves_another_armed() {
        let _serial = serial();
        let first = arm_owned(std::sync::Arc::from([1.0_f32; 4]), 4, 1).expect("first");
        let second = arm_owned(std::sync::Arc::from([2.0_f32; 8]), 4, 2).expect("second");
        assert_ne!(first, second);
        assert_eq!(position(first), None, "the replaced plan is gone");
        clear_if(first);
        assert_eq!(total_frames(second), Some(4), "clear_if must not touch it");
        clear_if(second);
        assert_eq!(total_frames(second), None);
    }

    #[test]
    fn an_inflight_callback_keeps_its_audio_after_a_clear() {
        static STARTED: AtomicBool = AtomicBool::new(false);
        static CLEARED: AtomicBool = AtomicBool::new(false);
        static SEEN: AtomicUsize = AtomicUsize::new(0);
        unsafe extern "C" fn consume(_closure: usize, block: usize) {
            // SAFETY: this test supplies the descriptor and keeps the audio alive.
            let block = unsafe { &*(block as *const Block) };
            STARTED.store(true, Ordering::Release);
            while !CLEARED.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
            // The plan has been cleared; the callback's own reference is what
            // still makes this read valid.
            SEEN.store(block.pointer, Ordering::Release);
        }
        let _serial = serial();
        let audio: std::sync::Arc<[f32]> = std::sync::Arc::from([5., 6., 7., 8.]);
        let audio_pointer = audio.as_ptr() as usize;
        arm_owned(audio, 2, 1).expect("armed");
        std::thread::spawn(|| {
            while !STARTED.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
            clear();
            CLEARED.store(true, Ordering::Release);
        });
        let mut block = block(2, 0x1234);
        // SAFETY: the descriptor and callback satisfy the writer contract.
        unsafe { write(7, std::ptr::from_mut(&mut block).cast(), Some(consume)) };
        assert_eq!(SEEN.load(Ordering::Acquire), audio_pointer);
        assert!(wait_idle(0));
    }

    #[test]
    fn a_nested_writer_callback_is_forwarded_and_does_not_advance_the_plan() {
        static NESTED_CALLED: AtomicBool = AtomicBool::new(false);
        unsafe extern "C" fn nested(_closure: usize, block: usize) {
            NESTED_CALLED.store(true, Ordering::Release);
            // SAFETY: this test supplies the inner descriptor.
            let block = unsafe { &*(block as *const Block) };
            assert_eq!(block.pointer, 0x999, "the nested block must be untouched");
            assert_eq!(block.frames, 2);
        }
        unsafe extern "C" fn outer(closure: usize, raw: usize) {
            // SAFETY: this test supplies the outer descriptor.
            let outer = unsafe { &*(raw as *const Block) };
            assert_ne!(outer.pointer, 0, "the outer block is substituted");
            let mut inner = block(2, 0x999);
            // SAFETY: the descriptor and callback satisfy the writer contract.
            unsafe { write(closure, std::ptr::from_mut(&mut inner).cast(), Some(nested)) };
        }
        let _serial = serial();
        let id = arm_owned(std::sync::Arc::from([1.0_f32; 4]), 4, 1).expect("armed");
        let mut block = block(2, 0x1111);
        // SAFETY: the descriptor and callback satisfy the writer contract.
        unsafe { write(7, std::ptr::from_mut(&mut block).cast(), Some(outer)) };
        assert!(NESTED_CALLED.load(Ordering::Acquire));
        assert_eq!(block.pointer, 0x1111);
        assert_eq!(position(id), Some(2), "only the outer block advanced");
    }

    #[test]
    fn an_id_from_a_previous_operation_cannot_read_the_current_plan() {
        let _serial = serial();
        let stale: ArmId = ArmId(0);
        let current = arm_owned(std::sync::Arc::from([1.0_f32; 4]), 4, 1).expect("armed");
        assert_eq!(position(stale), None);
        assert_eq!(total_frames(stale), None);
        clear_if(stale);
        assert_eq!(total_frames(current), Some(4));
        clear();
    }
}
