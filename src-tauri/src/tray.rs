//! The macOS status item: logo only, its menu, and the actions the menu
//! dispatches.

use std::sync::{Arc, Mutex, PoisonError};

use tauri::image::Image;
use tauri::include_image;
use tauri::menu::{Menu, MenuBuilder, MenuEvent, MenuItem};
use tauri::tray::{TrayIcon, TrayIconBuilder};
use tauri::{App, AppHandle, Listener, Manager};

use crate::cloud::is_bidi_control;
use crate::daemon::Cmd;
use crate::git;

use crate::show_window;
use crate::state::{load_cfg, AppState};
use crate::updater;

/// Longest menu-item label. A status item's menu is not a log viewer, and
/// `RootStatus::Error` is not bounded (see [`one_line`]).
const LABEL_MAX: usize = 80;

const ICON: Image<'_> = include_image!("../assets/logo_256.png");

const ID_OPEN: &str = "open";
const ID_SYNC: &str = "sync";
const ID_UPDATE: &str = "update";
const ID_QUIT: &str = "quit";

/// The update row's label is `{UPDATE_PREFIX}{version}…`.
const UPDATE_PREFIX: &str = "Update to Dotlore ";

/// What the tray draws. No root names — the window lists those.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrayView {
    pub git_missing: bool,
    pub no_provider: bool,
    pub error: Option<String>,
    /// The version the updater found, if any.
    pub update: Option<String>,
}

impl TrayView {
    fn from_app(app: &AppHandle) -> Self {
        let git_missing = git::which_git().is_none();
        let no_provider = match load_cfg(&app.state::<AppState>().home) {
            Ok(cfg) => cfg.provider_dir.is_none(),
            Err(_) => false,
        };
        Self {
            git_missing,
            no_provider,
            error: None,
            update: updater::available_version(app),
        }
    }

    fn from_status_json(payload: &str, git_missing: bool, no_provider: bool) -> Self {
        let v: serde_json::Value = serde_json::from_str(payload).unwrap_or(serde_json::Value::Null);
        let error = v.get("error").and_then(|e| e.as_str()).map(str::to_string);
        Self {
            git_missing,
            no_provider,
            error,
            update: None,
        }
    }

    /// Take a status event's fields and keep the update row, which only the
    /// updater's event sets.
    fn apply_status(&mut self, status: TrayView) {
        *self = TrayView {
            update: self.update.take(),
            ..status
        };
    }
}

/// Create the status item. The icon stays registered on the app handle.
pub fn build(app: &App) -> tauri::Result<TrayIcon> {
    // `app.trayIcon` already spawned one so the PNG is embedded; drop it so
    // this builder owns the single icon and the menu. No title — logo only.
    let _ = app.remove_tray_by_id("main");

    let view = TrayView::from_app(app.handle());
    let menu = build_menu(app.handle(), &view)?;
    // Shared by both listeners: a status event must not drop the update row,
    // and an update event must not drop the last error. With no provider the
    // daemon never runs, so the update event cannot wait for a status tick.
    let shared = Arc::new(Mutex::new(view));
    let tray = TrayIconBuilder::with_id("main")
        .icon(ICON)
        .icon_as_template(false)
        .tooltip("Dotlore")
        .menu(&menu)
        .on_menu_event(on_menu)
        .build(app)?;

    let tray_handle = tray.clone();
    let app_handle = app.handle().clone();
    let view = shared.clone();
    app.listen("dotlore://status", move |event| {
        let git_missing = git::which_git().is_none();
        let no_provider = match load_cfg(&app_handle.state::<AppState>().home) {
            Ok(cfg) => cfg.provider_dir.is_none(),
            Err(_) => false,
        };
        let mut view = view.lock().unwrap_or_else(PoisonError::into_inner);
        let next = TrayView::from_status_json(event.payload(), git_missing, no_provider);
        view.apply_status(next);
        refresh(&app_handle, &tray_handle, &view);
    });

    let tray_handle = tray.clone();
    let app_handle = app.handle().clone();
    // The version comes from the updater's state, not the payload: the
    // webview may emit events too, and must not be able to relabel this row.
    app.listen(updater::UPDATE_EVENT, move |_| {
        let mut view = shared.lock().unwrap_or_else(PoisonError::into_inner);
        view.update = updater::available_version(&app_handle);
        refresh(&app_handle, &tray_handle, &view);
    });

    Ok(tray)
}

fn refresh(app: &AppHandle, tray: &TrayIcon, view: &TrayView) {
    match build_menu(app, view) {
        Ok(menu) => {
            if let Err(e) = tray.set_menu(Some(menu)) {
                eprintln!("dotlore: tray menu: {e}");
            }
        }
        Err(e) => eprintln!("dotlore: tray menu: {e}"),
    }
}

fn on_menu(app: &AppHandle, event: MenuEvent) {
    match event.id().as_ref() {
        ID_OPEN => show_window(app),
        ID_SYNC => {
            let _ = app.state::<AppState>().send(Cmd::SyncNow);
        }
        ID_UPDATE => updater::prompt_available(app),
        ID_QUIT => {
            let _ = app.state::<AppState>().send(Cmd::Quit);
            app.exit(0);
        }
        _ => {}
    }
}

fn build_menu(app: &AppHandle, view: &TrayView) -> tauri::Result<Menu<tauri::Wry>> {
    let mut menu = MenuBuilder::new(app);
    let mut warns = 0usize;
    for row in menu_labels(view) {
        match row.as_deref() {
            None => menu = menu.separator(),
            Some("Open Dotlore") => menu = menu.text(ID_OPEN, "Open Dotlore"),
            Some("Sync Now") => menu = menu.text(ID_SYNC, "Sync Now"),
            Some("Quit Dotlore") => menu = menu.text(ID_QUIT, "Quit Dotlore"),
            Some(text) if text.starts_with(UPDATE_PREFIX) => menu = menu.text(ID_UPDATE, text),
            Some(text) => {
                warns += 1;
                menu = menu.item(&disabled(app, format!("warn-{warns}"), text)?);
            }
        }
    }
    menu.build()
}

fn disabled(
    app: &AppHandle,
    id: impl Into<tauri::menu::MenuId>,
    text: impl AsRef<str>,
) -> tauri::Result<MenuItem<tauri::Wry>> {
    MenuItem::with_id(app, id, text, false, None::<&str>)
}

/// Menu rows: `Some(label)` or `None` for a separator. An available update
/// first, then warnings, then the actions, and never a root name.
pub fn menu_labels(view: &TrayView) -> Vec<Option<String>> {
    let mut items = Vec::new();
    if let Some(version) = &view.update {
        items.push(Some(format!("{UPDATE_PREFIX}{}…", one_line(version))));
        items.push(None);
    }
    if view.git_missing {
        items.push(Some(
            "git not found — run: xcode-select --install".to_string(),
        ));
    }
    if view.no_provider {
        items.push(Some("No cloud folder set".to_string()));
    }
    if let Some(e) = &view.error {
        items.push(Some(format!("Error: {}", one_line(e))));
    }
    items.push(None);
    items.push(Some("Open Dotlore".to_string()));
    items.push(Some("Sync Now".to_string()));
    items.push(None);
    items.push(Some("Quit Dotlore".to_string()));
    items
}

/// Collapse an engine message to one bounded line.
///
/// The menu receives `RootStatus::Error` exactly as the engine
/// produced it: several hundred characters of git stderr, embedding path
/// names that came out of another device's bundle. AppKit lays a newline out
/// inside the menu item rather than interpreting it, so this is a legibility
/// guard, not an escaping one — C0/C1 controls and the bidi controls are
/// dropped, runs of whitespace collapse, and the result is truncated.
///
/// That is the whole guarantee: it is not a spoofing filter. Other invisible
/// format characters, homoglyphs and right-to-left script all survive, and a
/// bounded one-line label is the point rather than a canonical one.
pub fn one_line(s: &str) -> String {
    let printable: String = s
        .chars()
        .filter(|c| (!c.is_control() || c.is_whitespace()) && !is_bidi_control(*c))
        .collect();
    let flat = printable.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= LABEL_MAX {
        return flat;
    }
    let head: String = flat.chars().take(LABEL_MAX).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::is_bidi_control;

    fn view() -> TrayView {
        TrayView::default()
    }

    #[test]
    fn a_peer_controlled_error_becomes_one_bounded_line() {
        let hostile = format!(
            "merge failed\n\r\x1b]0;pwn\x07\u{202e}{}",
            "/very/long/path".repeat(80)
        );
        let label = format!("Error: {}", one_line(&hostile));
        assert!(!label.contains('\n'), "{label}");
        assert!(!label.contains('\x1b'), "{label}");
        assert!(!label.contains('\x07'), "{label}");
        assert!(
            !label.chars().any(is_bidi_control),
            "a bidi override reverses the rest of the line: {label:?}"
        );
        assert!(
            label.chars().count() <= LABEL_MAX + "Error: …".len(),
            "{label}"
        );
        assert!(label.starts_with("Error: merge failed "));
    }

    /// Guard: the warning branches, and the tail that is the only way out
    /// of the app when the window will not open. No root names.
    #[test]
    fn the_menu_leads_with_every_warning_and_always_ends_with_the_way_out() {
        let s = TrayView {
            git_missing: true,
            no_provider: true,
            error: Some("cycle failed".into()),
            update: None,
        };
        assert_eq!(
            menu_labels(&s),
            vec![
                Some("git not found — run: xcode-select --install".into()),
                Some("No cloud folder set".into()),
                Some("Error: cycle failed".into()),
                None,
                Some("Open Dotlore".into()),
                Some("Sync Now".into()),
                None,
                Some("Quit Dotlore".into()),
            ]
        );
    }

    /// Guard: a healthy cycle does not list roots in the tray.
    #[test]
    fn a_healthy_menu_has_no_root_names() {
        let s = TrayView {
            no_provider: false,
            ..view()
        };
        let labels = menu_labels(&s);
        assert_eq!(
            labels,
            vec![
                None,
                Some("Open Dotlore".into()),
                Some("Sync Now".into()),
                None,
                Some("Quit Dotlore".into()),
            ]
        );
        for label in labels.into_iter().flatten() {
            assert!(!label.contains(" — "), "tray must not list roots: {label}");
        }
    }

    /// Guard: an available update leads the menu, and the row is the one
    /// `build_menu` wires to the updater.
    #[test]
    fn an_available_update_leads_the_menu() {
        let s = TrayView {
            update: Some("0.3.0".into()),
            ..view()
        };
        let labels = menu_labels(&s);
        assert_eq!(labels[0], Some("Update to Dotlore 0.3.0…".into()));
        assert_eq!(labels[1], None);
    }

    /// Guard: the 30 s status tick must not drop the update row.
    #[test]
    fn a_status_event_keeps_the_update_row() {
        let mut shared = TrayView {
            update: Some("0.3.0".into()),
            ..view()
        };
        shared.apply_status(TrayView::from_status_json(
            r#"{"roots":[],"error":"cycle failed"}"#,
            false,
            true,
        ));
        assert_eq!(shared.update.as_deref(), Some("0.3.0"));
        assert_eq!(shared.error.as_deref(), Some("cycle failed"));
        assert!(shared.no_provider);
    }

    #[test]
    fn a_short_error_is_not_truncated() {
        let label = format!("Error: {}", one_line("no such provider"));
        assert_eq!(label, "Error: no such provider");
    }

    #[test]
    fn a_status_event_never_copies_slugs() {
        let json = r#"{
            "roots": [
                {"slug":"alpha","path":"/a","name":"a","is_agent":false,"status":{"kind":"Conflicts","detail":2}},
                {"slug":"beta","path":"/b","name":"b","is_agent":false,"status":{"kind":"Synced"}}
            ],
            "error": null
        }"#;
        let view = TrayView::from_status_json(json, false, false);
        let labels = menu_labels(&view);
        for label in labels.into_iter().flatten() {
            assert!(
                !label.contains("alpha") && !label.contains("beta"),
                "tray must not list roots: {label}"
            );
        }
    }
}
