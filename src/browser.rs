//! Native file-browser extensions.
//!
//! Virtual directories are injected at the POSIX directory-enumeration seam
//! used by the firmware. Their entries keep their original paths: directory
//! metadata and file reads are redirected to those paths, so no audio is
//! copied to create a browser entry.

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};

#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
use std::ffi::{CStr, CString};

/// Observe a successfully opened file. The path is the real source path, even
/// when the app opened it through a virtual directory entry.
///
/// The callback runs on the app thread that opened the file. It must be fast,
/// must not panic, and must not call file I/O that would recursively invoke the
/// observer.
pub type FileOpenObserver = unsafe extern "C" fn(*const libc::c_char);

#[derive(Clone)]
struct VirtualDirectory {
    parent: PathBuf,
    name: OsStringOwned,
    entries: Vec<VirtualFile>,
}

#[derive(Clone)]
struct OsStringOwned(Vec<u8>);

/// One visible name and the real source path it represents in a virtual folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VirtualFile {
    name: OsString,
    source: PathBuf,
}

impl VirtualFile {
    /// Create a virtual file entry. The display name must be one path component
    /// and the source path must be absolute.
    #[must_use]
    pub fn new(name: &OsStr, source: PathBuf) -> Option<Self> {
        if name.is_empty()
            || name.as_bytes().contains(&b'/')
            || name.as_bytes().contains(&0)
            || !source.is_absolute()
        {
            return None;
        }
        Some(Self {
            name: name.to_os_string(),
            source,
        })
    }

    /// The name shown by the native browser.
    #[must_use]
    pub fn name(&self) -> &OsStr {
        &self.name
    }

    /// The original path opened by the native browser.
    #[must_use]
    pub fn source(&self) -> &Path {
        &self.source
    }
}

impl VirtualDirectory {
    #[cfg(any(all(target_arch = "aarch64", target_os = "linux"), test))]
    fn path(&self) -> PathBuf {
        self.parent.join(OsStr::from_bytes(&self.name.0))
    }
}

#[derive(Clone)]
#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
enum OpenKind {
    Parent {
        folder: VirtualDirectory,
        emitted: bool,
    },
    Virtual {
        entries: Vec<VirtualFile>,
        next: usize,
    },
}

#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
struct OpenDirectory {
    handle: usize,
    kind: OpenKind,
    entry: Box<libc::dirent>,
}

static DIRECTORIES: Mutex<Vec<VirtualDirectory>> = Mutex::new(Vec::new());
#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
static OPEN_DIRECTORIES: Mutex<Vec<OpenDirectory>> = Mutex::new(Vec::new());
static OPEN_OBSERVER: AtomicUsize = AtomicUsize::new(0);

/// Add or update a virtual directory in the native browser.
///
/// `parent` is the real directory where the virtual folder should appear.
/// Each entry has a visible filename and the absolute path of an existing
/// audio file. The source remains at that path and is opened there when
/// selected in the browser.
///
/// Returns `false` for relative paths or a folder name that is not one path
/// component. An empty `entries` slice still shows an empty virtual folder.
#[must_use]
pub fn set_virtual_directory(parent: &Path, name: &OsStr, entries: &[VirtualFile]) -> bool {
    if !parent.is_absolute()
        || name.is_empty()
        || name.as_bytes().contains(&b'/')
        || name.as_bytes().contains(&0)
    {
        return false;
    }

    let mut directories = lock(&DIRECTORIES);
    if let Some(folder) = directories
        .iter_mut()
        .find(|folder| folder.parent == parent && folder.name.0 == name.as_bytes())
    {
        folder.entries.clear();
        folder.entries.extend_from_slice(entries);
    } else {
        directories.push(VirtualDirectory {
            parent: parent.to_path_buf(),
            name: OsStringOwned(name.as_bytes().to_vec()),
            entries: entries.to_vec(),
        });
    }
    true
}

/// Remove a virtual directory previously registered with
/// [`set_virtual_directory`].
pub fn remove_virtual_directory(parent: &Path, name: &OsStr) {
    lock(&DIRECTORIES).retain(|folder| folder.parent != parent || folder.name.0 != name.as_bytes());
}

/// Observe native file opens. Pass `None` to stop observing.
pub fn set_file_open_observer(observer: Option<FileOpenObserver>) {
    OPEN_OBSERVER.store(
        observer.map_or(0, |callback| callback as usize),
        Ordering::Release,
    );
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
fn path_from_ptr(path: *const libc::c_char) -> Option<PathBuf> {
    if path.is_null() {
        return None;
    }
    // SAFETY: POSIX path arguments are NUL-terminated strings supplied by libc callers.
    let path = unsafe { CStr::from_ptr(path) };
    Some(PathBuf::from(OsStr::from_bytes(path.to_bytes())))
}

#[cfg(any(all(target_arch = "aarch64", target_os = "linux"), test))]
fn normalized(path: &Path) -> &OsStr {
    let bytes = path.as_os_str().as_bytes();
    OsStr::from_bytes(bytes.strip_suffix(b"/").unwrap_or(bytes))
}

#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
fn directory_for_parent(path: &Path) -> Option<VirtualDirectory> {
    let path = normalized(path);
    lock(&DIRECTORIES)
        .iter()
        .find(|folder| normalized(&folder.parent) == path)
        .cloned()
}

#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
fn virtual_directory(path: &Path) -> Option<VirtualDirectory> {
    let path = normalized(path);
    lock(&DIRECTORIES)
        .iter()
        .find(|folder| normalized(&folder.path()) == path)
        .cloned()
}

#[cfg(any(all(target_arch = "aarch64", target_os = "linux"), test))]
fn source_path(path: &Path) -> Option<PathBuf> {
    let path = normalized(path);
    for folder in lock(&DIRECTORIES).iter() {
        if normalized(&folder.path()) == path {
            return Some(folder.parent.clone());
        }
        let prefix = folder.path();
        if let Ok(relative) = Path::new(path).strip_prefix(&prefix)
            && relative.components().count() == 1
        {
            let name = relative.as_os_str().as_bytes();
            if let Some(entry) = folder
                .entries
                .iter()
                .find(|entry| entry.name.as_bytes() == name)
            {
                return Some(entry.source.clone());
            }
        }
    }
    None
}

#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
fn c_path(path: &Path) -> Option<CString> {
    CString::new(path.as_os_str().as_bytes()).ok()
}

#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
fn observe_open(path: &Path) {
    let callback = OPEN_OBSERVER.load(Ordering::Acquire);
    if callback == 0 {
        return;
    }
    let Some(path) = c_path(path) else { return };
    // SAFETY: the stored pointer is set only from a `FileOpenObserver`.
    let callback: FileOpenObserver = unsafe { core::mem::transmute(callback) };
    // SAFETY: `path` stays alive for this synchronous callback.
    unsafe { callback(path.as_ptr()) };
}

#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
mod interpose {
    use super::{
        CStr, OPEN_DIRECTORIES, OpenDirectory, OpenKind, OsStrExt, VirtualDirectory, c_path,
        directory_for_parent, lock, observe_open, path_from_ptr, source_path, virtual_directory,
    };
    use core::ffi::c_void;
    use std::mem::size_of;

    unsafe extern "C" {
        fn dlsym(handle: *mut c_void, symbol: *const libc::c_char) -> *mut c_void;
    }

    fn next(symbol: &'static CStr) -> *mut c_void {
        // SAFETY: RTLD_NEXT resolves the next implementation after this preload library.
        unsafe { dlsym(libc::RTLD_NEXT, symbol.as_ptr()) }
    }

    unsafe fn open_dir(path: *const libc::c_char) -> *mut libc::DIR {
        type OpenDir = unsafe extern "C" fn(*const libc::c_char) -> *mut libc::DIR;
        let symbol = c"opendir";
        let address = next(symbol);
        if address.is_null() {
            return core::ptr::null_mut();
        }
        // SAFETY: `opendir` has this signature.
        let open: OpenDir = unsafe { core::mem::transmute(address) };
        // SAFETY: forwarded unchanged to libc.
        unsafe { open(path) }
    }

    unsafe fn close_dir(dir: *mut libc::DIR) -> libc::c_int {
        type CloseDir = unsafe extern "C" fn(*mut libc::DIR) -> libc::c_int;
        let address = next(c"closedir");
        if address.is_null() {
            return -1;
        }
        // SAFETY: `closedir` has this signature.
        let close: CloseDir = unsafe { core::mem::transmute(address) };
        // SAFETY: forwarded unchanged to libc.
        unsafe { close(dir) }
    }

    unsafe fn read_dir(dir: *mut libc::DIR) -> *mut libc::dirent {
        type ReadDir = unsafe extern "C" fn(*mut libc::DIR) -> *mut libc::dirent;
        let address = next(c"readdir");
        if address.is_null() {
            return core::ptr::null_mut();
        }
        // SAFETY: `readdir` has this signature.
        let read: ReadDir = unsafe { core::mem::transmute(address) };
        // SAFETY: forwarded unchanged to libc.
        unsafe { read(dir) }
    }

    fn make_entry(out: &mut libc::dirent, name: &[u8], kind: u8) -> *mut libc::dirent {
        // SAFETY: a zeroed dirent is valid before its fields and name are filled.
        *out = unsafe { core::mem::zeroed() };
        out.d_reclen = size_of::<libc::dirent>() as libc::c_ushort;
        out.d_type = kind;
        let n = name.len().min(out.d_name.len().saturating_sub(1));
        for (dest, source) in out.d_name.iter_mut().zip(name.iter()).take(n) {
            *dest = *source as libc::c_char;
        }
        core::ptr::from_mut(out)
    }

    fn emit(folder: &VirtualDirectory, out: &mut libc::dirent, kind: u8) -> *mut libc::dirent {
        make_entry(out, &folder.name.0, kind)
    }

    /// Intercept directory opens so the browser can enumerate a virtual folder.
    #[unsafe(no_mangle)]
    pub(super) unsafe extern "C" fn opendir(path: *const libc::c_char) -> *mut libc::DIR {
        let Some(input) = path_from_ptr(path) else {
            // SAFETY: forwarded unchanged; libc reports EFAULT for a null path.
            return unsafe { open_dir(path) };
        };
        let virtual_folder = virtual_directory(&input);
        let opened = if let Some(folder) = &virtual_folder {
            // Use its parent as a valid opaque DIR handle; readdir below serves
            // the registered entries instead of reading this directory.
            c_path(&folder.parent).map_or(core::ptr::null_mut(), |p| {
                // SAFETY: the backing directory is the registered real parent.
                unsafe { open_dir(p.as_ptr()) }
            })
        } else {
            // SAFETY: the caller supplied a valid path or libc reports the error.
            unsafe { open_dir(path) }
        };
        if opened.is_null() {
            return opened;
        }
        let kind = if let Some(folder) = virtual_folder {
            OpenKind::Virtual {
                entries: folder.entries,
                next: 0,
            }
        } else if let Some(folder) = directory_for_parent(&input) {
            OpenKind::Parent {
                folder,
                emitted: false,
            }
        } else {
            return opened;
        };
        lock(&OPEN_DIRECTORIES).push(OpenDirectory {
            handle: opened as usize,
            kind,
            // SAFETY: a zeroed dirent is a valid initial output buffer.
            entry: Box::new(unsafe { core::mem::zeroed() }),
        });
        opened
    }

    /// Add the virtual folder to the native directory stream.
    #[unsafe(no_mangle)]
    pub(super) unsafe extern "C" fn readdir(dir: *mut libc::DIR) -> *mut libc::dirent {
        let handle = dir as usize;
        let mut opened = lock(&OPEN_DIRECTORIES);
        let Some(record) = opened.iter_mut().find(|record| record.handle == handle) else {
            // SAFETY: the caller supplied its original libc DIR pointer.
            return unsafe { read_dir(dir) };
        };
        match &mut record.kind {
            OpenKind::Parent { folder, emitted } => {
                if *emitted {
                    // SAFETY: forwards the terminal read to libc.
                    let entry = unsafe { read_dir(dir) };
                    if entry.is_null() {
                        return entry;
                    }
                    // SAFETY: libc returned a valid dirent for this open stream.
                    let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
                    if name == folder.name.0 {
                        return emit(folder, &mut record.entry, libc::DT_DIR);
                    }
                    return entry;
                }
                // SAFETY: the caller supplied its original libc DIR pointer.
                let entry = unsafe { read_dir(dir) };
                if entry.is_null() {
                    *emitted = true;
                    return emit(folder, &mut record.entry, libc::DT_DIR);
                }
                // SAFETY: libc returned a valid dirent for this open stream.
                let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
                if name == folder.name.0 {
                    *emitted = true;
                    return emit(folder, &mut record.entry, libc::DT_DIR);
                }
                entry
            }
            OpenKind::Virtual { entries, next } => {
                let Some(path) = entries.get(*next) else {
                    return core::ptr::null_mut();
                };
                *next += 1;
                make_entry(&mut record.entry, path.name.as_bytes(), libc::DT_REG)
            }
        }
    }

    /// Release the real directory handle and its virtual enumeration state.
    #[unsafe(no_mangle)]
    pub(super) unsafe extern "C" fn closedir(dir: *mut libc::DIR) -> libc::c_int {
        lock(&OPEN_DIRECTORIES).retain(|record| record.handle != dir as usize);
        // SAFETY: forwards the original handle to libc.
        unsafe { close_dir(dir) }
    }

    unsafe fn path_stat(
        symbol: &'static CStr,
        path: *const libc::c_char,
        out: *mut libc::stat,
    ) -> libc::c_int {
        type Stat = unsafe extern "C" fn(*const libc::c_char, *mut libc::stat) -> libc::c_int;
        let address = next(symbol);
        if address.is_null() {
            return -1;
        }
        // SAFETY: stat and stat64 use the same AArch64 structure and signature.
        let stat: Stat = unsafe { core::mem::transmute(address) };
        let replacement = path_from_ptr(path).and_then(|input| source_path(&input));
        let Some(replacement) = replacement else {
            // SAFETY: forwarded unchanged to libc.
            return unsafe { stat(path, out) };
        };
        let Some(replacement) = c_path(&replacement) else {
            return -1;
        };
        // SAFETY: replacement is a NUL-terminated native path.
        unsafe { stat(replacement.as_ptr(), out) }
    }

    /// Map virtual file metadata requests to each source file.
    #[unsafe(no_mangle)]
    pub(super) unsafe extern "C" fn stat(
        path: *const libc::c_char,
        out: *mut libc::stat,
    ) -> libc::c_int {
        // SAFETY: forwards to libc's stat implementation.
        unsafe { path_stat(c"stat", path, out) }
    }

    /// Map large-file metadata requests to each source file.
    #[unsafe(no_mangle)]
    pub(super) unsafe extern "C" fn stat64(
        path: *const libc::c_char,
        out: *mut libc::stat,
    ) -> libc::c_int {
        // SAFETY: forwards to libc's stat64 implementation.
        unsafe { path_stat(c"stat64", path, out) }
    }

    unsafe fn access_path(path: *const libc::c_char, mode: libc::c_int) -> libc::c_int {
        type Access = unsafe extern "C" fn(*const libc::c_char, libc::c_int) -> libc::c_int;
        let address = next(c"access");
        if address.is_null() {
            return -1;
        }
        // SAFETY: `access` has this signature.
        let access: Access = unsafe { core::mem::transmute(address) };
        let replacement = path_from_ptr(path).and_then(|input| source_path(&input));
        let replacement = replacement.as_deref().and_then(c_path);
        let actual = replacement.as_ref().map_or(path, |p| p.as_ptr());
        // SAFETY: forwards a valid path and the caller's access mode.
        unsafe { access(actual, mode) }
    }

    /// Map native file existence checks to virtual-directory source paths.
    #[unsafe(no_mangle)]
    pub(super) unsafe extern "C" fn access(
        path: *const libc::c_char,
        mode: libc::c_int,
    ) -> libc::c_int {
        // SAFETY: forwards to libc's access implementation.
        unsafe { access_path(path, mode) }
    }

    unsafe fn open_path(
        symbol: &'static CStr,
        path: *const libc::c_char,
        flags: libc::c_int,
        mode: libc::mode_t,
    ) -> libc::c_int {
        type Open =
            unsafe extern "C" fn(*const libc::c_char, libc::c_int, libc::mode_t) -> libc::c_int;
        let address = next(symbol);
        if address.is_null() {
            return -1;
        }
        // SAFETY: open/open64 share the AArch64 calling convention for these arguments.
        let open: Open = unsafe { core::mem::transmute(address) };
        let source = path_from_ptr(path).and_then(|input| source_path(&input));
        let replacement = source.as_deref().and_then(c_path);
        let actual = replacement.as_ref().map_or(path, |p| p.as_ptr());
        // SAFETY: forwards flags and mode to libc; mode is ignored without O_CREAT.
        let fd = unsafe { open(actual, flags, mode) };
        if fd >= 0 {
            if let Some(source) = source {
                observe_open(&source);
            } else if let Some(path) = path_from_ptr(actual) {
                observe_open(&path);
            }
        }
        fd
    }

    /// Open the source path for a virtual file entry.
    #[unsafe(no_mangle)]
    pub(super) unsafe extern "C" fn open(
        path: *const libc::c_char,
        flags: libc::c_int,
        mode: libc::mode_t,
    ) -> libc::c_int {
        // SAFETY: forwards the POSIX open arguments to libc.
        unsafe { open_path(c"open", path, flags, mode) }
    }

    /// Open the source path using the large-file entry point.
    #[unsafe(no_mangle)]
    pub(super) unsafe extern "C" fn open64(
        path: *const libc::c_char,
        flags: libc::c_int,
        mode: libc::mode_t,
    ) -> libc::c_int {
        // SAFETY: forwards the POSIX open64 arguments to libc.
        unsafe { open_path(c"open64", path, flags, mode) }
    }

    unsafe fn fopen_path(
        symbol: &'static CStr,
        path: *const libc::c_char,
        mode: *const libc::c_char,
    ) -> *mut libc::FILE {
        type Fopen =
            unsafe extern "C" fn(*const libc::c_char, *const libc::c_char) -> *mut libc::FILE;
        let address = next(symbol);
        if address.is_null() {
            return core::ptr::null_mut();
        }
        // SAFETY: fopen/fopen64 share this signature.
        let fopen: Fopen = unsafe { core::mem::transmute(address) };
        let source = path_from_ptr(path).and_then(|input| source_path(&input));
        let replacement = source.as_deref().and_then(c_path);
        let actual = replacement.as_ref().map_or(path, |p| p.as_ptr());
        // SAFETY: forwards the mode string and selected path to libc.
        let file = unsafe { fopen(actual, mode) };
        if !file.is_null() {
            if let Some(source) = source {
                observe_open(&source);
            } else if let Some(path) = path_from_ptr(actual) {
                observe_open(&path);
            }
        }
        file
    }

    /// Open a file through stdio, preserving virtual-entry source paths.
    #[unsafe(no_mangle)]
    pub(super) unsafe extern "C" fn fopen(
        path: *const libc::c_char,
        mode: *const libc::c_char,
    ) -> *mut libc::FILE {
        // SAFETY: forwards to libc's fopen.
        unsafe { fopen_path(c"fopen", path, mode) }
    }

    /// Open a large file through stdio, preserving virtual-entry source paths.
    #[unsafe(no_mangle)]
    pub(super) unsafe extern "C" fn fopen64(
        path: *const libc::c_char,
        mode: *const libc::c_char,
    ) -> *mut libc::FILE {
        // SAFETY: forwards to libc's fopen64.
        unsafe { fopen_path(c"fopen64", path, mode) }
    }
}

#[cfg(test)]
mod tests {
    use super::{PathBuf, VirtualFile, set_virtual_directory, source_path};
    use std::ffi::OsStr;

    #[test]
    fn virtual_entries_resolve_to_their_original_paths() {
        let parent = PathBuf::from("/tmp/mpc-browser-test");
        let original = PathBuf::from("/media/MPC SAMPLE/Kits/kick.wav");
        let entry = VirtualFile::new(OsStr::new("kick.wav"), original.clone()).unwrap();
        assert!(set_virtual_directory(
            &parent,
            OsStr::new("Recent Samples"),
            &[entry],
        ));
        assert_eq!(
            source_path(&parent.join("Recent Samples/kick.wav")),
            Some(original)
        );
        assert_eq!(
            source_path(&parent.join("Recent Samples")),
            Some(parent.clone())
        );
        super::remove_virtual_directory(&parent, OsStr::new("Recent Samples"));
    }

    #[test]
    fn virtual_directory_rejects_relative_or_nested_paths() {
        assert!(!set_virtual_directory(
            PathBuf::from("relative").as_path(),
            OsStr::new("Recent Samples"),
            &[],
        ));
        assert!(!set_virtual_directory(
            PathBuf::from("/tmp").as_path(),
            OsStr::new("nested/name"),
            &[],
        ));
        assert!(
            VirtualFile::new(OsStr::new("nested/name"), PathBuf::from("/tmp/file.wav")).is_none()
        );
        assert!(VirtualFile::new(OsStr::new("file.wav"), PathBuf::from("relative.wav")).is_none());
    }
}
