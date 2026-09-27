//! Shell features that differ by OS.
//!
//! macOS is implemented in [`macos`]. The other bodies are temporary so
//! commands registered on every target still compile; they are not the
//! Linux or Windows behaviour.

use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
mod macos;

/// iCloud Drive's Documents folder, then each visible `~/Library/CloudStorage` mount.
#[cfg(target_os = "macos")]
pub fn cloud_suggestions(home_dir: &Path) -> Vec<PathBuf> {
    macos::cloud_suggestions(home_dir)
}

/// Temporary: no cloud-folder suggestions on this OS yet.
#[cfg(not(target_os = "macos"))]
pub fn cloud_suggestions(_home_dir: &Path) -> Vec<PathBuf> {
    Vec::new()
}

/// Tray text when `git` is missing. macOS keeps the `xcode-select --install` line.
#[cfg(target_os = "macos")]
pub fn git_install_hint() -> &'static str {
    macos::git_install_hint()
}

/// Temporary: not a real install instruction.
#[cfg(not(target_os = "macos"))]
pub fn git_install_hint() -> &'static str {
    "git not found"
}

/// Name of the system file manager. `"Finder"` on macOS.
#[cfg(target_os = "macos")]
pub fn file_manager_name() -> &'static str {
    macos::file_manager_name()
}

/// Temporary label until this OS has its own name.
#[cfg(not(target_os = "macos"))]
pub fn file_manager_name() -> &'static str {
    "file manager"
}

/// Whether start-at-login is installed for this user.
#[cfg(target_os = "macos")]
pub fn autostart_enabled(home_dir: &Path) -> bool {
    macos::autostart_enabled(home_dir)
}

/// Temporary: start-at-login is off.
#[cfg(not(target_os = "macos"))]
pub fn autostart_enabled(_home_dir: &Path) -> bool {
    false
}

/// Install or remove start-at-login.
#[cfg(target_os = "macos")]
pub fn set_autostart(home_dir: &Path, enabled: bool) -> anyhow::Result<()> {
    macos::set_autostart(home_dir, enabled)
}

/// Temporary: nothing is written.
#[cfg(not(target_os = "macos"))]
pub fn set_autostart(_home_dir: &Path, _enabled: bool) -> anyhow::Result<()> {
    anyhow::bail!("start at login is not available on this system yet")
}

/// `Some(uploaded)` when this OS can report cloud upload state for `path`.
#[cfg(target_os = "macos")]
pub fn upload_status(path: &Path) -> Option<bool> {
    macos::upload_status(path)
}

/// Temporary: this OS has no upload probe.
#[cfg(not(target_os = "macos"))]
pub fn upload_status(_path: &Path) -> Option<bool> {
    None
}

/// Whether [`upload_status`] can return an answer on this OS.
#[cfg(target_os = "macos")]
pub fn supports_upload_status() -> bool {
    macos::supports_upload_status()
}

/// Temporary: no upload probe.
#[cfg(not(target_os = "macos"))]
pub fn supports_upload_status() -> bool {
    false
}

/// Temporary: this OS cannot open a folder in its file manager yet.
/// AppKit stays on the macOS side of `commands::open_folder`.
#[cfg(not(target_os = "macos"))]
pub fn open_folder(dir: &Path) -> Result<(), String> {
    Err(format!(
        "opening {} in the {} is not available on this system yet",
        dir.display(),
        file_manager_name()
    ))
}
