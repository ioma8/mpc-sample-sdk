//! The stock app as a loaded image: where it is mapped, how to reach into it,
//! and how to read and write its memory.
//!
//! Everything the SDK does to the firmware goes through this module: the
//! address of a hooked site, the bytes of a function prologue, the fields of an
//! object the app owns.  Memory access deliberately goes through
//! `/proc/self/mem` instead of a raw pointer: it cannot fault when the app
//! unmaps a page underneath us, and it sidesteps the aliasing rules a Rust
//! reference to arbitrary firmware memory would have to violate.
//!
//! Two descriptors are kept: on this kernel a *read* through an `O_RDWR`
//! `/proc/self/mem` returns `EIO`, which silently breaks every read, so reads
//! and writes open the file separately.

/// Where the stock app is loaded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Firmware {
    base: usize,
}

/// The image base the glue found, so the crate can address firmware calls
/// without being handed it at every entry point.
static BASE: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// Record the verified image base once before callbacks can run.
///
/// # Safety
/// The base must identify the supported firmware for the process lifetime.
pub(crate) unsafe fn set_base(base: usize) {
    BASE.store(base, core::sync::atomic::Ordering::Release);
}

/// The image base the constructor recorded, or zero before it runs.
#[must_use]
pub fn base() -> usize {
    BASE.load(core::sync::atomic::Ordering::Acquire)
}

impl Firmware {
    /// Wrap `base` as an image handle without authenticating it.
    ///
    /// # Safety
    ///
    /// `base` must be the load address of the supported firmware image for the
    /// life of the process, as [`Self::find`] establishes.  Every address
    /// derived from this handle is called or read as firmware.
    #[must_use]
    pub const unsafe fn from_base(base: usize) -> Self {
        Self { base }
    }

    /// Find the stock app among the loaded images.
    ///
    /// The app is the only executable without a name whose text segment covers
    /// the probe offset inside the tested firmware; a library never does.
    #[must_use]
    pub fn find() -> Option<Self> {
        platform::find_base().map(|base| {
            // SAFETY: find_base only reports the unnamed image covering the
            // tested firmware's probe offset.
            unsafe { Self::from_base(base) }
        })
    }

    /// Load address of the image.
    #[must_use]
    pub const fn base(self) -> usize {
        self.base
    }

    /// Absolute address of `offset` in the image.
    #[must_use]
    pub const fn addr(self, offset: usize) -> usize {
        self.base.wrapping_add(offset)
    }
}

/// Read exactly `out.len()` bytes of process memory.
#[must_use]
pub fn read(address: usize, out: &mut [u8]) -> bool {
    platform::read(address, out)
}

/// Write all of `bytes` to process memory.
///
/// # Safety
/// The destination must be writable for this operation, with no conflicting
/// references or concurrent accesses. The bytes must preserve the destination
/// object’s validity and all firmware invariants. A failed write may be partial.
#[must_use]
pub unsafe fn write(address: usize, bytes: &[u8]) -> bool {
    platform::write(address, bytes)
}

/// A firmware function taking one pointer and returning nothing.
pub type VoidOfPointer = unsafe extern "C" fn(usize);
/// A firmware function taking a pointer and a value.
pub type VoidOfPointerValue = unsafe extern "C" fn(usize, u32);
/// A firmware function taking two pointers.
pub type VoidOfTwoPointers = unsafe extern "C" fn(usize, usize);
/// A firmware function taking three pointers.
pub type VoidOfThreePointers = unsafe extern "C" fn(usize, usize, usize);
/// A firmware function taking a pointer and a slot index, returning the index
/// the app assigned.
pub type IntOfPointerSlots = unsafe extern "C" fn(usize, u32, u32, u32) -> i32;

/// Call a firmware function taking one pointer and returning nothing.
///
/// # Safety
///
/// `address` must be the entry point of a function with this exact signature.
pub unsafe fn void_of_pointer(address: usize, argument: usize) {
    // SAFETY: the caller guarantees the address and the signature.
    let function: VoidOfPointer = unsafe { core::mem::transmute(address) };
    // SAFETY: see above.
    unsafe { function(argument) };
}

/// Call a firmware function taking two pointers.
///
/// # Safety
///
/// `address` must be the entry point of a function with this exact signature.
pub unsafe fn void_of_two_pointers(address: usize, first: usize, second: usize) {
    // SAFETY: the caller guarantees the address and the signature.
    let function: VoidOfTwoPointers = unsafe { core::mem::transmute(address) };
    // SAFETY: see above.
    unsafe { function(first, second) };
}

/// Call a firmware function taking three pointers.
///
/// # Safety
///
/// `address` must be the entry point of a function with this exact signature.
pub unsafe fn void_of_three_pointers(address: usize, first: usize, second: usize, third: usize) {
    // SAFETY: the caller guarantees the address and the signature.
    let function: VoidOfThreePointers = unsafe { core::mem::transmute(address) };
    // SAFETY: see above.
    unsafe { function(first, second, third) };
}

/// Call a firmware function taking a pointer and a value.
///
/// # Safety
///
/// `address` must be the entry point of a function with this exact signature.
pub unsafe fn void_of_pointer_value(address: usize, pointer: usize, value: u32) {
    // SAFETY: the caller guarantees the address and the signature.
    let function: VoidOfPointerValue = unsafe { core::mem::transmute(address) };
    // SAFETY: see above.
    unsafe { function(pointer, value) };
}

/// Call the app's slice insert.
///
/// # Safety
///
/// `address` must be the app's insert entry and the arguments must be valid for
/// it.
#[must_use]
pub unsafe fn insert_slice(
    address: usize,
    continuation: usize,
    start: u32,
    count: u32,
    frames: u32,
) -> i32 {
    // SAFETY: the caller guarantees the address and the signature.
    let function: IntOfPointerSlots = unsafe { core::mem::transmute(address) };
    // SAFETY: see above.
    unsafe { function(continuation, start, count, frames) }
}

/// A firmware function taking one pointer and returning an `int`.
pub type IntOfPointer = unsafe extern "C" fn(usize) -> i32;

/// Call the firmware function at `address` with one pointer argument.
///
/// # Safety
///
/// `address` must be the entry point of a function with this exact signature,
/// and `argument` must be a valid argument for it.
#[must_use]
pub unsafe fn int_of_pointer(address: usize, argument: usize) -> i32 {
    // SAFETY: the caller guarantees the address and the signature.
    let function: IntOfPointer = unsafe { core::mem::transmute(address) };
    // SAFETY: see above.
    unsafe { function(argument) }
}

/// Read one 32-bit word of process memory.
#[must_use]
pub fn read_u32(address: usize) -> Option<u32> {
    let mut buffer = [0_u8; 4];
    read(address, &mut buffer).then(|| u32::from_le_bytes(buffer))
}

/// Read one 64-bit word of process memory.
#[must_use]
pub fn read_u64(address: usize) -> Option<u64> {
    let mut buffer = [0_u8; 8];
    read(address, &mut buffer).then(|| u64::from_le_bytes(buffer))
}

/// Read a pointer-sized value; zero reads as `None`.
#[must_use]
pub fn read_pointer(address: usize) -> Option<usize> {
    read_u64(address)
        .filter(|value| *value != 0)
        .map(|value| value as usize)
}

/// The parts that only exist on the device's platform.
///
/// The host builds (tests, clippy) must still compile, so the Linux-only
/// machinery — the dynamic loader walk and `/proc/self/mem` — lives behind this
/// module and reports failure everywhere else.
#[cfg(target_os = "linux")]
mod platform {
    use std::fs::File;
    use std::os::raw::c_void;
    use std::os::unix::fs::FileExt;
    use std::sync::OnceLock;

    /// Offset inside the stock app's text segment, used to recognise which
    /// loaded image is the app.  Any `PT_LOAD` that covers it belongs to firmware
    /// 1.3.0.12.
    const APP_PROBE_OFFSET: usize = 0x00b5_5b00;

    struct Search {
        base: Option<usize>,
    }

    /// `dl_iterate_phdr` callback: remember the unnamed image that covers the
    /// probe offset, and stop iterating.
    ///
    /// # Safety
    ///
    /// Called by the dynamic loader with a valid `info` and with `data` pointing
    /// at the [`Search`] that outlives the iteration.
    unsafe extern "C" fn find_main(
        info: *mut libc::dl_phdr_info,
        _size: usize,
        data: *mut c_void,
    ) -> i32 {
        // SAFETY: data points to the live Search supplied by find_base.
        let search = unsafe { &mut *data.cast::<Search>() };
        // SAFETY: the loader supplies a valid dl_phdr_info.
        let info = unsafe { &*info };
        // SAFETY: a nonnull loader name points to a terminated string.
        if !info.dlpi_name.is_null() && unsafe { *info.dlpi_name } != 0 {
            return 0;
        }
        let count = usize::from(info.dlpi_phnum);
        if info.dlpi_phdr.is_null() || count == 0 {
            return 0;
        }
        // SAFETY: the loader provides `dlpi_phnum` program headers at `dlpi_phdr`.
        let headers = unsafe { core::slice::from_raw_parts(info.dlpi_phdr, count) };
        for header in headers {
            if header.p_type != libc::PT_LOAD {
                continue;
            }
            let start = header.p_vaddr as usize;
            let end = start.saturating_add(header.p_memsz as usize);
            if (start..end).contains(&APP_PROBE_OFFSET) {
                search.base = Some(info.dlpi_addr as usize);
                return 1;
            }
        }
        0
    }

    /// Load address of the stock app, or `None` when it is not the image that
    /// was loaded.
    pub(super) fn find_base() -> Option<usize> {
        let mut search = Search { base: None };
        // SAFETY: the callback only reads the `dl_phdr_info` the loader hands it
        // and writes into `search`, which outlives the call.
        let found = unsafe {
            libc::dl_iterate_phdr(
                Some(find_main),
                std::ptr::from_mut(&mut search).cast::<c_void>(),
            )
        };
        if found == 0 {
            return None;
        }
        search.base
    }

    pub(super) fn read(address: usize, out: &mut [u8]) -> bool {
        let Some(file) = reader() else {
            return false;
        };
        file.read_at(out, address as u64)
            .is_ok_and(|got| got == out.len())
    }

    pub(super) fn write(address: usize, bytes: &[u8]) -> bool {
        let Some(file) = writer() else {
            return false;
        };
        file.write_at(bytes, address as u64)
            .is_ok_and(|put| put == bytes.len())
    }

    fn reader() -> Option<&'static File> {
        static FILE: OnceLock<Option<File>> = OnceLock::new();
        FILE.get_or_init(|| File::open("/proc/self/mem").ok())
            .as_ref()
    }

    fn writer() -> Option<&'static File> {
        static FILE: OnceLock<Option<File>> = OnceLock::new();
        FILE.get_or_init(|| {
            std::fs::OpenOptions::new()
                .write(true)
                .open("/proc/self/mem")
                .ok()
        })
        .as_ref()
    }
}

/// Everywhere else (host tests, clippy) there is no firmware to reach into.
#[cfg(not(target_os = "linux"))]
mod platform {
    pub(super) fn find_base() -> Option<usize> {
        None
    }

    pub(super) fn read(_address: usize, _out: &mut [u8]) -> bool {
        false
    }

    pub(super) fn write(_address: usize, _bytes: &[u8]) -> bool {
        false
    }
}

/// A firmware function taking a pointer and four integers.
pub type VoidOfFive = unsafe extern "C" fn(usize, i32, i32, i32, i32);
/// A firmware function taking eight arguments: pointer, pointer, six numbers.
pub type VoidOfEight = unsafe extern "C" fn(usize, usize, i32, i32, i32, i32, i32, i32, f32);
/// A firmware function taking a pointer and a colour.
pub type VoidOfColour = unsafe extern "C" fn(usize, u32);

/// Call a firmware function taking a pointer and a colour.
///
/// # Safety
///
/// `address` must be the entry point of a function with this exact signature.
pub unsafe fn void_of_colour(address: usize, target: usize, colour: u32) {
    // SAFETY: the caller guarantees the address and the signature.
    let function: VoidOfColour = unsafe { core::mem::transmute(address) };
    // SAFETY: see above.
    unsafe { function(target, colour) };
}

/// Call a firmware function taking a pointer and four integers.
///
/// # Safety
///
/// `address` must be the entry point of a function with this exact signature.
pub unsafe fn void_of_five(address: usize, target: usize, x: i32, y: i32, width: i32, height: i32) {
    // SAFETY: the caller guarantees the address and the signature.
    let function: VoidOfFive = unsafe { core::mem::transmute(address) };
    // SAFETY: see above.
    unsafe { function(target, x, y, width, height) };
}

/// The arguments of a text draw: where, how big, and the glyph alignment.
#[derive(Clone, Copy, Debug)]
pub struct TextBox {
    /// Left edge.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Width.
    pub width: i32,
    /// Height.
    pub height: i32,
    /// Glyph flags the app passes.
    pub flags: i32,
    /// Horizontal alignment.
    pub align: i32,
    /// Opacity.
    pub opacity: f32,
}

/// Call a firmware function taking eight arguments.
///
/// # Safety
///
/// `address` must be the entry point of a function with this exact signature.
pub unsafe fn void_of_eight(address: usize, first: usize, second: usize, text: TextBox) {
    // SAFETY: the caller guarantees the address and the signature.
    let function: VoidOfEight = unsafe { core::mem::transmute(address) };
    // SAFETY: see above.
    unsafe {
        function(
            first,
            second,
            text.x,
            text.y,
            text.width,
            text.height,
            text.flags,
            text.align,
            text.opacity,
        );
    }
}

/// Build a string object from a C string.
///
/// # Safety
///
/// `address` must be the entry point of `String(const char *)` and `out`/`text`
/// must be valid.
pub unsafe fn string_from_c(address: usize, out: usize, text: usize) {
    // SAFETY: the caller guarantees the address and the signature.
    let function: unsafe extern "C" fn(usize, usize) = unsafe { core::mem::transmute(address) };
    // SAFETY: see above.
    unsafe { function(out, text) };
}

/// Whether an address lies inside any mapping.
///
/// The loader asks this about the app's read-only pages too - the screen's
/// vtable among them - so every range is kept, not only the writable ones.
/// Unknown stays permissive: the reads and writes fail safely on their own.
///
/// # Safety
///
/// The pointer is only compared, never dereferenced.
pub unsafe fn mapped(address: *const u8) -> bool {
    use std::sync::OnceLock;
    static RANGES: OnceLock<Vec<(usize, usize)>> = OnceLock::new();
    let ranges = RANGES.get_or_init(|| {
        let Ok(text) = std::fs::read_to_string("/proc/self/maps") else {
            return Vec::new();
        };
        text.lines()
            .filter_map(|line| {
                let span = line.split_whitespace().next()?;
                let (low, high) = span.split_once('-')?;
                Some((
                    usize::from_str_radix(low, 16).ok()?,
                    usize::from_str_radix(high, 16).ok()?,
                ))
            })
            .collect()
    });
    if ranges.is_empty() {
        return true;
    }
    let address = address as usize;
    ranges
        .iter()
        .any(|&(low, high)| address >= low && address < high)
}

#[cfg(test)]
mod tests {
    use super::Firmware;

    #[test]
    fn addresses_are_relative_to_the_load_base() {
        // SAFETY: the test only checks address arithmetic; no memory is read.
        let firmware = unsafe { Firmware::from_base(0xaaaa_aaaa_0000) };
        assert_eq!(firmware.addr(0x1234), 0xaaaa_aaaa_1234);
        // SAFETY: as above.
        assert_eq!(unsafe { Firmware::from_base(0) }.addr(0x10), 0x10);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn reading_our_own_stack_works_and_unmapped_memory_does_not() {
        let value = 0x1234_5678_9abc_def0_u64;
        let address = std::ptr::from_ref(&value) as usize;
        assert_eq!(super::read_u64(address), Some(value));
        // A page that is certainly not mapped: the read must fail, not fault.
        assert_eq!(super::read_u32(1), None);
    }
}
