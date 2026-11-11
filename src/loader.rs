//! The mod lifecycle: find the app, verify that it is the tested firmware,
//! install the SDK's hooks, and hand control to the mod.
//!
//! A mod does not call any of this itself; it emits the [`crate::export_mod!`]
//! entry point once and implements [`crate::Mod`].

use core::cell::Cell;
use core::fmt;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::event::Mod;
use crate::image::{self, Firmware};
use crate::sites;

/// Two words of every site the SDK patches, so a different firmware is refused
/// instead of hooked at the wrong offsets.
const FIRMWARE_SITES: [(usize, u32); 37] = [
    (0x00f5_35c0, 0xd503_233f),
    (0x0114_9880, 0xd503_233f),
    (0x013b_b930, 0xd503_233f),
    (0x0140_dbc0, 0xd503_233f),
    (0x0143_ed40, 0xd503_233f),
    (0x0143_f0b0, 0xd503_233f),
    (0x014a_7580, 0xd503_233f),
    (0x0154_5a50, 0xd503_233f),
    (0x0159_f230, 0x3940_2003),
    (0x0159_f2f0, 0xd503_245f),
    (0x015e_70c0, 0xd503_233f),
    (0x016b_6060, 0xd503_245f),
    (0x0192_d040, 0xd503_233f),
    (0x0195_cf40, 0xd503_233f),
    (0x0195_d1c0, 0xd503_233f),
    (0x0196_0af0, 0xd503_233f),
    (0x0196_0d50, 0xd503_233f),
    (0x0196_8020, 0xd503_233f),
    (0x01d8_aff0, 0xd503_233f),
    (0x01df_3330, 0xd503_233f),
    (0x01e2_70d0, 0xd503_233f),
    (0x01e2_7360, 0xd503_233f),
    (0x020a_3b10, 0xd503_233f),
    (0x020a_7140, 0xd503_233f),
    (0x020a_9fa0, 0xd503_233f),
    (0x020a_a9e0, 0xd503_233f),
    (0x0211_b090, 0xd503_233f),
    (0x0214_72f0, 0xd503_233f),
    (0x023a_ff60, 0xd503_233f),
    (0x027e_4400, 0xd503_233f),
    (0x0280_4e90, 0xd503_233f),
    (0x0280_dca0, 0xd503_233f),
    (0x0281_00c0, 0xd503_233f),
    (0x0281_2f10, 0xd503_245f),
    (0x0281_4ec0, 0xd503_233f),
    (0x0282_2260, 0xd503_233f),
    (0x0294_1ff0, 0xd503_245f),
];

/// Why a mod could not be installed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallError {
    /// The stock app is not loaded.
    NotFound,
    /// The loaded image is not the firmware this SDK was built against.
    WrongFirmware,
    /// Not one hook site could be installed.
    NoHooks,
}

impl fmt::Display for InstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => f.write_str("the MPC Sample is not loaded"),
            Self::WrongFirmware => f.write_str("unsupported firmware image"),
            Self::NoHooks => f.write_str("no hook site could be installed"),
        }
    }
}

impl std::error::Error for InstallError {}

/// The installed mod, if any.  Written once from the constructor, read from the
/// app's threads. The mutex enforces exclusive access without waiting.
static MODULE: std::sync::OnceLock<std::sync::Mutex<&'static mut dyn Mod>> =
    std::sync::OnceLock::new();

/// How many audio callbacks are inside the module right now.
///
/// Only an audio callback sets this, and only while it holds the module lock.
/// A UI callback may wait for that holder; it never waits for another UI
/// callback, which is what keeps a native operation called from `on_paint` from
/// deadlocking against a callback it is waiting for.
static AUDIO_ACTIVE: AtomicU32 = AtomicU32::new(0);

thread_local! {
    /// Whether this thread is already inside a mod callback.
    ///
    /// A nested callback (paint -> string constructor, audio -> native input)
    /// must not wait for itself and must not observe a second `&mut`.
    static IN_CALLBACK: Cell<bool> = const { Cell::new(false) };
}

/// Resets the per-thread reentry marker even if the callback unwinds.
struct Reentry;

impl Reentry {
    fn enter() -> Option<Self> {
        if IN_CALLBACK.with(Cell::get) {
            return None;
        }
        IN_CALLBACK.with(|flag| flag.set(true));
        Some(Self)
    }
}

impl Drop for Reentry {
    fn drop(&mut self) {
        IN_CALLBACK.with(|flag| flag.set(false));
    }
}

/// Run `f` with exclusive access to the installed mod from the UI side.
///
/// UI callbacks wait for an in-flight audio callback instead of being dropped:
/// an encoder turn or button press must reach the mod.  A nested callback on the
/// same thread (for example a draw callback reached through the hooked string
/// constructor) observes `None` and forwards the stock event unchanged, which is
/// what prevents reentrant `&mut` aliasing.
pub fn with_mod<R>(f: impl FnOnce(&mut dyn Mod) -> R) -> Option<R> {
    dispatch(MODULE.get()?, Dispatch::Ui, f)
}

/// Run `f` from an audio callback without ever blocking the audio thread.
///
/// If a UI callback currently owns the module, the audio event is forwarded
/// unchanged rather than waiting.  Audio and UI still never share `&mut self`.
pub fn with_mod_audio<R>(f: impl FnOnce(&mut dyn Mod) -> R) -> Option<R> {
    dispatch(MODULE.get()?, Dispatch::Audio, f)
}

/// How a callback waits for the module slot.
#[derive(Clone, Copy)]
enum Dispatch {
    /// UI: wait only while an audio callback owns the slot.
    Ui,
    /// Audio: never wait; forward when the slot is owned.
    Audio,
}

/// Marks an audio callback as holding the module.
struct AudioActive;

impl AudioActive {
    fn enter() -> Self {
        AUDIO_ACTIVE.fetch_add(1, Ordering::AcqRel);
        Self
    }
}

impl Drop for AudioActive {
    fn drop(&mut self) {
        AUDIO_ACTIVE.fetch_sub(1, Ordering::Release);
    }
}

fn dispatch<R>(
    slot: &std::sync::Mutex<&'static mut dyn Mod>,
    how: Dispatch,
    f: impl FnOnce(&mut dyn Mod) -> R,
) -> Option<R> {
    let _reentry = Reentry::enter()?;
    let mut module = match how {
        Dispatch::Audio => match slot.try_lock() {
            Ok(module) => module,
            Err(std::sync::TryLockError::WouldBlock) => return None,
            Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner(),
        },
        Dispatch::Ui => loop {
            match slot.try_lock() {
                Ok(module) => break module,
                Err(std::sync::TryLockError::Poisoned(error)) => break error.into_inner(),
                Err(std::sync::TryLockError::WouldBlock) => {
                    // Wait only for an audio callback.  Waiting for another UI
                    // callback would deadlock a native operation that the
                    // current UI callback is itself waiting on.
                    if AUDIO_ACTIVE.load(Ordering::Acquire) == 0 {
                        return None;
                    }
                    std::thread::yield_now();
                }
            }
        },
    };
    let audio = matches!(how, Dispatch::Audio).then(AudioActive::enter);
    let result = f(&mut **module);
    // Release the module before clearing the audio marker: a UI callback that
    // sees the marker must be able to acquire the slot, not merely wait for the
    // marker to disappear while the lock is still held.
    drop(module);
    drop(audio);
    Some(result)
}

/// Whether every site word matches the tested firmware.
#[must_use]
pub fn firmware_ok(base: usize) -> bool {
    for (offset, expected) in FIRMWARE_SITES {
        let Some(word) = image::read_u32(base + offset) else {
            return false;
        };
        if word != expected {
            return false;
        }
    }
    true
}

/// The stock app's load address, or zero when it is not loaded.
#[must_use]
pub fn find_main() -> usize {
    Firmware::find().map_or(0, Firmware::base)
}

/// Install `module`: find and verify the firmware, install the hooks, and call
/// [`Mod::on_start`].
///
/// # Errors
///
/// Returns [`InstallError`] when the app is missing, the image is not the tested
/// firmware, or no hook site could be installed.
///
/// # Safety
///
/// Must run once, from the loader's `.init_array`, before the app's threads
/// start.  The module must stay alive for the life of the process.
pub unsafe fn install(module: &'static mut dyn Mod) -> Result<(), InstallError> {
    let Some(firmware) = Firmware::find() else {
        return Err(InstallError::NotFound);
    };
    // SAFETY: verified image, loader runs once before application threads.
    unsafe { image::set_base(firmware.base()) };
    if !firmware_ok(firmware.base()) {
        return Err(InstallError::WrongFirmware);
    }

    // Keep the ALSA taps linked; they are resolved by name by the app.
    crate::audio::keep_exports();
    // SAFETY: verified image, one installation before application threads.
    if !unsafe { sites::install(firmware.base()) } {
        return Err(InstallError::NoHooks);
    }
    module.on_start();
    let _ = MODULE.set(std::sync::Mutex::new(module));
    Ok(())
}

/// Emit the `.init_array` entry point for a mod type.
///
/// The type must implement [`crate::Mod`] and [`Default`].
#[macro_export]
macro_rules! export_mod {
    ($ty:ty) => {
        #[used]
        #[cfg_attr(target_os = "linux", unsafe(link_section = ".init_array"))]
        static MPC_SDK_ENTRY: extern "C" fn() = {
            extern "C" fn start() {
                static mut MODULE: ::core::option::Option<$ty> = None;
                // SAFETY: the loader runs this once, before the app's threads.
                let module: &'static mut $ty = unsafe {
                    let slot = &mut *::core::ptr::addr_of_mut!(MODULE);
                    slot.get_or_insert_with(<$ty>::default)
                };
                // SAFETY: as above; the module lives for the process.
                let _ = unsafe { $crate::install(module) };
            }
            start
        };
    };
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex, mpsc};

    use super::{Dispatch, dispatch, firmware_ok};
    use crate::event::Mod;

    #[derive(Default)]
    struct Probe;

    impl Mod for Probe {}

    fn leaked() -> &'static mut dyn Mod {
        Box::leak(Box::new(Probe))
    }

    #[test]
    fn a_zero_image_is_not_the_firmware() {
        assert!(!firmware_ok(0));
    }

    #[test]
    fn reentrant_dispatch_is_refused_instead_of_aliasing() {
        let slot: Mutex<&'static mut dyn Mod> = Mutex::new(leaked());
        let outer = dispatch(&slot, Dispatch::Ui, |_| {
            assert!(
                dispatch(&slot, Dispatch::Ui, |_| ()).is_none(),
                "reentry must see None"
            );
            7_u8
        });
        assert_eq!(outer, Some(7));
        assert_eq!(dispatch(&slot, Dispatch::Ui, |_| 8_u8), Some(8));
    }

    #[test]
    fn a_ui_callback_waits_for_an_audio_callback_and_audio_skips() {
        let slot = Arc::new(Mutex::new(leaked()));
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let holder = {
            let slot = Arc::clone(&slot);
            std::thread::spawn(move || {
                dispatch(&slot, Dispatch::Audio, |_| {
                    started_tx.send(()).expect("started");
                    release_rx.recv().expect("release");
                })
            })
        };
        started_rx.recv().expect("audio callback running");

        // Audio must not wait: it forwards while the slot is owned.
        assert!(dispatch(&slot, Dispatch::Audio, |_| ()).is_none());

        // UI must wait rather than drop the event.
        let (done_tx, done_rx) = mpsc::channel();
        let waiter = {
            let slot = Arc::clone(&slot);
            std::thread::spawn(move || {
                let result = dispatch(&slot, Dispatch::Ui, |_| 9_u8);
                done_tx.send(result).expect("done");
            })
        };
        assert!(
            done_rx
                .recv_timeout(std::time::Duration::from_millis(100))
                .is_err(),
            "the UI callback must still be waiting"
        );
        release_tx.send(()).expect("release holder");
        assert_eq!(
            done_rx.recv_timeout(std::time::Duration::from_secs(1)),
            Ok(Some(9_u8)),
            "the UI callback must run after the audio callback"
        );
        holder.join().expect("holder");
        waiter.join().expect("waiter");
    }

    #[test]
    fn a_ui_callback_does_not_wait_for_another_ui_callback() {
        let slot = Arc::new(Mutex::new(leaked()));
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let holder = {
            let slot = Arc::clone(&slot);
            std::thread::spawn(move || {
                dispatch(&slot, Dispatch::Ui, |_| {
                    started_tx.send(()).expect("started");
                    release_rx.recv().expect("release");
                })
            })
        };
        started_rx.recv().expect("UI callback running");
        // Waiting here would deadlock a native operation called from the first
        // callback; the second UI callback must be forwarded instead.
        assert!(dispatch(&slot, Dispatch::Ui, |_| ()).is_none());
        release_tx.send(()).expect("release holder");
        holder.join().expect("holder");
    }
}
