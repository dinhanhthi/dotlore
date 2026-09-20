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
use dotlore_core::engine::{
    self, ConflictView, ResolutionSnapshot, ResolveOutcome, RootStatus, SiblingView,
};
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

/// One side of an open conflict, with no blob ids or raw bytes.
#[derive(Serialize, Clone, Debug)]
pub struct SiblingDto {
    pub path: String,
    pub device_name: String,
    pub is_me: bool,
    pub text: Option<String>,
    pub bytes_len: usize,
}

/// What the webview needs to draw a resolver. The matching
/// [`ResolutionSnapshot`] stays in [`AppState::snapshots`].
#[derive(Serialize, Clone, Debug)]
pub struct ResolutionDto {
    pub slug: String,
    pub live: String,
    pub live_text: Option<String>,
    pub binary: bool,
    pub live_bytes_len: usize,
    pub siblings: Vec<SiblingDto>,
}

/// Outcome of a save. The snapshot never rides along — Stale carries a
/// refreshed [`ResolutionDto`] built from the new snapshot in the registry.
#[derive(Serialize, Clone, Debug)]
#[serde(tag = "outcome", rename_all = "lowercase")]
pub enum ResolveResultDto {
    Applied,
    Stale { refreshed: ResolutionDto },
    Pending,
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
pub async fn open_resolution(
    state: State<'_, AppState>,
    slug: String,
    rel: String,
) -> Result<ResolutionDto, String> {
    let engine = state.shared_engine().map_err(front_msg)?;
    let slug_key = slug.clone();
    let rel_key = rel.clone();
    let (snap, me) = tauri::async_runtime::spawn_blocking(move || {
        let mut e = engine.lock().unwrap_or_else(PoisonError::into_inner);
        let snap = e
            .open_resolution(&slug, Path::new(&rel))
            .map_err(front_err)?;
        let me = e.cfg.id8().to_string();
        Ok::<_, String>((snap, me))
    })
    .await
    .map_err(front_msg)??;
    let dto = resolution_dto(&snap, &me);
    state.open_snapshot(&slug_key, &rel_key, snap);
    Ok(dto)
}

#[tauri::command]
pub fn close_resolution(state: State<'_, AppState>, slug: String, rel: String) {
    state.close_snapshot(&slug, &rel);
}

/// `discard_siblings` is the list of sibling paths that are **deleted**,
/// not kept. Same polarity as `conflict::resolve` / `Engine::resolve_conflict`.
/// Inverting this deletes the user's file.
#[tauri::command]
pub async fn resolve_conflict(
    app: AppHandle,
    state: State<'_, AppState>,
    slug: String,
    rel: String,
    discard_siblings: Vec<String>,
    content: String,
) -> Result<ResolveResultDto, String> {
    let snap = state
        .get_snapshot(&slug, &rel)
        .ok_or_else(|| "open the conflict first".to_string())?;
    let discarded: Vec<PathBuf> = discard_siblings.into_iter().map(PathBuf::from).collect();
    apply_resolution(
        &app,
        &state,
        slug,
        rel,
        snap,
        discarded,
        content.into_bytes(),
    )
    .await
}

/// Keep one side of a binary conflict from the held snapshot — never from
/// the webview. Every sibling of that live path is discarded, same as
/// `dotlore resolve`.
#[tauri::command]
pub async fn resolve_binary(
    app: AppHandle,
    state: State<'_, AppState>,
    slug: String,
    rel: String,
    keep: String,
    sibling: Option<String>,
) -> Result<ResolveResultDto, String> {
    let snap = state
        .get_snapshot(&slug, &rel)
        .ok_or_else(|| "open the conflict first".to_string())?;
    let content = binary_content(&snap, &keep, sibling.as_deref())?;
    // Resolving a live path clears every sibling of that path; the chosen
    // content is the one survivor.
    let discarded: Vec<PathBuf> = snap.siblings.iter().map(|s| s.path.clone()).collect();
    apply_resolution(&app, &state, slug, rel, snap, discarded, content).await
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

/// Run `Engine::resolve_conflict` on a blocking thread, then map
/// [`ResolveOutcome`]. Errors leave the snapshot (and the draft) in place.
async fn apply_resolution(
    app: &AppHandle,
    state: &AppState,
    slug: String,
    rel: String,
    snap: ResolutionSnapshot,
    discard_siblings: Vec<PathBuf>,
    content: Vec<u8>,
) -> Result<ResolveResultDto, String> {
    let engine = state.shared_engine().map_err(front_msg)?;
    let slug_key = slug.clone();
    let rel_key = rel.clone();
    let (outcome, me) = tauri::async_runtime::spawn_blocking(move || {
        let mut e = engine.lock().unwrap_or_else(PoisonError::into_inner);
        let me = e.cfg.id8().to_string();
        let outcome = e
            .resolve_conflict(&slug, &snap, &discard_siblings, &content)
            .map_err(front_err)?;
        Ok::<_, String>((outcome, me))
    })
    .await
    .map_err(front_msg)??;

    match outcome {
        ResolveOutcome::Applied(_) => {
            state.close_snapshot(&slug_key, &rel_key);
            notify(app, state)?;
            Ok(ResolveResultDto::Applied)
        }
        ResolveOutcome::Stale(fresh) => {
            let refreshed = resolution_dto(&fresh, &me);
            state.replace_snapshot(&slug_key, &rel_key, *fresh);
            Ok(ResolveResultDto::Stale { refreshed })
        }
        ResolveOutcome::Pending => Ok(ResolveResultDto::Pending),
    }
}

/// Bytes of the chosen side, taken from the held snapshot.
fn binary_content(
    snap: &ResolutionSnapshot,
    keep: &str,
    sibling: Option<&str>,
) -> Result<Vec<u8>, String> {
    match (keep, sibling) {
        ("live", None) => Ok(snap.live_bytes.clone()),
        ("live", Some(_)) => Err(front_msg("sibling only applies with keep other")),
        ("other", s) => Ok(pick_other(snap, s)?.bytes.clone()),
        (other, _) => Err(front_msg(format!(
            "keep must be live or other, not {other}"
        ))),
    }
}

fn pick_other<'a>(
    snap: &'a ResolutionSnapshot,
    sibling: Option<&str>,
) -> Result<&'a SiblingView, String> {
    match (sibling, snap.siblings.as_slice()) {
        (None, []) => Err(front_msg(format!(
            "{} has no conflicting sibling",
            snap.live.display()
        ))),
        (None, [one]) => Ok(one),
        (None, several) => Err(front_msg(format!(
            "keep other is ambiguous, pass sibling: {}",
            several
                .iter()
                .map(|s| s.path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
        (Some(p), _) => snap
            .siblings
            .iter()
            .find(|s| s.path == Path::new(p))
            .ok_or_else(|| front_msg(format!("{p} is not a sibling of {}", snap.live.display()))),
    }
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

/// Valid UTF-8 on every side, or the whole resolution is binary.
fn binary_mode(snap: &ResolutionSnapshot) -> bool {
    std::str::from_utf8(&snap.live_bytes).is_err()
        || snap
            .siblings
            .iter()
            .any(|s| std::str::from_utf8(&s.bytes).is_err())
}

fn utf8_text(bytes: &[u8], binary: bool) -> Option<String> {
    if binary {
        return None;
    }
    String::from_utf8(bytes.to_vec()).ok()
}

fn sibling_dto(view: &SiblingView, me: &str, binary: bool) -> SiblingDto {
    SiblingDto {
        path: view.path.to_string_lossy().into_owned(),
        device_name: view.loser_name.clone(),
        is_me: view.loser_id8 == me,
        text: utf8_text(&view.bytes, binary),
        bytes_len: view.bytes.len(),
    }
}

pub(crate) fn resolution_dto(snap: &ResolutionSnapshot, me: &str) -> ResolutionDto {
    let binary = binary_mode(snap);
    ResolutionDto {
        slug: snap.slug.clone(),
        live: snap.live.to_string_lossy().into_owned(),
        live_text: utf8_text(&snap.live_bytes, binary),
        binary,
        live_bytes_len: snap.live_bytes.len(),
        siblings: snap
            .siblings
            .iter()
            .map(|s| sibling_dto(s, me, binary))
            .collect(),
    }
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
