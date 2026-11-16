//! The mod lifecycle: find the app, verify that it is the tested firmware,
//! install the SDK's hooks, and hand control to the mod.
//!
//! A mod does not call any of this itself; it emits the [`crate::export_mod!`]
//! entry point once and implements [`crate::Mod`].

use core::cell::Cell;
use core::fmt;

use crate::event::{AudioHooks, InputHooks, Mod};
use crate::image::{self, Firmware};
use crate::sites;

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

/// The audio-side hooks, copied out of the mod at install.
///
/// Audio callbacks call these function pointers directly instead of taking the
/// module mutex, so a slow UI callback (Extract, a file write, a paint) can
/// never drop an audio block.
static AUDIO: std::sync::OnceLock<AudioHooks> = std::sync::OnceLock::new();
/// Independent live-input callbacks.
static INPUT: std::sync::OnceLock<InputHooks> = std::sync::OnceLock::new();

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
/// UI callbacks never wait: if another UI callback owns the slot (a native
/// operation re-entering from a second thread), the event is forwarded
/// unchanged.  Audio callbacks never take this slot, so there is nothing for a
/// UI callback to wait for.  A nested callback on the same thread observes
/// `None`, which prevents reentrant `&mut` aliasing.
pub fn with_mod<R>(f: impl FnOnce(&mut dyn Mod) -> R) -> Option<R> {
    dispatch(MODULE.get()?, f)
}

/// The audio hooks, or the empty set before install.
#[must_use]
pub fn audio_hooks() -> AudioHooks {
    AUDIO.get().copied().unwrap_or_default()
}

/// The independent live-input callbacks, if installed.
#[must_use]
pub fn input_hooks() -> InputHooks {
    INPUT.get().copied().unwrap_or_default()
}

/// Run `f` if no other UI callback owns the module.
fn dispatch<R>(
    slot: &std::sync::Mutex<&'static mut dyn Mod>,
    f: impl FnOnce(&mut dyn Mod) -> R,
) -> Option<R> {
    let _reentry = Reentry::enter()?;
    let mut module = match slot.try_lock() {
        Ok(module) => module,
        Err(std::sync::TryLockError::WouldBlock) => return None,
        Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner(),
    };
    Some(f(&mut **module))
}

/// Whether every site word matches the tested firmware.
#[must_use]
pub fn firmware_ok(base: usize) -> bool {
    for (offset, expected) in crate::firmware::fingerprints() {
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
    if !firmware_ok(firmware.base()) {
        return Err(InstallError::WrongFirmware);
    }
    // SAFETY: the image has passed the firmware fingerprint check.
    unsafe { image::set_base(firmware.base()) };

    // Keep the ALSA taps linked; they are resolved by name by the app.
    crate::audio::keep_exports();
    // SAFETY: verified image, one installation before application threads.
    if !unsafe { sites::install(firmware.base()) } {
        return Err(InstallError::NoHooks);
    }
    let hooks = module.audio();
    let input = module.input();
    let _ = AUDIO.set(hooks);
    let _ = INPUT.set(input);
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

    use super::{dispatch, firmware_ok};
    use crate::event::Mod;
    use crate::image::test_memory;

    #[derive(Default)]
    struct Probe;

    impl Mod for Probe {}

    fn leaked() -> &'static mut dyn Mod {
        Box::leak(Box::new(Probe))
    }

    /// Fixture bytes for every fingerprint word, laid out at `base`.
    fn tested_image(base: usize) -> Vec<(usize, Vec<u8>)> {
        crate::firmware::fingerprints()
            .map(|(offset, word)| (base + offset, word.to_le_bytes().to_vec()))
            .collect()
    }

    #[test]
    fn the_tested_firmware_passes_every_fingerprint_word() {
        let base = 0x4000_0000;
        assert!(test_memory::with(tested_image(base), || firmware_ok(base)));
    }

    #[test]
    fn one_wrong_word_refuses_the_whole_image() {
        let base = 0x4000_0000;
        let mut image = tested_image(base);
        image[20].1 = 0_u32.to_le_bytes().to_vec();
        assert!(!test_memory::with(image, || firmware_ok(base)));
    }

    #[test]
    fn a_zero_image_is_not_the_firmware() {
        assert!(!firmware_ok(0));
    }

    #[test]
    fn reentrant_dispatch_is_refused_instead_of_aliasing() {
        let slot: Mutex<&'static mut dyn Mod> = Mutex::new(leaked());
        let outer = dispatch(&slot, |_| {
            assert!(dispatch(&slot, |_| ()).is_none(), "reentry must see None");
            7_u8
        });
        assert_eq!(outer, Some(7));
        assert_eq!(dispatch(&slot, |_| 8_u8), Some(8));
    }

    #[test]
    fn a_ui_callback_does_not_wait_for_another_ui_callback() {
        let slot = Arc::new(Mutex::new(leaked()));
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let holder = {
            let slot = Arc::clone(&slot);
            std::thread::spawn(move || {
                dispatch(&slot, |_| {
                    started_tx.send(()).expect("started");
                    release_rx.recv().expect("release");
                })
            })
        };
        started_rx.recv().expect("UI callback running");
        // Waiting here would deadlock a native operation called from the first
        // callback; the second UI callback must be forwarded instead.
        assert!(dispatch(&slot, |_| ()).is_none());
        release_tx.send(()).expect("release holder");
        holder.join().expect("holder");
    }
}
