//! Read-only IPC commands. Path input from the webview is untrusted.
//!
//! Engine-touching commands run in `tauri::async_runtime::spawn_blocking` and
//! never hold `config::lock` across an `.await`.

use std::path::{Path, PathBuf};
use std::sync::PoisonError;

use anyhow::{bail, Context, Result};
use serde::Serialize;
use tauri::State;

use dotlore_core::cloud;
use dotlore_core::config;
use dotlore_core::daemon::Cmd;
use dotlore_core::engine::{ConflictView, RootStatus};
use dotlore_core::git;

use crate::state::{load_cfg, AppState, RootRow};

/// Files larger than this are not loaded into the webview.
const MAX_PREVIEW_BYTES: u64 = 2 * 1024 * 1024;

/// Bytes of a tracked file, or a reason the webview should not render them.
#[derive(Serialize, Clone, Debug)]
pub struct FileContent {
    pub text: Option<String>,
    pub binary: bool,
    pub too_large: bool,
    pub bytes_len: usize,
}

/// Resolve `rel` against a tracked root. Rejects paths (and symlinks) that
/// escape the root. A `Kind::File` root is the file itself.
fn resolve_in_root(root: &config::Root, rel: &str) -> Result<PathBuf> {
    // A Kind::File root IS the file; staging calls its single entry
    // "content" but the live path is the root itself (repo.rs:748-758).
    if root.kind == cloud::Kind::File {
        let want = root.path.file_name().context("file root has no name")?;
        if rel != want {
            bail!("unknown entry {rel} for a file root");
        }
        return Ok(root.path.clone());
    }
    let real = std::fs::canonicalize(root.path.join(rel))?; // resolves symlinks
    let base = std::fs::canonicalize(&root.path)?;
    if !real.starts_with(&base) {
        bail!("path escapes the tracked root");
    }
    Ok(real)
}

#[tauri::command]
pub fn list_roots(state: State<'_, AppState>) -> Result<Vec<RootRow>, String> {
    let cfg = load_cfg(&state.home)?;
    Ok(cfg
        .roots
        .iter()
        .map(|root| RootRow::from_root(root, &state.home_dir, RootStatus::Pending))
        .collect())
}

#[tauri::command]
pub async fn tracked_files(
    state: State<'_, AppState>,
    slug: String,
) -> Result<Vec<String>, String> {
    let engine = state.shared_engine()?;
    tauri::async_runtime::spawn_blocking(move || {
        engine
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .tracked_files(&slug)
            .map_err(|e| format!("{e:#}"))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn read_file(
    state: State<'_, AppState>,
    slug: String,
    rel: String,
) -> Result<FileContent, String> {
    let home = state.home.clone();
    tauri::async_runtime::spawn_blocking(move || read_tracked_file(&home, &slug, &rel))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn conflicts(
    state: State<'_, AppState>,
    slug: String,
) -> Result<Vec<ConflictView>, String> {
    let engine = state.shared_engine()?;
    tauri::async_runtime::spawn_blocking(move || {
        engine
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .conflicts(&slug)
            .map_err(|e| format!("{e:#}"))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn provider_dir(state: State<'_, AppState>) -> Result<Option<String>, String> {
    let cfg = load_cfg(&state.home)?;
    Ok(cfg.provider_dir.map(|p| p.to_string_lossy().into_owned()))
}

#[tauri::command]
pub fn git_missing() -> bool {
    git::which_git().is_none()
}

#[tauri::command]
pub fn sync_now(state: State<'_, AppState>) -> Result<(), String> {
    state.send(Cmd::SyncNow)
}

fn read_tracked_file(home: &Path, slug: &str, rel: &str) -> Result<FileContent, String> {
    let cfg = load_cfg(home)?;
    let root = cfg
        .roots
        .iter()
        .find(|r| r.slug == slug)
        .ok_or_else(|| format!("unknown root {slug}"))?;
    let path = resolve_in_root(root, rel).map_err(|e| format!("{e:#}"))?;
    let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
    let bytes_len = usize::try_from(meta.len()).unwrap_or(usize::MAX);
    if meta.len() > MAX_PREVIEW_BYTES {
        return Ok(FileContent {
            text: None,
            binary: false,
            too_large: true,
            bytes_len,
        });
    }
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    let bytes_len = bytes.len();
    // Core has no `mirror::lossy_to_string`; valid UTF-8 is taken as-is.
    match String::from_utf8(bytes) {
        Ok(text) => Ok(FileContent {
            text: Some(text),
            binary: false,
            too_large: false,
            bytes_len,
        }),
        Err(_) => Ok(FileContent {
            text: None,
            binary: true,
            too_large: false,
            bytes_len,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;

    use tempfile::TempDir;

    fn dir_root(path: PathBuf) -> config::Root {
        config::Root {
            slug: "test".into(),
            path,
            kind: cloud::Kind::Dir,
            initializing: false,
        }
    }

    fn file_root(path: PathBuf) -> config::Root {
        config::Root {
            slug: "test".into(),
            path,
            kind: cloud::Kind::File,
            initializing: false,
        }
    }

    #[test]
    fn resolve_in_root_rejects_paths_escaping_the_root() {
        let tmp = TempDir::new().unwrap();
        let root_dir = tmp.path().join("root");
        fs::create_dir(&root_dir).unwrap();
        fs::write(root_dir.join("inside.txt"), "ok").unwrap();

        let secret = tmp.path().join("secret");
        fs::write(&secret, "nope").unwrap();

        let link = root_dir.join("escape");
        symlink(&secret, &link).unwrap();

        let root = dir_root(root_dir);
        assert!(
            resolve_in_root(&root, "../secret").is_err(),
            "../secret must not escape the tracked root"
        );
        assert!(
            resolve_in_root(&root, "escape").is_err(),
            "a symlink pointing outside the root must be rejected"
        );
    }

    #[test]
    fn resolve_in_root_maps_a_file_root_to_itself() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("CLAUDE.md");
        fs::write(&path, "hi").unwrap();
        let root = file_root(path.clone());

        assert_eq!(resolve_in_root(&root, "CLAUDE.md").unwrap(), path);
        assert!(
            resolve_in_root(&root, "other").is_err(),
            "a file root only has one live entry"
        );
        assert!(
            resolve_in_root(&root, "content").is_err(),
            "staging's 'content' name is not a live path"
        );
    }
}
