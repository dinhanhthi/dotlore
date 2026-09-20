//! IPC commands. Path input from the webview is untrusted.
//!
//! Engine-touching commands run in `tauri::async_runtime::spawn_blocking` and
//! never hold `config::lock` across an `.await`. Write commands emit a fresh
//! `dotlore://status` payload and send [`Cmd::Reload`]; they do not return
//! state alongside the result.

use std::path::{Path, PathBuf};
use std::sync::PoisonError;

use anyhow::{bail, Context, Result};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use dotlore_core::cloud;
use dotlore_core::config;
use dotlore_core::daemon::Cmd;
use dotlore_core::engine::{self, ConflictView, RootStatus};
use dotlore_core::git;

use crate::login_item;
use crate::state::{load_cfg, AppState, RootRow, StatusPayload};
use crate::tray::one_line;

/// Inside `~`, where iCloud Drive's Documents folder lives.
const ICLOUD: &str = "Library/Mobile Documents/com~apple~CloudDocs";
/// Inside `~`, where the Google Drive client mounts each account.
const CLOUD_STORAGE: &str = "Library/CloudStorage";

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

#[tauri::command]
pub async fn set_provider(
    app: AppHandle,
    state: State<'_, AppState>,
    dir: String,
) -> Result<(), String> {
    let home = state.home.clone();
    let home_dir = state.home_dir.clone();
    // Engine mutex first when a runtime exists, then the home lock inside
    // `configure_provider` — the order a daemon cycle takes them in.
    let existing = state
        .engine
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _serialized = existing
            .as_ref()
            .map(|e| e.lock().unwrap_or_else(PoisonError::into_inner));
        engine::configure_provider(&home, &home_dir, Path::new(&dir)).map_err(front_err)
    })
    .await
    .map_err(front_msg)??;

    // First run has no engine: `Engine::new` bails without a provider, so
    // setup's `start_runtime` was a no-op. Reload alone would go nowhere.
    state.start_runtime(&app);
    notify(&app, &state)
}

#[tauri::command]
pub async fn add_root(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
    slug: Option<String>,
) -> Result<String, String> {
    let engine = state.shared_engine().map_err(front_msg)?;
    let slug = tauri::async_runtime::spawn_blocking(move || {
        engine
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .add_root(Path::new(&path), slug.as_deref())
            .map_err(front_err)
    })
    .await
    .map_err(front_msg)??;
    notify(&app, &state)?;
    Ok(slug)
}

#[tauri::command]
pub async fn link_root(
    app: AppHandle,
    state: State<'_, AppState>,
    slug: String,
    path: String,
) -> Result<(), String> {
    let engine = state.shared_engine().map_err(front_msg)?;
    tauri::async_runtime::spawn_blocking(move || {
        engine
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .link_root(&slug, Path::new(&path))
            .map_err(front_err)
    })
    .await
    .map_err(front_msg)??;
    notify(&app, &state)
}

#[tauri::command]
pub async fn remove_root(
    app: AppHandle,
    state: State<'_, AppState>,
    slug: String,
) -> Result<(), String> {
    let engine = state.shared_engine().map_err(front_msg)?;
    tauri::async_runtime::spawn_blocking(move || {
        engine
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove_root(&slug)
            .map_err(front_err)
    })
    .await
    .map_err(front_msg)??;
    notify(&app, &state)
}

#[tauri::command]
pub async fn recover_root(
    app: AppHandle,
    state: State<'_, AppState>,
    slug: String,
) -> Result<(), String> {
    let engine = state.shared_engine().map_err(front_msg)?;
    tauri::async_runtime::spawn_blocking(move || {
        engine
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .recover_root(&slug)
            .map_err(front_err)
    })
    .await
    .map_err(front_msg)??;
    notify(&app, &state)
}

#[tauri::command]
pub async fn list_linkable(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let engine = state.shared_engine().map_err(front_msg)?;
    tauri::async_runtime::spawn_blocking(move || {
        let e = engine.lock().unwrap_or_else(PoisonError::into_inner);
        let tracked: Vec<&str> = e.cfg.roots.iter().map(|r| r.slug.as_str()).collect();
        Ok(e.cloud
            .list_slugs()
            .into_iter()
            .filter(|s| !tracked.contains(&s.as_str()))
            .collect::<Vec<_>>())
    })
    .await
    .map_err(front_msg)?
}

#[tauri::command]
pub fn list_gdrive_mounts(state: State<'_, AppState>) -> Vec<String> {
    google_drive_dirs(&state.home_dir)
        .into_iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect()
}

#[tauri::command]
pub fn icloud_dir(state: State<'_, AppState>) -> String {
    state.home_dir.join(ICLOUD).to_string_lossy().into_owned()
}

#[tauri::command]
pub fn login_item_enabled(state: State<'_, AppState>) -> bool {
    login_item::is_enabled(&state.home_dir)
}

#[tauri::command]
pub fn set_login_item(state: State<'_, AppState>, on: bool) -> Result<(), String> {
    login_item::set(&state.home_dir, on).map_err(front_err)
}

/// Emit the current config as status and ask the daemon to reload.
fn notify(app: &AppHandle, state: &AppState) -> Result<(), String> {
    let _ = app.emit("dotlore://status", status_payload(state));
    state.send(Cmd::Reload).map_err(front_msg)
}

fn status_payload(state: &AppState) -> StatusPayload {
    match load_cfg(&state.home) {
        Ok(cfg) => StatusPayload {
            roots: cfg
                .roots
                .iter()
                .map(|root| RootRow::from_root(root, &state.home_dir, RootStatus::Pending))
                .collect(),
            error: None,
        },
        Err(e) => StatusPayload {
            roots: Vec::new(),
            error: Some(one_line(&e)),
        },
    }
}

/// Google Drive account mounts under `~/Library/CloudStorage`.
fn google_drive_dirs(home_dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(home_dir.join(CLOUD_STORAGE))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() && name_of(p).starts_with("GoogleDrive-"))
        .collect();
    out.sort();
    out
}

fn name_of(p: &Path) -> String {
    p.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string()
}

fn front_err(e: anyhow::Error) -> String {
    one_line(&format!("{e:#}"))
}

fn front_msg(e: impl std::fmt::Display) -> String {
    one_line(&e.to_string())
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
