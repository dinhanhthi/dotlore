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
