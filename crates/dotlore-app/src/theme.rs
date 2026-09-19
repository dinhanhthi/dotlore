//! Design tokens for the window: the palette, the type scale, the geometry
//! every row aligns to, and the two pure helpers that shape text to fit.
//!
//! Everything visual that is not a layout decision lives here, so the window
//! can be re-skinned without going near an engine call.
//!
//! ## Where the colours come from
//!
//! `gpui_component 0.5.1`'s **dark** theme is the base, forced by
//! [`install`]: `background` `#0a0a0a`, `foreground` `#fafafa`, `border`
//! `#262626`, `list_hover` `#262626`, `title_bar` `#171717`. Those are read
//! through `cx.theme()` rather than copied, so the crate's own `Button`,
//! `Input` and `Checkbox` match the rest of the window for free.
//!
//! The **state** colours are the theme's base ramp — `green` `#22c55e`,
//! `yellow` `#eab308`, `red` `#ef4444` — and deliberately *not* `success` /
//! `warning` / `danger`. In the dark theme those three are filled-button
//! *backgrounds* (`#14532d`, `#713f12`, `#7f1d1d`); as text on `#0a0a0a` they
//! are all but invisible, which is exactly what the first pass was drawing.
//!
//! [`dim`] is the one colour literal in the app. The theme's
//! `muted_foreground` `#737373` is 4.2:1 on `#0a0a0a` — under AA for 12 px
//! body text — and a path is the one thing on screen worth reading closely.
//! `#a3a3a3` is the next step up the same neutral ramp the theme is built
//! from, at 7.8:1. `muted_foreground` is kept for furniture that carries no
//! information on its own.
//!
//! ## Scales
//!
//! Spacing is gpui's own `_0p5 / _1 / _2 / _3 / _4 / _6` — 2, 4, 8, 12, 16,
//! 24 px. `_1p5` (6 px) is deliberately *not* in the set: it was the value
//! that made the first pass drift, because `gap_1p5` plus a child `pt_1`
//! reads as an unnameable 10 px.
//!
//! Type is three sizes, and every element on screen is one of them:
//!
//! | Role                                   | px | Family | Weight   |
//! |----------------------------------------|----|--------|----------|
//! | Caps section labels ([`LABEL_PX`])     | 10 | UI     | Semibold |
//! | Paths, conflicts, error detail, footer | 12 | Mono*  | Normal   |
//! | Slugs, status, every control label     | 14 | UI     | Normal   |
//!
//! \* mono for paths and conflict lines only; the footer and the empty-state
//! notes are 12 px UI.
//!
//! The 14 px row is what `gpui_component`'s `Size::Small` resolves to for
//! `Button`, `Input` **and** `Checkbox` — `button_text_size` and
//! `input_text_size` both map `Small` to `text_sm`, and `Checkbox` maps it to
//! `text_sm` too. That is the whole reason the window is uniformly
//! `.small()`: it is the one `Sizable` step that agrees with the hand-drawn
//! text. `Medium` — the library default, and what an unsized `Checkbox` falls
//! back to — is `text_base`, 16 px, which is why the checkbox used to be the
//! largest thing on screen. Nothing in this window may take a library
//! default.
//!
//! `Size::Small` also fixes one control height, [`ROW_H`] 24 px, for all
//! three, which is what lets a row share a baseline.

use std::path::Path;

use gpui::prelude::*;
use gpui::{div, px, App, FontWeight, Hsla, Pixels, SharedString};
use gpui_component::{ActiveTheme, Theme, ThemeMode};

use dotlore_core::engine::RootStatus;

/// The leading status-dot column. Fixed, so every glyph and every slug below
/// it starts at the same x. 16, not 14: at 14 px text the widest glyph (`▲`)
/// is a shade over 14 px and would nudge the slug column off the grid.
pub const GLYPH_COL: Pixels = px(16.);
/// The trailing status column. Fixed and right-aligned, so the labels line up
/// down the list instead of ragging against whatever the slug left over.
/// Sized for the longest [`crate::tray::status_label`], `Folder missing`, at
/// 14 px.
pub const STATUS_COL: Pixels = px(104.);
/// The left edge of a root row's second line — [`GLYPH_COL`] plus the row's
/// `gap_2` — so the path hangs under the slug rather than under the dot.
pub const INDENT: Pixels = px(24.);
/// One control height: `Button`, `Input` and `Checkbox` at `Size::Small`, and
/// therefore the height of every row whose contents must share a baseline.
pub const ROW_H: Pixels = px(24.);
/// Caps section labels.
pub const LABEL_PX: Pixels = px(10.);
/// Rough character budget for a path on a root row, at 12 px Menlo. The path
/// has a line of its own now, indented by [`INDENT`]: roughly 504 px of a
/// 560 px window, about 70 characters, so 60 leaves margin for a wide glyph.
pub const PATH_CHARS: usize = 60;
/// The same, for the provider line, which spans nearly the whole window.
pub const PROVIDER_CHARS: usize = 56;

/// Force the dark theme.
///
/// `gpui_component::init` syncs the mode to the system appearance once and
/// nothing re-syncs it afterwards, so a single call here holds for the life
/// of the process. This app is dark whatever the Mac is set to — the window
/// is a dev tool, not a system panel, and half its palette is chosen against
/// `#0a0a0a`.
pub fn install(cx: &mut App) {
    Theme::change(ThemeMode::Dark, None, cx);
}

/// The readable mid grey. See the module docs for why this is not
/// `cx.theme().muted_foreground`.
pub fn dim() -> Hsla {
    gpui::rgb(0xa3a3a3).into()
}

/// What a status badge says and how alarmed it looks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tone {
    Good,
    Warn,
    Muted,
    Bad,
}

pub fn tone(status: &RootStatus) -> Tone {
    match status {
        RootStatus::Synced => Tone::Good,
        RootStatus::Conflicts(_) => Tone::Warn,
        RootStatus::Pending => Tone::Muted,
        RootStatus::RootMissing | RootStatus::GitMissing | RootStatus::Error(_) => Tone::Bad,
    }
}

pub fn tone_color(tone: Tone, cx: &App) -> Hsla {
    match tone {
        Tone::Good => cx.theme().green,
        Tone::Warn => cx.theme().yellow,
        Tone::Muted => dim(),
        Tone::Bad => cx.theme().red,
    }
}

/// The leading glyph for a root's status.
///
/// Shape, not only colour: the same information has to survive a colour-blind
/// reader and a screenshot in greyscale. All six live in the Geometric Shapes
/// block or Latin-1, which every macOS system font covers, so none of them
/// can fall back to tofu.
pub fn glyph(status: &RootStatus) -> &'static str {
    match status {
        RootStatus::Synced => "●",
        RootStatus::Conflicts(_) => "▲",
        RootStatus::Pending => "○",
        RootStatus::RootMissing => "◇",
        RootStatus::GitMissing => "◆",
        RootStatus::Error(_) => "×",
    }
}

/// A section label: 10 px, uppercase, letter-spaced, quiet.
///
/// gpui 0.2.2 has no letter-spacing, so the spacing is a thin space (U+2009)
/// woven between the characters. It is the one typographic trick in here.
pub fn section_label(text: &str, cx: &App) -> impl IntoElement {
    div()
        .text_size(LABEL_PX)
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(cx.theme().muted_foreground)
        .child(SharedString::from(spaced(&text.to_uppercase())))
}

/// One dimmed monospace line — a path, a conflict, a bounded error detail.
pub fn mono_line(text: impl Into<SharedString>, color: Hsla, cx: &App) -> impl IntoElement {
    div()
        .truncate()
        .text_xs()
        .font_family(cx.theme().mono_font_family.clone())
        .text_color(color)
        .child(text.into())
}

fn spaced(s: &str) -> String {
    s.chars().flat_map(|c| [c, '\u{2009}']).collect()
}

/// A path as the UI shows it: `~`-relative, and middle-elided to `budget`
/// characters so the informative tail survives.
///
/// Elision is by path component and every length is counted in `char`s, so
/// no input — including a multi-byte path off another device's bundle — can
/// panic this inside `render`.
pub fn short_path(path: &Path, home_dir: &Path, budget: usize) -> String {
    let full = tilde(path, home_dir);
    if full.chars().count() <= budget {
        return full;
    }
    let parts: Vec<&str> = full.split('/').collect();
    let head = parts.first().copied().unwrap_or_default();
    let head_len = head.chars().count();

    // Grow a tail from the right while `head/…/tail` still fits.
    let mut keep = 0;
    let mut tail_len = 0;
    for part in parts.iter().skip(1).rev() {
        let next = tail_len + part.chars().count() + 1;
        if head_len + 2 + next > budget {
            break;
        }
        tail_len = next;
        keep += 1;
    }
    if keep == 0 {
        // One component on its own overruns the budget: keep its tail.
        let last = parts.last().copied().unwrap_or_default();
        let room = budget.saturating_sub(1);
        let skip = last.chars().count().saturating_sub(room);
        return format!("…{}", last.chars().skip(skip).collect::<String>());
    }
    format!("{head}/…/{}", parts[parts.len() - keep..].join("/"))
}

fn tilde(path: &Path, home_dir: &Path) -> String {
    match path.strip_prefix(home_dir) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = "/Users/someone";

    fn short(path: &str, budget: usize) -> String {
        short_path(Path::new(path), Path::new(HOME), budget)
    }

    #[test]
    fn the_home_itself_is_just_a_tilde() {
        assert_eq!(short(HOME, 60), "~");
    }

    #[test]
    fn a_path_that_fits_is_left_alone() {
        // 5 characters, 14 bytes: the budget is counted in `char`s, so this
        // is under it. A path outside `$HOME` keeps its absolute form.
        assert_eq!(short("/Users/someone/项目笔记", 10), "~/项目笔记");
        assert_eq!(short("/etc/hosts", 60), "/etc/hosts");
    }

    #[test]
    fn a_long_path_keeps_its_tail_and_lands_on_the_budget() {
        // "~/…/c/d/e" is exactly 9: the last component that still fits is
        // kept, and one more would overrun.
        assert_eq!(short("/Users/someone/a/b/c/d/e", 9), "~/…/c/d/e");
    }

    #[test]
    fn a_multibyte_path_is_elided_by_characters_not_bytes() {
        // Same shape as the ASCII case above, and the same 9 characters out.
        assert_eq!(short("/Users/someone/项/目/文/件/档", 9), "~/…/文/件/档");
    }

    #[test]
    fn one_oversized_component_is_cut_to_the_budget() {
        let long = "字".repeat(30);
        let out = short(&format!("/Users/someone/{long}"), 10);
        assert_eq!(out.chars().count(), 10);
        assert_eq!(out, format!("…{}", "字".repeat(9)));
    }

    #[test]
    fn a_zero_budget_still_returns_something_printable() {
        // The `saturating_sub`s are the only thing between this and a panic.
        assert_eq!(short("/Users/someone/notes.md", 0), "…");
        assert_eq!(short("/", 0), "…");
    }
}
