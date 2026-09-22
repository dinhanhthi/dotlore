//! "Check for Updates…" in the app menu, and a silent check at launch.
//!
//! Both paths run through one seam, `check(app, interactive)`. The difference
//! is what a failure is allowed to do: the menu item may say so in a dialog,
//! the launch check may not. Until v0.1.0 is published the configured endpoint
//! 404s, and it fails for anyone offline too — a launch check that surfaced
//! that would put an error dialog in front of every user on every start.
//!
//! No tray item, no frontend, no `invoke` command: nothing here crosses the
//! JS→Rust boundary, so `capabilities/default.json` is untouched.

use tauri::menu::{MenuItem, MenuItemKind};
use tauri::{App, AppHandle};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_updater::UpdaterExt;

const CHECK_ID: &str = "check-for-updates";
const TITLE: &str = "Software Update";

/// Add the menu item next to About, then start the launch check.
///
/// Runs after `about::install`, so item 0 of the app menu is that custom About
/// item and the new one goes at index 1. A menu that does not have the shape
/// this expects is reported and skipped — the app still starts, it just has no
/// menu entry, exactly as `about.rs` does.
pub fn install(app: &App) -> tauri::Result<()> {
    let handle = app.handle().clone();
    tauri::async_runtime::spawn(async move { check(&handle, false).await });

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

async fn check(app: &AppHandle, interactive: bool) {
    let updater = match app.updater() {
        Ok(updater) => updater,
        Err(e) => return failed(app, interactive, e.to_string()),
    };
    match updater.check().await {
        Ok(Some(update)) => prompt(app, update),
        Ok(None) => {
            if interactive {
                app.dialog()
                    .message("Dotlore is up to date.")
                    .title(TITLE)
                    .show(|_| {});
            }
        }
        Err(e) => failed(app, interactive, e.to_string()),
    }
}

/// The launch check logs and stops here. Only the menu item may raise a dialog,
/// and it gets one sentence rather than the library's error text.
fn failed(app: &AppHandle, interactive: bool, detail: String) {
    eprintln!("dotlore: updater: {detail}");
    if interactive {
        app.dialog()
            .message("Could not check for updates. Check your connection and try again.")
            .title(TITLE)
            .kind(MessageDialogKind::Warning)
            .show(|_| {});
    }
}

fn prompt(app: &AppHandle, update: tauri_plugin_updater::Update) {
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
            if !install {
                return;
            }
            tauri::async_runtime::spawn(async move {
                match update.download_and_install(|_, _| {}, || {}).await {
                    // `restart` does not return; the new bundle is already in place.
                    Ok(()) => app.restart(),
                    Err(e) => failed(&app, true, e.to_string()),
                }
            });
        });
}
