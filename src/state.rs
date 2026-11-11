//! Process-wide state for a mod's single-threaded UI.
//!
//! A mod's menu, job and MIDI flags live for the process and are touched only
//! from the app's UI thread.  [`Global`] gives that state an explicit unsafe accessor while
//! keeping the same memory layout as the value it holds, so the device tests can
//! still read the exported symbol by address.

use core::cell::UnsafeCell;

/// A value owned by the process and touched from one thread.
///
/// `Global` is not a general-purpose synchronisation primitive: the caller must
/// keep every access on the thread that owns the value.  It exists so the
/// feature code does not need `static mut` or `addr_of!` for state that
/// is already single-threaded by construction.
#[repr(transparent)]
pub struct Global<T> {
    value: UnsafeCell<T>,
}

// SAFETY: the documented contract is single-thread access; `Sync` is required
// only so the value can be a `static`.
unsafe impl<T: Send> Sync for Global<T> {}

impl<T> Global<T> {
    /// Wrap a value.
    #[must_use]
    pub const fn new(value: T) -> Self {
        Self {
            value: UnsafeCell::new(value),
        }
    }

    /// Read the value.
    ///
    /// # Safety
    /// Access must stay on the owning thread. No mutable reference may overlap
    /// the returned reference.
    #[must_use]
    pub unsafe fn get(&self) -> &T {
        // SAFETY: the caller keeps every access on the owning thread.
        unsafe { &*self.value.get() }
    }

    /// Write the value.
    ///
    /// # Safety
    /// Access must stay on the owning thread. No other reference to the value
    /// may overlap the returned reference.
    #[must_use]
    #[allow(
        clippy::mut_from_ref,
        reason = "Global is a single-threaded cell; the caller keeps access on its owner"
    )]
    pub unsafe fn get_mut(&self) -> &mut T {
        // SAFETY: the caller keeps every access on the owning thread.
        unsafe { &mut *self.value.get() }
    }

    /// Run a closure with the value.
    ///
    /// # Safety
    /// The requirements of [`Self::get`] apply, including during the closure.
    pub unsafe fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        // SAFETY: the caller guarantees exclusive-thread access and valid borrowing.
        f(unsafe { self.get() })
    }

    /// Run a closure with the value, mutably.
    ///
    /// # Safety
    /// The requirements of [`Self::get_mut`] apply, including during the closure.
    pub unsafe fn with_mut<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        // SAFETY: the caller guarantees exclusive access for the closure.
        f(unsafe { self.get_mut() })
    }
}

/// Export a static under its Rust name, for a symbol the device harness reads.
///
/// `export_name` is an unsafe attribute; this macro keeps that one token out of
/// the feature crate.
#[macro_export]
macro_rules! export_static {
    ($(#[$meta:meta])* $name:ident : $ty:ty = $value:expr) => {
        $(#[$meta])*
        #[allow(non_upper_case_globals)]
        #[unsafe(export_name = stringify!($name))]
        pub static $name: $ty = $value;
    };
}

#[cfg(test)]
mod tests {
    use super::Global;

    #[test]
    fn a_global_round_trips() {
        static VALUE: Global<i32> = Global::new(7);
        // SAFETY: only this test accesses VALUE, with non-overlapping borrows.
        unsafe {
            assert_eq!(VALUE.get(), &7);
            *VALUE.get_mut() = 9;
            assert_eq!(VALUE.with(|value| *value), 9);
            VALUE.with_mut(|value| *value += 1);
            assert_eq!(VALUE.get(), &10);
        }
    }
}
