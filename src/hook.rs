//! In-memory branch hooks.
//!
//! The SDK never touches the firmware on disk: it patches one instruction in
//! the loaded image and keeps everything else in pages it maps next to the app.
//!
//! A site's first instruction is replaced; the trampoline carries the displaced
//! prologue so the original still runs on its own frame:
//!
//! ```text
//!   entry : b slot            (slot lives in a page mapped near the app)
//!   slot  : ldr x16, #8 ; br x16 ; .quad hook
//!   tramp : <n displaced words> ; b entry+n*4
//! ```
//!
//! `n` is everything up to the first PC-relative instruction, so a prologue that
//! computes an address from its own location is refused instead of moved.  A `b`
//! reaches +-128 MiB, so both pages are mapped just above the app's image.

use crate::image::{self, Firmware};

/// Largest number of hooks this build installs.
pub const MAX_HOOKS: usize = 60;
/// Distance between two relay slots (and between two trampolines).
const SLOT_STRIDE: usize = 32;
/// Page size on the device.
const PAGE_SIZE: usize = 4096;
/// Where the search for a free relay page starts, relative to the load base.
const RELAY_FIRST: usize = 0x0790_0000;
/// Last candidate for the relay page.
const RELAY_LAST: usize = 0x07f0_0000;
/// Where the search for a free trampoline page starts.
const TRAMP_FIRST: usize = 0x0792_0000;
/// Last candidate for the trampoline page.
const TRAMP_LAST: usize = 0x07f8_0000;
/// Step between candidate addresses.
#[cfg(target_os = "linux")]
const SEARCH_STEP: usize = 0x1_0000;
/// `ldr x16, [pc, #8]`, the first word of a relay slot.
const LOAD_LITERAL: u32 = 0x5800_0050;
/// `br x16`, the second word of a relay slot.
const BRANCH_REGISTER: u32 = 0xd61f_0200;
/// Bytes copied from the prologue at most: the hook erases one instruction and
/// the trampoline has to reproduce everything it displaced.
const PROLOGUE_WORDS: usize = 4;

/// A hook that could not be installed, with the reason.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookError {
    /// The relay or trampoline page could not be mapped near the app.
    NoPages,
    /// The site could not be read.
    Unreadable,
    /// The site starts with a PC-relative instruction, which cannot be moved
    /// into a trampoline.
    PcRelative {
        /// The offending instruction.
        instruction: u32,
    },
    /// No room left in the fixed page tables.
    Full,
    /// The site is too far from the relay page for a `b`.
    OutOfRange,
    /// The page could not be made writable, or the patch failed.
    PatchFailed,
}

impl core::fmt::Display for HookError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NoPages => f.write_str("no relay/trampoline page near the app"),
            Self::Unreadable => f.write_str("site is unreadable"),
            Self::PcRelative { instruction } => {
                write!(f, "site starts PC-relative ({instruction:#010x})")
            }
            Self::Full => f.write_str("hook table is full"),
            Self::OutOfRange => f.write_str("site is out of branch range"),
            Self::PatchFailed => f.write_str("patching the site failed"),
        }
    }
}

impl std::error::Error for HookError {}

/// Encode an unconditional `b` from `from` to `to`, or `None` when the target
/// is out of the +-128 MiB range.
#[must_use]
pub fn encode_branch(from: usize, to: usize) -> Option<u32> {
    let (Ok(from), Ok(to)) = (i64::try_from(from), i64::try_from(to)) else {
        return None;
    };
    let offset = to.wrapping_sub(from);
    if !(-(1 << 27)..(1 << 27)).contains(&offset) || offset & 3 != 0 {
        return None;
    }
    let imm26 = u32::try_from((offset >> 2) & 0x03ff_ffff).unwrap_or(0);
    Some(0x1400_0000 | imm26)
}

/// Whether an instruction computes an address from its own location and
/// therefore cannot be copied into a trampoline.
#[must_use]
pub fn is_pc_relative(instruction: u32) -> bool {
    instruction & 0x7c00_0000 == 0x1400_0000       // B / BL
        || instruction & 0xff00_0010 == 0x5400_0000 // B.cond
        || instruction & 0x9f00_0000 == 0x9000_0000 // ADRP
        || instruction & 0x9f00_0000 == 0x1000_0000 // ADR
        || instruction & 0x3b00_0000 == 0x1800_0000 // LDR literal
        || instruction & 0x7e00_0000 == 0x3400_0000 // CBZ / CBNZ
        || instruction & 0x7e00_0000 == 0x3600_0000 // TBZ / TBNZ
        || instruction & 0xffff_fc1f == 0xd61f_0000 // BR xN
        || instruction & 0xffff_fc1f == 0xd63f_0000 // BLR xN
        // RET is `0xd65f0000 | Rn << 5`; masking the register field catches
        // `ret` and `ret xN` alike.  (The reference implementation masked only
        // bit 10, which matched `ret x0` and nothing else.)
        || instruction & 0xffff_fc1f == 0xd65f_0000 // RET
}

/// How many words of a prologue the trampoline may copy: everything up to the
/// first PC-relative instruction, and never fewer than the one being erased.
#[must_use]
fn displaced_words(prologue: &[u32; PROLOGUE_WORDS]) -> usize {
    for (index, &instruction) in prologue.iter().enumerate().skip(1) {
        if is_pc_relative(instruction) {
            return index;
        }
    }
    PROLOGUE_WORDS
}

/// One prepared hook: the site to patch once the code pages are executable.
#[derive(Clone, Copy, Debug)]
struct Pending {
    site: usize,
    branch: u32,
    original: u32,
}

impl Pending {
    const EMPTY: Self = Self {
        site: 0,
        branch: 0,
        original: 0,
    };
}

/// The two pages the hooks live in, plus the bookkeeping of what is pending.
///
/// Installation is deliberately two phase: [`Self::prepare`] writes the relay
/// slots and trampolines into writable, non-executable pages, [`Self::seal`]
/// makes those pages executable, and only then does [`Self::commit`] publish the
/// branches at the hook sites.  A failure to seal therefore cannot leave a live
/// branch pointing at non-executable memory.
#[derive(Debug)]
pub struct HookEngine {
    firmware: Firmware,
    relay: *mut u8,
    trampoline: *mut u8,
    pending: [Pending; MAX_HOOKS],
    slots: usize,
    sealed: bool,
    used: bool,
}

// SAFETY: the pages are process-wide mappings owned by the engine; the raw
// pointers are only used to compute slot addresses and to write the trampoline
// bytes, both under `&mut self`.
unsafe impl Send for HookEngine {}

impl HookEngine {
    /// Map the relay and trampoline pages just above the app's image.
    ///
    /// # Errors
    ///
    /// Returns [`HookError::NoPages`] when either page cannot be placed; the
    /// caller then installs nothing and the app runs unmodified.
    pub fn new(firmware: Firmware) -> Result<Self, HookError> {
        let relay = map_near(firmware, RELAY_FIRST, RELAY_LAST).ok_or(HookError::NoPages)?;
        let Some(trampoline) = map_near(firmware, TRAMP_FIRST, TRAMP_LAST) else {
            // SAFETY: relay is the unshared mapping just allocated above.
            unsafe { libc::munmap(relay.cast(), PAGE_SIZE) };
            return Err(HookError::NoPages);
        };
        Ok(Self {
            firmware,
            relay,
            trampoline,
            pending: [Pending::EMPTY; MAX_HOOKS],
            slots: 0,
            sealed: false,
            used: false,
        })
    }

    /// Whether at least one hook was published.
    #[must_use]
    pub const fn touched(&self) -> bool {
        self.used
    }

    /// An engine with no pages, so a failed mapping is reported per hook
    /// instead of aborting the whole installation.
    #[must_use]
    pub const fn disabled(firmware: Firmware) -> Self {
        Self {
            firmware,
            relay: core::ptr::null_mut(),
            trampoline: core::ptr::null_mut(),
            pending: [Pending::EMPTY; MAX_HOOKS],
            slots: 0,
            sealed: false,
            used: false,
        }
    }

    /// Prepare a hook at `offset` in the image.
    ///
    /// The site is left untouched: the relay and trampoline are written, and the
    /// branch is remembered until [`Self::commit`].  On success the returned
    /// address is the trampoline that runs the displaced prologue and continues
    /// into the original body.
    ///
    /// # Errors
    ///
    /// Returns the matching [`HookError`] when the site cannot be hooked; in
    /// that case nothing is changed.
    ///
    /// # Safety
    /// The site and handler must have matching ABIs. Preparation must occur
    /// before app threads start, and these pages must not have been sealed.
    pub unsafe fn prepare(&mut self, offset: usize, hook: usize) -> Result<usize, HookError> {
        if self.sealed {
            return Err(HookError::PatchFailed);
        }
        if self.slots >= MAX_HOOKS {
            return Err(HookError::Full);
        }
        if self.relay.is_null() || self.trampoline.is_null() {
            return Err(HookError::NoPages);
        }
        let site = self.firmware.addr(offset);
        let mut prologue = [0_u32; PROLOGUE_WORDS];
        let mut buffer = [0_u8; PROLOGUE_WORDS * 4];
        if !image::read(site, &mut buffer) {
            return Err(HookError::Unreadable);
        }
        for (word, chunk) in prologue.iter_mut().zip(buffer.chunks_exact(4)) {
            *word = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        }
        if is_pc_relative(prologue[0]) {
            return Err(HookError::PcRelative {
                instruction: prologue[0],
            });
        }
        let count = displaced_words(&prologue);

        // SAFETY: the offsets stay inside the mapped pages, which hold
        // `MAX_HOOKS` slots of `SLOT_STRIDE` bytes each.
        let slot = unsafe { self.relay.add(self.slots * SLOT_STRIDE) };
        // SAFETY: same mapping, one trampoline per slot.
        let trampoline = unsafe { self.trampoline.add(self.slots * SLOT_STRIDE) };

        let back = encode_branch(trampoline as usize + count * 4, site + count * 4)
            .ok_or(HookError::OutOfRange)?;
        let branch = encode_branch(site, slot as usize).ok_or(HookError::OutOfRange)?;

        // slot: ldr x16, [pc, #8] ; br x16 ; .quad hook
        // SAFETY: `slot` has SLOT_STRIDE bytes and `trampoline` has at least
        // `count * 4 + 4` bytes available in its slot.
        unsafe {
            let write = |address: *mut u8, bytes: &[u8]| {
                core::ptr::copy_nonoverlapping(bytes.as_ptr(), address, bytes.len());
            };
            write(slot, &LOAD_LITERAL.to_le_bytes());
            write(slot.add(4), &BRANCH_REGISTER.to_le_bytes());
            write(slot.add(8), &(hook as u64).to_le_bytes());
            for (index, word) in prologue.iter().take(count).enumerate() {
                write(trampoline.add(index * 4), &word.to_le_bytes());
            }
            write(trampoline.add(count * 4), &back.to_le_bytes());
        }
        self.pending[self.slots] = Pending {
            site,
            branch,
            original: prologue[0],
        };
        self.slots += 1;
        Ok(trampoline as usize)
    }

    /// Make the prepared code executable.
    ///
    /// # Errors
    ///
    /// Returns [`HookError::PatchFailed`] when the protection cannot be changed.
    /// The pages are then released and every pending hook is dropped, so no
    /// branch is ever published into non-executable memory.
    pub fn seal(&mut self) -> Result<(), HookError> {
        if self.sealed {
            return Ok(());
        }
        if self.relay.is_null()
            || self.trampoline.is_null()
            || make_executable(self.relay).is_err()
            || make_executable(self.trampoline).is_err()
        {
            self.discard_pages();
            return Err(HookError::PatchFailed);
        }
        // The code was written while the pages were writable; publish it before
        // the first branch can execute.
        for index in 0..self.slots {
            let slot = (self.relay as usize) + index * SLOT_STRIDE;
            flush_icache(slot, slot + 16);
            let trampoline = (self.trampoline as usize) + index * SLOT_STRIDE;
            flush_icache(trampoline, trampoline + PROLOGUE_WORDS * 4 + 4);
        }
        self.sealed = true;
        Ok(())
    }

    /// Publish every prepared branch, rolling all of them back on failure.
    ///
    /// # Errors
    ///
    /// Returns [`HookError::PatchFailed`] when a site cannot be patched or read
    /// back.  Previously patched sites are restored before the error is
    /// returned.  The code pages are released only when every branch was
    /// restored; otherwise they stay mapped so a live branch stays valid.
    pub fn commit(&mut self) -> Result<(), HookError> {
        if !self.sealed {
            return Err(HookError::PatchFailed);
        }
        for index in 0..self.slots {
            let pending = self.pending[index];
            if !patch_site(pending.site, pending.branch) {
                // Restore every site this commit already published, newest
                // first.  Only when every restore succeeds may the trampoline
                // pages be released; a live branch must never point at an
                // unmapped page.
                // The failed site may have been written before its read-back
                // failed; treat "not readable as the original" as landed.
                let landed = image::read_u32(pending.site) != Some(pending.original);
                let mut restored = !landed || patch_site(pending.site, pending.original);
                for done in (0..index).rev() {
                    let pending = self.pending[done];
                    restored &= patch_site(pending.site, pending.original);
                }
                if restored {
                    self.discard_pages();
                } else {
                    // Some branch is still live: keep its page mapped and mark
                    // the engine as touched so `abort` cannot unmap it.
                    self.used = true;
                }
                return Err(HookError::PatchFailed);
            }
        }
        self.pending = [Pending::EMPTY; MAX_HOOKS];
        self.slots = 0;
        self.used = true;
        Ok(())
    }

    /// Drop every prepared hook and release the code pages.
    ///
    /// Does nothing once a branch was published: live branches must keep their
    /// trampolines mapped.
    pub fn abort(&mut self) {
        if self.used {
            return;
        }
        self.discard_pages();
    }

    /// Unmap both pages and reset the hook bookkeeping.
    fn discard_pages(&mut self) {
        // SAFETY: both pointers are unshared mappings owned by this engine, or
        // null when a previous discard already released them.
        unsafe {
            if !self.relay.is_null() {
                libc::munmap(self.relay.cast(), PAGE_SIZE);
            }
            if !self.trampoline.is_null() {
                libc::munmap(self.trampoline.cast(), PAGE_SIZE);
            }
        }
        self.relay = core::ptr::null_mut();
        self.trampoline = core::ptr::null_mut();
        self.pending = [Pending::EMPTY; MAX_HOOKS];
        self.slots = 0;
        self.sealed = false;
    }
}

/// Write one branch word at `site` and verify it landed.
///
/// The page protection is restored to read-execute afterwards.
fn patch_site(site: usize, word: u32) -> bool {
    let page = site & !(PAGE_SIZE - 1);
    let last = (site + 3) & !(PAGE_SIZE - 1);
    let span = (last - page) + PAGE_SIZE;
    // SAFETY: only the pages the four-byte patch touches are remapped.
    let writable = unsafe { make_writable(page, span) };
    let patched = writable
        // SAFETY: the hook site is writable and installation precedes execution.
        && unsafe { image::write(site, &word.to_le_bytes()) }
        // Read the word back: a write that did not land must not count.
        && image::read_u32(site) == Some(word);
    if writable {
        // SAFETY: same range as above, restoring its executable mapping.
        unsafe {
            libc::mprotect(
                page as *mut libc::c_void,
                span,
                libc::PROT_READ | libc::PROT_EXEC,
            );
        }
    }
    flush_icache(site, site + 4);
    patched
}

/// Map one page at the first free address in `first..last`, above the app.
#[cfg(target_os = "linux")]
fn map_near(firmware: Firmware, first: usize, last: usize) -> Option<*mut u8> {
    let mut offset = first;
    while offset < last {
        let address = firmware.addr(offset);
        // SAFETY: a private anonymous mapping at an address chosen by us; the
        // kernel refuses (MAP_FIXED_NOREPLACE) rather than clobbering anything.
        let mapped = unsafe {
            libc::mmap(
                address as *mut libc::c_void,
                PAGE_SIZE,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_FIXED_NOREPLACE,
                -1,
                0,
            )
        };
        if mapped != libc::MAP_FAILED {
            return Some(mapped.cast::<u8>());
        }
        offset += SEARCH_STEP;
    }
    None
}

/// The host cannot map pages inside the app; hooks only exist on the device.
#[cfg(not(target_os = "linux"))]
fn map_near(_firmware: Firmware, _first: usize, _last: usize) -> Option<*mut u8> {
    None
}

/// See [`map_near`]: nothing can be sealed on the host either.
#[cfg(not(target_os = "linux"))]
fn make_executable(_page: *mut u8) -> Result<(), HookError> {
    Err(HookError::PatchFailed)
}

/// See [`map_near`]: nothing is ever writable on the host.
///
/// # Safety
///
/// Unreachable on the host; kept for a single call site.
#[cfg(not(target_os = "linux"))]
unsafe fn make_writable(_address: usize, _length: usize) -> bool {
    false
}

/// Make a page range writable and executable for the patch.
///
/// # Safety
///
/// `address..address + length` must be a mapped page range owned by this process.
#[cfg(target_os = "linux")]
unsafe fn make_writable(address: usize, length: usize) -> bool {
    // SAFETY: the caller guarantees the mapped range owned by this process.
    unsafe {
        libc::mprotect(
            address as *mut libc::c_void,
            length,
            libc::PROT_READ | libc::PROT_WRITE | libc::PROT_EXEC,
        ) == 0
    }
}

/// Make a page read-execute.
#[cfg(target_os = "linux")]
fn make_executable(page: *mut u8) -> Result<(), HookError> {
    // SAFETY: the engine mapped this page itself.
    let result = unsafe {
        libc::mprotect(
            page.cast::<libc::c_void>(),
            PAGE_SIZE,
            libc::PROT_READ | libc::PROT_EXEC,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(HookError::PatchFailed)
    }
}

/// Flush the instruction cache for a freshly written code range.
#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
fn flush_icache(start: usize, end: usize) {
    let line = 64;
    let mut address = start & !(line - 1);
    while address < end {
        // SAFETY: cache maintenance has no memory side effects.
        unsafe { core::arch::asm!("dc civac, {}", in(reg) address) };
        address += line;
    }
    // SAFETY: barriers and the instruction-cache invalidate complete the
    // publish of the new code to every core.
    unsafe {
        core::arch::asm!("dsb ish");
        let mut address = start & !(line - 1);
        while address < end {
            core::arch::asm!("ic ivau, {}", in(reg) address);
            address += line;
        }
        core::arch::asm!("dsb ish");
        core::arch::asm!("isb");
    }
}

/// The host builds never patch code; keep the call sites identical.
#[cfg(not(all(target_arch = "aarch64", target_os = "linux")))]
fn flush_icache(_start: usize, _end: usize) {}

/// The hook engine for the loaded app, created on first use.
static ENGINE: std::sync::OnceLock<std::sync::Mutex<HookEngine>> = std::sync::OnceLock::new();

/// Prepare a hook at `base + offset`, returning the trampoline that runs the
/// displaced prologue.  The site is patched only by [`commit`].
///
/// # Safety
///
/// `base` must be the app's load address, `offset` a site inside the image and
/// `handler` a function with the same ABI as the hooked one.  Preparation must
/// run before application threads start.
pub unsafe fn prepare(base: usize, offset: usize, handler: usize) -> Option<usize> {
    if base == 0 || offset == 0 {
        return None;
    }
    let engine = ENGINE.get_or_init(|| {
        // SAFETY: the caller guarantees `base` is the verified image.
        let firmware = unsafe { Firmware::from_base(base) };
        std::sync::Mutex::new(
            HookEngine::new(firmware).unwrap_or_else(|_| HookEngine::disabled(firmware)),
        )
    });
    let mut engine = engine.lock().ok()?;
    // SAFETY: the caller guarantees the site, handler and startup-only preparation.
    unsafe { engine.prepare(offset, handler) }.ok()
}

/// Seal the prepared code pages and publish every branch.
///
/// Returns `false` when sealing or patching failed; in that case no branch is
/// live, previously patched sites have been restored, and the pages are
/// released.
pub fn commit() -> bool {
    let Some(engine) = ENGINE.get() else {
        return false;
    };
    let Ok(mut engine) = engine.lock() else {
        return false;
    };
    engine.seal().is_ok() && engine.commit().is_ok()
}

/// Drop every prepared hook and release the code pages.
pub fn abort() {
    if let Some(engine) = ENGINE.get()
        && let Ok(mut engine) = engine.lock()
    {
        engine.abort();
    }
}

/// Whether at least one hook was published.
#[must_use]
pub fn touched() -> bool {
    ENGINE
        .get()
        .and_then(|engine| engine.lock().ok())
        .is_some_and(|engine| engine.touched())
}

#[cfg(test)]
mod tests {
    use super::{HookEngine, HookError, displaced_words, encode_branch, is_pc_relative};
    use crate::image::Firmware;

    #[test]
    fn a_failed_site_patch_stops_before_writing() {
        // The host cannot make an arbitrary page writable, so the patch must
        // fail without reaching the write.
        assert!(!super::patch_site(0, 0x1400_0000));
    }

    #[test]
    fn nothing_is_published_without_an_executable_seal() {
        // SAFETY: the handle is only used for address arithmetic; no page maps
        // on the host, so every phase must refuse.
        let firmware = unsafe { Firmware::from_base(0x1000) };
        let mut engine = HookEngine::disabled(firmware);
        // SAFETY: the handler is never reached because there are no pages.
        let prepared = unsafe { engine.prepare(0x10, 0x20) };
        assert_eq!(prepared, Err(HookError::NoPages));
        assert_eq!(engine.seal(), Err(HookError::PatchFailed));
        assert_eq!(engine.commit(), Err(HookError::PatchFailed));
        assert!(!engine.touched());
    }

    #[test]
    fn branch_encoding_matches_the_architecture() {
        // A `b` from a page-aligned site to the next page, 4096 bytes ahead.
        let encoded = encode_branch(0x1000, 0x2000).expect("in range");
        assert_eq!(encoded & 0xfc00_0000, 0x1400_0000, "opcode");
        // offset 0x1000 >> 2 = 0x400
        assert_eq!(encoded & 0x03ff_ffff, 0x400);
        // Backwards: offset -0x1000 >> 2 = -0x400, i.e. 0x3ffc00 in the field.
        assert_eq!(
            encode_branch(0x2000, 0x1000).expect("in range") & 0x03ff_ffff,
            0x03ff_fc00
        );
    }

    #[test]
    fn branch_encoding_rejects_out_of_range_and_unaligned() {
        assert!(encode_branch(0, 1 << 28).is_none());
        assert!(encode_branch(1 << 28, 0).is_none());
        assert!(encode_branch(0, 2).is_none(), "unaligned target");
        assert!(encode_branch(0, (1 << 27) - 4).is_some());
        assert!(encode_branch(0, 1 << 27).is_none());
    }

    #[test]
    fn pc_relative_classification() {
        // The prologues the mod hooks are all safe to copy.
        assert!(!is_pc_relative(0xd503_233f), "paciasp");
        assert!(!is_pc_relative(0xd503_245f), "bti c");
        assert!(!is_pc_relative(0x3940_2003), "ldrb w3, [x0]");
        // A branch, an address computation, a literal load and a return are not.
        assert!(is_pc_relative(0x1400_0010), "b");
        assert!(is_pc_relative(0x9400_0010), "bl");
        assert!(is_pc_relative(0x9000_0000), "adrp");
        assert!(is_pc_relative(0x1000_0000), "adr");
        assert!(is_pc_relative(0x1800_0010), "ldr literal");
        assert!(is_pc_relative(0xb400_0000), "cbz");
        assert!(is_pc_relative(0x5400_0040), "b.eq");
        assert!(is_pc_relative(0x54ff_ffe0), "b.cond, negative offset");
        assert!(!is_pc_relative(0x5400_0010), "bit 4 set is not a B.cond");
        assert!(is_pc_relative(0x3600_0000), "tbz");
        assert!(is_pc_relative(0xd65f_03c0), "ret (Rn = 30)");
        assert!(is_pc_relative(0xd65f_0000), "ret x0");
        assert!(is_pc_relative(0xd61f_0000), "br x0");
        assert!(is_pc_relative(0xd63f_0000), "blr x0");
    }

    #[test]
    fn the_trampoline_copies_up_to_the_first_relocated_instruction() {
        // No PC-relative instruction: copy all four words.
        assert_eq!(
            displaced_words(&[0xd503_233f, 0xa9bf_7bfd, 0x9100_03fd, 0xaa00_03e0]),
            4
        );
        // The third word is a `b`: only the first two are displaced.
        assert_eq!(
            displaced_words(&[0xd503_233f, 0xa9bf_7bfd, 0x1400_0008, 0xaa00_03e0]),
            2
        );
        // The second word is a literal load: only the first one moves.
        assert_eq!(displaced_words(&[0xd503_233f, 0x1800_0020, 0, 0]), 1);
    }
}
