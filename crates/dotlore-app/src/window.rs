//! The one window: provider, tracked roots, conflicts.
//!
//! Nothing here calls the engine on the UI thread. Every engine call goes
//! through [`MainWindow::engine_op`], which locks the shared engine on the
//! background executor and posts the result back into the entity — `Engine`
//! takes a blocking `File::lock` on the home that a CLI can be holding, and a
//! UI-thread wait on it would freeze the whole app.
//!
//! Every colour, size and glyph comes from [`crate::theme`]; this file only
//! decides layout, and nothing below it holds a colour literal.
//!
//! ## Two rules this file holds to
//!
//! 1. **Every interactive element is explicitly `.small()`.** No `Button`,
//!    `Input` or `Checkbox` may take a `gpui_component` default — `Medium` is
//!    `text_base`, 16 px, two steps off everything around it. `Small` is the
//!    one step whose text is 14 px, which is the window's body size. Ghost
//!    buttons that sit *inline with text* add `.compact()`, which trades the
//!    12 px side padding for 6 px; buttons that stand in a row of their own
//!    (the provider choices, `Add`/`Cancel`) keep the full padding, so peers
//!    always look alike.
//! 2. **One left edge and one right edge.** The scroll body is the only thing
//!    with horizontal padding (`px_4`); nothing inside a section adds its own,
//!    so every label, path, row and note starts at the same x, and every
//!    trailing control ends at the same x.
//!
//! UI-only state (the Add draft, the Link list, the Google Drive list) lives
//! on this view rather than in [`AppState`]: every `AppState` notify rebuilds
//! the menu-bar menu, and a keystroke in the slug field must not do that.

use std::path::{Path, PathBuf};
use std::sync::PoisonError;

use gpui::prelude::*;
use gpui::{
    div, App, Context, Entity, FontWeight, PathPromptOptions, SharedString, Subscription, Window,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::checkbox::Checkbox;
use gpui_component::input::{Input, InputState};
use gpui_component::{h_flex, v_flex, ActiveTheme, Disableable, Sizable, TitleBar};

use dotlore_core::config::{self, Config};
use dotlore_core::daemon::Cmd;
use dotlore_core::engine::{self, ConflictView, Engine, RootStatus};

use crate::login_item;
use crate::state::{AppState, Next, Transition};
use crate::theme::{self, tone, Tone};
use crate::tray;

/// Inside `~`, the folder iCloud Drive syncs.
const ICLOUD: &str = "Library/Mobile Documents/com~apple~CloudDocs";
/// Inside `~`, where the Google Drive client mounts each account.
const CLOUD_STORAGE: &str = "Library/CloudStorage";
/// What the Google Drive client calls the account's own root.
const MY_DRIVE: &str = "My Drive";

/// A path picked by `Add…`, with the slug the user may still edit.
struct Draft {
    path: PathBuf,
    slug: Entity<InputState>,
}

pub struct MainWindow {
    state: Entity<AppState>,
    draft: Option<Draft>,
    /// Cloud slugs this device does not track yet; `Some(vec![])` is "asked,
    /// and there are none".
    linkable: Option<Vec<String>>,
    /// A slug picked in the Link list, waiting for its local path.
    gdrive: Option<Vec<PathBuf>>,
    /// The last UI action that failed, as opposed to the last *cycle* that
    /// failed — that one is `AppState::last_error`.
    error: Option<String>,
    /// How many background operations are in flight. A counter, not a flag:
    /// [`Self::engine_op`] and [`Self::set_login_item`] can overlap, and
    /// whichever finished first used to re-enable every control while the
    /// other was still running.
    inflight: usize,
    /// Whether the provider section shows its buttons. Purely presentational:
    /// the disclosure reveals actions that were always there, it does not add
    /// any.
    provider_open: bool,
    /// The start-at-login plist, as of the last time it was worth a `stat`:
    /// startup, opening the disclosure, and the end of a toggle. Not per
    /// render — the checkbox is not even drawn most frames.
    login_on: bool,
    _tracked: Subscription,
}

impl MainWindow {
    pub fn new(state: Entity<AppState>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Dark whatever the Mac is set to — see `theme`. Nothing in
        // `gpui_component` re-syncs the mode after `init`, so once is enough.
        theme::install(cx);
        let tracked = cx.observe(&state, |_, _, cx| cx.notify());
        // Once, here: on first run there is no provider, so the disclosure is
        // forced open and the checkbox is drawn before anything can toggle it.
        let login_on = login_item::is_enabled(&state.read(cx).home_dir);
        Self {
            state,
            draft: None,
            linkable: None,
            gdrive: None,
            error: None,
            inflight: 0,
            provider_open: false,
            login_on,
            _tracked: tracked,
        }
    }

    // --- engine plumbing ---------------------------------------------------

    /// Run `op` against the shared engine off the UI thread, then `done` back
    /// on it. The pattern task 3 and Phase 6 copy.
    fn engine_op<T: Send + 'static>(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
        op: impl FnOnce(&mut Engine) -> anyhow::Result<T> + Send + 'static,
        done: impl FnOnce(&mut Self, Result<T, String>, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let Some(engine) = self.state.read(cx).engine.clone() else {
            self.error = Some("set a cloud folder first".into());
            cx.notify();
            return;
        };
        self.inflight += 1;
        self.error = None;
        let task = cx.background_spawn(async move {
            // A panic elsewhere cannot leave the engine half-written: every
            // entry point reloads config from disk under the home lock.
            let mut e = engine.lock().unwrap_or_else(PoisonError::into_inner);
            let out = op(&mut e).map_err(|e| format!("{e:#}"));
            // The config as the operation left it, already reloaded under the
            // home lock — the rows on screen must not lag behind it.
            (out, e.cfg.clone())
        });
        cx.spawn_in(window, async move |this, cx| {
            let (out, cfg) = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.inflight = this.inflight.saturating_sub(1);
                this.adopt(cfg, cx);
                done(this, out, window, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Replace the config the window renders from, notifying only when
    /// something visible moved — a notify here rebuilds the tray menu.
    fn adopt(&self, cfg: Config, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            if s.adopt_cfg(cfg) {
                cx.notify();
            }
        });
    }

    fn send(&self, cmd: Cmd, cx: &Context<Self>) {
        self.state.read(cx).send(cmd);
    }

    /// Whether every control is disabled.
    ///
    /// A provider change in flight counts, not just [`Self::engine_op`]: until
    /// it finishes there is no engine, and `Add`/`Link`/`Remove` would answer
    /// `set a cloud folder first` while a cloud folder is visibly being set.
    fn busy(&self, cx: &Context<Self>) -> bool {
        self.inflight > 0 || matches!(self.state.read(cx).transition, Some(Transition::Pending))
    }

    // --- provider ----------------------------------------------------------

    /// Point the engine at `dir` and bring the runtime with it.
    ///
    /// First setup builds the engine and daemon here, without a restart. A
    /// change with a runtime already going serializes through that runtime's
    /// own mutex — the same order the daemon takes it in, engine then home —
    /// and then only reloads it. There is never a second daemon.
    fn set_provider(&mut self, dir: PathBuf, window: &Window, cx: &mut Context<Self>) {
        let (home, home_dir, engine) = {
            let s = self.state.read(cx);
            (s.home.clone(), s.home_dir.clone(), s.engine.clone())
        };
        self.error = None;
        self.gdrive = None;
        self.linkable = None;
        self.state.update(cx, |s, cx| {
            s.begin_transition();
            cx.notify();
        });
        let task = cx.background_spawn(async move {
            // Engine mutex first, then the home lock inside
            // `configure_provider` — the order the daemon's cycle takes them
            // in, so the transition queues behind a cycle instead of racing it.
            let _serialized = engine
                .as_ref()
                .map(|e| e.lock().unwrap_or_else(PoisonError::into_inner));
            let roots =
                engine::configure_provider(&home, &home_dir, &dir).map_err(|e| format!("{e:#}"))?;
            let cfg = (|| -> anyhow::Result<Config> {
                let _g = config::lock(&home)?;
                Config::load(&home)
            })()
            .map_err(|e| format!("{e:#}"))?;
            Ok((cfg, roots))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, _window, cx| {
                this.state.update(cx, |s, cx| {
                    match s.finish_transition(result) {
                        Next::Idle => {}
                        Next::StartRuntime => s.start_runtime(),
                        // The running engine still holds the old provider in
                        // memory; nothing reads it, because every entry point
                        // reloads config and cloud from disk under the home
                        // lock, and this cycle is the first to do so.
                        Next::Reload => s.send(Cmd::Reload),
                    }
                    cx.notify();
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// Install or remove the start-at-login LaunchAgent, off the UI thread.
    ///
    /// Not [`Self::engine_op`]: that one refuses without a configured engine,
    /// and start-at-login has to work on a Mac that has not picked a cloud
    /// folder yet. `launchctl` is a subprocess all the same, so it goes to the
    /// background executor like every other blocking call in this file.
    fn set_login_item(&mut self, enabled: bool, window: &Window, cx: &mut Context<Self>) {
        let home_dir = self.state.read(cx).home_dir.clone();
        self.inflight += 1;
        self.error = None;
        let task = cx.background_spawn(async move {
            // The checkbox is a cached `stat`, so a plist created or deleted
            // from outside the app while the disclosure was open makes the
            // click ask for what is already true. Running `launchctl` anyway
            // means `bootstrap` on a loaded job, which exits non-zero and puts
            // a failure banner over a plist that is exactly right. Re-read
            // here — off the UI thread, like the rest of this — and if disk
            // already agrees, the only thing left to do is refresh the
            // checkbox.
            if login_item::is_enabled(&home_dir) == enabled {
                return (Ok(()), enabled);
            }
            let out = login_item::set(&home_dir, enabled).map_err(|e| format!("{e:#}"));
            // Read back here rather than trusting the click: a half-failed
            // toggle must leave the checkbox showing the plist. Off the UI
            // thread with the rest of the operation.
            (out, login_item::is_enabled(&home_dir))
        });
        cx.spawn_in(window, async move |this, cx| {
            let (out, on) = task.await;
            let _ = this.update_in(cx, |this, _window, cx| {
                this.inflight = this.inflight.saturating_sub(1);
                this.login_on = on;
                if let Err(e) = out {
                    this.error = Some(e);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// List the mounted Google Drive accounts, off the UI thread.
    ///
    /// `~/Library/CloudStorage` is a File Provider mount served by the Google
    /// Drive helper: when the helper is wedged, `read_dir` and the `is_dir`
    /// stat behind it block for as long as it takes. On the UI thread that
    /// freezes the window *and* the tray, which shares the foreground
    /// executor. No `inflight`: a second click just posts the same list back
    /// twice.
    fn list_gdrive(&mut self, window: &Window, cx: &mut Context<Self>) {
        let home_dir = self.state.read(cx).home_dir.clone();
        let task = cx.background_spawn(async move { google_drive_dirs(&home_dir) });
        cx.spawn_in(window, async move |this, cx| {
            let dirs = task.await;
            let _ = this.update_in(cx, |this, _window, cx| {
                this.gdrive = Some(dirs);
                cx.notify();
            });
        })
        .detach();
    }

    fn pick_provider(&mut self, window: &Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            let Some(dir) = one_path(rx.await) else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| this.set_provider(dir, window, cx));
        })
        .detach();
    }

    // --- add / link / remove ------------------------------------------------

    fn pick_add(&mut self, window: &Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: true,
            multiple: false,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            let Some(path) = one_path(rx.await) else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| {
                let home_dir = this.state.read(cx).home_dir.clone();
                let slug = config::default_slug(&path, &home_dir);
                let input = cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder("slug")
                        .default_value(slug)
                });
                input.update(cx, |input, cx| input.focus(window, cx));
                this.linkable = None;
                this.draft = Some(Draft { path, slug: input });
                cx.notify();
            });
        })
        .detach();
    }

    fn confirm_add(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(draft) = self.draft.as_ref() else {
            return;
        };
        let path = draft.path.clone();
        let typed = draft.slug.read(cx).value().trim().to_string();
        // An emptied field means "whatever the engine would have picked".
        let slug = (!typed.is_empty()).then_some(typed);
        self.engine_op(
            window,
            cx,
            move |e| e.add_root(&path, slug.as_deref()),
            |this, out, _window, cx| match out {
                Ok(_) => {
                    this.draft = None;
                    this.send(Cmd::Reload, cx);
                }
                Err(e) => this.error = Some(e),
            },
        );
    }

    fn list_linkable(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.draft = None;
        self.engine_op(
            window,
            cx,
            |e| {
                // ponytail: reads `e.cloud` without a reload, so the first
                // listing after a provider change can name the old provider's
                // slugs until the `Cmd::Reload` cycle refreshes the engine.
                let tracked: Vec<&str> = e.cfg.roots.iter().map(|r| r.slug.as_str()).collect();
                Ok(e.cloud
                    .list_slugs()
                    .into_iter()
                    .filter(|s| !tracked.contains(&s.as_str()))
                    .collect::<Vec<_>>())
            },
            |this, out, _window, cx| match out {
                Ok(slugs) => {
                    this.linkable = Some(slugs);
                    cx.notify();
                }
                Err(e) => this.error = Some(e),
            },
        );
    }

    fn pick_link_path(&mut self, slug: String, window: &Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: true,
            multiple: false,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            let Some(path) = one_path(rx.await) else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| {
                let slug = slug.clone();
                this.engine_op(
                    window,
                    cx,
                    move |e| e.link_root(&slug, &path),
                    |this, out, _window, cx| match out {
                        // A `Pending` link is not a failure: the daemon
                        // finishes it on a later cycle, and the row shows the
                        // grey badge until it does.
                        Ok(_) => {
                            this.linkable = None;
                            this.send(Cmd::Reload, cx);
                        }
                        Err(e) => this.error = Some(e),
                    },
                );
            });
        })
        .detach();
    }

    fn remove(&mut self, slug: String, window: &Window, cx: &mut Context<Self>) {
        self.engine_op(
            window,
            cx,
            move |e| e.remove_root(&slug),
            |this, out, _window, cx| match out {
                // Reload, not just SyncNow: the watch set has to lose the root.
                Ok(()) => this.send(Cmd::Reload, cx),
                Err(e) => this.error = Some(e),
            },
        );
    }

    // --- rendering -----------------------------------------------------------

    /// A section header: the caps label, and its affordances right-aligned on
    /// the same line.
    fn header(label: &str, actions: impl IntoElement, cx: &App) -> impl IntoElement {
        h_flex()
            // The same height a `.small()` button is, so a header with
            // affordances and one without sit on the same rhythm.
            .h(theme::ROW_H)
            .items_center()
            .justify_between()
            .child(theme::section_label(label, cx))
            .child(actions)
    }

    fn provider_section(&self, cx: &Context<Self>) -> impl IntoElement {
        let s = self.state.read(cx);
        let home_dir = s.home_dir.clone();
        let (current, current_tone) = match (&s.transition, &s.cfg.provider_dir) {
            (Some(Transition::Pending), _) => ("Setting up…".to_string(), Tone::Muted),
            (_, Some(p)) => (
                theme::short_path(p, &home_dir, theme::PROVIDER_CHARS),
                Tone::Good,
            ),
            (_, None) => ("Not set".to_string(), Tone::Muted),
        };
        let failed = match &s.transition {
            Some(Transition::Error(e)) => Some(e.clone()),
            _ => None,
        };
        let busy = self.busy(cx);
        let git_missing = s.git_missing;
        // Nothing can sync without a provider, so on first run the section is
        // open and its disclosure is inert.
        let unset = s.cfg.provider_dir.is_none();
        let open = self.provider_open || unset;

        let icloud = home_dir.join(ICLOUD);
        let gdrive = self.gdrive.clone();
        // The plist as of the last read, not a remembered click: a `launchctl
        // unload` or a deleted agent from outside the app shows up the next
        // time the disclosure is opened.
        let login_on = self.login_on;

        v_flex()
            .gap_2()
            .child(Self::header(
                "Provider",
                Button::new("provider-toggle")
                    .ghost()
                    .small()
                    .compact()
                    .label(if open { "▾" } else { "▸" })
                    .tooltip("Change the cloud folder")
                    .disabled(unset)
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.provider_open = !this.provider_open;
                        // One `stat`, when the checkbox is about to be drawn.
                        if this.provider_open {
                            this.login_on = login_item::is_enabled(&this.state.read(cx).home_dir);
                        }
                        cx.notify();
                    })),
                cx,
            ))
            .child(theme::mono_line(
                current,
                match current_tone {
                    Tone::Good => cx.theme().foreground,
                    _ => theme::dim(),
                },
                cx,
            ))
            .when(git_missing, |el| {
                el.child(banner("git not found — run: xcode-select --install", cx))
            })
            .when_some(failed, |el, e| el.child(banner(&e, cx)))
            .when(open, |el| {
                el.child(
                    v_flex()
                        // Same gap as the section's own, so the disclosure
                        // adds rows to one rhythm instead of starting a
                        // second one.
                        .gap_2()
                        .child(
                            h_flex()
                                .gap_2()
                                .child(
                                    Button::new("icloud")
                                        .outline()
                                        .small()
                                        .label("iCloud Drive")
                                        .disabled(busy)
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.set_provider(icloud.clone(), window, cx)
                                        })),
                                )
                                .child(
                                    Button::new("gdrive")
                                        .outline()
                                        .small()
                                        .label("Google Drive…")
                                        .disabled(busy)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.list_gdrive(window, cx)
                                        })),
                                )
                                .child(
                                    Button::new("other")
                                        .outline()
                                        .small()
                                        .label("Other…")
                                        .disabled(busy)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.pick_provider(window, cx)
                                        })),
                                ),
                        )
                        .when_some(gdrive, |el, dirs| {
                            if dirs.is_empty() {
                                return el.child(note("No Google Drive folder found", cx));
                            }
                            // `items_start`: a `v_flex` stretches its
                            // children, and a stretched `Button` centres its
                            // label — which would put this list on an x of
                            // its own.
                            let col = v_flex().gap_1().items_start();
                            el.child(dirs.into_iter().fold(col, |col, dir| {
                                let label = name_of(&dir);
                                let target = dir.join(MY_DRIVE);
                                col.child(
                                    Button::new(SharedString::from(format!("gd-{label}")))
                                        .ghost()
                                        .small()
                                        .compact()
                                        .label(label)
                                        .disabled(busy)
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.set_provider(target.clone(), window, cx)
                                        })),
                                )
                            }))
                        })
                        .child(
                            Checkbox::new("login")
                                // Explicit, like every other control: unsized,
                                // this falls back to `Medium` — a 16 px label
                                // and a 16 px box, the largest thing on screen.
                                .small()
                                .label("Start at login")
                                .checked(login_on)
                                .disabled(busy)
                                // `gpui-component` hands the handler the *new*
                                // value, so this is what the user asked for,
                                // not what was on screen.
                                .on_click(cx.listener(|this, want: &bool, window, cx| {
                                    this.set_login_item(*want, window, cx)
                                })),
                        ),
                )
            })
    }

    /// One tracked root: dot, slug, path, status, and the quiet way out.
    fn root_row(
        &self,
        slug: String,
        path: PathBuf,
        status: RootStatus,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let hue = theme::tone_color(tone(&status), cx);
        let label_color = match tone(&status) {
            Tone::Good | Tone::Muted => theme::dim(),
            Tone::Warn | Tone::Bad => hue,
        };
        // The engine's own words, bounded exactly where the tray bounds them:
        // `RootStatus::Error` is unsanitised peer-derived output.
        let detail = match &status {
            RootStatus::Error(e) => Some(tray::one_line(e)),
            _ => None,
        };
        let home_dir = self.state.read(cx).home_dir.clone();
        let short = theme::short_path(&path, &home_dir, theme::PATH_CHARS);
        let busy = self.busy(cx);
        let removing = slug.clone();

        v_flex()
            .id(SharedString::from(format!("root-{slug}")))
            .w_full()
            .gap_0p5()
            .py_1()
            .rounded_md()
            .hover(|s| s.bg(cx.theme().list_hover))
            // Line one is a fixed `ROW_H` box with everything centred in it,
            // so the dot, the slug, the status label and the `Remove` button
            // share one baseline however tall the button happens to be.
            .child(
                h_flex()
                    .h(theme::ROW_H)
                    .w_full()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .w(theme::GLYPH_COL)
                            .flex_none()
                            .text_color(hue)
                            .child(theme::glyph(&status)),
                    )
                    .child(div().flex_1().overflow_hidden().truncate().child(slug))
                    .child(
                        div()
                            .w(theme::STATUS_COL)
                            .flex_none()
                            .truncate()
                            .text_right()
                            .text_color(label_color)
                            .child(tray::status_label(&status)),
                    )
                    .child(
                        Button::new(SharedString::from(format!("rm-{removing}")))
                            .ghost()
                            .small()
                            .compact()
                            .label("Remove")
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.remove(removing.clone(), window, cx)
                            })),
                    ),
            )
            // Line two and three hang under the slug, not under the dot, and
            // get the whole width the row is no longer sharing with a button.
            .child(
                div()
                    .pl(theme::INDENT)
                    .child(theme::mono_line(short, theme::dim(), cx)),
            )
            .when_some(detail, |el, d| {
                el.child(
                    div()
                        .pl(theme::INDENT)
                        .child(theme::mono_line(d, cx.theme().red, cx)),
                )
            })
    }

    fn roots_section(&self, cx: &Context<Self>) -> impl IntoElement {
        let s = self.state.read(cx);
        let busy = self.busy(cx);
        let statuses = s.roots.clone();
        let empty = s.cfg.roots.is_empty();

        let list = rows(&s.cfg)
            .into_iter()
            .fold(v_flex().gap_1(), |col, (slug, path)| {
                // A root the daemon has not reported on yet is Pending, not gone.
                let status = statuses
                    .iter()
                    .find(|(s, _)| *s == slug)
                    .map(|(_, st)| st.clone())
                    .unwrap_or(RootStatus::Pending);
                col.child(self.root_row(slug, path, status, cx))
            });

        v_flex()
            .gap_2()
            .child(Self::header(
                "Roots",
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("link")
                            .ghost()
                            .small()
                            .compact()
                            .label("Link")
                            .tooltip("Adopt a folder another Mac already publishes")
                            .disabled(busy)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.list_linkable(window, cx)),
                            ),
                    )
                    .child(
                        Button::new("add")
                            .ghost()
                            .small()
                            .compact()
                            .label("+")
                            .tooltip("Track another folder")
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, window, cx| this.pick_add(window, cx))),
                    ),
                cx,
            ))
            .when(empty, |el| el.child(note("No tracked folders", cx)))
            .when(!empty, |el| el.child(list))
            .when_some(self.draft.as_ref(), |el, draft| {
                el.child(
                    h_flex()
                        .gap_2()
                        .child(div().flex_1().child(Input::new(&draft.slug).small()))
                        // `Add` and `Cancel` are peers, so neither is
                        // `.compact()`: same height, same padding, same size.
                        .child(
                            Button::new("confirm")
                                .primary()
                                .small()
                                .label("Add")
                                .disabled(busy)
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.confirm_add(window, cx)),
                                ),
                        )
                        .child(
                            Button::new("cancel")
                                .ghost()
                                .small()
                                .label("Cancel")
                                .on_click(cx.listener(|this, _, _window, cx| {
                                    this.draft = None;
                                    cx.notify();
                                })),
                        ),
                )
            })
            .when_some(self.linkable.clone(), |el, slugs| {
                if slugs.is_empty() {
                    return el.child(note("Nothing to link in the cloud folder", cx));
                }
                el.child(
                    slugs
                        .into_iter()
                        .fold(v_flex().gap_1().items_start(), |col, slug| {
                            let id = SharedString::from(format!("ln-{slug}"));
                            let picked = slug.clone();
                            col.child(
                                Button::new(id)
                                    .ghost()
                                    .small()
                                    .compact()
                                    .label(slug)
                                    .disabled(busy)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.pick_link_path(picked.clone(), window, cx)
                                    })),
                            )
                        }),
                )
            })
    }

    fn conflicts_section(&self, cx: &Context<Self>) -> impl IntoElement {
        let s = self.state.read(cx);
        let lines: Vec<String> = s
            .conflicts
            .iter()
            .map(|(slug, c)| conflict_line(slug, c))
            .collect();
        v_flex()
            .gap_2()
            .child(Self::header("Conflicts", div(), cx))
            .child(lines.into_iter().fold(v_flex().gap_1(), |col, l| {
                col.child(theme::mono_line(l, theme::dim(), cx))
            }))
    }

    /// The status bar: one line that says where the whole app stands, and the
    /// two error channels above it.
    fn footer(&self, cx: &Context<Self>) -> impl IntoElement {
        let s = self.state.read(cx);
        let (glyph, summary, hue) = footer_summary(s);
        let hue = theme::tone_color(hue, cx);
        let roots = s.cfg.roots.len();
        let count = format!("{roots} {}", if roots == 1 { "root" } else { "roots" });

        v_flex()
            .flex_none()
            .border_t_1()
            .border_color(cx.theme().border)
            .when_some(self.error.clone(), |el, e| {
                el.child(div().px_4().pt_2().child(banner(&e, cx)))
            })
            .when_some(s.last_error.clone(), |el, e| {
                el.child(div().px_4().pt_2().child(banner(&e, cx)))
            })
            .child(
                h_flex()
                    .px_4()
                    .py_2()
                    .items_center()
                    .justify_between()
                    // One size for the whole bar, set once: the glyph used to
                    // inherit the body's 14 px while the labels beside it were
                    // 12, which is the mismatch this pass exists to remove.
                    .text_xs()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(div().text_color(hue).child(glyph))
                            .child(div().text_color(theme::dim()).child(summary)),
                    )
                    .child(div().text_color(cx.theme().muted_foreground).child(count)),
            )
    }
}

impl Render for MainWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let conflicts = !self.state.read(cx).conflicts.is_empty();
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .text_sm()
            .overflow_hidden()
            // Drawn, not native: the body is dark whatever the Mac is set to,
            // and a light system titlebar over it reads as a seam.
            .child(
                TitleBar::new().child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme::dim())
                        .child("Dotlore"),
                ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .overflow_hidden()
                    .px_4()
                    .py_3()
                    .gap_6()
                    .child(self.provider_section(cx))
                    .child(self.roots_section(cx))
                    .when(conflicts, |el| el.child(self.conflicts_section(cx))),
            )
            .child(self.footer(cx))
    }
}

// --- pure helpers -----------------------------------------------------------

/// `(slug, path)` per tracked root, in config order.
fn rows(cfg: &Config) -> Vec<(String, PathBuf)> {
    cfg.roots
        .iter()
        .map(|r| (r.slug.clone(), r.path.clone()))
        .collect()
}

/// The accounts the Google Drive client has mounted, sorted.
pub fn google_drive_dirs(home_dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(home_dir.join(CLOUD_STORAGE))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() && name_of(p).starts_with("GoogleDrive-"))
        .collect();
    out.sort();
    out
}

/// The status bar's one line: glyph, summary, tone.
///
/// Worst first, so the line can never be calmer than the rows above it — and
/// `Synced` over a root the daemon has not reported on yet *is* calmer. The
/// rows are drawn from `cfg`, defaulting a slug the last cycle did not
/// mention to `Pending`, so this asks `cfg` the same question rather than
/// counting `roots`: a stale entry left by a `remove` plus an `add` can make
/// the two lists the same length while naming different slugs.
fn footer_summary(s: &AppState) -> (&'static str, String, Tone) {
    let broken = s
        .roots
        .iter()
        .filter(|(_, st)| tone(st) == Tone::Bad)
        .count();
    let pending = s.cfg.roots.iter().any(|r| {
        s.roots
            .iter()
            .find(|(slug, _)| *slug == r.slug)
            .is_none_or(|(_, st)| tone(st) == Tone::Muted)
    });
    if s.cfg.provider_dir.is_none() {
        ("○", "No cloud folder".to_string(), Tone::Muted)
    } else if s.cfg.roots.is_empty() {
        ("○", "Nothing tracked".to_string(), Tone::Muted)
    } else if broken > 0 {
        ("×", format!("{broken} need attention"), Tone::Bad)
    } else if s.conflicts_total > 0 {
        let n = s.conflicts_total;
        let word = if n == 1 { "conflict" } else { "conflicts" };
        ("▲", format!("{n} {word}"), Tone::Warn)
    } else if pending {
        ("○", "Pending".to_string(), Tone::Muted)
    } else {
        ("●", "Synced".to_string(), Tone::Good)
    }
}

/// One conflict, as the section lists it.
///
/// `c.live` alone goes through `tray::one_line`, not the whole line: it is
/// the one peer-authored piece here — a path off another device, and a
/// filename may legally hold a newline — while `slug` and `loser_name` are
/// already bounded and control-free in core. `mono_line`'s `truncate` is a
/// visual bound, not a character-class one. Bounding the assembled line
/// instead would put `one_line`'s 80-character cut through `from <device>`,
/// which is the half of the line worth reading.
pub fn conflict_line(slug: &str, c: &ConflictView) -> String {
    let me = if c.loser_is_me { " (this Mac)" } else { "" };
    let live = tray::one_line(&c.live.display().to_string());
    format!("{slug} · {live} · from {}{me}", c.loser_name)
}

fn name_of(p: &Path) -> String {
    p.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string()
}

/// The single path out of a `prompt_for_paths` reply, if the user picked one.
fn one_path<E>(reply: Result<anyhow::Result<Option<Vec<PathBuf>>>, E>) -> Option<PathBuf> {
    reply.ok()?.ok()??.into_iter().next()
}

// --- small elements ----------------------------------------------------------

/// An empty state or an aside — present, but not competing with the rows.
fn note(text: impl Into<SharedString>, cx: &App) -> impl IntoElement {
    div()
        .truncate()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

/// Something went wrong, in the engine's own words.
///
/// Every string that reaches here is unsanitised engine output — hundreds of
/// characters of git stderr embedding path names that came off another
/// device's bundle — so the bound is `tray::one_line`, the same one the menu
/// uses, and `truncate` is only the second line of defence.
fn banner(text: &str, cx: &App) -> impl IntoElement {
    let red = cx.theme().red;
    h_flex()
        .w_full()
        .gap_2()
        .items_start()
        // One size for the whole banner, like the footer: the `×` would
        // otherwise inherit the body's 14 px beside a 12 px mono message.
        .text_xs()
        .child(
            div()
                .w(theme::GLYPH_COL)
                .flex_none()
                .text_color(red)
                .child("×"),
        )
        .child(div().flex_1().overflow_hidden().child(theme::mono_line(
            tray::one_line(text),
            red,
            cx,
        )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_status_gets_the_colour_it_deserves() {
        assert_eq!(tone(&RootStatus::Synced), Tone::Good);
        assert_eq!(tone(&RootStatus::Conflicts(3)), Tone::Warn);
        assert_eq!(tone(&RootStatus::Pending), Tone::Muted);
        assert_eq!(tone(&RootStatus::RootMissing), Tone::Bad);
        assert_eq!(tone(&RootStatus::GitMissing), Tone::Bad);
        assert_eq!(tone(&RootStatus::Error("boom".into())), Tone::Bad);
    }

    fn view(loser_is_me: bool) -> ConflictView {
        ConflictView {
            live: PathBuf::from("notes/todo.md"),
            sibling: PathBuf::from("notes/todo.conflict-abcd1234.md"),
            loser_id8: "abcd1234".into(),
            loser_name: "Air".into(),
            loser_is_me,
        }
    }

    #[test]
    fn a_conflict_names_the_device_that_lost() {
        assert_eq!(
            conflict_line("proj-claude", &view(false)),
            "proj-claude · notes/todo.md · from Air"
        );
    }

    #[test]
    fn a_conflict_this_mac_lost_says_so() {
        assert_eq!(
            conflict_line("proj-claude", &view(true)),
            "proj-claude · notes/todo.md · from Air (this Mac)"
        );
    }

    #[test]
    fn a_conflict_path_off_another_device_becomes_one_line() {
        let mut c = view(false);
        c.live = PathBuf::from("notes/to\ndo\u{1b}]0;pwn\u{7}.md");
        let line = conflict_line("proj-claude", &c);
        assert!(!line.contains('\n'), "{line}");
        assert!(!line.contains('\u{1b}'), "{line}");
        assert!(!line.contains('\u{7}'), "{line}");
        // And the bound is on the path, not on the line: a long path must not
        // cost the reader the device name.
        let mut long = view(false);
        long.live = PathBuf::from("a".repeat(200));
        assert!(
            conflict_line("proj-claude", &long).ends_with(" · from Air"),
            "the device name survives a path that overruns the bound"
        );
    }

    /// A state whose config names `slugs` and whose last cycle reported
    /// `reported`.
    fn footer_state(slugs: &[&str], reported: Vec<(String, RootStatus)>) -> AppState {
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut s = crate::state::test_state(tx);
        s.cfg.provider_dir = Some(PathBuf::from("/cloud"));
        s.cfg.roots = slugs
            .iter()
            .map(|slug| config::Root {
                slug: (*slug).to_string(),
                path: PathBuf::from("/tmp").join(slug),
                kind: dotlore_core::cloud::Kind::Dir,
                initializing: false,
            })
            .collect();
        s.conflicts_total = crate::state::conflicts_total(&reported);
        s.roots = reported;
        s
    }

    #[test]
    fn the_footer_is_calm_only_when_every_root_is() {
        let s = footer_state(
            &["a", "b"],
            vec![
                ("a".into(), RootStatus::Synced),
                ("b".into(), RootStatus::Synced),
            ],
        );
        assert_eq!(footer_summary(&s), ("●", "Synced".to_string(), Tone::Good));
    }

    #[test]
    fn a_freshly_linked_root_keeps_the_footer_pending() {
        let s = footer_state(&["a"], vec![("a".into(), RootStatus::Pending)]);
        assert_eq!(
            footer_summary(&s),
            ("○", "Pending".to_string(), Tone::Muted)
        );
    }

    #[test]
    fn a_root_the_daemon_has_not_reported_keeps_the_footer_pending() {
        // The row for `b` renders `Pending`; the footer must not say `Synced`
        // just because the one root it *has* heard about is fine.
        let s = footer_state(&["a", "b"], vec![("a".into(), RootStatus::Synced)]);
        assert_eq!(
            footer_summary(&s),
            ("○", "Pending".to_string(), Tone::Muted)
        );
    }

    #[test]
    fn a_stale_report_of_the_same_length_does_not_pass_for_synced() {
        // `remove a` + `add b`, before the next cycle: same count, different
        // slug, and the only row on screen is `b`'s, drawn `Pending`.
        let s = footer_state(&["b"], vec![("a".into(), RootStatus::Synced)]);
        assert_eq!(
            footer_summary(&s),
            ("○", "Pending".to_string(), Tone::Muted)
        );
    }

    #[test]
    fn trouble_outranks_pending() {
        let s = footer_state(
            &["a", "b"],
            vec![
                ("a".into(), RootStatus::Pending),
                ("b".into(), RootStatus::RootMissing),
            ],
        );
        assert_eq!(
            footer_summary(&s),
            ("×", "1 need attention".to_string(), Tone::Bad)
        );

        let s = footer_state(
            &["a", "b"],
            vec![
                ("a".into(), RootStatus::Pending),
                ("b".into(), RootStatus::Conflicts(1)),
            ],
        );
        assert_eq!(
            footer_summary(&s),
            ("▲", "1 conflict".to_string(), Tone::Warn)
        );
    }

    #[test]
    fn nothing_tracked_and_no_provider_come_first() {
        let mut s = footer_state(&[], vec![]);
        assert_eq!(
            footer_summary(&s),
            ("○", "Nothing tracked".to_string(), Tone::Muted)
        );
        s.cfg.provider_dir = None;
        assert_eq!(
            footer_summary(&s),
            ("○", "No cloud folder".to_string(), Tone::Muted)
        );
    }

    #[test]
    fn google_drive_accounts_are_listed_and_nothing_else_is() {
        let home = crate::tmpdir("gdrive");
        let base = home.join(CLOUD_STORAGE);
        for n in [
            "GoogleDrive-b@example.com",
            "GoogleDrive-a@example.com",
            "OneDrive-Work",
        ] {
            std::fs::create_dir_all(base.join(n)).unwrap();
        }
        std::fs::write(base.join("GoogleDrive-notadir"), b"x").unwrap();

        let found: Vec<String> = google_drive_dirs(&home)
            .iter()
            .map(|p| name_of(p))
            .collect();
        assert_eq!(
            found,
            vec!["GoogleDrive-a@example.com", "GoogleDrive-b@example.com"]
        );
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn a_home_without_cloud_storage_lists_nothing() {
        assert!(google_drive_dirs(Path::new("/nonexistent-dotlore-home")).is_empty());
    }
}
