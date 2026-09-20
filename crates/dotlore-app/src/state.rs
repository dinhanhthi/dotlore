//! Shared engine, daemon thread, and the status events they push to the UI.
//!
//! Engine access from a Tauri command must go through
//! `tauri::async_runtime::spawn_blocking` and must never hold `config::lock`
//! across an `.await`. `std::fs::File::lock` is not reentrant, and a lock
//! taken on an async worker that then awaits would stall the daemon's next
//! cycle (and deadlock if that cycle tried to take the same lock).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, PoisonError};

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use dotlore_core::config::{self, Config, Root};
use dotlore_core::daemon::{self, Cmd, Event, SharedEngine};
use dotlore_core::engine::{Engine, ResolutionSnapshot, RootStatus};

/// One daemon cycle, as it crosses into the webview.
#[derive(Serialize, Clone, Debug)]
pub struct StatusPayload {
    pub roots: Vec<RootRow>,
    pub error: Option<String>,
}

/// One tracked root, ready to draw.
#[derive(Serialize, Clone, Debug)]
pub struct RootRow {
    pub slug: String,
    pub path: String,
    pub name: String,
    pub is_agent: bool,
    pub status: RootStatus,
}

impl RootRow {
    pub(crate) fn from_root(root: &Root, home_dir: &Path, status: RootStatus) -> Self {
        let name = root
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        Self {
            slug: root.slug.clone(),
            path: root.path.to_string_lossy().into_owned(),
            name,
            is_agent: root.is_agent(home_dir),
            status,
        }
    }

    fn from_cycle(cfg: &Config, home_dir: &Path, slug: String, status: RootStatus) -> Self {
        match cfg.roots.iter().find(|r| r.slug == slug) {
            Some(root) => Self::from_root(root, home_dir, status),
            None => Self {
                slug,
                path: String::new(),
                name: String::new(),
                is_agent: false,
                status,
            },
        }
    }
}

/// Process-wide handles the window and the daemon share.
///
/// `engine` and `cmd_tx` are `None` until a provider folder exists —
/// [`Engine::new`] refuses to build without one. `snapshots` never crosses
/// IPC: a `ResolutionSnapshot` pins blob ids the resolver must not lose.
pub struct AppState {
    pub home: PathBuf,
    pub home_dir: PathBuf,
    pub engine: Mutex<Option<SharedEngine>>,
    pub cmd_tx: Mutex<Option<Sender<daemon::Event>>>,
    /// Open resolution, keyed by [`resolution_key`]. At most one entry.
    pub snapshots: Mutex<HashMap<String, ResolutionSnapshot>>,
}

/// Registry key for one open resolution. `\0` cannot appear in a slug.
pub(crate) fn resolution_key(slug: &str, rel: &str) -> String {
    format!("{slug}\u{0}{rel}")
}

impl AppState {
    pub fn new(home: PathBuf, home_dir: PathBuf) -> Self {
        Self {
            home,
            home_dir,
            engine: Mutex::new(None),
            cmd_tx: Mutex::new(None),
            snapshots: Mutex::new(HashMap::new()),
        }
    }

    /// Build the engine and start the daemon thread for the configured
    /// provider. No-op without one, and no-op when a runtime already exists —
    /// a second daemon on the same home would double every cycle.
    ///
    /// `Engine::new` only assembles a struct; it takes no lock. Config is
    /// loaded and the home lock dropped before the engine slot is taken,
    /// matching the old app's order (`state.rs` ~75–105).
    pub fn start_runtime(&self, app: &AppHandle) {
        let cfg = match load_cfg(&self.home) {
            Ok(cfg) => cfg,
            Err(e) => {
                emit_error(app, e);
                return;
            }
        };
        if cfg.provider_dir.is_none() {
            return;
        }

        let mut engine_slot = self.engine.lock().unwrap_or_else(PoisonError::into_inner);
        if engine_slot.is_some() {
            return;
        }

        let engine = match Engine::new(&self.home, &self.home_dir, cfg) {
            Ok(engine) => Arc::new(Mutex::new(engine)),
            Err(e) => {
                emit_error(app, format!("{e:#}"));
                return;
            }
        };

        let (tx, rx) = mpsc::channel::<Event>();
        let daemon_engine = engine.clone();
        let daemon_tx = tx.clone();
        let handle = app.clone();
        std::thread::spawn(move || {
            daemon::run(daemon_engine.clone(), daemon_tx, rx, move |status| {
                let _ = handle.emit("dotlore://status", payload_from(&daemon_engine, status));
            });
        });
        *engine_slot = Some(engine);
        *self.cmd_tx.lock().unwrap_or_else(PoisonError::into_inner) = Some(tx);
    }

    /// Ask the daemon to do something. Returns a clear error when no runtime
    /// is running rather than panicking on a missing sender.
    pub fn send(&self, cmd: Cmd) -> Result<(), String> {
        let tx = self.cmd_tx.lock().unwrap_or_else(PoisonError::into_inner);
        match tx.as_ref() {
            Some(tx) => tx
                .send(Event::Control(cmd))
                .map_err(|_| "the sync daemon has stopped".into()),
            None => Err("no sync runtime is running — pick a provider folder first".into()),
        }
    }

    /// The shared engine, or a clear error when first-run has no provider.
    pub fn shared_engine(&self) -> Result<SharedEngine, String> {
        self.engine
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .ok_or_else(|| "no sync runtime is running — pick a provider folder first".into())
    }

    /// Remember this snapshot and drop every other open resolution.
    pub(crate) fn open_snapshot(&self, slug: &str, rel: &str, snap: ResolutionSnapshot) {
        let key = resolution_key(slug, rel);
        let mut map = self
            .snapshots
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        map.clear();
        map.insert(key, snap);
    }

    /// Forget the resolution for this live path, if it is the one open.
    pub(crate) fn close_snapshot(&self, slug: &str, rel: &str) {
        let key = resolution_key(slug, rel);
        self.snapshots
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&key);
    }

    /// Clone the snapshot for this live path, if it is the one open.
    pub(crate) fn get_snapshot(&self, slug: &str, rel: &str) -> Option<ResolutionSnapshot> {
        let key = resolution_key(slug, rel);
        self.snapshots
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
            .cloned()
    }

    /// Swap in a refreshed snapshot for this live path, if that key is still open.
    pub(crate) fn replace_snapshot(&self, slug: &str, rel: &str, snap: ResolutionSnapshot) {
        let key = resolution_key(slug, rel);
        let mut map = self
            .snapshots
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if map.contains_key(&key) {
            map.insert(key, snap);
        }
    }

    /// How many snapshots the registry currently holds.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.snapshots
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }
}

/// Fresh config under the home lock; the lock is dropped before return.
pub(crate) fn load_cfg(home: &Path) -> Result<Config, String> {
    let _guard = config::lock(home).map_err(|e| format!("{e:#}"))?;
    Config::load(home).map_err(|e| format!("{e:#}"))
}

fn payload_from(
    engine: &SharedEngine,
    status: Result<Vec<(String, RootStatus)>, anyhow::Error>,
) -> StatusPayload {
    match status {
        Ok(rows) => {
            let e = engine.lock().unwrap_or_else(PoisonError::into_inner);
            StatusPayload {
                roots: rows
                    .into_iter()
                    .map(|(slug, st)| RootRow::from_cycle(&e.cfg, &e.home_dir, slug, st))
                    .collect(),
                error: None,
            }
        }
        Err(e) => StatusPayload {
            roots: Vec::new(),
            error: Some(format!("{e:#}")),
        },
    }
}

fn emit_error(app: &AppHandle, error: impl Into<String>) {
    let _ = app.emit(
        "dotlore://status",
        StatusPayload {
            roots: Vec::new(),
            error: Some(error.into()),
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_snap(slug: &str, live: &str) -> ResolutionSnapshot {
        ResolutionSnapshot {
            slug: slug.to_string(),
            live: PathBuf::from(live),
            head: "0".into(),
            live_blob: "0".into(),
            live_executable: false,
            live_bytes: b"x".to_vec(),
            root: None,
            siblings: vec![],
        }
    }

    #[test]
    fn open_resolution_replaces_the_previous_snapshot() {
        let state = AppState::new(PathBuf::from("/tmp/unused"), PathBuf::from("/tmp"));
        state.open_snapshot("a", "one.md", dummy_snap("a", "one.md"));
        state.open_snapshot("b", "two.md", dummy_snap("b", "two.md"));
        assert_eq!(
            state.len(),
            1,
            "the registry must keep exactly one open resolution"
        );
        let map = state.snapshots.lock().unwrap();
        assert!(
            map.contains_key(&resolution_key("b", "two.md")),
            "the surviving key is the one just opened"
        );
    }
}
