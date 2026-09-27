//! Native `NSAlert` dialogs carrying the Dotlore icon.
//!
//! `tauri-plugin-dialog` drives `CFUserNotificationDisplayAlert` on macOS,
//! whose icon this app cannot set — it always draws AppKit's generic caution
//! glyph. `updater.rs` is the only caller, so its dialogs get the app's own
//! mark instead.
//!
//! `NSAlert` is main-thread-only, and `updater::check` runs off it, so every
//! call here hops to the main thread through `run_on_main_thread` before
//! building and running the alert.

use objc2::MainThreadMarker;
use objc2_app_kit::{NSAlert, NSAlertFirstButtonReturn, NSAlertStyle, NSApplication};
use objc2_foundation::NSString;
use tauri::AppHandle;

use crate::about;

#[derive(Clone, Copy)]
pub enum Style {
    Info,
    Warning,
}

/// A single "OK" button; nothing to report back.
pub fn info(app: &AppHandle, style: Style, title: &str, message: &str) {
    show(app, style, title, message, vec!["OK".to_string()], |_| {});
}

/// Two buttons. `on_result` gets `true` when the first (`ok_label`) was
/// clicked.
pub fn confirm(
    app: &AppHandle,
    title: &str,
    message: &str,
    ok_label: &str,
    cancel_label: &str,
    on_result: impl FnOnce(bool) + Send + 'static,
) {
    show(
        app,
        Style::Info,
        title,
        message,
        vec![ok_label.to_string(), cancel_label.to_string()],
        on_result,
    );
}

fn show(
    app: &AppHandle,
    style: Style,
    title: &str,
    message: &str,
    buttons: Vec<String>,
    on_result: impl FnOnce(bool) + Send + 'static,
) {
    let title = title.to_string();
    let message = message.to_string();
    let _ = app.run_on_main_thread(move || {
        let Some(mtm) = MainThreadMarker::new() else {
            eprintln!("dotlore: alert: not on the main thread");
            return;
        };
        on_result(present(mtm, style, &title, &message, &buttons));
    });
}

/// Runs modally, so this returns only once the user has dismissed it.
fn present(
    mtm: MainThreadMarker,
    style: Style,
    title: &str,
    message: &str,
    buttons: &[String],
) -> bool {
    // The tray can raise this while the app is Accessory (window closed), and
    // a status-item click alone does not activate the process — without
    // this the alert opens behind whatever app is frontmost.
    NSApplication::sharedApplication(mtm).activate();
    let alert = NSAlert::new(mtm);
    unsafe {
        alert.setAlertStyle(match style {
            Style::Info => NSAlertStyle::Informational,
            Style::Warning => NSAlertStyle::Warning,
        });
        alert.setMessageText(&NSString::from_str(title));
        alert.setInformativeText(&NSString::from_str(message));
        if let Some(icon) = about::logo() {
            alert.setIcon(Some(&icon));
        }
        for label in buttons {
            alert.addButtonWithTitle(&NSString::from_str(label));
        }
        alert.runModal() == NSAlertFirstButtonReturn
    }
}
