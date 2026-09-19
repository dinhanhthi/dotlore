//! The macOS status item: its title, its menu, and the actions the menu
//! dispatches.

use gpui::{App, Entity, MenuItem};
use gpui_tray::{Result, Tray};

use dotlore_core::cloud::is_bidi_control;
use dotlore_core::engine::RootStatus;

use crate::state::AppState;

gpui::actions!(dotlore, [Noop, OpenWindow, SyncNow, Quit]);

/// Longest menu-item label. A status item's menu is not a log viewer, and
/// `RootStatus::Error` is not bounded (see [`one_line`]).
const LABEL_MAX: usize = 80;

/// Create the status item. Dropping the returned [`Tray`] removes it, so the
/// caller has to keep it alive for as long as the app runs.
pub fn build(cx: &mut App, state: Entity<AppState>) -> Result<Tray> {
    let title = title(state.read(cx));
    Tray::builder()
        .title(title)
        .tooltip("Dotlore")
        // Re-run by `Tray::refresh_menu` on every notify, so it reads the
        // entity rather than a snapshot taken here.
        .menu(move |cx| menu(state.read(cx)))
        .build(cx)
}

/// Bring the status item back in step with the state.
///
/// Two calls, because the menu builder cannot reach the title. Both are
/// no-ops when nothing changed.
pub fn refresh(tray: &Tray, state: &Entity<AppState>, cx: &mut App) {
    let title = title(state.read(cx));
    if let Err(e) = tray.set_title(Some(title), cx) {
        eprintln!("dotlore: tray title: {e}");
    }
    if let Err(e) = tray.refresh_menu(cx) {
        eprintln!("dotlore: tray menu: {e}");
    }
}

/// The text beside the menu-bar icon.
///
/// `▲`, not `●`: the window's legend is `●` synced / `▲` conflicts, and a
/// conflict count badged with the synced glyph contradicts it.
pub fn title(state: &AppState) -> String {
    if state.conflicts_total > 0 {
        format!("Dotlore ▲{}", state.conflicts_total)
    } else {
        "Dotlore".to_string()
    }
}

fn menu(state: &AppState) -> Vec<MenuItem> {
    let mut items = Vec::new();
    if state.git_missing {
        items.push(MenuItem::action(
            "git not found — run: xcode-select --install",
            Noop,
        ));
    }
    if state.cfg.provider_dir.is_none() {
        items.push(MenuItem::action("No cloud folder set", Noop));
    }
    if let Some(e) = &state.last_error {
        items.push(MenuItem::action(format!("Error: {}", one_line(e)), Noop));
    }
    if state.roots.is_empty() {
        items.push(MenuItem::action("No tracked folders", Noop));
    }
    for (slug, status) in &state.roots {
        // `gpui 0.2.2`'s `MenuItem` has no `disabled()` — only the `gpui-pre`
        // fork does — so these status lines are ordinary items that dispatch
        // `Noop`.
        items.push(MenuItem::action(
            format!("{slug} — {}", show_status(status)),
            Noop,
        ));
    }
    items.push(MenuItem::separator());
    items.push(MenuItem::action("Open Dotlore", OpenWindow));
    items.push(MenuItem::action("Sync Now", SyncNow));
    items.push(MenuItem::separator());
    items.push(MenuItem::action("Quit Dotlore", Quit));
    items
}

/// One root's status, short enough for the window's fixed status column.
///
/// Shared with the tray so a root reads the same in both places, and so the
/// column never has to widen for a label.
pub fn status_label(status: &RootStatus) -> String {
    match status {
        RootStatus::Synced => "Synced".to_string(),
        RootStatus::Conflicts(1) => "1 conflict".to_string(),
        RootStatus::Conflicts(n) => format!("{n} conflicts"),
        RootStatus::Pending => "Pending".to_string(),
        RootStatus::RootMissing => "Folder missing".to_string(),
        RootStatus::GitMissing => "No git".to_string(),
        RootStatus::Error(_) => "Error".to_string(),
    }
}

/// One root's status, as a menu-item suffix: [`status_label`], plus — for
/// `Error` only — the bounded engine message the label drops.
pub fn show_status(status: &RootStatus) -> String {
    match status {
        RootStatus::Error(e) => format!("Error: {}", one_line(e)),
        other => status_label(other),
    }
}

/// Collapse an engine message to one bounded line.
///
/// Unlike the CLI, this UI receives `RootStatus::Error` exactly as the engine
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

    /// Every item's label, `None` for a separator — so one `assert_eq!` pins
    /// content and order together.
    fn labels(items: &[MenuItem]) -> Vec<Option<&str>> {
        items
            .iter()
            .map(|i| match i {
                MenuItem::Separator => None,
                MenuItem::Action { name, .. } => Some(name.as_ref()),
                _ => panic!("the status menu is flat: no submenus, no system menus"),
            })
            .collect()
    }

    fn state(roots: Vec<(String, RootStatus)>) -> AppState {
        // The sender is dropped with the test; nothing here sends on it.
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut s = crate::state::test_state(tx);
        s.conflicts_total = crate::state::conflicts_total(&roots);
        s.roots = roots;
        s
    }

    #[test]
    fn title_is_bare_until_a_root_reports_conflicts() {
        let clean = state(vec![("a".into(), RootStatus::Synced)]);
        assert_eq!(title(&clean), "Dotlore");
    }

    #[test]
    fn title_badges_the_total_across_roots() {
        let s = state(vec![
            ("a".into(), RootStatus::Conflicts(2)),
            ("b".into(), RootStatus::Synced),
            ("c".into(), RootStatus::Conflicts(3)),
        ]);
        assert_eq!(s.conflicts_total, 5);
        assert_eq!(title(&s), "Dotlore ▲5");
    }

    #[test]
    fn a_peer_controlled_error_becomes_one_bounded_line() {
        let hostile = format!(
            "merge failed\n\r\x1b]0;pwn\x07\u{202e}{}",
            "/very/long/path".repeat(80)
        );
        let label = show_status(&RootStatus::Error(hostile));
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

    /// Guard: the four banner branches, and the tail that is the only way out
    /// of the app when the window will not open.
    #[test]
    fn the_menu_leads_with_every_warning_and_always_ends_with_the_way_out() {
        let mut s = state(Vec::new());
        s.git_missing = true;
        s.last_error = Some("cycle failed".into());
        assert_eq!(
            labels(&menu(&s)),
            vec![
                Some("git not found — run: xcode-select --install"),
                Some("No cloud folder set"),
                Some("Error: cycle failed"),
                Some("No tracked folders"),
                None,
                Some("Open Dotlore"),
                Some("Sync Now"),
                None,
                Some("Quit Dotlore"),
            ]
        );
    }

    /// Guard: with nothing wrong, the menu is one line per root — in the order
    /// the last cycle reported them — and nothing else above the separator.
    #[test]
    fn a_healthy_menu_is_one_line_per_root_in_the_reported_order() {
        let mut s = state(vec![
            ("b".into(), RootStatus::Synced),
            ("a".into(), RootStatus::Conflicts(2)),
            ("c".into(), RootStatus::Error("boom".into())),
        ]);
        s.cfg.provider_dir = Some(std::path::PathBuf::from("/cloud"));
        assert_eq!(
            labels(&menu(&s)),
            vec![
                Some("b — Synced"),
                Some("a — 2 conflicts"),
                Some("c — Error: boom"),
                None,
                Some("Open Dotlore"),
                Some("Sync Now"),
                None,
                Some("Quit Dotlore"),
            ]
        );
    }

    #[test]
    fn a_short_error_is_not_truncated() {
        let label = show_status(&RootStatus::Error("no such provider".into()));
        assert_eq!(label, "Error: no such provider");
    }
}
