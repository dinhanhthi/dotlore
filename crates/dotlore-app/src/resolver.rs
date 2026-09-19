//! The side-by-side conflict resolver: LIVE, OTHER, and the Result the user
//! saves.
//!
//! ## Its own window, not a sheet
//!
//! The plan first said "a modal sheet inside the main window", written before
//! the main window settled at 560×520 — two side-by-side editors would get
//! about 35 characters and seven lines each in there, which is unusable for
//! the one job this screen exists to do. Amended 2026-09-19: the resolver is
//! its own 900×640 window opened with `App::open_window`, where each column
//! is around 60 characters wide. Only one is ever open; [`focus_existing`]
//! brings it forward instead of opening a second.
//!
//! ## What Save is allowed to do
//!
//! [`Engine::resolve_conflict`] re-reads HEAD, the root and every sibling and
//! refuses to touch anything if one version moved, handing back
//! [`ResolveOutcome::Stale`] with refreshed data rather than deleting
//! something the user never saw. This file is the other half of that
//! guarantee: [`next`] is the only place that decides what a returned call
//! means, and **only `Applied` closes the window**. `Stale` keeps the draft,
//! redisplays both sides and asks for another look; `Pending` and an error
//! keep the draft and say so. Nothing here closes the window or clears a
//! badge merely because a call returned.
//!
//! The other half of that rule is where the *effect* lands. [`settle`] takes
//! `&mut AppState`, not `&self`, and the save's completion folds it in
//! through the app rather than through this view — a window the user closed
//! mid-save (Cancel, Cmd-W, the red traffic light; none of which stops the
//! engine call already running) must not swallow a deletion that happened.
//!
//! Only siblings the user left ticked, from the snapshot that is on screen,
//! for that exact live path, are ever passed as removable — the engine
//! rejects anything else, and this side never offers it.
//!
//! ## Two library facts this file is built on
//!
//! - **Read-only is `Input::disabled(true)`.** `gpui-component 0.5.1` has no
//!   read-only flag; `disabled` is the one that fits: the element binds no
//!   editing action, both of `InputState`'s writers — `replace_text_in_range`
//!   and the IME's `replace_and_mark_text_in_range` — return early on it, and
//!   selection, copy and scrolling all stay live. `set_value` clears the flag
//!   around its own write, so refreshing a side still works.
//! - **Syntax highlighting is json-only.** `code_editor(lang)` resolves the
//!   language through a registry that, without the crate's non-default
//!   `tree-sitter-languages` feature, holds exactly one entry — and
//!   `Language::from_str` maps *every* name to `Json`, so `code_editor(
//!   "markdown")` would JSON-highlight a `CLAUDE.md`. The feature pulls in
//!   some thirty tree-sitter crates, which this workspace's pins do not
//!   allow, so [`code_lang`] asks for a code editor only where the pin can
//!   honour it, and everything else gets a plain multi-line editor.

use std::path::{Path, PathBuf};
use std::sync::PoisonError;

use gpui::prelude::*;
use gpui::{
    div, px, size, App, Bounds, Context, Entity, Global, SharedString, TitlebarOptions, Window,
    WindowBounds, WindowHandle, WindowOptions,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::checkbox::Checkbox;
use gpui_component::input::{Input, InputState};
use gpui_component::{h_flex, v_flex, ActiveTheme, Disableable, Root, Sizable, TitleBar};

use dotlore_core::engine::{
    ConflictView, ResolutionSnapshot, ResolveOutcome, RootStatus, SiblingView,
};
use dotlore_core::mirror;

use crate::state::{conflicts_total, AppState};
use crate::theme;
use crate::tray;

/// The resolver window. Two 430-odd px columns, about 60 characters of 12 px
/// mono each, and room for a Result of about fifteen lines.
const RESOLVER: (f32, f32) = (900., 640.);
/// The Result editor's height. The two read-only sides take what is left, so
/// growing the window grows the comparison rather than the draft.
const RESULT_H: f32 = 150.;

const STALE: &str = "This file changed since you opened it. Nothing was written. \
                     Review the refreshed LIVE and OTHER above, then Save again.";
const PENDING: &str = "Not finished: this root has an apply that could not complete. \
                       Check the main window — its error line names the blocked path.";
/// Cancel stays enabled while this is on screen, so it says what closing now
/// would mean. See [`Resolver::save`].
const SAVING: &str = "Saving… Closing this window will not stop it: the result still \
                      reaches the main window, and an unsaved draft is lost.";

// --- opening ----------------------------------------------------------------

/// The one resolver window, if one is open, keyed by `(slug, live path)`.
///
/// Both halves, not just the path: `ConflictView::live` is **root-relative**,
/// so two tracked roots that each hold a `CLAUDE.md` — the headline case —
/// would otherwise share one key, and a `Resolve` on the second would silently
/// activate the first one's window.
///
/// A `gpui` global rather than a field on the main window: the resolver
/// outlives it — closing the dashboard must not orphan an unsaved draft — and
/// the tray can reopen the dashboard underneath it.
#[derive(Default)]
struct OpenResolver(Option<(String, PathBuf, WindowHandle<Root>)>);

impl Global for OpenResolver {}

/// Bring the open resolver forward, if there is one, and say which conflict
/// it is showing.
///
/// `None` also covers "the handle is stale" — the user closed the window —
/// which is the same liveness check the main window's action uses: `gpui
/// 0.2.2` has no *per-window* closed callback. `App::on_window_closed`
/// (`app.rs:1806`, fired from `app.rs:1378`) is global and carries no
/// `WindowId`, and `Window::on_window_should_close` (`window.rs:4329`) is a
/// pre-close veto, not a notification — neither can retire one handle.
pub fn focus_existing(cx: &mut App) -> Option<(String, PathBuf)> {
    let open = cx.default_global::<OpenResolver>().0.clone();
    let (slug, live, handle) = open?;
    if handle
        .update(cx, |_, window, _| window.activate_window())
        .is_ok()
    {
        cx.activate(true);
        return Some((slug, live));
    }
    cx.default_global::<OpenResolver>().0 = None;
    None
}

/// Open the resolver on a snapshot the caller already took off the UI thread.
///
/// `snapshot` must come from `Engine::open_resolution`, which captures HEAD,
/// the live blob, the root state and the complete sibling set under one lock.
///
/// Two `Resolve` clicks can both reach `open_resolution` before either
/// returns — the button's `disabled` is only re-evaluated on the next render
/// — so the "only one resolver" rule is enforced here too, where the global
/// lives, and not only at the click. The second snapshot is dropped.
pub fn open(
    cx: &mut App,
    state: Entity<AppState>,
    slug: String,
    view: ConflictView,
    snapshot: ResolutionSnapshot,
) {
    if focus_existing(cx).is_some() {
        return;
    }
    let live = snapshot.live.clone();
    let title = format!("Resolve — {slug}");
    let bounds = Bounds::centered(None, size(px(RESOLVER.0), px(RESOLVER.1)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        // The window draws its own header, so the system bar is transparent
        // with the traffic lights placed for it — the same deal the dashboard
        // makes. The title still reaches Mission Control and VoiceOver.
        titlebar: Some(TitlebarOptions {
            title: Some(title.into()),
            ..TitleBar::title_bar_options()
        }),
        ..Default::default()
    };
    let key_slug = slug.clone();
    let opened = cx.open_window(options, |window, cx| {
        let resolver = cx.new(|cx| Resolver::new(state, slug, view, snapshot, window, cx));
        // `Input` reads its focus state back through the window's `Root`.
        cx.new(|cx| Root::new(resolver, window, cx))
    });
    match opened {
        Ok(handle) => {
            cx.default_global::<OpenResolver>().0 = Some((key_slug, live, handle));
            cx.activate(true);
        }
        Err(e) => eprintln!("dotlore: cannot open the resolver: {e:#}"),
    }
}

// --- the view ---------------------------------------------------------------

/// Which side wins, for a conflict with no editable Result.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Side {
    Live,
    Other,
}

pub struct Resolver {
    state: Entity<AppState>,
    slug: String,
    /// Exactly what is on screen, and what Save is version-checked against.
    snapshot: ResolutionSnapshot,
    /// The conflict row the user clicked. Its `live` is what the header says.
    view: ConflictView,
    live_editor: Entity<InputState>,
    other_editor: Entity<InputState>,
    result: Entity<InputState>,
    /// Either side is not text this app may round-trip; see [`binary_mode`].
    binary: bool,
    /// Which sibling the OTHER column shows, when the live path has several.
    tab: usize,
    /// Per-sibling "clear this copy", parallel to `snapshot.siblings`.
    remove: Vec<bool>,
    /// Only consulted when [`Self::binary`].
    chosen: Side,
    status: Option<String>,
    inflight: bool,
}

impl Resolver {
    fn new(
        state: Entity<AppState>,
        slug: String,
        view: ConflictView,
        snapshot: ResolutionSnapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Dark whatever the Mac is set to, like the dashboard. Nothing in
        // `gpui_component` re-syncs the mode after `init`.
        theme::install(cx);
        let binary = binary_mode(&snapshot);
        let other = snapshot.siblings.first().map(|s| s.bytes.as_slice());
        let live_editor = editor(&snapshot.live, &snapshot.live_bytes, binary, window, cx);
        let other_editor = editor(
            &snapshot.live,
            other.unwrap_or_default(),
            binary,
            window,
            cx,
        );
        // Pre-filled with LIVE: the version every Mac already has is the one
        // an interrupted resolution should leave behind.
        let result = editor(&snapshot.live, &snapshot.live_bytes, binary, window, cx);
        Self {
            remove: vec![true; snapshot.siblings.len()],
            state,
            slug,
            snapshot,
            view,
            live_editor,
            other_editor,
            result,
            binary,
            tab: 0,
            chosen: Side::Live,
            status: None,
            inflight: false,
        }
    }

    fn current(&self) -> Option<&SiblingView> {
        self.snapshot.siblings.get(self.tab)
    }

    /// The bytes Save would write.
    fn content(&self, cx: &App) -> Vec<u8> {
        if !self.binary {
            return self.result.read(cx).value().to_string().into_bytes();
        }
        match self.chosen {
            Side::Live => self.snapshot.live_bytes.clone(),
            Side::Other => self
                .current()
                .map(|s| s.bytes.clone())
                .unwrap_or_else(|| self.snapshot.live_bytes.clone()),
        }
    }

    fn show_tab(&mut self, tab: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.tab = tab;
        // Never on a binary conflict: no editor is drawn, and `lossy` on a
        // blob is megabytes of U+FFFD nobody would see.
        if !self.binary {
            let text = lossy(
                self.current()
                    .map(|s| s.bytes.as_slice())
                    .unwrap_or_default(),
            );
            self.other_editor
                .update(cx, |s, cx| s.set_value(text, window, cx));
        }
        cx.notify();
    }

    // --- save --------------------------------------------------------------

    /// Run the resolution off the UI thread, then act on exactly what came
    /// back. See [`next`] — this method never decides on its own.
    fn save(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine.clone() else {
            self.status = Some("no cloud folder is configured".into());
            cx.notify();
            return;
        };
        let slug = self.slug.clone();
        let snapshot = self.snapshot.clone();
        let selected = selected_paths(&self.snapshot.siblings, &self.remove);
        let content = self.content(cx);
        let state = self.state.clone();
        let settled_slug = self.slug.clone();
        self.inflight = true;
        self.status = Some(SAVING.into());
        cx.notify();

        let task = cx.background_spawn(async move {
            // `Engine` takes a blocking lock on the home that a CLI can be
            // holding; this must never run on the UI thread.
            let mut e = engine.lock().unwrap_or_else(PoisonError::into_inner);
            let out = e
                .resolve_conflict(&slug, &snapshot, &selected, &content)
                .map_err(|e| format!("{e:#}"));
            // Only when something was actually applied: `conflicts` takes the
            // home lock again, and a save that changed nothing has nothing to
            // re-list.
            let conflicts = match &out {
                Ok(ResolveOutcome::Applied(_)) => e.conflicts(&slug).ok(),
                _ => None,
            };
            (out, conflicts, e.cfg.clone())
        });
        cx.spawn_in(window, async move |this, cx| {
            let (out, conflicts, cfg) = task.await;
            let step = next(out);
            // Through the *app*, not through this view: the window may
            // already be gone — Cancel, Cmd-W and the red traffic light all
            // tear the entity down, and none of them stops the engine call
            // that is already running. An applied resolution has deleted
            // siblings and rewritten the live file by now, so the badge, the
            // conflicts list and the root's status must follow whether or not
            // there is still a window to close. `AsyncWindowContext::
            // update_entity` routes through the window handle and would fail
            // here; `AsyncApp`'s goes straight to the app.
            let app: &mut gpui::AsyncApp = cx;
            let _ = state.update(app, |s, cx| {
                let mut moved = s.adopt_cfg(cfg);
                if let Step::Close(status) = &step {
                    settle(s, &settled_slug, status.clone(), conflicts);
                    moved = true;
                }
                if moved {
                    cx.notify();
                }
            });
            let _ = this.update_in(cx, |this, window, cx| {
                this.inflight = false;
                match step {
                    Step::Close(_) => window.remove_window(),
                    Step::Refresh(fresh) => {
                        this.adopt(*fresh, window, cx);
                        this.status = Some(STALE.into());
                    }
                    Step::Stay(message) => this.status = Some(message),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Redisplay a snapshot that moved under us, keeping the user's draft.
    ///
    /// The Result editor is deliberately not touched: it holds work the user
    /// did, and the whole point of `Stale` is that nothing was written.
    fn adopt(&mut self, fresh: ResolutionSnapshot, window: &mut Window, cx: &mut Context<Self>) {
        let was_binary = self.binary;
        self.remove = carry_selection(&self.snapshot.siblings, &self.remove, &fresh.siblings);
        self.binary = binary_mode(&fresh);
        self.tab = self.tab.min(fresh.siblings.len().saturating_sub(1));
        self.snapshot = fresh;
        if self.binary {
            return;
        }
        let live = lossy(&self.snapshot.live_bytes);
        let other = lossy(
            self.current()
                .map(|s| s.bytes.as_slice())
                .unwrap_or_default(),
        );
        self.live_editor
            .update(cx, |s, cx| s.set_value(live.clone(), window, cx));
        self.other_editor
            .update(cx, |s, cx| s.set_value(other, window, cx));
        // The one case where the Result *must* be written: a conflict that was
        // binary and is now text never loaded a draft — its editor holds the
        // empty string, and saving that would truncate the live file.
        if was_binary {
            self.result
                .update(cx, |s, cx| s.set_value(live, window, cx));
        }
    }

    // --- pieces -------------------------------------------------------------

    fn header(&self, cx: &Context<Self>) -> impl IntoElement {
        let home_dir = self.state.read(cx).home_dir.clone();
        let path = theme::short_path(&self.view.live, &home_dir, theme::PATH_CHARS);
        h_flex()
            .gap_2()
            .items_center()
            .child(div().child(SharedString::from(self.slug.clone())))
            .child(theme::mono_line(tray::one_line(&path), theme::dim(), cx))
    }

    /// One tab per sibling, shown only when the live path has more than one.
    fn tabs(&self, cx: &Context<Self>) -> impl IntoElement {
        let current = self.tab;
        self.snapshot.siblings.iter().enumerate().fold(
            h_flex().gap_2().items_center(),
            |row, (i, s)| {
                let label = tray::one_line(&s.loser_name);
                let button = Button::new(SharedString::from(format!("tab-{i}")))
                    .small()
                    .compact()
                    .label(label)
                    .on_click(cx.listener(move |this, _, window, cx| this.show_tab(i, window, cx)));
                row.child(if i == current {
                    button.primary()
                } else {
                    button.ghost()
                })
            },
        )
    }

    /// The two read-only sides, or — for a conflict this app cannot show as
    /// text — two cards with their byte sizes.
    fn sides(&self, cx: &Context<Self>) -> impl IntoElement {
        let is_me = self
            .current()
            .is_some_and(|s| s.loser_id8 == self.state.read(cx).cfg.id8());
        let other = other_label(
            self.current().map(|s| s.loser_name.as_str()).unwrap_or(""),
            is_me,
        );
        let (live_body, other_body) = if self.binary {
            (
                card(
                    self.snapshot.live_bytes.len(),
                    self.chosen == Side::Live,
                    cx,
                )
                .into_any_element(),
                card(
                    self.current().map(|s| s.bytes.len()).unwrap_or(0),
                    self.chosen == Side::Other,
                    cx,
                )
                .into_any_element(),
            )
        } else {
            (
                pane(&self.live_editor).into_any_element(),
                pane(&self.other_editor).into_any_element(),
            )
        };
        h_flex()
            .flex_1()
            .gap_4()
            .overflow_hidden()
            .child(column(LIVE_LABEL, live_body, cx))
            .child(column(&other, other_body, cx))
    }

    /// One checkbox per sibling: what Save is allowed to clear.
    ///
    /// Ticked by default, which is what the `dotlore resolve` CLI does
    /// unconditionally — a resolution that leaves the conflict copy behind
    /// leaves the badge up, and that is the surprising outcome, not this one.
    /// Unticking is how a sibling survives, and every tick is on screen
    /// before Save, which is what makes the selection explicit.
    fn removals(&self, cx: &Context<Self>) -> impl IntoElement {
        let busy = self.inflight;
        self.snapshot.siblings.iter().enumerate().fold(
            v_flex().gap_1().items_start(),
            |col, (i, s)| {
                let ticked = self.remove.get(i).copied().unwrap_or(false);
                col.child(
                    Checkbox::new(SharedString::from(format!("rm-{i}")))
                        .small()
                        .label(format!(
                            "Clear the copy from {}",
                            tray::one_line(&s.loser_name)
                        ))
                        .checked(ticked)
                        .disabled(busy)
                        .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                            if let Some(slot) = this.remove.get_mut(i) {
                                *slot = *checked;
                            }
                            cx.notify();
                        })),
                )
            },
        )
    }

    fn actions(&self, cx: &Context<Self>) -> impl IntoElement {
        // ponytail: copies the draft out of the editor once per render to
        // decide whether Save would do anything. Renders are notify-driven,
        // not per-frame, and a tracked config file is kilobytes; cache it on
        // an input subscription if someone ever resolves a huge file.
        let no_op = self.content(cx) == self.snapshot.live_bytes && !self.remove.iter().any(|r| *r);
        let busy = self.inflight;
        let has_other = self.current().is_some();
        // Zipped with the siblings, so the count is exactly what
        // `selected_paths` would hand the engine.
        let ticked = self
            .remove
            .iter()
            .zip(&self.snapshot.siblings)
            .filter(|(r, _)| **r)
            .count();
        h_flex()
            .gap_2()
            .items_center()
            .justify_end()
            .when_some(removal_summary(ticked), |el, line| {
                el.child(
                    div()
                        .flex_1()
                        .text_xs()
                        .text_color(cx.theme().yellow)
                        .child(SharedString::from(line)),
                )
            })
            .child(
                Button::new("use-live")
                    .small()
                    .outline()
                    .label("Use LIVE")
                    .disabled(busy)
                    .on_click(
                        cx.listener(|this, _, window, cx| this.use_side(Side::Live, window, cx)),
                    ),
            )
            .child(
                Button::new("use-other")
                    .small()
                    .outline()
                    .label("Use OTHER")
                    .disabled(busy || !has_other)
                    .on_click(
                        cx.listener(|this, _, window, cx| this.use_side(Side::Other, window, cx)),
                    ),
            )
            .child(
                // Deliberately *not* `disabled(busy)`. A save in flight can
                // be waiting on a home lock a CLI holds, and disabling this
                // would pin the window behind it — while the red traffic
                // light and Cmd-W close it anyway and cannot be disabled.
                // What made closing mid-save dangerous was that `settle` ran
                // on this view; it runs on `AppState` now, so an applied
                // resolution lands whether or not this window survives, and
                // [`SAVING`] says as much.
                Button::new("cancel")
                    .small()
                    .ghost()
                    .label("Cancel")
                    .on_click(|_, window: &mut Window, _| window.remove_window()),
            )
            .child(
                Button::new("save")
                    .small()
                    .primary()
                    .label("Save")
                    // A save with nothing ticked and an untouched Result is a
                    // no-op the engine reports as `Applied`, which would close
                    // this window over a conflict that is still there.
                    .disabled(busy || no_op)
                    .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
            )
    }

    fn use_side(&mut self, side: Side, window: &mut Window, cx: &mut Context<Self>) {
        self.chosen = side;
        if !self.binary {
            let bytes = match side {
                Side::Live => self.snapshot.live_bytes.clone(),
                Side::Other => self.current().map(|s| s.bytes.clone()).unwrap_or_default(),
            };
            let text = lossy(&bytes);
            self.result
                .update(cx, |s, cx| s.set_value(text, window, cx));
        }
        cx.notify();
    }
}

const LIVE_LABEL: &str = "LIVE — the version every Mac currently has";

impl Render for Resolver {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs = self.snapshot.siblings.len() > 1;
        let siblings = !self.snapshot.siblings.is_empty();
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .text_sm()
            .overflow_hidden()
            .child(
                TitleBar::new().child(
                    div()
                        .text_xs()
                        .text_color(theme::dim())
                        .child("Resolve conflict"),
                ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .overflow_hidden()
                    .px_4()
                    .py_3()
                    .gap_3()
                    .child(self.header(cx))
                    .when(tabs, |el| el.child(self.tabs(cx)))
                    .child(self.sides(cx))
                    .when(!self.binary, |el| {
                        el.child(column(
                            "RESULT — what every Mac will hold",
                            div()
                                .h(px(RESULT_H))
                                .font_family(cx.theme().mono_font_family.clone())
                                .child(Input::new(&self.result).xsmall().h_full()),
                            cx,
                        ))
                    })
                    .when(siblings, |el| el.child(self.removals(cx))),
            )
            .child(
                v_flex()
                    .flex_none()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .px_4()
                    .py_2()
                    .gap_2()
                    .when_some(self.status.clone(), |el, s| {
                        el.child(
                            div()
                                .text_xs()
                                .text_color(theme::dim())
                                .child(SharedString::from(s)),
                        )
                    })
                    .child(self.actions(cx)),
            )
    }
}

// --- pure helpers -------------------------------------------------------------

/// What a returned [`ResolveOutcome`] means for the window.
///
/// The one place that decides, so the rule that only `Applied` closes is a
/// single testable branch rather than something spread over a callback.
#[derive(PartialEq, Eq, Debug)]
pub enum Step {
    Close(RootStatus),
    Refresh(Box<ResolutionSnapshot>),
    Stay(String),
}

fn next(out: Result<ResolveOutcome, String>) -> Step {
    match out {
        Ok(ResolveOutcome::Applied(status)) => Step::Close(status),
        Ok(ResolveOutcome::Stale(fresh)) => Step::Refresh(fresh),
        Ok(ResolveOutcome::Pending) => Step::Stay(PENDING.to_string()),
        // Engine output: hundreds of characters of git stderr, embedding
        // paths that came off another device's bundle.
        Err(e) => Step::Stay(tray::one_line(&e)),
    }
}

/// Fold an applied resolution into the state the tray and dashboard read.
///
/// A free function over [`AppState`], not a method on the view: the effect
/// belongs to state that outlives the resolver window, and it has to land
/// even when the user closed that window while the save was in flight.
///
/// The status the engine returned is authoritative for this root, and the
/// freshly listed conflicts replace this slug's rows — the ones for every
/// other slug are left exactly as they were.
fn settle(s: &mut AppState, slug: &str, status: RootStatus, conflicts: Option<Vec<ConflictView>>) {
    let mut roots = s.roots.clone();
    match roots.iter_mut().find(|(name, _)| name == slug) {
        Some(row) => row.1 = status,
        None => roots.push((slug.to_string(), status)),
    }
    s.roots = roots;
    s.conflicts_total = conflicts_total(&s.roots);
    if let Some(fresh) = conflicts {
        s.conflicts.retain(|(name, _)| name != slug);
        s.conflicts
            .extend(fresh.into_iter().map(|c| (slug.to_string(), c)));
    }
}

/// The destructive half of Save, spelled out beside the button.
///
/// The ticks default to on — which is what `dotlore resolve` does — so the
/// count is what makes the deletion legible at the moment of decision.
fn removal_summary(ticked: usize) -> Option<String> {
    match ticked {
        0 => None,
        1 => Some("Save will remove 1 copy".to_string()),
        n => Some(format!("Save will remove {n} copies")),
    }
}

/// The siblings the user left ticked, as paths from the displayed snapshot.
///
/// Nothing else is ever offered for removal: the engine rejects a path it did
/// not show, and a UI that sent one would be asking it to.
fn selected_paths(siblings: &[SiblingView], remove: &[bool]) -> Vec<PathBuf> {
    siblings
        .iter()
        .enumerate()
        .filter(|(i, _)| remove.get(*i).copied().unwrap_or(false))
        .map(|(_, s)| s.path.clone())
        .collect()
}

/// Carry the tick state across a refreshed snapshot, by path.
///
/// A sibling that survived keeps what the user decided about it; one that
/// appeared while the window was open starts ticked, like any other.
fn carry_selection(old: &[SiblingView], remove: &[bool], fresh: &[SiblingView]) -> Vec<bool> {
    fresh
        .iter()
        .map(|s| {
            old.iter()
                .position(|o| o.path == s.path)
                .and_then(|i| remove.get(i).copied())
                .unwrap_or(true)
        })
        .collect()
}

/// Whether this conflict can be shown and saved as text.
///
/// `mirror::is_binary` only looks for a NUL byte, which Latin-1 text passes.
/// Editing that through a `String` would silently rewrite every high byte as
/// U+FFFD on Save, so anything that is not valid UTF-8 counts as binary too:
/// the byte-size cards keep the original bytes intact.
fn binary_mode(snapshot: &ResolutionSnapshot) -> bool {
    !is_text(&snapshot.live_bytes) || snapshot.siblings.iter().any(|s| !is_text(&s.bytes))
}

fn is_text(bytes: &[u8]) -> bool {
    !mirror::is_binary(bytes) && std::str::from_utf8(bytes).is_ok()
}

/// The language to hand `InputState::code_editor`, if the pinned crate can
/// actually highlight it.
///
/// `json` is the only entry in `gpui-component 0.5.1`'s registry without the
/// non-default `tree-sitter-languages` feature, and `Language::from_str` maps
/// every other name onto it — so asking for `markdown` would draw a
/// `CLAUDE.md` through a JSON parser. Everything else gets a plain multi-line
/// editor instead.
fn code_lang(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()? {
        "json" | "jsonc" => Some("json"),
        _ => None,
    }
}

/// The OTHER column's label. `loser_name` is peer-authored — it comes out of
/// another device's `device.json` — so it is bounded before display.
fn other_label(loser_name: &str, loser_is_me: bool) -> String {
    let me = if loser_is_me { " (this Mac)" } else { "" };
    format!("OTHER — from {}{me}", tray::one_line(loser_name))
}

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

// --- small elements ------------------------------------------------------------

/// A read-only side, or the editable Result: one `Input` filling its column.
fn editor(
    live: &Path,
    bytes: &[u8],
    binary: bool,
    window: &mut Window,
    cx: &mut Context<Resolver>,
) -> Entity<InputState> {
    // A binary conflict never draws an editor, so it never loads the bytes
    // either — `lossy` on a binary blob is megabytes of U+FFFD.
    let text = if binary { String::new() } else { lossy(bytes) };
    let lang = code_lang(live);
    cx.new(|cx| {
        let state = InputState::new(window, cx);
        match lang {
            // `code_editor` switches search on, and its panel draws
            // `IconName` SVGs — this app installs no asset source, so Cmd-F
            // would render a row of unresolvable icons. Off again.
            Some(lang) => state.code_editor(lang).searchable(false),
            None => state.multi_line(true),
        }
        .default_value(text)
    })
}

/// A labelled half of the window.
fn column(label: &str, body: impl IntoElement, cx: &App) -> impl IntoElement {
    v_flex()
        .flex_1()
        .gap_1()
        .overflow_hidden()
        .child(theme::section_label(label, cx))
        .child(body)
}

/// One read-only side, at the 12 px mono the column widths were measured in:
/// `Size::XSmall` is the only step whose `input_text_size` is 12.
fn pane(state: &Entity<InputState>) -> impl IntoElement {
    div()
        .flex_1()
        .overflow_hidden()
        .child(Input::new(state).xsmall().disabled(true).h_full())
}

/// What a binary side gets instead of an editor.
fn card(bytes: usize, chosen: bool, cx: &App) -> impl IntoElement {
    let border = if chosen {
        cx.theme().green
    } else {
        cx.theme().border
    };
    v_flex()
        .flex_1()
        .gap_1()
        .p_3()
        .border_1()
        .border_color(border)
        .rounded(cx.theme().radius)
        .child(div().child(format!("{bytes} bytes")))
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(if chosen {
                    "This version will be kept"
                } else {
                    "Not text — choose a side to keep"
                }),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sibling(path: &str, bytes: &[u8]) -> SiblingView {
        SiblingView {
            path: PathBuf::from(path),
            blob: "0".repeat(40),
            bytes: bytes.to_vec(),
            loser_id8: "abcd1234".into(),
            loser_name: "Air".into(),
        }
    }

    fn snapshot(live: &[u8], siblings: Vec<SiblingView>) -> ResolutionSnapshot {
        ResolutionSnapshot {
            slug: "proj-claude".into(),
            live: PathBuf::from("CLAUDE.md"),
            head: "1".repeat(40),
            live_blob: "2".repeat(40),
            live_executable: false,
            live_bytes: live.to_vec(),
            root: None,
            siblings,
        }
    }

    #[test]
    fn only_an_applied_save_closes_the_window() {
        assert_eq!(
            next(Ok(ResolveOutcome::Applied(RootStatus::Synced))),
            Step::Close(RootStatus::Synced)
        );
        let fresh = snapshot(b"new", vec![]);
        assert_eq!(
            next(Ok(ResolveOutcome::Stale(Box::new(fresh.clone())))),
            Step::Refresh(Box::new(fresh)),
            "stale must redisplay, never close"
        );
        assert!(
            matches!(next(Ok(ResolveOutcome::Pending)), Step::Stay(_)),
            "pending must keep the draft on screen"
        );
        assert!(
            matches!(next(Err("boom".into())), Step::Stay(_)),
            "an error must keep the draft on screen"
        );
    }

    #[test]
    fn an_error_reaching_the_status_line_is_bounded() {
        // Engine output is git stderr with peer-authored paths in it.
        let Step::Stay(message) = next(Err(format!("x\n{}", "y".repeat(500)))) else {
            panic!("an error must not close or refresh");
        };
        assert!(message.chars().count() <= 81, "{message}");
        assert!(!message.contains('\n'));
    }

    #[test]
    fn only_ticked_siblings_are_offered_for_removal() {
        let siblings = vec![
            sibling("a.conflict-1.md", b"a"),
            sibling("b.conflict-2.md", b"b"),
        ];
        assert_eq!(
            selected_paths(&siblings, &[false, true]),
            vec![PathBuf::from("b.conflict-2.md")]
        );
        assert!(selected_paths(&siblings, &[false, false]).is_empty());
        // A shorter mask must not panic and must not select what it does not
        // cover — the refresh path rebuilds it, and a half-built one is not a
        // licence to delete.
        assert!(selected_paths(&siblings, &[]).is_empty());
    }

    #[test]
    fn a_refresh_keeps_what_the_user_decided_about_each_sibling() {
        let old = vec![
            sibling("a.conflict-1.md", b"a"),
            sibling("b.conflict-2.md", b"b"),
        ];
        let fresh = vec![
            sibling("b.conflict-2.md", b"b2"),
            sibling("c.conflict-3.md", b"c"),
        ];
        // `a` is gone, `b` was unticked and stays unticked, `c` is new.
        assert_eq!(
            carry_selection(&old, &[true, false], &fresh),
            vec![false, true]
        );
    }

    #[test]
    fn text_that_cannot_survive_a_round_trip_is_treated_as_binary() {
        assert!(!binary_mode(&snapshot(
            b"hello\n",
            vec![sibling("s", b"world\n")]
        )));
        assert!(
            binary_mode(&snapshot(b"a\0b", vec![sibling("s", b"ok")])),
            "a NUL on the live side"
        );
        assert!(
            binary_mode(&snapshot(b"ok", vec![sibling("s", b"a\0b")])),
            "a NUL on a sibling"
        );
        assert!(
            binary_mode(&snapshot(b"caf\xe9\n", vec![sibling("s", b"ok")])),
            "Latin-1 has no NUL, but editing it through a String would eat the byte"
        );
    }

    #[test]
    fn a_peer_authored_device_name_is_bounded_before_it_is_a_label() {
        assert_eq!(other_label("Air", false), "OTHER — from Air");
        assert_eq!(other_label("Air", true), "OTHER — from Air (this Mac)");
        let hostile = other_label(&format!("a\nb{}", "c".repeat(200)), false);
        assert!(!hostile.contains('\n'));
        assert!(hostile.chars().count() <= "OTHER — from ".chars().count() + 81);
    }

    #[test]
    fn the_destructive_half_of_save_is_counted_beside_the_button() {
        assert_eq!(removal_summary(0), None, "nothing ticked, nothing to warn");
        assert_eq!(
            removal_summary(1).as_deref(),
            Some("Save will remove 1 copy")
        );
        assert_eq!(
            removal_summary(3).as_deref(),
            Some("Save will remove 3 copies")
        );
    }

    fn conflict(live: &str) -> ConflictView {
        ConflictView {
            live: PathBuf::from(live),
            sibling: PathBuf::from(format!("{live}.conflict-abcd1234")),
            loser_id8: "abcd1234".into(),
            loser_name: "Air".into(),
            loser_is_me: false,
        }
    }

    /// An applied resolution lands on `AppState`, which outlives the window —
    /// the user may have closed it while the save was in flight, and the
    /// siblings are deleted either way.
    #[test]
    fn an_applied_resolution_replaces_only_its_own_slugs_rows() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut s = crate::state::test_state(tx);
        s.roots = vec![
            ("proj-a".into(), RootStatus::Conflicts(2)),
            ("proj-b".into(), RootStatus::Conflicts(1)),
        ];
        s.conflicts = vec![
            ("proj-a".into(), conflict("CLAUDE.md")),
            ("proj-a".into(), conflict("notes.md")),
            ("proj-b".into(), conflict("CLAUDE.md")),
        ];
        s.conflicts_total = conflicts_total(&s.roots);

        settle(
            &mut s,
            "proj-a",
            RootStatus::Conflicts(1),
            Some(vec![conflict("notes.md")]),
        );

        assert_eq!(
            s.roots,
            vec![
                ("proj-a".into(), RootStatus::Conflicts(1)),
                ("proj-b".into(), RootStatus::Conflicts(1)),
            ]
        );
        assert_eq!(s.conflicts_total, 2, "the badge follows the rows");
        let left: Vec<(&str, &str)> = s
            .conflicts
            .iter()
            .map(|(slug, c)| (slug.as_str(), c.live.to_str().unwrap()))
            .collect();
        assert_eq!(
            left,
            // `retain` then `extend`: this slug's rows are re-appended.
            vec![("proj-b", "CLAUDE.md"), ("proj-a", "notes.md")],
            "the other root's identically-named conflict must survive"
        );
    }

    #[test]
    fn a_root_the_window_never_heard_about_is_added_rather_than_dropped() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut s = crate::state::test_state(tx);
        settle(&mut s, "proj-a", RootStatus::Synced, None);
        assert_eq!(s.roots, vec![("proj-a".into(), RootStatus::Synced)]);
        assert_eq!(s.conflicts_total, 0);
    }

    #[test]
    fn a_code_editor_is_only_asked_for_where_this_build_can_highlight_it() {
        assert_eq!(code_lang(Path::new("settings.json")), Some("json"));
        assert_eq!(code_lang(Path::new("CLAUDE.md")), None);
        assert_eq!(code_lang(Path::new("Makefile")), None);
    }
}
