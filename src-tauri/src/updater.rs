//! "Check for Updates…" in the app menu, a silent check at launch and every
//! few hours after, and the install that follows.
//!
//! Every check runs through one seam, `check(app, interactive)`. The difference
//! is what a failure is allowed to do: the menu item may say so in a dialog,
//! the background checks may not. The endpoint fails for anyone offline — a
//! background check that surfaced that would put an error dialog in front of
//! the user every few hours.
//!
//! A found update is kept in [`UpdateState`] and announced as
//! `dotlore://update` (the version, or `null`), which drives the menu-bar row
//! and the title-bar badge. A background check prompts only for a version it
//! has not already recorded, so "Later" is not asked again every few hours;
//! the row and the badge stay until the update is installed. Installing emits
//! `dotlore://update-progress`.
//!
//! This is the only file that reaches `tauri-plugin-updater`. The tray and
//! the commands call the functions below and never the plugin itself. App
//! commands need no entry in `capabilities/default.json`, so it is untouched.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use serde::Serialize;
use tauri::menu::{MenuItem, MenuItemKind};
use tauri::{App, AppHandle, Emitter, Manager};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::show_window;

const CHECK_ID: &str = "check-for-updates";
const TITLE: &str = "Software Update";
const CHECK_FAILED: &str = "Could not check for updates. Check your connection and try again.";
const INSTALL_FAILED: &str =
    "Could not install the update. Dotlore is unchanged — try again from Check for Updates…";

/// Time between background checks. The app lives in the menu bar for days,
/// so a launch-only check would leave it behind until the next login.
const INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// Bounds the `latest.json` request only — the plugin does not carry it over
/// to the download. Without it, one request hung on a half-open connection
/// would stall the background loop for the rest of the session.
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);

pub const UPDATE_EVENT: &str = "dotlore://update";
const PROGRESS_EVENT: &str = "dotlore://update-progress";

/// The update the last check found, and whether one is being installed.
#[derive(Default)]
pub struct UpdateState {
    available: Mutex<Option<Update>>,
    installing: AtomicBool,
    prompting: AtomicBool,
}

/// `dotlore://update-progress`. `Idle` resets the badge after a failed install.
#[derive(Serialize, Clone)]
#[serde(tag = "phase", rename_all = "lowercase")]
enum Progress {
    Downloading { percent: Option<u8> },
    Installing,
    Idle,
}

/// Add the menu item next to About, then start the background checks.
///
/// Runs after `about::install`, so item 0 of the app menu is that custom About
/// item and the new one goes at index 1. A menu that does not have the shape
/// this expects is reported and skipped — the app still starts, it just has no
/// menu entry, exactly as `about.rs` does.
pub fn install(app: &App) -> tauri::Result<()> {
    app.manage(UpdateState::default());

    let handle = app.handle().clone();
    std::thread::spawn(move || loop {
        tauri::async_runtime::block_on(check(&handle, false));
        std::thread::sleep(INTERVAL);
    });

    let Some(menu) = app.menu() else {
        eprintln!("dotlore: updater: no app menu");
        return Ok(());
    };
    let Some(MenuItemKind::Submenu(app_menu)) = menu.items()?.into_iter().next() else {
        eprintln!("dotlore: updater: app menu is not a submenu");
        return Ok(());
    };

    let item = MenuItem::with_id(
        app.handle(),
        CHECK_ID,
        "Check for Updates…",
        true,
        None::<&str>,
    )?;
    app_menu.insert(&item, 1)?;

    // A second listener, not a replacement: Tauri keeps them in a Vec and
    // dispatches to all of them, so `about.rs`'s handler still runs.
    app.on_menu_event(|app, event| {
        if event.id().as_ref() == CHECK_ID {
            let app = app.clone();
            tauri::async_runtime::spawn(async move { check(&app, true).await });
        }
    });
    Ok(())
}

/// The version the last check found, if any.
pub fn available_version(app: &AppHandle) -> Option<String> {
    let state = app.try_state::<UpdateState>()?;
    let available = state
        .available
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    available.as_ref().map(|u| u.version.clone())
}

/// The tray row and the title-bar badge: prompt for the update already found,
/// without another round trip. With none recorded, check as the menu item does.
pub fn prompt_available(app: &AppHandle) {
    let Some(state) = app.try_state::<UpdateState>() else {
        return;
    };
    if state.installing.load(Ordering::SeqCst) {
        return installing(app);
    }
    let found = state
        .available
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    match found {
        Some(update) => prompt(app, update),
        None => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move { check(&app, true).await });
        }
    }
}

async fn check(app: &AppHandle, interactive: bool) {
    let state = app.state::<UpdateState>();
    if state.installing.load(Ordering::SeqCst) {
        if interactive {
            installing(app);
        }
        return;
    }
    let updater = match app.updater_builder().timeout(CHECK_TIMEOUT).build() {
        Ok(updater) => updater,
        Err(e) => return failed(app, interactive, CHECK_FAILED, e.to_string()),
    };
    match updater.check().await {
        Ok(Some(update)) => {
            let fresh = record(app, Some(update.clone()));
            if interactive || fresh {
                prompt(app, update);
            }
        }
        Ok(None) => {
            record(app, None);
            if interactive {
                app.dialog()
                    .message("Dotlore is up to date.")
                    .title(TITLE)
                    .show(|_| {});
            }
        }
        Err(e) => failed(app, interactive, CHECK_FAILED, e.to_string()),
    }
}

fn installing(app: &AppHandle) {
    app.dialog()
        .message("An update is already being installed.")
        .title(TITLE)
        .show(|_| {});
}

/// Keep what a check found and announce it. True when the version is one the
/// user has not been asked about yet.
fn record(app: &AppHandle, update: Option<Update>) -> bool {
    let version = update.as_ref().map(|u| u.version.clone());
    let state = app.state::<UpdateState>();
    let previous = {
        let mut available = state
            .available
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        std::mem::replace(&mut *available, update).map(|u| u.version)
    };
    let _ = app.emit(UPDATE_EVENT, &version);
    is_fresh(previous.as_deref(), version.as_deref())
}

/// A found version the previous check had not already recorded.
fn is_fresh(previous: Option<&str>, found: Option<&str>) -> bool {
    found.is_some() && found != previous
}

/// A background check logs and stops here. Only the menu item may raise a
/// dialog, and it gets one sentence rather than the library's error text.
///
/// `message` is the caller's, because the two failure paths fail for different
/// reasons: a check fails on the network, an install most often fails because
/// the user dismissed the admin password prompt — the plugin reports that as
/// `PermissionDenied("Failed to move the new app into place")`, and telling that
/// user to check their connection points them at the wrong thing.
fn failed(app: &AppHandle, interactive: bool, message: &str, detail: String) {
    eprintln!("dotlore: updater: {detail}");
    if interactive {
        app.dialog()
            .message(message)
            .title(TITLE)
            .kind(MessageDialogKind::Warning)
            .show(|_| {});
    }
}

/// One dialog at a time: a badge click while the launch prompt is still open
/// would otherwise stack a second one.
fn prompt(app: &AppHandle, update: Update) {
    let state = app.state::<UpdateState>();
    if state.prompting.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    let version = update.version.clone();
    app.clone()
        .dialog()
        .message(format!(
            "Dotlore {version} is available. You are running {}.",
            update.current_version
        ))
        .title(TITLE)
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Install and Restart".into(),
            "Later".into(),
        ))
        .show(move |install| {
            app.state::<UpdateState>()
                .prompting
                .store(false, Ordering::SeqCst);
            if install {
                download(app, update);
            }
        });
}

/// Download and install, reporting progress to the window, then restart.
fn download(app: AppHandle, update: Update) {
    if app
        .state::<UpdateState>()
        .installing
        .swap(true, Ordering::SeqCst)
    {
        return;
    }
    // The progress lives in the title bar; a hidden window would show none.
    show_window(&app);
    tauri::async_runtime::spawn(async move {
        let _ = app.emit(PROGRESS_EVENT, Progress::Downloading { percent: None });
        let mut received: u64 = 0;
        let mut last: Option<u8> = None;
        let result = update
            .download_and_install(
                |chunk, total| {
                    received += chunk as u64;
                    let percent = total
                        .filter(|t| *t > 0)
                        .map(|t| (received.saturating_mul(100) / t).min(100) as u8);
                    if percent.is_some() && percent != last {
                        last = percent;
                        let _ = app.emit(PROGRESS_EVENT, Progress::Downloading { percent });
                    }
                },
                || {
                    let _ = app.emit(PROGRESS_EVENT, Progress::Installing);
                },
            )
            .await;
        match result {
            // `restart` does not return; the new bundle is already in place.
            Ok(()) => app.restart(),
            Err(e) => {
                app.state::<UpdateState>()
                    .installing
                    .store(false, Ordering::SeqCst);
                let _ = app.emit(PROGRESS_EVENT, Progress::Idle);
                failed(&app, true, INSTALL_FAILED, e.to_string());
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::is_fresh;

    /// Guard: "Later" is not asked again for the same version every few hours.
    #[test]
    fn only_a_version_not_already_recorded_is_fresh() {
        assert!(is_fresh(None, Some("0.3.0")));
        assert!(!is_fresh(Some("0.3.0"), Some("0.3.0")));
        assert!(is_fresh(Some("0.3.0"), Some("0.4.0")));
        assert!(!is_fresh(Some("0.3.0"), None));
        assert!(!is_fresh(None, None));
    }
}
