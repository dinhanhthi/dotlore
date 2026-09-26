//! IPC commands. Path input from the webview is untrusted.
//!
//! A command that takes the home lock or the engine mutex runs in
//! `tauri::async_runtime::spawn_blocking` and never holds `config::lock`
//! across an `.await`. The home lock is the easy one to miss: `load_cfg`
//! alone touches no engine, but a daemon cycle holds that lock for the whole
//! of `Engine::sync_all`, and a blocking command waits for it on the main
//! thread, which freezes the window. Write commands emit a fresh
//! `dotlore://status` payload and send [`Cmd::Reload`]; they do not return
//! state alongside the result.

use std::ffi::{CString, OsStr, OsString};
use std::fs::File;
use std::io::{self, ErrorKind};
use std::os::raw::c_char as libc_c_char;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd};
use std::path::{Component, Path, PathBuf};
use std::sync::PoisonError;

use anyhow::{bail, Result};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::config;
use crate::daemon::{Cmd, SharedEngine};
use crate::engine::{
    self, AddRootStart, ConflictView, Engine, EntryKind, EntryView, ImportAgentsReport,
    InspectedEntry, ResolutionSnapshot, ResolveOutcome, RootStatus, SiblingView, TrackOutcome,
    TrackedFile, WipeReport,
};
use crate::git;
use crate::project;

use crate::login_item;
use crate::state::{load_cfg, AppState, RootRow, StatusPayload};
use crate::tray::one_line;
use crate::updater;

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

/// A cloud slug this device does not yet track.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct LinkableRow {
    pub slug: String,
    pub display_name: String,
    pub is_agent: bool,
}

/// One immediate child of a project folder, for the root-scoped picker.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct PickerRow {
    pub name: String,
    pub kind: String,
    pub rel: String,
}

/// Preview of a path the user may add.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct InspectedEntryDto {
    pub kind: EntryKind,
    pub bytes: u64,
    pub folder_limit: u64,
    pub confirmation_required: bool,
    pub skipped_too_large: Vec<SkippedFileDto>,
}

/// One file excluded from a folder measurement by the per-file limit.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct SkippedFileDto {
    pub rel: String,
    pub bytes: u64,
}

/// What [`import_installed_agents`] added, and the homes it could not.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct ImportAgentsDto {
    pub added: Vec<String>,
    pub failed: Vec<ImportAgentFailureDto>,
}

/// One catalog home [`import_installed_agents`] could not add.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct ImportAgentFailureDto {
    pub path: String,
    pub message: String,
}

/// What [`wipe_cloud_data`] re-added, and the slugs it could not.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct WipeReportDto {
    pub readded: Vec<String>,
    pub failed: Vec<WipeFailureDto>,
}

/// One slug [`wipe_cloud_data`] could not re-add.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct WipeFailureDto {
    pub slug: String,
    pub error: String,
}

/// Result of [`track_entry`]. Confirmation does not mutate.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum TrackResultDto {
    Done,
    NeedsConfirmation {
        bytes: u64,
        folder_limit: u64,
        confirmation_required: bool,
        skipped_too_large: Vec<SkippedFileDto>,
    },
}

/// One include-list catalog. Named apart from [`engine::PatternCatalog`], which
/// does not implement [`Serialize`].
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct PatternCatalogDto {
    pub id: String,
    pub label: String,
    pub lines: Vec<String>,
}

/// Resolve `rel` against a tracked root. Rejects paths (and symlinks) that
/// escape the root.
fn resolve_in_root(root: &config::Root, rel: &str) -> Result<PathBuf> {
    let real = std::fs::canonicalize(root.path.join(rel))?; // resolves symlinks
    let base = std::fs::canonicalize(&root.path)?;
    if !real.starts_with(&base) {
        bail!("path escapes the tracked root");
    }
    Ok(real)
}

/// Off the main thread, like every command that takes the home lock or the
/// engine mutex: `load_cfg` blocks on the home lock, a daemon cycle holds that
/// lock for the whole of [`Engine::sync_all`], and a `flock` on the main
/// thread freezes the window.
#[tauri::command]
pub async fn list_roots(state: State<'_, AppState>) -> Result<Vec<RootRow>, String> {
    let home = state.home.clone();
    let home_dir = state.home_dir.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let cfg = load_cfg(&home)?;
        Ok(cfg
            .roots
            .iter()
            .map(|root| RootRow::from_root(root, &home_dir, RootStatus::Pending))
            .collect())
    })
    .await
    .map_err(front_msg)?
}

#[tauri::command]
pub async fn tracked_files(
    state: State<'_, AppState>,
    slug: String,
) -> Result<Vec<TrackedFile>, String> {
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
/// resolving a text conflict.
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

/// Off the main thread for the same reason as [`list_roots`].
#[tauri::command]
pub async fn provider_dir(state: State<'_, AppState>) -> Result<Option<String>, String> {
    let home = state.home.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let cfg = load_cfg(&home)?;
        Ok(cfg.provider_dir.map(|p| p.to_string_lossy().into_owned()))
    })
    .await
    .map_err(front_msg)?
}

/// Open the provider's `dotlore` folder in Finder, or the provider itself
/// before the first publish has created it. AppKit opens it directly: no
/// child process, so `open(1)` stays off the list of programs this app runs.
#[tauri::command]
pub async fn open_cloud_folder(state: State<'_, AppState>) -> Result<(), String> {
    let home = state.home.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let Some(provider) = load_cfg(&home)?.provider_dir else {
            return Err("No cloud folder set".to_string());
        };
        open_folder(&cloud_folder_target(&provider))
    })
    .await
    .map_err(front_msg)?
}

/// The state dir this app runs on (`DOTLORE_HOME`), for Settings to show.
#[tauri::command]
pub fn app_home(state: State<'_, AppState>) -> String {
    state.home.to_string_lossy().into_owned()
}

/// Open the state dir in Finder the same way as [`open_cloud_folder`].
#[tauri::command]
pub fn open_app_home(state: State<'_, AppState>) -> Result<(), String> {
    open_folder(&state.home)
}

fn cloud_folder_target(provider: &Path) -> PathBuf {
    let dotlore = engine::cloud_folder(provider);
    if dotlore.is_dir() {
        dotlore
    } else {
        provider.to_path_buf()
    }
}

fn open_folder(dir: &Path) -> Result<(), String> {
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::{NSString, NSURL};

    let url = NSURL::fileURLWithPath_isDirectory(&NSString::from_str(&dir.to_string_lossy()), true);
    if NSWorkspace::sharedWorkspace().openURL(&url) {
        Ok(())
    } else {
        Err(format!("Could not open {}", dir.display()))
    }
}

/// `async` so the main thread never waits on it: [`git::which_git`] spawns
/// `git --version`. No `spawn_blocking` and no `Result` — a child process is
/// milliseconds, not the open-ended wait the home lock can be, and there is
/// no join error to report through a bare `bool`.
#[tauri::command(async)]
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
    let engine_for_begin = engine.clone();
    let started = tauri::async_runtime::spawn_blocking(move || {
        engine_for_begin
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .begin_add_root(Path::new(&path), slug.as_deref())
            .map_err(front_err)
    })
    .await
    .map_err(front_msg)??;

    let slug = match started {
        AddRootStart::Done(report) => report.slug,
        AddRootStart::Seed(seed) => {
            // Pattern walk and the first copy stay off the engine mutex, on a
            // utility-priority thread, so the window can keep using other
            // projects while this folder is copied.
            let slug_for_cancel = seed.slug.clone();
            let (tx, rx) = std::sync::mpsc::channel();
            if let Err(err) = std::thread::Builder::new()
                .name("dotlore-seed".into())
                .spawn(move || {
                    background_disk();
                    let materialized = seed.materialize().map_err(front_err);
                    let _ = tx.send((seed, materialized));
                })
            {
                cancel_add(&engine, &slug_for_cancel).await;
                return Err(front_msg(err));
            }
            let seeded = tauri::async_runtime::spawn_blocking(move || rx.recv()).await;
            let (seed, materialized) = match seeded {
                Ok(Ok(pair)) => pair,
                Ok(Err(err)) => {
                    cancel_add(&engine, &slug_for_cancel).await;
                    return Err(front_msg(err));
                }
                Err(err) => {
                    cancel_add(&engine, &slug_for_cancel).await;
                    return Err(front_msg(err));
                }
            };
            if let Err(err) = materialized {
                cancel_add(&engine, &seed.slug).await;
                return Err(err);
            }
            let engine_for_finish = engine.clone();
            let slug_for_finish = seed.slug.clone();
            let finished = tauri::async_runtime::spawn_blocking(move || {
                let mut guard = engine_for_finish
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                let slug = seed.slug.clone();
                match guard.complete_add_root(&seed) {
                    Ok(()) => Ok(slug),
                    Err(err) => {
                        guard.cancel_add_root(&slug);
                        Err(front_err(err))
                    }
                }
            })
            .await;
            match finished {
                Ok(result) => result?,
                Err(err) => {
                    cancel_add(&engine, &slug_for_finish).await;
                    return Err(front_msg(err));
                }
            }
        }
    };
    notify(&app, &state)?;
    Ok(slug)
}

async fn cancel_add(engine: &SharedEngine, slug: &str) {
    let engine = engine.clone();
    let slug = slug.to_string();
    let _ = tauri::async_runtime::spawn_blocking(move || {
        engine
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .cancel_add_root(&slug);
    })
    .await;
}

#[tauri::command]
pub async fn import_installed_agents(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<ImportAgentsDto, String> {
    let engine = state.shared_engine().map_err(front_msg)?;
    let report = tauri::async_runtime::spawn_blocking(move || {
        engine
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .import_installed_agents()
            .map_err(front_err)
    })
    .await
    .map_err(front_msg)??;
    notify(&app, &state)?;
    Ok(import_agents_dto(report))
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
pub async fn wipe_cloud_data(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<WipeReportDto, String> {
    let engine = state.shared_engine().map_err(front_msg)?;
    let report = tauri::async_runtime::spawn_blocking(move || {
        engine
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .wipe_cloud_data()
            .map_err(front_err)
    })
    .await
    .map_err(front_msg)??;
    notify(&app, &state)?;
    Ok(wipe_report_dto(report))
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
pub async fn list_linkable(state: State<'_, AppState>) -> Result<Vec<LinkableRow>, String> {
    let engine = match state.shared_engine() {
        Ok(engine) => engine,
        Err(message) => {
            // The engine stays unset until a provider folder exists. That
            // empty cloud list is onboarding, not a failure. A configured
            // provider whose runtime never started is still an error.
            let home = state.home.clone();
            let configured = tauri::async_runtime::spawn_blocking(move || {
                load_cfg(&home).map(|cfg| cfg.provider_dir.is_some())
            })
            .await
            .map_err(front_msg)?;
            return list_linkable_unstarted(configured?, message);
        }
    };
    tauri::async_runtime::spawn_blocking(move || {
        let e = engine.lock().unwrap_or_else(PoisonError::into_inner);
        list_linkable_sync(&e)
    })
    .await
    .map_err(front_msg)?
}

#[tauri::command]
pub async fn list_entries(
    state: State<'_, AppState>,
    slug: String,
) -> Result<Vec<EntryView>, String> {
    let engine = state.shared_engine().map_err(front_msg)?;
    tauri::async_runtime::spawn_blocking(move || {
        engine
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .list_entries(&slug)
            .map_err(front_err)
    })
    .await
    .map_err(front_msg)?
}

#[tauri::command]
pub async fn list_entry_children(
    state: State<'_, AppState>,
    slug: String,
    rel: String,
) -> Result<Vec<PickerRow>, String> {
    let engine = state.shared_engine().map_err(front_msg)?;
    tauri::async_runtime::spawn_blocking(move || {
        let e = engine.lock().unwrap_or_else(PoisonError::into_inner);
        list_entry_children_sync(&e, &slug, &rel)
    })
    .await
    .map_err(front_msg)?
}

#[tauri::command]
pub async fn inspect_entry(
    state: State<'_, AppState>,
    slug: String,
    rel: String,
) -> Result<InspectedEntryDto, String> {
    let engine = state.shared_engine().map_err(front_msg)?;
    tauri::async_runtime::spawn_blocking(move || {
        let mut e = engine.lock().unwrap_or_else(PoisonError::into_inner);
        inspect_entry_sync(&mut e, &slug, &rel)
    })
    .await
    .map_err(front_msg)?
}

#[tauri::command]
pub async fn track_entry(
    app: AppHandle,
    state: State<'_, AppState>,
    slug: String,
    rel: String,
    confirmed_folder_bytes: Option<u64>,
) -> Result<TrackResultDto, String> {
    let engine = state.shared_engine().map_err(front_msg)?;
    let result = tauri::async_runtime::spawn_blocking(move || {
        let mut e = engine.lock().unwrap_or_else(PoisonError::into_inner);
        track_entry_sync(&mut e, &slug, &rel, confirmed_folder_bytes)
    })
    .await
    .map_err(front_msg)??;
    if matches!(result, TrackResultDto::Done) {
        notify(&app, &state)?;
    }
    Ok(result)
}

#[tauri::command]
pub async fn untrack_entry(
    app: AppHandle,
    state: State<'_, AppState>,
    slug: String,
    rel: String,
) -> Result<Vec<EntryView>, String> {
    let engine = state.shared_engine().map_err(front_msg)?;
    let remaining = tauri::async_runtime::spawn_blocking(move || {
        let mut e = engine.lock().unwrap_or_else(PoisonError::into_inner);
        untrack_entry_sync(&mut e, &slug, &rel)
    })
    .await
    .map_err(front_msg)??;
    notify(&app, &state)?;
    Ok(remaining)
}

#[tauri::command]
pub async fn default_patterns(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let engine = state.shared_engine().map_err(front_msg)?;
    tauri::async_runtime::spawn_blocking(move || {
        engine
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .default_patterns()
    })
    .await
    .map_err(front_msg)
}

#[tauri::command]
pub async fn set_default_patterns(
    app: AppHandle,
    state: State<'_, AppState>,
    patterns: Vec<String>,
) -> Result<(), String> {
    let engine = state.shared_engine().map_err(front_msg)?;
    tauri::async_runtime::spawn_blocking(move || {
        engine
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .set_default_patterns(patterns)
            .map_err(front_err)
    })
    .await
    .map_err(front_msg)??;
    notify(&app, &state)
}

#[tauri::command]
pub async fn pattern_catalogs(
    state: State<'_, AppState>,
) -> Result<Vec<PatternCatalogDto>, String> {
    let engine = state.shared_engine().map_err(front_msg)?;
    tauri::async_runtime::spawn_blocking(move || {
        engine
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pattern_catalogs()
            .into_iter()
            .map(|catalog| PatternCatalogDto {
                id: catalog.id,
                label: catalog.label,
                lines: catalog.lines,
            })
            .collect()
    })
    .await
    .map_err(front_msg)
}

#[tauri::command]
pub async fn set_pattern_catalog(
    app: AppHandle,
    state: State<'_, AppState>,
    catalog: String,
    patterns: Vec<String>,
) -> Result<(), String> {
    let engine = state.shared_engine().map_err(front_msg)?;
    tauri::async_runtime::spawn_blocking(move || {
        engine
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .set_pattern_catalog(&catalog, patterns)
            .map_err(front_err)
    })
    .await
    .map_err(front_msg)??;
    notify(&app, &state)
}

#[tauri::command]
pub async fn default_ignore(state: State<'_, AppState>) -> Result<String, String> {
    let engine = state.shared_engine().map_err(front_msg)?;
    tauri::async_runtime::spawn_blocking(move || {
        engine
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .default_ignore()
    })
    .await
    .map_err(front_msg)
}

#[tauri::command]
pub async fn set_default_ignore(
    app: AppHandle,
    state: State<'_, AppState>,
    ignore: String,
) -> Result<(), String> {
    let engine = state.shared_engine().map_err(front_msg)?;
    tauri::async_runtime::spawn_blocking(move || {
        engine
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .set_default_ignore(ignore)
            .map_err(front_err)
    })
    .await
    .map_err(front_msg)??;
    notify(&app, &state)
}

#[tauri::command]
pub async fn max_file_mb(state: State<'_, AppState>) -> Result<u64, String> {
    let engine = state.shared_engine().map_err(front_msg)?;
    tauri::async_runtime::spawn_blocking(move || {
        engine
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .max_file_mb()
    })
    .await
    .map_err(front_msg)
}

#[tauri::command]
pub async fn set_max_file_mb(
    app: AppHandle,
    state: State<'_, AppState>,
    mb: u64,
) -> Result<(), String> {
    let engine = state.shared_engine().map_err(front_msg)?;
    tauri::async_runtime::spawn_blocking(move || {
        engine
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .set_max_file_mb(mb)
            .map_err(front_err)
    })
    .await
    .map_err(front_msg)??;
    notify(&app, &state)
}

#[tauri::command]
pub async fn max_seed_folder_mb(state: State<'_, AppState>) -> Result<u64, String> {
    let engine = state.shared_engine().map_err(front_msg)?;
    tauri::async_runtime::spawn_blocking(move || {
        engine
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .max_seed_folder_mb()
    })
    .await
    .map_err(front_msg)
}

#[tauri::command]
pub async fn set_max_seed_folder_mb(
    app: AppHandle,
    state: State<'_, AppState>,
    mb: u64,
) -> Result<(), String> {
    let engine = state.shared_engine().map_err(front_msg)?;
    tauri::async_runtime::spawn_blocking(move || {
        engine
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .set_max_seed_folder_mb(mb)
            .map_err(front_err)
    })
    .await
    .map_err(front_msg)??;
    notify(&app, &state)
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

/// The version the last update check found, if any.
#[tauri::command]
pub fn update_available(app: AppHandle) -> Option<String> {
    updater::available_version(&app)
}

/// The title-bar badge: the same prompt the menu-bar row opens.
#[tauri::command]
pub fn prompt_update(app: AppHandle) {
    updater::prompt_available(&app);
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

fn import_agents_dto(report: ImportAgentsReport) -> ImportAgentsDto {
    ImportAgentsDto {
        added: report.added,
        failed: report
            .failed
            .into_iter()
            .map(|(path, message)| ImportAgentFailureDto {
                path: path.to_string_lossy().into_owned(),
                message,
            })
            .collect(),
    }
}

fn wipe_report_dto(report: WipeReport) -> WipeReportDto {
    WipeReportDto {
        readded: report.readded,
        failed: report
            .failed
            .into_iter()
            .map(|(slug, error)| WipeFailureDto { slug, error })
            .collect(),
    }
}

fn front_msg(e: impl std::fmt::Display) -> String {
    one_line(&e.to_string())
}

/// Drop this thread below the window so a pattern-file copy does not stall it.
///
/// Utility QoS is what `git` children spawned here inherit. The disk policy
/// applies to this thread's own reads and writes. Failures are ignored: the
/// copy still runs, only without the throttle.
fn background_disk() {
    #[cfg(target_os = "macos")]
    {
        const QOS_CLASS_UTILITY: u32 = 0x11;
        const IOPOL_TYPE_DISK: i32 = 0;
        const IOPOL_SCOPE_THREAD: i32 = 1;
        const IOPOL_UTILITY: i32 = 4;
        extern "C" {
            fn pthread_set_qos_class_self_np(qos_class: u32, relative_priority: i32) -> i32;
            fn setiopolicy_np(typ: i32, scope: i32, policy: i32) -> i32;
        }
        // SAFETY: both symbols live in libSystem. 0x11 is QOS_CLASS_UTILITY
        // and (0, 1, 4) is disk / this thread / utility, from the macOS SDK.
        unsafe {
            let _ = pthread_set_qos_class_self_np(QOS_CLASS_UTILITY, 0);
            let _ = setiopolicy_np(IOPOL_TYPE_DISK, IOPOL_SCOPE_THREAD, IOPOL_UTILITY);
        }
    }
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

fn require_plain_rel(rel: &str) -> Result<(), String> {
    let p = Path::new(rel);
    if rel.is_empty()
        || p.is_absolute()
        || !p.components().all(|c| matches!(c, Component::Normal(_)))
    {
        return Err(front_msg(format!("unsafe path {rel}")));
    }
    Ok(())
}

fn picker_rel_ok(rel: &str) -> bool {
    if rel.is_empty() {
        return true;
    }
    let p = Path::new(rel);
    !p.is_absolute() && p.components().all(|c| matches!(c, Component::Normal(_)))
}

fn skipped_dto(rows: &[(PathBuf, u64)]) -> Vec<SkippedFileDto> {
    rows.iter()
        .map(|(p, n)| SkippedFileDto {
            rel: p.to_string_lossy().into_owned(),
            bytes: *n,
        })
        .collect()
}

fn inspected_dto(info: &InspectedEntry) -> InspectedEntryDto {
    InspectedEntryDto {
        kind: info.kind,
        bytes: info.bytes,
        folder_limit: info.folder_limit,
        confirmation_required: info.confirmation_required,
        skipped_too_large: skipped_dto(&info.skipped_too_large),
    }
}

fn track_entry_sync(
    engine: &mut Engine,
    slug: &str,
    rel: &str,
    confirmed_folder_bytes: Option<u64>,
) -> Result<TrackResultDto, String> {
    require_plain_rel(rel)?;
    match engine
        .track_entry(slug, Path::new(rel), confirmed_folder_bytes)
        .map_err(front_err)?
    {
        TrackOutcome::Done(_) => Ok(TrackResultDto::Done),
        TrackOutcome::NeedsConfirmation(info) => Ok(TrackResultDto::NeedsConfirmation {
            bytes: info.bytes,
            folder_limit: info.folder_limit,
            confirmation_required: info.confirmation_required,
            skipped_too_large: skipped_dto(&info.skipped_too_large),
        }),
    }
}

fn inspect_entry_sync(
    engine: &mut Engine,
    slug: &str,
    rel: &str,
) -> Result<InspectedEntryDto, String> {
    require_plain_rel(rel)?;
    engine
        .inspect_entry(slug, Path::new(rel))
        .map(|info| inspected_dto(&info))
        .map_err(front_err)
}

fn untrack_entry_sync(
    engine: &mut Engine,
    slug: &str,
    rel: &str,
) -> Result<Vec<EntryView>, String> {
    require_plain_rel(rel)?;
    engine
        .untrack_entry(slug, Path::new(rel))
        .map_err(front_err)?;
    engine.list_entries(slug).map_err(front_err)
}

fn list_linkable_unstarted(
    provider_configured: bool,
    runtime_error: String,
) -> Result<Vec<LinkableRow>, String> {
    if provider_configured {
        Err(runtime_error)
    } else {
        Ok(Vec::new())
    }
}

fn list_linkable_sync(engine: &Engine) -> Result<Vec<LinkableRow>, String> {
    let tracked: Vec<&str> = engine.cfg.roots.iter().map(|r| r.slug.as_str()).collect();
    Ok(engine
        .cloud
        .list_slugs()
        .into_iter()
        .filter(|info| !tracked.contains(&info.slug.as_str()))
        .map(|info| LinkableRow {
            slug: info.slug,
            display_name: info.display_name,
            is_agent: info.is_agent,
        })
        .collect())
}

fn list_entry_children_sync(
    engine: &Engine,
    slug: &str,
    rel: &str,
) -> Result<Vec<PickerRow>, String> {
    let root = engine
        .cfg
        .roots
        .iter()
        .find(|r| r.slug == slug)
        .ok_or_else(|| front_msg(format!("unknown root {slug}")))?;
    list_children_in(&root.path, rel)
}

fn skip_picker_name(name: &str) -> bool {
    name == ".DS_Store" || project::staging_private(name)
}

fn list_children_in(root: &Path, rel: &str) -> Result<Vec<PickerRow>, String> {
    if !picker_rel_ok(rel) {
        return Err(front_msg(format!("unsafe path {rel}")));
    }
    let rel_path = Path::new(rel);
    let target = if rel.is_empty() {
        root.to_path_buf()
    } else {
        root.join(rel_path)
    };
    listing_replace_hook(&target);
    let dir = match open_dir_nofollow(root, rel_path) {
        Ok(d) => d,
        Err(e) if is_unsafe_open(&e) => return Err(front_msg(format!("unsafe path {rel}"))),
        Err(e) => return Err(front_msg(e)),
    };
    let names = {
        let clone = dir.try_clone().map_err(front_msg)?;
        match read_dir_fd(clone) {
            Ok(n) => n,
            Err(e) if is_unsafe_open(&e) => {
                return Err(front_msg(format!("unsafe path {rel}")));
            }
            Err(e) => return Err(front_msg(e)),
        }
    };
    let mut out = Vec::new();
    for name in names {
        let Some(name_str) = name.to_str().map(str::to_string) else {
            continue;
        };
        if skip_picker_name(&name_str) {
            continue;
        }
        let Some(kind) = child_kind(&dir, &name) else {
            continue;
        };
        let child_rel = if rel.is_empty() {
            name_str.clone()
        } else {
            format!("{rel}/{name_str}")
        };
        out.push(PickerRow {
            name: name_str,
            kind,
            rel: child_rel,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

fn child_kind(dir: &File, name: &OsStr) -> Option<String> {
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

// Darwin / Linux open(2) flags. Same values as mirror.rs — Mac-only product.
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

// `mirror` declares the same libc functions with its own opaque `DIR`.
// The two wrappers never exchange pointers; the layouts match.
#[cfg(target_os = "macos")]
#[allow(clashing_extern_declarations)]
extern "C" {
    fn close(fd: i32) -> i32;
    fn fdopendir(fd: i32) -> *mut DIR;
    fn readdir(dirp: *mut DIR) -> *mut Dirent;
    fn closedir(dirp: *mut DIR) -> i32;
}

fn is_unsafe_open(e: &io::Error) -> bool {
    matches!(e.raw_os_error(), Some(ELOOP) | Some(ENOTDIR)) || e.kind() == ErrorKind::InvalidInput
}

fn open_root_dir(root: &Path) -> io::Result<File> {
    let mut opts = std::fs::OpenOptions::new();
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

fn open_chain(root: &Path, rel: &Path) -> io::Result<File> {
    let mut fd = open_root_dir(root)?;
    for component in rel.components() {
        let name = match component {
            Component::Normal(n) => n,
            _ => {
                return Err(io::Error::new(
                    ErrorKind::InvalidInput,
                    "path is not a plain relative path",
                ))
            }
        };
        fd = openat_child(&fd, name, O_RDONLY | O_CLOEXEC | O_NOFOLLOW | O_DIRECTORY)?;
    }
    Ok(fd)
}

fn open_dir_nofollow(root: &Path, rel: &Path) -> io::Result<File> {
    if rel.as_os_str().is_empty() {
        open_root_dir(root)
    } else {
        open_chain(root, rel)
    }
}

#[cfg(target_os = "macos")]
fn read_dir_fd(dir: File) -> io::Result<Vec<OsString>> {
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
fn read_dir_fd(dir: File) -> io::Result<Vec<OsString>> {
    let listing = std::fs::read_dir(format!("/proc/self/fd/{}", dir.as_raw_fd()))?;
    let mut names = Vec::new();
    for entry in listing {
        names.push(entry?.file_name());
    }
    Ok(names)
}

#[cfg(test)]
thread_local! {
    static LISTING_REPLACE_AT: std::cell::RefCell<Option<Box<dyn Fn(&Path)>>> =
        const { std::cell::RefCell::new(None) };
}

fn listing_replace_hook(path: &Path) {
    #[cfg(test)]
    LISTING_REPLACE_AT.with(|c| {
        if let Some(f) = c.borrow().as_ref() {
            f(path);
        }
    });
    let _ = path;
}

#[cfg(test)]
fn with_listing_replace<T>(hook: impl Fn(&Path) + 'static, f: impl FnOnce() -> T) -> T {
    LISTING_REPLACE_AT.with(|c| *c.borrow_mut() = Some(Box::new(hook)));
    let out = f();
    LISTING_REPLACE_AT.with(|c| *c.borrow_mut() = None);
    out
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

    use crate::cloud::Manifest;
    use crate::config::Config;
    use crate::engine::Engine;
    use tempfile::TempDir;

    #[test]
    fn cloud_folder_target_opens_the_dotlore_folder_inside_the_provider() {
        let provider = TempDir::new().unwrap();
        let dotlore = provider.path().join("dotlore");
        fs::create_dir(&dotlore).unwrap();
        assert_eq!(
            cloud_folder_target(provider.path()),
            dotlore.canonicalize().unwrap()
        );
    }

    #[test]
    fn cloud_folder_target_falls_back_to_the_provider_before_the_first_publish() {
        let provider = TempDir::new().unwrap();
        assert_eq!(cloud_folder_target(provider.path()), provider.path());
    }

    fn dir_root(path: PathBuf) -> config::Root {
        config::Root {
            slug: "test".into(),
            path,
            initializing: false,
        }
    }

    struct Fx {
        _home: TempDir,
        root: TempDir,
        _provider: TempDir,
        engine: Engine,
        slug: String,
    }

    fn fixture() -> Fx {
        let home = TempDir::new().unwrap();
        let root = TempDir::new().unwrap();
        let provider = TempDir::new().unwrap();
        let cfg = Config {
            device_id: "a".repeat(32),
            device_name: "Mac A Pro".into(),
            provider_dir: Some(provider.path().to_path_buf()),
            roots: Vec::new(),
            default_patterns: Some(vec!["CLAUDE.md".into()]),
            default_ignore: None,
            ..Default::default()
        };
        cfg.save(home.path()).unwrap();
        let mut engine = Engine::new(home.path(), home.path(), cfg).unwrap();
        fs::write(root.path().join("CLAUDE.md"), b"one\n").unwrap();
        let slug = engine.add_root(root.path(), Some("proj")).unwrap().slug;
        Fx {
            _home: home,
            root,
            _provider: provider,
            engine,
            slug,
        }
    }

    fn write(fx: &Fx, rel: &str, body: &[u8]) {
        let p = fx.root.path().join(rel);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(p, body).unwrap();
    }

    fn fill(fx: &Fx, rel: &str, n: usize) {
        write(fx, rel, &vec![b'x'; n]);
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
    fn track_entry_rejects_a_path_outside_the_project() {
        let mut fx = fixture();
        let before = fx.engine.list_entries(&fx.slug).unwrap();
        assert!(
            track_entry_sync(&mut fx.engine, &fx.slug, "../secret", None).is_err(),
            "a relative escape must be rejected"
        );
        assert!(
            track_entry_sync(&mut fx.engine, &fx.slug, "/tmp/secret", None).is_err(),
            "an absolute path must be rejected"
        );
        assert_eq!(
            fx.engine.list_entries(&fx.slug).unwrap(),
            before,
            "a rejected track must not mutate the include-list"
        );
    }

    #[test]
    fn track_entry_rejects_a_file_over_the_max_file_size() {
        let mut fx = fixture();
        fx.engine.set_max_file_mb(1).unwrap();
        fill(&fx, "huge.bin", 1024 * 1024 + 1);
        let before = fx.engine.list_entries(&fx.slug).unwrap();
        let err = track_entry_sync(&mut fx.engine, &fx.slug, "huge.bin", None)
            .expect_err("over-limit file must be rejected");
        assert!(
            err.contains("bytes") || err.contains("limit") || err.contains("huge.bin"),
            "error should name the limit: {err}"
        );
        assert_eq!(
            fx.engine.list_entries(&fx.slug).unwrap(),
            before,
            "rejecting an over-limit file must not add it"
        );
    }

    #[test]
    fn entry_picker_cannot_list_above_root_or_follow_symlinks() {
        let fx = fixture();
        let outside = fx.root.path().parent().unwrap().join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret.txt"), "nope").unwrap();
        write(&fx, "sub/ok.txt", b"ok\n");
        symlink(&outside, fx.root.path().join("link")).unwrap();

        assert!(
            list_entry_children_sync(&fx.engine, &fx.slug, "..").is_err(),
            "`..` must not list above the project root"
        );
        assert!(
            list_entry_children_sync(&fx.engine, &fx.slug, "/").is_err(),
            "an absolute path must be rejected"
        );
        assert!(
            list_entry_children_sync(&fx.engine, &fx.slug, "../outside").is_err(),
            "a traversal must be rejected"
        );
        assert!(
            list_entry_children_sync(&fx.engine, &fx.slug, "link").is_err(),
            "listing through a symlink must be rejected"
        );

        let kids = list_entry_children_sync(&fx.engine, &fx.slug, "").unwrap();
        assert!(
            !kids
                .iter()
                .any(|k| k.name == ".." || k.name == "." || k.rel.contains("..")),
            "root listing must not return navigable ancestors: {kids:?}"
        );
        assert!(
            !kids.iter().any(|k| k.name == "link"),
            "a symlink child must not be returned as a candidate: {kids:?}"
        );
        assert!(
            kids.iter()
                .any(|k| k.name == "sub" && k.kind == "directory" && k.rel == "sub"),
            "the real subdirectory must be listed: {kids:?}"
        );

        let listed = with_listing_replace(
            {
                let outside = outside.clone();
                move |p| {
                    if p.file_name().and_then(|n| n.to_str()) == Some("sub") {
                        let _ = fs::remove_dir_all(p);
                        let _ = symlink(&outside, p);
                    }
                }
            },
            || list_entry_children_sync(&fx.engine, &fx.slug, "sub"),
        );
        assert!(
            listed.is_err(),
            "a directory replaced by a symlink before read must not be listed: {listed:?}"
        );
        if let Ok(rows) = &listed {
            assert!(
                !rows.iter().any(|k| k.name == "secret.txt"),
                "must not leak the planted symlink target: {rows:?}"
            );
        }
    }

    #[test]
    fn untrack_missing_explicit_entry_succeeds() {
        let mut fx = fixture();
        write(&fx, "gone.md", b"bye\n");
        track_entry_sync(&mut fx.engine, &fx.slug, "gone.md", None)
            .expect("tracking gone.md should succeed");
        fs::remove_file(fx.root.path().join("gone.md")).unwrap();

        let remaining =
            untrack_entry_sync(&mut fx.engine, &fx.slug, "gone.md").expect("missing live path");
        assert!(
            !remaining.iter().any(|e| e.key == "gone.md"),
            "tombstoned entry must leave the explicit list: {remaining:?}"
        );

        let again = untrack_entry_sync(&mut fx.engine, &fx.slug, "gone.md")
            .expect("untrack of a tombstone is idempotent");
        assert!(!again.iter().any(|e| e.key == "gone.md"));
    }

    #[test]
    fn oversized_folder_confirmation_is_revalidated() {
        let mut fx = fixture();
        fx.engine.set_max_file_mb(1).unwrap();
        fx.engine.set_max_seed_folder_mb(1).unwrap();
        fill(&fx, "notes/a.md", 400_000);
        fill(&fx, "notes/b.md", 400_000);
        fill(&fx, "notes/c.md", 400_000);

        let preview =
            inspect_entry_sync(&mut fx.engine, &fx.slug, "notes").expect("inspect notes/");
        assert!(preview.confirmation_required);
        assert!(preview.bytes > preview.folder_limit);

        fill(&fx, "notes/d.md", 400_000);
        let before = fx.engine.list_entries(&fx.slug).unwrap();
        match track_entry_sync(&mut fx.engine, &fx.slug, "notes", Some(preview.bytes))
            .expect("stale confirmation is not an error")
        {
            TrackResultDto::NeedsConfirmation {
                bytes,
                confirmation_required,
                ..
            } => {
                assert!(confirmation_required);
                assert!(
                    bytes > preview.bytes,
                    "remeasured size {bytes} must exceed stale confirm {}",
                    preview.bytes
                );
            }
            TrackResultDto::Done => panic!("stale smaller byte count must not mutate"),
        }
        assert_eq!(
            fx.engine.list_entries(&fx.slug).unwrap(),
            before,
            "NeedsConfirmation must not add the folder"
        );
    }

    #[test]
    fn list_linkable_without_a_provider_is_empty() {
        let rows = list_linkable_unstarted(false, "no sync runtime".into())
            .expect("onboarding is an empty list");
        assert!(rows.is_empty());
    }

    #[test]
    fn list_linkable_without_a_runtime_keeps_the_error_when_a_provider_is_set() {
        let err = list_linkable_unstarted(true, "runtime down".into()).unwrap_err();
        assert_eq!(err, "runtime down");
    }

    #[test]
    fn list_linkable_reports_unlinked_projects_with_display_names() {
        let fx = fixture();
        fx.engine
            .cloud
            .write_manifest_once(&Manifest {
                slug: "old-mac-notes".into(),
                display_name: "Notes".into(),
                is_agent: false,
            })
            .unwrap();
        fx.engine
            .cloud
            .write_manifest_once(&Manifest {
                slug: "claude".into(),
                display_name: ".claude".into(),
                is_agent: true,
            })
            .unwrap();

        let rows = list_linkable_sync(&fx.engine).expect("list_linkable");
        assert!(
            !rows.iter().any(|r| r.slug == fx.slug),
            "already-tracked slugs must be filtered out: {rows:?}"
        );
        let notes = rows
            .iter()
            .find(|r| r.slug == "old-mac-notes")
            .expect("unlinked project must be listed");
        assert_eq!(notes.display_name, "Notes");
        assert!(!notes.is_agent);
        let agent = rows
            .iter()
            .find(|r| r.slug == "claude")
            .expect("unlinked agent must be listed");
        assert_eq!(agent.display_name, ".claude");
        assert!(agent.is_agent);
    }
}
