//! macOS shell features, delegated to the existing macOS code.

use std::path::{Path, PathBuf};

/// Inside `~`, where iCloud Drive's Documents folder lives.
const ICLOUD: &str = "Library/Mobile Documents/com~apple~CloudDocs";
/// Inside `~`, where File Provider clients such as Google Drive, Dropbox and
/// OneDrive mount each account.
const CLOUD_STORAGE: &str = "Library/CloudStorage";

pub(super) fn cloud_suggestions(home_dir: &Path) -> Vec<PathBuf> {
    let mut out = vec![home_dir.join(ICLOUD)];
    let mut mounts: Vec<PathBuf> = std::fs::read_dir(home_dir.join(CLOUD_STORAGE))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|p| p.is_dir() && !name_of(p).starts_with('.'))
        .collect();
    mounts.sort();
    out.extend(mounts);
    out
}

pub(super) fn git_install_hint() -> &'static str {
    "git not found — run: xcode-select --install"
}

pub(super) fn file_manager_name() -> &'static str {
    "Finder"
}

pub(super) fn autostart_enabled(home_dir: &Path) -> bool {
    crate::login_item::is_enabled(home_dir)
}

pub(super) fn set_autostart(home_dir: &Path, enabled: bool) -> anyhow::Result<()> {
    crate::login_item::set(home_dir, enabled)
}

pub(super) fn upload_status(path: &Path) -> Option<bool> {
    crate::upload_mac::is_uploaded(path)
}

pub(super) fn supports_upload_status() -> bool {
    true
}

fn name_of(p: &Path) -> String {
    p.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string()
}

/// APFS accepts `:`, reserved device names, and trailing dots or spaces.
/// NUL and an empty path are the only names that cannot exist. Case
/// collisions are [`super::case_insensitive_fs`]'s decision, not this one.
pub(super) fn name_representable_here(rel: &str) -> bool {
    !rel.is_empty() && !rel.contains('\0')
}

/// `true` when `root`'s volume folds case. A missing path or a failed probe
/// is treated as insensitive: that is the APFS default, and checking out two
/// casings there is the failure mode worth avoiding.
pub(super) fn case_insensitive_fs(root: &Path) -> bool {
    use std::os::raw::{c_char, c_int, c_long};
    use std::os::unix::ffi::OsStrExt;

    extern "C" {
        fn pathconf(path: *const c_char, name: c_int) -> c_long;
    }

    // sys/unistd.h: `_PC_CASE_SENSITIVE` is 11. 1 = sensitive, 0 = not.
    const PC_CASE_SENSITIVE: c_int = 11;

    let Ok(c) = std::ffi::CString::new(root.as_os_str().as_bytes()) else {
        return true;
    };
    // SAFETY: pathconf does not retain `c`.
    let v = unsafe { pathconf(c.as_ptr(), PC_CASE_SENSITIVE) };
    if v < 0 {
        return true;
    }
    v == 0
}
