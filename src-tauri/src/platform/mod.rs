//! Shell features that differ by OS.
//!
//! macOS is implemented in [`macos`]. The other bodies are temporary so
//! commands registered on every target still compile; they are not the
//! Linux or Windows behaviour.

use std::collections::BTreeMap;
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

/// Whether `rel` can exist as a relative path on this OS.
///
/// macOS accepts `:`. Windows applies [`crate::portable::name_issue`]. Linux
/// accepts every non-empty path without NUL. A test seam can replace this.
pub fn name_representable_here(rel: &str) -> bool {
    if let Some(answer) = representable_override(rel) {
        return answer;
    }
    native_representable(rel)
}

#[cfg(target_os = "macos")]
fn native_representable(rel: &str) -> bool {
    macos::name_representable_here(rel)
}

#[cfg(target_os = "windows")]
fn native_representable(rel: &str) -> bool {
    crate::portable::name_issue(rel).is_none()
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn native_representable(rel: &str) -> bool {
    !rel.is_empty() && !rel.contains('\0')
}

/// Whether `root`'s volume treats `A.md` and `a.md` as the same file.
#[cfg(target_os = "macos")]
pub fn case_insensitive_fs(root: &Path) -> bool {
    macos::case_insensitive_fs(root)
}

/// NTFS default. Phase 5 may probe per volume.
#[cfg(target_os = "windows")]
pub fn case_insensitive_fs(_root: &Path) -> bool {
    true
}

/// ext4 default.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn case_insensitive_fs(_root: &Path) -> bool {
    false
}

/// Paths in `rels` that cannot be checked out on this device, with the reason.
///
/// A name [`name_representable_here`] rejects is included. When `case_fold`
/// is set, every case-collision member except the first in byte-wise order is
/// included too. A name rejection wins over the collision wording. Order is
/// byte-wise by path.
pub fn unrepresentable<'a>(
    rels: impl IntoIterator<Item = &'a str>,
    case_fold: bool,
) -> Vec<(String, String)> {
    let paths: Vec<&str> = rels.into_iter().collect();
    let mut reasons: BTreeMap<&str, String> = BTreeMap::new();
    for path in &paths {
        if name_representable_here(path) {
            continue;
        }
        let reason = crate::portable::name_issue(path)
            .map(|issue| format!("name cannot exist on Windows: {}", issue.reason))
            .unwrap_or_else(|| "name cannot exist on this system".to_string());
        reasons.insert(*path, reason);
    }
    if case_fold {
        let mut groups: BTreeMap<String, Vec<&str>> = BTreeMap::new();
        for path in &paths {
            groups
                .entry((*path).to_lowercase())
                .or_default()
                .push(*path);
        }
        for mut group in groups.into_values() {
            group.sort_unstable();
            group.dedup();
            if group.len() < 2 {
                continue;
            }
            let winner = group[0];
            for loser in group.iter().skip(1) {
                reasons
                    .entry(*loser)
                    .or_insert_with(|| format!("differs only by case from {winner}"));
            }
        }
    }
    reasons
        .into_iter()
        .map(|(path, reason)| (path.to_string(), reason))
        .collect()
}

#[cfg(test)]
thread_local! {
    static REPRESENTABLE: std::cell::Cell<Option<fn(&str) -> bool>> =
        const { std::cell::Cell::new(None) };
}

#[cfg(test)]
fn representable_override(rel: &str) -> Option<bool> {
    REPRESENTABLE.with(|slot| slot.get().map(|rule| rule(rel)))
}

#[cfg(not(test))]
fn representable_override(_rel: &str) -> Option<bool> {
    None
}

/// Test seam: `rule` replaces [`name_representable_here`] for this thread.
/// Windows rules are [`windows_names`].
#[cfg(test)]
pub fn with_representable_here<T>(rule: fn(&str) -> bool, body: impl FnOnce() -> T) -> T {
    REPRESENTABLE.with(|slot| slot.set(Some(rule)));
    let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));
    REPRESENTABLE.with(|slot| slot.set(None));
    match out {
        Ok(value) => value,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

/// [`name_representable_here`] as Windows will answer: [`crate::portable::name_issue`].
#[cfg(test)]
pub fn windows_names(rel: &str) -> bool {
    crate::portable::name_issue(rel).is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "macos")]
    fn macos_accepts_a_colon() {
        assert!(name_representable_here("a:b.md"));
        assert!(name_representable_here("CON.md"));
        assert!(name_representable_here("ok.md"));
    }

    #[test]
    fn the_windows_seam_rejects_a_colon_and_then_lifts() {
        with_representable_here(windows_names, || {
            assert!(!name_representable_here("a:b.md"));
            assert!(name_representable_here("ok.md"));
            assert_eq!(
                unrepresentable(["ok.md", "a:b.md"], false),
                vec![(
                    "a:b.md".to_string(),
                    "name cannot exist on Windows: contains ':'".to_string()
                )]
            );
        });
        #[cfg(target_os = "macos")]
        assert!(name_representable_here("a:b.md"));
    }

    #[test]
    fn case_fold_blocks_only_the_byte_wise_loser() {
        assert_eq!(
            unrepresentable(["a.md", "A.md", "ok.md"], true),
            vec![(
                "a.md".to_string(),
                "differs only by case from A.md".to_string()
            )]
        );
        assert!(unrepresentable(["a.md", "A.md", "ok.md"], false).is_empty());
    }

    #[test]
    fn a_name_rejection_wins_over_the_collision_wording() {
        with_representable_here(windows_names, || {
            let got = unrepresentable(["A:B", "a:b"], true);
            assert_eq!(got.len(), 2, "{got:?}");
            assert!(got
                .iter()
                .all(|(_, reason)| reason.contains("contains ':'")));
        });
    }

    #[test]
    fn case_probe_accepts_a_directory() {
        let dir = tempfile::TempDir::new().unwrap();
        let _ = case_insensitive_fs(dir.path());
    }
}
