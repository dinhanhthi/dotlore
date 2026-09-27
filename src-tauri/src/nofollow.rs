//! No-follow path walk shared by the mirror and the entry picker.
//!
//! Each component is opened with `openat` and `O_NOFOLLOW`, so a symlink
//! swapped in anywhere along the path cannot redirect the walk. Non-unix
//! targets have no walker yet and return `ErrorKind::Unsupported`.

#[cfg(unix)]
mod imp {
    use std::ffi::{CString, OsStr, OsString};
    use std::fs::{self, File};
    use std::io::{self, ErrorKind};
    use std::os::raw::c_char as libc_c_char;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::OpenOptionsExt;
    #[cfg(target_os = "macos")]
    use std::os::unix::io::IntoRawFd;
    use std::os::unix::io::{AsRawFd, FromRawFd};
    use std::path::{Component, Path, PathBuf};

    // Darwin / Linux open(2) flags. The values differ by OS; each is the
    // real flag for that OS, so the same walker builds on Linux and macOS.
    #[cfg(target_os = "macos")]
    const O_NOFOLLOW: i32 = 0x0100;
    #[cfg(target_os = "macos")]
    const O_DIRECTORY: i32 = 0x0010_0000;
    #[cfg(target_os = "macos")]
    const O_CLOEXEC: i32 = 0x0100_0000;
    #[cfg(target_os = "linux")]
    const O_NOFOLLOW: i32 = 0x20000;
    #[cfg(target_os = "linux")]
    const O_DIRECTORY: i32 = 0x10000;
    #[cfg(target_os = "linux")]
    const O_CLOEXEC: i32 = 0o2000000;
    const O_RDONLY: i32 = 0;

    #[cfg(target_os = "macos")]
    const ELOOP: i32 = 62;
    #[cfg(target_os = "linux")]
    const ELOOP: i32 = 40;
    const ENOTDIR: i32 = 20;

    extern "C" {
        fn openat(dirfd: i32, pathname: *const libc_c_char, flags: i32) -> i32;
    }

    #[cfg(target_os = "macos")]
    enum DIR {}

    #[cfg(target_os = "macos")]
    #[repr(C)]
    struct Dirent {
        d_ino: u64,
        d_seekoff: u64,
        d_reclen: u16,
        d_namlen: u16,
        d_type: u8,
        d_name: [i8; 1024],
    }

    #[cfg(target_os = "macos")]
    extern "C" {
        fn close(fd: i32) -> i32;
        fn fdopendir(fd: i32) -> *mut DIR;
        fn readdir(dirp: *mut DIR) -> *mut Dirent;
        fn closedir(dirp: *mut DIR) -> i32;
    }

    pub(crate) fn is_unsafe_open(e: &io::Error) -> bool {
        matches!(e.raw_os_error(), Some(ELOOP) | Some(ENOTDIR))
            || e.kind() == ErrorKind::InvalidInput
    }

    pub(crate) fn open_root_dir(root: &Path) -> io::Result<File> {
        let mut opts = fs::OpenOptions::new();
        opts.read(true)
            .custom_flags(O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
        opts.open(root)
    }

    fn openat_child(parent: &File, name: &OsStr, flags: i32) -> io::Result<File> {
        let c_name = CString::new(name.as_bytes())
            .map_err(|_| io::Error::new(ErrorKind::InvalidInput, "path component contains NUL"))?;
        let fd = unsafe { openat(parent.as_raw_fd(), c_name.as_ptr(), flags) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { File::from_raw_fd(fd) })
    }

    /// Open `rel` from `root` one component at a time, never following a symlink.
    pub(crate) fn open_chain(root: &Path, rel: &Path, directory: bool) -> io::Result<File> {
        let mut fd = open_root_dir(root)?;
        let mut acc = PathBuf::new();
        let comps: Vec<_> = rel.components().collect();
        for (i, component) in comps.iter().enumerate() {
            let name = match component {
                Component::Normal(n) => *n,
                _ => {
                    return Err(io::Error::new(
                        ErrorKind::InvalidInput,
                        "path is not a plain relative path",
                    ))
                }
            };
            acc.push(name);
            replace_hook(&root.join(&acc));
            let last = i + 1 == comps.len();
            let mut flags = O_RDONLY | O_CLOEXEC | O_NOFOLLOW;
            if !last || directory {
                flags |= O_DIRECTORY;
            }
            fd = openat_child(&fd, name, flags)?;
        }
        Ok(fd)
    }

    pub(crate) fn open_dir_nofollow(root: &Path, rel: &Path) -> io::Result<File> {
        if rel.as_os_str().is_empty() {
            open_root_dir(root)
        } else {
            open_chain(root, rel, true)
        }
    }

    pub(crate) fn child_kind(dir: &File, name: &OsStr) -> Option<String> {
        let file = openat_child(dir, name, O_RDONLY | O_CLOEXEC | O_NOFOLLOW).ok()?;
        let md = file.metadata().ok()?;
        if md.is_dir() {
            Some("directory".into())
        } else if md.is_file() {
            Some("file".into())
        } else {
            None
        }
    }

    /// List through an already-open no-follow directory fd (`fdopendir`).
    #[cfg(target_os = "macos")]
    pub(crate) fn read_dir_fd(dir: File) -> io::Result<Vec<OsString>> {
        let raw = dir.into_raw_fd();
        let dirp = unsafe { fdopendir(raw) };
        if dirp.is_null() {
            let err = io::Error::last_os_error();
            unsafe { close(raw) };
            return Err(err);
        }
        let mut names = Vec::new();
        loop {
            let ent = unsafe { readdir(dirp) };
            if ent.is_null() {
                break;
            }
            let namlen = unsafe { (*ent).d_namlen as usize };
            let bytes =
                unsafe { std::slice::from_raw_parts((*ent).d_name.as_ptr().cast::<u8>(), namlen) };
            if bytes == b"." || bytes == b".." {
                continue;
            }
            names.push(OsStr::from_bytes(bytes).to_os_string());
        }
        unsafe { closedir(dirp) };
        Ok(names)
    }

    #[cfg(not(target_os = "macos"))]
    pub(crate) fn read_dir_fd(dir: File) -> io::Result<Vec<OsString>> {
        let listing = fs::read_dir(format!("/proc/self/fd/{}", dir.as_raw_fd()))?;
        let mut names = Vec::new();
        for entry in listing {
            names.push(entry?.file_name());
        }
        Ok(names)
    }

    #[cfg(test)]
    thread_local! {
        static REPLACE_AT: std::cell::RefCell<Option<Box<dyn Fn(&Path)>>> =
            const { std::cell::RefCell::new(None) };
        static LISTING_REPLACE_AT: std::cell::RefCell<Option<Box<dyn Fn(&Path)>>> =
            const { std::cell::RefCell::new(None) };
    }

    fn replace_hook(path: &Path) {
        #[cfg(test)]
        REPLACE_AT.with(|c| {
            if let Some(f) = c.borrow().as_ref() {
                f(path);
            }
        });
        let _ = path;
    }

    pub(crate) fn listing_replace_hook(path: &Path) {
        #[cfg(test)]
        LISTING_REPLACE_AT.with(|c| {
            if let Some(f) = c.borrow().as_ref() {
                f(path);
            }
        });
        let _ = path;
    }

    /// Test seam: run `hook` between inspecting a path component and opening it.
    #[cfg(test)]
    pub(crate) fn with_replace_hook<F, T>(hook: F, body: impl FnOnce() -> T) -> T
    where
        F: Fn(&Path) + 'static,
    {
        REPLACE_AT.with(|c| *c.borrow_mut() = Some(Box::new(hook)));
        let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));
        REPLACE_AT.with(|c| *c.borrow_mut() = None);
        match out {
            Ok(v) => v,
            Err(e) => std::panic::resume_unwind(e),
        }
    }

    /// Test seam: run `hook` after inspect classified a directory and before
    /// the no-follow listing open.
    #[cfg(test)]
    pub(crate) fn with_listing_hook<F, T>(hook: F, body: impl FnOnce() -> T) -> T
    where
        F: Fn(&Path) + 'static,
    {
        LISTING_REPLACE_AT.with(|c| *c.borrow_mut() = Some(Box::new(hook)));
        let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));
        LISTING_REPLACE_AT.with(|c| *c.borrow_mut() = None);
        match out {
            Ok(v) => v,
            Err(e) => std::panic::resume_unwind(e),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_listing_replace<T>(
        hook: impl Fn(&Path) + 'static,
        f: impl FnOnce() -> T,
    ) -> T {
        LISTING_REPLACE_AT.with(|c| *c.borrow_mut() = Some(Box::new(hook)));
        let out = f();
        LISTING_REPLACE_AT.with(|c| *c.borrow_mut() = None);
        out
    }
}

#[cfg(unix)]
pub(crate) use imp::{
    child_kind, is_unsafe_open, listing_replace_hook, open_chain, open_dir_nofollow, open_root_dir,
    read_dir_fd,
};

#[cfg(all(unix, test))]
pub(crate) use imp::{with_listing_hook, with_listing_replace, with_replace_hook};

// No NT walker. Stubs exist so non-unix targets compile; every open fails.
#[cfg(not(unix))]
use std::ffi::{OsStr, OsString};
#[cfg(not(unix))]
use std::fs::File;
#[cfg(not(unix))]
use std::io::{self, ErrorKind};
#[cfg(not(unix))]
use std::path::Path;

#[cfg(not(unix))]
fn unsupported() -> io::Error {
    io::Error::new(
        ErrorKind::Unsupported,
        "no-follow walk is not implemented on this platform",
    )
}

#[cfg(not(unix))]
pub(crate) fn is_unsafe_open(_e: &io::Error) -> bool {
    false
}

#[cfg(not(unix))]
pub(crate) fn open_root_dir(_root: &Path) -> io::Result<File> {
    Err(unsupported())
}

#[cfg(not(unix))]
pub(crate) fn open_chain(_root: &Path, _rel: &Path, _directory: bool) -> io::Result<File> {
    Err(unsupported())
}

#[cfg(not(unix))]
pub(crate) fn open_dir_nofollow(_root: &Path, _rel: &Path) -> io::Result<File> {
    Err(unsupported())
}

#[cfg(not(unix))]
pub(crate) fn read_dir_fd(_dir: File) -> io::Result<Vec<OsString>> {
    Err(unsupported())
}

#[cfg(not(unix))]
pub(crate) fn child_kind(_dir: &File, _name: &OsStr) -> Option<String> {
    None
}

#[cfg(not(unix))]
pub(crate) fn listing_replace_hook(_path: &Path) {}
