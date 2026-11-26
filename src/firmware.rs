//! Firmware profiles: every address the SDK depends on, one table per build.
//!
//! The crate compiles against exactly one stock build, selected by a Cargo
//! feature:
//!
//! * `fw-1-3` — MPC Sample 1.3.0.12 (`40e46ca7991`), the default.
//! * `fw-1-4` — MPC Sample 1.4.0.8 (`d87ac8c12e9`).
//!
//! Both tables use the same names, so the rest of the crate never branches on
//! the firmware. Enable exactly one profile; enabling both or neither is a
//! compile error.
//!
//! Each entry names one place in the stock app: a hook the SDK patches, a
//! function the SDK calls, data it reads, or a guard word that proves the image
//! is the tested firmware. Modules refer to these constants instead of spelling
//! addresses, so one firmware bump edits one file.

#[cfg(feature = "fw-1-3")]
mod v1_3;
#[cfg(feature = "fw-1-4")]
mod v1_4;

#[cfg(all(feature = "fw-1-3", feature = "fw-1-4"))]
compile_error!("enable only one firmware profile: `fw-1-3` or `fw-1-4`");
#[cfg(not(any(feature = "fw-1-3", feature = "fw-1-4")))]
compile_error!("select a firmware profile: `fw-1-3` or `fw-1-4`");

#[cfg(all(feature = "fw-1-3", not(feature = "fw-1-4")))]
pub(crate) use v1_3::*;
#[cfg(feature = "fw-1-4")]
pub(crate) use v1_4::*;

/// What the SDK does with an entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Purpose {
    /// The SDK patches this address with a branch.
    Hook,
    /// The SDK calls this address.
    Call,
    /// An image guard with no caller in this crate.
    Probe,
    /// Data the SDK reads or compares, not code. It has no instruction word; the
    /// caller checks the value it reads at run time.
    Data,
}

/// One firmware location.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Entry {
    /// Offset from the image base.
    pub(crate) offset: usize,
    /// What the SDK does with the address; read by the table's own tests.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) purpose: Purpose,
    /// The instruction word at `offset` in the tested firmware. `None` only for
    /// data entries, which have no instruction to compare.
    pub(crate) word: Option<u32>,
}

const fn hook(offset: usize, word: u32) -> Entry {
    Entry {
        offset,
        purpose: Purpose::Hook,
        word: Some(word),
    }
}

const fn call(offset: usize, word: u32) -> Entry {
    Entry {
        offset,
        purpose: Purpose::Call,
        word: Some(word),
    }
}

const fn data(offset: usize) -> Entry {
    Entry {
        offset,
        purpose: Purpose::Data,
        word: None,
    }
}

const fn probe(offset: usize, word: u32) -> Entry {
    Entry {
        offset,
        purpose: Purpose::Probe,
        word: Some(word),
    }
}

/// `paciasp`, the usual first word of a function.
const PACIASP: u32 = 0xd503_233f;
/// `bti c`, the landing pad at the start of an indirect target.
const BTI_C: u32 = 0xd503_245f;

/// Every entry with an instruction word, which the loader verifies.
pub(crate) fn fingerprints() -> impl Iterator<Item = (usize, u32)> {
    HOOKS
        .iter()
        .chain(GUARDS.iter())
        .chain(CALLS.iter())
        .filter_map(|entry| entry.word.map(|word| (entry.offset, word)))
}

#[cfg(test)]
mod tests {
    use super::{CALLS, CC, GUARDS, HOOKS, Purpose, fingerprints};

    #[test]
    fn every_hook_is_fingerprinted_and_marked_as_a_hook() {
        for hook in HOOKS {
            assert_eq!(hook.purpose, Purpose::Hook);
            assert!(hook.word.is_some(), "hook at {:#x}", hook.offset);
        }
    }

    #[test]
    fn every_call_is_a_call_or_data_and_only_data_lacks_a_word() {
        for entry in CALLS {
            match entry.purpose {
                Purpose::Call => assert!(entry.word.is_some(), "call at {:#x}", entry.offset),
                Purpose::Data => assert!(entry.word.is_none(), "data at {:#x}", entry.offset),
                other => panic!("unexpected {other:?} at {:#x}", entry.offset),
            }
        }
    }

    #[test]
    fn no_address_is_listed_twice() {
        let mut offsets: Vec<usize> = HOOKS
            .iter()
            .chain(GUARDS.iter())
            .chain(CALLS.iter())
            .map(|entry| entry.offset)
            .collect();
        let listed = offsets.len();
        offsets.sort_unstable();
        offsets.dedup();
        assert_eq!(offsets.len(), listed);
    }

    #[test]
    fn the_fingerprint_set_covers_every_word_the_tested_image_has() {
        // The vtables are data: they have no instruction word to verify, and
        // they are the only entries that may lack one.  A new one is a decision
        // about what `firmware_ok` checks, so it has to be reviewed here.
        let data = HOOKS
            .iter()
            .chain(GUARDS.iter())
            .chain(CALLS.iter())
            .filter(|entry| entry.word.is_none())
            .count();
        assert_eq!(data, 4, "only the four vtables are data entries");
        // Every hook, guard and call with code, checked by `firmware_ok`.
        let code_entries = HOOKS.len() + GUARDS.len() + CALLS.len() - data;
        assert_eq!(fingerprints().count(), code_entries);
        assert!(fingerprints().any(|(offset, word)| offset == CC.offset && word == 0x3940_2003));
    }
}
