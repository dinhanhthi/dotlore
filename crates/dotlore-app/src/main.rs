//! Dotlore's macOS menu-bar app: a status item and one window over the same
//! engine and daemon loop the `dotlore` CLI runs.
//!
//! ## Verified API
//!
//! Read out of the pinned crate sources on 2026-09-19. All three come from
//! crates.io, not a git source: `gpui =0.2.2`, `gpui-component =0.5.1`,
//! `gpui-tray =0.1.3` (default features, so `gpui-tray`'s `menu-state` is
//! off — it calls `MenuItem::is_disabled`, which only the fork has).
//! `cargo tree -i gpui` is a single node used by all three; `cargo tree -i
//! gpui-pre` matches no package.
//!
//! - Bootstrap: `gpui::Application::new().run(|cx: &mut gpui::App| …)`
//!   (`Application::headless()` exists for no-window use).
//! - Components: `gpui_component::init(cx: &mut gpui::App)`. 0.5.1 is the last
//!   release on upstream `gpui ^0.2.2`; 0.6.x builds on the `gpui-pre` fork,
//!   whose `App` is a different type and cannot take this `cx`.
//! - Entities: `cx.new(|cx| …) -> Entity<T>`, `cx.update_entity(&e, |v, cx| …)`,
//!   `Entity::read(&App)`, `Entity::update(&mut cx, …)`, `Context::notify()`,
//!   `App::observe(&e, f)` / `Context::observe(&e, |this, e, cx| …) ->
//!   Subscription`. `new`, `update_entity` and `background_spawn` are
//!   `gpui::AppContext` trait methods, so `gpui::prelude::*` must be in scope.
//! - Background work: `cx.background_spawn(fut) -> Task<R>`. Foreground:
//!   `cx.spawn(async move |cx: &mut AsyncApp| …)` and, from a view that needs
//!   the window afterwards, `Context::spawn_in(&Window, async move |weak,
//!   cx: &mut AsyncWindowContext| …)` with `WeakEntity::update_in(cx, |this,
//!   window, cx| …)`. Sleeping: `cx.background_executor().timer(Duration)`.
//! - Path picker: `App::prompt_for_paths(PathPromptOptions { files,
//!   directories, multiple, prompt }) -> oneshot::Receiver<Result<
//!   Option<Vec<PathBuf>>>>`.
//! - Window: `App::open_window(WindowOptions, |&mut Window, &mut App| ->
//!   Entity<V>) -> Result<WindowHandle<V>>`; size and title through
//!   `WindowOptions { window_bounds: Some(WindowBounds::Windowed(
//!   Bounds::centered(None, size(px(w), px(h)), cx))), titlebar:
//!   Some(TitlebarOptions { title, appears_transparent, traffic_light_position
//!   }), .. }`. Task 4 draws its own header instead, so the titlebar comes
//!   from `gpui_component::TitleBar::title_bar_options()` — transparent, with
//!   the traffic lights placed for the component's 34 px bar.
//! - Actions: `gpui::actions!(namespace, [Name, …])`; the tray dispatches
//!   through `App::dispatch_action`, which reaches `App::on_action` listeners
//!   both with and without an open window.
//! - Tray: `gpui_tray::Tray::builder() -> TrayBuilder` with `.icon`,
//!   `.icon_name`, `.icon_theme_path`, `.title(impl Into<String>)`,
//!   `.tooltip`, `.visible`, `.on_activate(impl gpui::Action)`,
//!   `.menu(impl Fn(&mut App) -> Vec<gpui::MenuItem> + 'static)`, then
//!   `.build(cx) -> gpui_tray::Result<Tray>`. On the tray:
//!   `set_title`, `set_icon`, `set_tooltip`, `set_visible`,
//!   `refresh_menu(cx)` — which re-runs the stored menu closure and is the
//!   "rebuild the menu when the entity notifies" hook — and `close(cx)`.
//!   `Tray` is `!Send` with creation-thread affinity (`Error::WrongThread`)
//!   and removes the status item when dropped.
//! - `gpui::MenuItem` is `Separator | Submenu | SystemMenu | Action` with no
//!   `disabled()`/`is_disabled()`, so status lines are ordinary items wired to
//!   a no-op action. `gpui_tray` rejects `MenuItem::SystemMenu`.
//!
//! Confirmed for task 2, against the same 0.5.1 sources — the names in
//! `gpui-component 0.6.x` do **not** all match these:
//!
//! - Layout: `gpui_component::{v_flex, h_flex}` (free functions returning
//!   `Div`) plus the `StyledExt`, `Sizable` (`xsmall`/`small`/`large`) and
//!   `Disableable` (`disabled(bool)`) traits.
//! - Theme: `ActiveTheme for App` → `cx.theme()`, which derefs to
//!   `ThemeColor` with `background`, `foreground`, `muted_foreground`,
//!   `success`, `warning`, `danger`.
//! - Text entry: `InputState::new(window, cx)` (a `gpui::Entity<InputState>`)
//!   with `.placeholder(…)`, `.default_value(…)`, `.multi_line(bool)`,
//!   `.value() -> SharedString`, `.focus(window, cx)`; rendered by
//!   `gpui_component::input::Input::new(&Entity<InputState>)` (the element is
//!   `Input`, not `TextInput`). **A window using it must be wrapped in `gpui_component::Root::new(view.into(), window, cx)`** —
//!   `Root` owns the focused-input tracking the input reads back.
//! - Buttons and checkboxes: `Button::new(impl Into<ElementId>)` with
//!   `.label(…)`, `.outline()`, `.on_click(|&ClickEvent, &mut Window, &mut
//!   App|)`; `Checkbox::new(id).label(…).checked(bool)`.
//! - Not used, and not probed: `sheet.rs`, `dialog.rs`, `popover.rs`. The Add
//!   and Link flows render inline instead, so nothing here depends on a modal
//!   API. `badge.rs` is an overlay dot/count for an icon, not a status pill,
//!   so a root's status is a leading glyph plus a right-aligned label built
//!   out of plain `div`s — see `theme`.
//! - Also used from task 4: `TitleBar` (`title_bar_options()` plus the
//!   `RenderOnce` component, which owns the macOS traffic-light inset) and
//!   `Theme::change(ThemeMode::Dark, …)`, which sticks because nothing in
//!   0.5.1 re-syncs the mode after `gpui_component::init`. `ButtonVariants`
//!   supplies `.ghost()` / `.primary()`. Icons are *not* used: this app
//!   installs no asset source, so `IconName` SVGs would not resolve.
//!
//! Read out of the same sources for task 5, and what the window's type scale
//! is built on — see [`crate::theme`]:
//!
//! - `Sizable::{xsmall, small, large}` and `with_size(Size)`. `Size` is
//!   `XSmall | Small | Medium | Large | Size(Pixels)`, **default `Medium`**.
//!   `StyleSized::button_text_size` maps `XSmall → text_xs` (12 px), `Small →
//!   text_sm` (14), everything else → `text_base` (16); `input_text_size`
//!   maps `XSmall → 12`, `Small`/`Medium → 14`, `Large → 16`; `Checkbox`
//!   matches on the size itself and maps `XSmall/Small/Medium/Large →
//!   text_xs/text_sm/text_base/text_lg`. `Size::Size(px)` does **not** reach
//!   `button_text_size`, so a custom pixel size on a `Button` still draws a
//!   16 px label — the only way to set a button's text size is the step.
//!   Heights: button `h_5/h_6/h_8`, input `h_5/h_6/h_8/h_11`, checkbox box
//!   `size_3/size_3p5/size_4/rems(1.125)`. `window.rem_size()` is 16 px, so
//!   `text_xs/sm/base` really are 12/14/16.
//! - `Button::compact()` is a public builder that swaps `Small`'s `px_3` for
//!   `px_1p5`.
//! - gpui's own `rems`-based `text_*`, `text_right()`, and no letter-spacing.
//!
//! Probed for Phase 6 against the same sources — task 2 rendered everything
//! inline and never went near these, so none of it was confirmed before:
//!
//! - Multi-line entry: `InputState::multi_line(bool)`, `code_editor(impl
//!   Into<SharedString>)`, `rows(usize)`, `soft_wrap(bool)` (**default on**),
//!   `set_value(impl Into<SharedString>, &mut Window, &mut Context<_>)`.
//!   There is **no read-only flag** anywhere in `src/input/`:
//!   `Input::disabled(true)` is the whole of it, and it is enough — the
//!   element skips binding every editing action, and both writers,
//!   `replace_text_in_range` and the IME's `replace_and_mark_text_in_range`,
//!   return early on `state.disabled`, while selection, `copy`, the scroll
//!   wheel and the `Scrollbar` stay bound. `set_value` clears the flag around
//!   its own write, so a disabled side can still be refreshed. A disabled
//!   input draws on `theme().muted`.
//! - `InputState::value()` is `SharedString::new(self.text.to_string())` off
//!   the `Rope`, and the rope keeps `\r` (see `rope_ext`'s own doctests), so
//!   a draft round-trips byte for byte — only a line the user adds gets a
//!   bare `\n`, as in any editor.
//! - `code_editor(lang)` sets `searchable = true`, and the search panel draws
//!   `IconName` SVGs this app has no asset source for, so the resolver turns
//!   it back off with `searchable(false)`.
//! - `Sizable for Input`: `xsmall()` is the only step whose `input_text_size`
//!   is 12 px, which is what the resolver's column arithmetic assumes.
//!   `Input::h_full()` / `h(DefiniteLength)` size a multi-line editor; the
//!   font family is inherited from the parent, not set by the component.
//! - `code_editor` is **json-only at these pins**: `highlighter/languages.rs`
//!   defines `enum Language { Json }` under `#[cfg(not(feature =
//!   "tree-sitter-languages"))]`, `Language::from_str` is a bare `return
//!   Self::Json`, and `registry.language(name)` falls back through it — so
//!   `code_editor("markdown")` would draw a `CLAUDE.md` through a JSON
//!   parser. `tree-sitter-languages` is not a default feature and pulls in
//!   ~30 crates. No `Cargo.toml` change was in scope, so the resolver asks
//!   for a code editor only for `.json`/`.jsonc`.
//! - **No modal API is used.** `sheet.rs` / `dialog.rs` were still not
//!   probed: the amendment of 2026-09-19 made the resolver its own window
//!   instead, so nothing depends on them.
//! - Second window: `App::open_window` again, `Window::remove_window()`
//!   (gpui `window.rs:1375` — it only sets `removed`; the teardown happens
//!   when the enclosing `update_window` returns) to close it from inside its
//!   own view, and `WindowHandle::update(cx, …)` — `Err` when the window is
//!   gone — as the liveness check, since 0.2.2 has no *per-window* close
//!   notification: `App::on_window_closed` (`app.rs:1806`, driven from
//!   `app.rs:1378`) is global and carries no `WindowId`, and
//!   `Window::on_window_should_close` (`window.rs:4329`) is a pre-close veto,
//!   not a notification.
//! - Async contexts are **not** interchangeable: `AsyncWindowContext::
//!   update_entity` routes through its window handle (`async_context.rs:384`)
//!   and fails once that window is gone, while `AsyncApp`'s goes straight to
//!   the app (`async_context.rs:23`). `AsyncWindowContext` derefs to
//!   `AsyncApp`, which is how the resolver's save lands its state change even
//!   if the user closed the window mid-flight.
//! - Cross-window state: `gpui::Global` plus `App::default_global::<G>() ->
//!   &mut G` / `try_global` / `set_global`, which is where the "only one
//!   resolver, focus the open one" rule lives.

mod login_item;
mod resolver;
mod state;
mod theme;
mod tray;
mod window;

use std::cell::RefCell;
use std::fs::{File, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::PoisonError;
use std::time::Duration;

use anyhow::Result;
use gpui::prelude::*;
use gpui::{
    px, size, App, Application, AsyncApp, Bounds, Entity, TitlebarOptions, WindowBounds,
    WindowHandle, WindowOptions,
};
use gpui_component::{Root, TitleBar};

use dotlore_core::config::{self, Config};
use dotlore_core::daemon::Cmd;
use dotlore_core::git;

use state::{AppState, StatusUpdate};
use tray::{Noop, OpenWindow, Quit, SyncNow};
use window::MainWindow;

/// How often the UI thread drains the daemon's status channel.
const POLL_UI: Duration = Duration::from_millis(500);
/// The window, up from the plan's 520×440.
///
/// Width: 16 px padding each side leaves 528 px of content. A root row's
/// first line spends 16 on the dot column, 104 on the status column, a
/// compact ghost `Remove` and three 8 px gaps, leaving about 300 for the
/// slug; its second line is the path, indented 24 and otherwise free — about
/// 504 px, which is the 70-odd characters [`crate::theme::PATH_CHARS`] is
/// budgeted under.
///
/// Height: the drawn titlebar takes 34 and the status bar 33; the provider
/// section with its disclosure open is about 150, and each root row is a
/// 24 px line plus a 12 px path line plus padding, about 46. 520 fits four
/// rows with the provider open and the Add draft showing; 440 fits two.
const WINDOW: (f32, f32) = (560., 520.);

fn main() {
    // The only environment this binary reads, once, at startup — the same
    // documented exception `dotlore-cli` has.
    let home = config::default_home();
    let home_dir = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());

    let git_missing = git::which_git().is_none();
    let cfg = match load(&home) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("dotlore: {e:#}");
            std::process::exit(1);
        }
    };
    // Held, not dropped: the binding keeps the file — and the lock — alive for
    // as long as the process, and `Application::run` never returns.
    let _instance = single_instance(&home);

    Application::new().run(move |cx: &mut App| {
        // First, and before anything draws: this is the `finish_launching`
        // callback, so gpui has just made the app a Dock app.
        hide_dock_icon();
        gpui_component::init(cx);

        // One status channel for the life of the process: the sender lives in
        // `AppState`, so the poller below survives a provider change that
        // starts the first daemon long after launch.
        let (status_tx, status_rx) = mpsc::channel::<StatusUpdate>();
        let state = cx.new(|_| AppState {
            home,
            home_dir,
            cfg,
            engine: None,
            roots: Vec::new(),
            conflicts_total: 0,
            conflicts: Vec::new(),
            git_missing,
            cmd_tx: None,
            last_error: None,
            transition: None,
            status_tx,
        });

        poll_status(cx, state.clone(), status_rx);
        register_actions(cx, state.clone());

        // With a provider already configured this starts the engine and the
        // daemon now; without one it does nothing, and the window's provider
        // buttons start them instead — no restart.
        let no_provider = state.update(cx, |s, _| {
            s.start_runtime();
            s.cfg.provider_dir.is_none()
        });

        match tray::build(cx, state.clone()) {
            // The closure owns the `Tray` — dropping it would remove the
            // status item — and `detach` keeps the subscription alive, which
            // is what rebuilds the menu after every notify.
            Ok(tray) => cx
                .observe(&state, move |state, cx| tray::refresh(&tray, &state, cx))
                .detach(),
            Err(e) => {
                eprintln!("dotlore: cannot create the menu bar item: {e}");
                cx.quit();
                // `quit` schedules the termination, it does not unwind — and
                // an app on its way out must not open a window first.
                return;
            }
        }

        // Nothing can sync until a provider is picked, and the window is the
        // only place to pick one.
        if no_provider {
            cx.dispatch_action(&OpenWindow);
        }
    });
}

/// Fresh config under the home lock.
///
/// The lock is dropped before anything builds an engine: every engine entry
/// point takes it again and `std::fs::File::lock` is not reentrant.
fn load(home: &Path) -> Result<Config> {
    let _guard = config::lock(home)?;
    Config::load(home)
}

/// Drop the Dock tile.
///
/// `Info.plist` sets `LSUIElement` and LaunchServices honours it — but gpui
/// 0.2.2 then calls `[NSApp setActivationPolicy:
/// NSApplicationActivationPolicyRegular]` unconditionally from its
/// `applicationDidFinishLaunching` (`gpui-0.2.2/src/platform/mac/platform.rs`
/// line 1390), which runs *after* the plist was applied and therefore wins:
/// `lsappinfo` reports the bundle as `Foreground`, and the app gets a Dock
/// icon it has no business having. 0.2.2 exposes no activation-policy API, so
/// this undoes it with the raw selector, from the `finish_launching` callback
/// that same function invokes a few statements later.
///
/// No new crate: `libobjc` is already linked by gpui. `objc_msgSend` is
/// declared **non-variadic** on purpose — on Apple arm64 a variadic call
/// passes its arguments on the stack while the method reads `x2`, so a
/// variadic declaration would deliver garbage as the policy.
fn hide_dock_icon() {
    use std::ffi::{c_char, c_void};

    /// `NSApplicationActivationPolicyAccessory`: no Dock tile, no app menu,
    /// windows still shown when the app asks for one.
    const ACCESSORY: isize = 1;

    #[link(name = "objc", kind = "dylib")]
    unsafe extern "C" {
        #[link_name = "objc_getClass"]
        fn objc_get_class(name: *const c_char) -> *mut c_void;
        #[link_name = "sel_getUid"]
        fn sel_get_uid(name: *const c_char) -> *mut c_void;
        // One declaration for both sends: `sharedApplication` ignores the
        // third argument and `setActivationPolicy:` returns a `BOOL` that
        // both ABIs hand back in the same register as an `id`.
        #[link_name = "objc_msgSend"]
        fn objc_msg_send(obj: *mut c_void, sel: *mut c_void, arg: isize) -> *mut c_void;
    }

    unsafe {
        let class = objc_get_class(c"NSApplication".as_ptr());
        if class.is_null() {
            return;
        }
        let app = objc_msg_send(class, sel_get_uid(c"sharedApplication".as_ptr()), 0);
        if app.is_null() {
            return;
        }
        let _ = objc_msg_send(
            app,
            sel_get_uid(c"setActivationPolicy:".as_ptr()),
            ACCESSORY,
        );
    }
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

/// Drain the daemon's status channel on the UI thread.
///
/// `try_recv` on a timer rather than a blocking receive, because this runs on
/// the foreground executor and must never park it.
fn poll_status(cx: &mut App, state: Entity<AppState>, rx: Receiver<StatusUpdate>) {
    cx.spawn(async move |cx: &mut AsyncApp| {
        loop {
            cx.background_executor().timer(POLL_UI).await;
            loop {
                let update = match rx.try_recv() {
                    Ok(update) => update,
                    Err(TryRecvError::Empty) => break,
                    // `AppState` holds the sender, so in practice this only
                    // happens once the entity is gone.
                    Err(TryRecvError::Disconnected) => return,
                };
                match &update {
                    Ok(roots) => println!("sync_all: {} roots", roots.len()),
                    Err(e) => eprintln!("dotlore: sync_all: {e}"),
                }
                let applied = cx.update_entity(&state, |state, cx| {
                    if state.apply(update) {
                        cx.notify();
                    }
                });
                if applied.is_err() {
                    return;
                }
                // Every cycle, not only the ones that moved a status: the
                // cycle that just ran reloaded config from disk, and this is
                // how the window notices a `dotlore add`, `rm` or `provider`
                // run from a terminal while the app is up.
                let state = state.clone();
                if cx.update(|cx| refresh_from_engine(cx, state)).is_err() {
                    return;
                }
            }
        }
    })
    .detach();
}

/// Re-read what the window renders from: the config the last cycle reloaded,
/// and the conflicts behind the third section.
///
/// Off the UI thread like every other engine call. The lock is the one the
/// daemon just released; `Engine::conflicts` is only called for the roots the
/// cycle reported conflicts on.
fn refresh_from_engine(cx: &mut App, state: Entity<AppState>) {
    let (engine, slugs) = {
        let s = state.read(cx);
        (s.engine.clone(), s.conflict_slugs())
    };
    let Some(engine) = engine else { return };
    let task = cx.background_spawn(async move {
        let mut e = engine.lock().unwrap_or_else(PoisonError::into_inner);
        let mut conflicts = Vec::new();
        for slug in slugs {
            // A root whose conflicts cannot be listed right now still has its
            // badge; the next cycle asks again.
            if let Ok(views) = e.conflicts(&slug) {
                conflicts.extend(views.into_iter().map(|v| (slug.clone(), v)));
            }
        }
        (e.cfg.clone(), conflicts)
    });
    cx.spawn(async move |cx: &mut AsyncApp| {
        let (cfg, conflicts) = task.await;
        let _ = cx.update_entity(&state, |s, cx| {
            let mut moved = s.adopt_cfg(cfg);
            if s.conflicts != conflicts {
                s.conflicts = conflicts;
                moved = true;
            }
            if moved {
                cx.notify();
            }
        });
    })
    .detach();
}

fn register_actions(cx: &mut App, state: Entity<AppState>) {
    // What the per-root status lines dispatch: they exist to be read, and
    // this `gpui` cannot render a disabled menu item.
    cx.on_action(|_: &Noop, _cx| {});

    let sync = state.clone();
    cx.on_action(move |_: &SyncNow, cx| sync.read(cx).send(Cmd::SyncNow));

    let quit = state.clone();
    cx.on_action(move |_: &Quit, cx| {
        // Best effort: the daemon only reads this between cycles, and the
        // process is about to go anyway. A cycle cut short is recovered from
        // its journal on the next run.
        quit.read(cx).send(Cmd::Quit);
        cx.quit();
    });

    let open = RefCell::new(None::<WindowHandle<Root>>);
    cx.on_action(move |_: &OpenWindow, cx| {
        let mut open = open.borrow_mut();
        if let Some(handle) = *open {
            if handle
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
            {
                cx.activate(true);
                return;
            }
            *open = None;
        }
        match open_window(cx, state.clone()) {
            Ok(handle) => {
                *open = Some(handle);
                cx.activate(true);
            }
            Err(e) => eprintln!("dotlore: cannot open the window: {e:#}"),
        }
    });
}

fn open_window(cx: &mut App, state: Entity<AppState>) -> Result<WindowHandle<Root>> {
    let bounds = Bounds::centered(None, size(px(WINDOW.0), px(WINDOW.1)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        // Transparent, with the traffic lights placed for a 34 px bar: the
        // window draws its own header, so the body's dark palette runs all the
        // way to the top instead of meeting a system-coloured strip. The title
        // is still set — a transparent titlebar does not draw it, but Mission
        // Control, Cmd-Tab, the Window menu and VoiceOver all read it.
        titlebar: Some(TitlebarOptions {
            title: Some("Dotlore".into()),
            ..TitleBar::title_bar_options()
        }),
        ..Default::default()
    };
    cx.open_window(options, |window, cx| {
        let main = cx.new(|cx| MainWindow::new(state, window, cx));
        // `TextInput` reads its focus state back through the window's `Root`.
        cx.new(|cx| Root::new(main, window, cx))
    })
}

/// A unique scratch directory for tests. `tempfile` is a `dotlore-core` dev
/// dependency; this crate has none, and one function is cheaper than one.
#[cfg(test)]
pub(crate) fn tmpdir(tag: &str) -> PathBuf {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let dir =
        std::env::temp_dir().join(format!("dotlore-app-{tag}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_guard_holds_the_lock_for_as_long_as_it_is_held() {
        let home = tmpdir("instance");
        // The `exit(0)` branch cannot be exercised in-process, but the
        // property it rests on can: a second opener must be turned away for
        // exactly as long as the handle lives. `flock` is per open file
        // description, so this contends with itself.
        let guard = single_instance(&home).expect("a lock on a fresh scratch home");
        let second = File::open(home.join("app.lock")).expect("the lock file exists");
        assert!(
            matches!(second.try_lock(), Err(TryLockError::WouldBlock)),
            "a second copy must not get the lock while the first is running"
        );

        drop(guard);
        // Bounded rather than immediate: measured, this assertion flaked
        // about once in 25 full-suite runs and never once alone or in 200k
        // single-threaded iterations. Another test in this binary runs a real
        // daemon, and `git::Git::command` spawns a bare `git` with a `PATH` in
        // its environment — the shape that makes std fall back to `fork`, so a
        // child forked while the lock file was open holds a duplicate of the
        // locked description until its `exec` closes it. Microseconds, not
        // zero.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let freed = loop {
            match second.try_lock() {
                Ok(()) => break true,
                Err(_) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(_) => break false,
            }
        };
        assert!(freed, "the lock must go when the app that held it does");

        drop(second);
        std::fs::remove_dir_all(&home).ok();
    }
}
