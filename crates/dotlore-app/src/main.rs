//! Dotlore's macOS menu-bar app.

mod commands;
mod login_item;
mod state;
mod tray;

use std::fs::{File, TryLockError};
use std::path::{Path, PathBuf};

use anyhow::Result;
use tauri::{AppHandle, Manager, RunEvent, WindowEvent};

use dotlore_core::config::{self, Config};

use state::AppState;

const WINDOW: &str = "main";

fn main() {
    // The only environment this binary reads, once, at startup — the same
    // documented exception `dotlore-cli` has: `DOTLORE_HOME` (through
    // `default_home`) and `$HOME`.
    let home = config::default_home();
    let home_dir = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());

    if let Err(e) = load(&home) {
        eprintln!("dotlore: {e:#}");
        std::process::exit(1);
    }
    // Held, not dropped: the binding keeps the file — and the lock — alive for
    // as long as the process, and `App::run` never returns.
    let _instance = single_instance(&home);

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(move |app| {
            // Regular (the Tauri default) keeps the Dock icon. Accessory
            // hid the app from the Dock and left only the menu-bar item.
            let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);

            if let Some(window) = app.get_webview_window(WINDOW) {
                if let Some(icon) = app.default_window_icon() {
                    let _ = window.set_icon(icon.clone());
                }
                let handle = app.handle().clone();
                window.on_window_event(move |event| {
                    if let WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        hide_window(&handle);
                    }
                });
            }

            app.manage(AppState::new(home, home_dir));
            app.state::<AppState>().start_runtime(app.handle());
            tray::build(app)?;
            show_window(app.handle());

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_roots,
            commands::tracked_files,
            commands::read_file,
            commands::conflicts,
            commands::open_resolution,
            commands::close_resolution,
            commands::resolve_conflict,
            commands::resolve_binary,
            commands::provider_dir,
            commands::git_missing,
            commands::sync_now,
            commands::set_provider,
            commands::add_root,
            commands::import_installed_agents,
            commands::link_root,
            commands::remove_root,
            commands::recover_root,
            commands::list_linkable,
            commands::list_entries,
            commands::list_entry_children,
            commands::inspect_entry,
            commands::track_entry,
            commands::untrack_entry,
            commands::default_patterns,
            commands::set_default_patterns,
            commands::pattern_catalogs,
            commands::set_pattern_catalog,
            commands::default_ignore,
            commands::set_default_ignore,
            commands::max_file_mb,
            commands::set_max_file_mb,
            commands::max_seed_folder_mb,
            commands::set_max_seed_folder_mb,
            commands::list_gdrive_mounts,
            commands::icloud_dir,
            commands::login_item_enabled,
            commands::set_login_item,
        ])
        .build(tauri::generate_context!())
        .expect("error while running Dotlore")
        .run(|app, event| {
            if let RunEvent::Reopen { .. } = event {
                show_window(app);
            }
        });
}

pub(crate) fn show_window(app: &AppHandle) {
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
    if let Some(window) = app.get_webview_window(WINDOW) {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

pub(crate) fn hide_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(WINDOW) {
        let _ = window.hide();
    }
}

/// Fresh config under the home lock.
///
/// The lock is dropped before anything builds an engine: every engine entry
/// point takes it again and `std::fs::File::lock` is not reentrant.
fn load(home: &Path) -> Result<Config> {
    let _guard = config::lock(home)?;
    Config::load(home)
}

/// One menu-bar app per home — or this copy leaves quietly.
///
/// Ticking "Start at login" bootstraps a `RunAtLoad` LaunchAgent, and launchd
/// starts the program the instant it is bootstrapped. Without this the user
/// would get a second status item and a second daemon on the same home just
/// for ticking a box. (The home lock serialises them, so nothing was ever
/// corrupted — it was just plainly broken.) The agent sets `KeepAlive false`,
/// so the copy that exits here is not respawned.
///
/// Deliberately **not** [`config::lock`]: that is the engine's home lock,
/// taken and released around every single operation. Holding it for the life
/// of the process would deadlock the daemon's first cycle.
///
/// A lock file that cannot be opened or flocked is not a reason to refuse to
/// run — only a lock another process is holding is.
fn single_instance(home: &Path) -> Option<File> {
    let path = home.join("app.lock");
    let opened = File::options()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path);
    let file = match opened {
        Ok(file) => file,
        Err(e) => {
            eprintln!("dotlore: {}: {e}", path.display());
            return None;
        }
    };
    match file.try_lock() {
        Ok(()) => Some(file),
        Err(TryLockError::WouldBlock) => {
            eprintln!("dotlore: already running — use the menu bar item");
            std::process::exit(0);
        }
        Err(TryLockError::Error(e)) => {
            eprintln!("dotlore: {}: {e}", path.display());
            None
        }
    }
}
