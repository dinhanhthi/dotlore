//! What the UI knows: the last cycle the daemon reported, the handles the
//! window acts through, and the one place the engine and daemon are started.

use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};

use dotlore_core::config::Config;
use dotlore_core::daemon::{self, Cmd, Event, SharedEngine};
use dotlore_core::engine::{ConflictView, Engine, RootStatus};

/// One daemon cycle, as it crosses the thread boundary.
///
/// `Engine::sync_all`'s error is rendered to a `String` here rather than
/// carried as an `anyhow::Error`: the UI only ever displays it, and a plain
/// `String` keeps the channel type free of the error's backtrace machinery.
pub type StatusUpdate = Result<Vec<(String, RootStatus)>, String>;

/// Where a provider change stands. `None` is "nothing in flight".
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Transition {
    Pending,
    Error(String),
}

/// What the caller of [`AppState::finish_transition`] still has to do, once it
/// is back on the UI thread with a `&mut App`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Next {
    /// Nothing: the transition failed, and the old runtime (if any) stands.
    Idle,
    /// First provider ever: build the engine and start the daemon now, so the
    /// user does not have to restart the app.
    StartRuntime,
    /// A provider *change*: the daemon already running must re-derive its
    /// watch set and cycle against the new destination. Never a second daemon.
    Reload,
}

/// The single `gpui::Entity` the tray and the window read.
pub struct AppState {
    /// Read once at startup, in `main`; every engine call takes it from here.
    pub home: PathBuf,
    pub home_dir: PathBuf,
    pub cfg: Config,
    /// The same engine the daemon thread drives. Every UI call through it
    /// must run on `cx.background_spawn`: `Engine` takes a blocking
    /// `File::lock` on the home, which a CLI can be holding.
    pub engine: Option<SharedEngine>,
    pub roots: Vec<(String, RootStatus)>,
    pub conflicts_total: usize,
    /// One entry per conflicting file, as `(slug, view)`; display only until
    /// Phase 6 adds the resolver.
    pub conflicts: Vec<(String, ConflictView)>,
    pub git_missing: bool,
    /// `None` when no provider is configured and no daemon is running.
    pub cmd_tx: Option<Sender<daemon::Event>>,
    /// The last failed cycle, if the one after it has not succeeded yet.
    pub last_error: Option<String>,
    /// A provider change the user started and its outcome.
    pub transition: Option<Transition>,
    /// Kept here so the status channel stays open across a provider change:
    /// the poller is started once, before any daemon exists, and a dropped
    /// sender would disconnect it forever.
    pub status_tx: Sender<StatusUpdate>,
}

impl AppState {
    /// Build the engine and start the daemon thread for the configured
    /// provider. No-op without one, and no-op when a runtime already exists —
    /// a second daemon on the same home would double every cycle.
    ///
    /// `Engine::new` only assembles a struct; it takes no lock, so this is
    /// safe on the UI thread. Everything after it runs on the daemon thread.
    pub fn start_runtime(&mut self) {
        if self.cfg.provider_dir.is_none() || self.engine.is_some() {
            return;
        }
        let engine = match Engine::new(&self.home, &self.home_dir, self.cfg.clone()) {
            Ok(engine) => Arc::new(Mutex::new(engine)),
            Err(e) => {
                self.last_error = Some(format!("{e:#}"));
                return;
            }
        };
        let (tx, rx) = mpsc::channel::<Event>();
        let status_tx = self.status_tx.clone();
        let daemon_engine = engine.clone();
        let daemon_tx = tx.clone();
        std::thread::spawn(move || {
            daemon::run(daemon_engine, daemon_tx, rx, move |status| {
                let _ = status_tx.send(status.map_err(|e| format!("{e:#}")));
            });
        });
        self.engine = Some(engine);
        self.cmd_tx = Some(tx);
    }

    /// Ask the daemon to do something. Nothing in the UI locks the engine on
    /// the UI thread, so this is how the UI makes anything happen promptly.
    pub fn send(&self, cmd: Cmd) {
        if let Some(tx) = self.cmd_tx.as_ref() {
            let _ = tx.send(Event::Control(cmd));
        }
    }

    /// A provider change has been handed to the background executor.
    pub fn begin_transition(&mut self) {
        self.transition = Some(Transition::Pending);
    }

    /// Fold the result of `engine::configure_provider` plus the config it
    /// persisted. The caller performs the returned [`Next`].
    pub fn finish_transition(
        &mut self,
        result: Result<(Config, Vec<(String, RootStatus)>), String>,
    ) -> Next {
        match result {
            // The old provider is still the saved one — `configure_provider`
            // only commits by saving the config — so the runtime stands.
            Err(e) => {
                self.transition = Some(Transition::Error(e));
                Next::Idle
            }
            Ok((cfg, roots)) => {
                self.transition = None;
                self.cfg = cfg;
                // Empty means "nothing was re-synced" (no roots, or the
                // destination was already the provider), which says nothing
                // about the roots the daemon last reported.
                if !roots.is_empty() {
                    self.apply(Ok(roots));
                }
                if self.engine.is_none() {
                    Next::StartRuntime
                } else {
                    Next::Reload
                }
            }
        }
    }

    /// Fold one cycle into the state; `true` when anything on screen moved.
    ///
    /// Most cycles report exactly what the last one did, and the caller uses
    /// this to skip the notify: rebuilding the menu bumps `gpui_tray`'s menu
    /// generation, and a click that lands on the superseded one is dropped.
    pub fn apply(&mut self, update: StatusUpdate) -> bool {
        match update {
            Ok(roots) => {
                if roots == self.roots && self.last_error.is_none() {
                    return false;
                }
                self.conflicts_total = conflicts_total(&roots);
                self.roots = roots;
                self.last_error = None;
                true
            }
            // The roots are left as they were: a cycle that failed before
            // `sync_all` returned says nothing about them, and emptying the
            // list would read as "nothing is tracked".
            Err(e) => {
                if self.last_error.as_deref() == Some(e.as_str()) {
                    return false;
                }
                self.last_error = Some(e);
                true
            }
        }
    }

    /// Take a freshly loaded config; `true` when anything the UI draws from
    /// it moved. A notify costs a tray-menu rebuild, so the caller needs to
    /// know, and `Config` has no `PartialEq` to lean on.
    pub fn adopt_cfg(&mut self, cfg: Config) -> bool {
        let rows = |c: &Config| -> Vec<(String, PathBuf)> {
            c.roots
                .iter()
                .map(|r| (r.slug.clone(), r.path.clone()))
                .collect()
        };
        if self.cfg.provider_dir == cfg.provider_dir && rows(&self.cfg) == rows(&cfg) {
            return false;
        }
        self.cfg = cfg;
        self.prune();
        true
    }

    /// Forget reported roots the config no longer names, and re-total.
    ///
    /// Called from [`Self::adopt_cfg`], because the last cycle's report
    /// outlives the change: after a `remove`, the window drops the row at
    /// once — it draws from `cfg` — while `self.roots` still names the slug
    /// until the next cycle lands. Everything that counts trouble reads the
    /// report instead: the footer's `need attention`, the menu-bar badge, the
    /// tray's per-root lines, `conflict_slugs`, and the conflicts section's
    /// own rows. Without this they stay alarmed about a root nobody can see
    /// any more.
    ///
    /// [`Self::finish_transition`] also replaces `cfg` and does not call this:
    /// a provider change never adds or removes a root, so there is nothing
    /// there to prune.
    ///
    /// Deliberately not done in [`Self::apply`] either: a cycle may
    /// legitimately report a root the CLI added a moment ago, which this
    /// `cfg` has not been reloaded to include yet, and dropping that one
    /// would hide a real root instead of a dead one.
    fn prune(&mut self) {
        let cfg = &self.cfg;
        let tracked = |slug: &String| cfg.roots.iter().any(|r| r.slug == *slug);
        self.roots.retain(|(slug, _)| tracked(slug));
        self.conflicts.retain(|(slug, _)| tracked(slug));
        self.conflicts_total = conflicts_total(&self.roots);
    }

    /// The roots worth asking `Engine::conflicts` about.
    pub fn conflict_slugs(&self) -> Vec<String> {
        self.roots
            .iter()
            .filter(|(_, st)| matches!(st, RootStatus::Conflicts(_)))
            .map(|(slug, _)| slug.clone())
            .collect()
    }
}

/// Conflicting files across every root — the number in the menu-bar badge.
pub fn conflicts_total(roots: &[(String, RootStatus)]) -> usize {
    roots
        .iter()
        .map(|(_, st)| match st {
            RootStatus::Conflicts(n) => *n,
            _ => 0,
        })
        .sum()
}

#[cfg(test)]
pub(crate) fn test_state(status_tx: Sender<StatusUpdate>) -> AppState {
    AppState {
        home: PathBuf::new(),
        home_dir: PathBuf::new(),
        cfg: Config::default(),
        engine: None,
        roots: Vec::new(),
        conflicts_total: 0,
        conflicts: Vec::new(),
        git_missing: false,
        cmd_tx: None,
        last_error: None,
        transition: None,
        status_tx,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::mpsc::Receiver;
    use std::time::Duration;

    fn state() -> (AppState, Receiver<StatusUpdate>) {
        let (tx, rx) = mpsc::channel();
        (test_state(tx), rx)
    }

    /// A config tracking exactly `slugs`, as the CLI or a `remove` leaves it.
    fn tracking(slugs: &[&str]) -> Config {
        Config {
            roots: slugs
                .iter()
                .map(|slug| dotlore_core::config::Root {
                    slug: (*slug).to_string(),
                    path: PathBuf::from("/tmp").join(slug),
                    kind: dotlore_core::cloud::Kind::Dir,
                    initializing: false,
                })
                .collect(),
            ..Config::default()
        }
    }

    #[test]
    fn a_good_cycle_replaces_the_roots_and_totals_the_conflicts() {
        let (mut s, _rx) = state();
        s.apply(Ok(vec![
            ("a".into(), RootStatus::Conflicts(2)),
            ("b".into(), RootStatus::Pending),
        ]));
        assert_eq!(s.roots.len(), 2);
        assert_eq!(s.conflicts_total, 2);
        assert_eq!(s.conflict_slugs(), vec!["a".to_string()]);
    }

    #[test]
    fn a_failed_cycle_records_the_error_and_keeps_the_roots() {
        let (mut s, _rx) = state();
        s.apply(Ok(vec![("a".into(), RootStatus::Conflicts(1))]));
        s.apply(Err("provider folder vanished".into()));
        assert_eq!(s.last_error.as_deref(), Some("provider folder vanished"));
        assert_eq!(s.roots.len(), 1, "a failed cycle must not blank the menu");
        assert_eq!(s.conflicts_total, 1);
    }

    #[test]
    fn a_repeated_cycle_reports_no_change() {
        let (mut s, _rx) = state();
        let rows = vec![("a".to_string(), RootStatus::Synced)];
        assert!(s.apply(Ok(rows.clone())), "the first cycle is a change");
        assert!(!s.apply(Ok(rows)), "an identical cycle must not notify");
        assert!(s.apply(Err("boom".into())));
        assert!(
            !s.apply(Err("boom".into())),
            "the same error must not notify"
        );
    }

    #[test]
    fn the_next_good_cycle_clears_the_error() {
        let (mut s, _rx) = state();
        s.apply(Err("boom".into()));
        s.apply(Ok(vec![("a".into(), RootStatus::Synced)]));
        assert_eq!(s.last_error, None);
        assert_eq!(s.conflicts_total, 0);
    }

    #[test]
    fn a_failed_provider_change_leaves_the_runtime_alone() {
        let (mut s, _rx) = state();
        s.begin_transition();
        assert_eq!(s.transition, Some(Transition::Pending));
        assert_eq!(
            s.finish_transition(Err("not a directory".into())),
            Next::Idle
        );
        assert_eq!(
            s.transition,
            Some(Transition::Error("not a directory".into()))
        );
        assert!(s.cfg.provider_dir.is_none());
    }

    #[test]
    fn the_first_provider_starts_a_runtime_and_a_later_one_only_reloads() {
        let (mut s, _rx) = state();
        let cfg = |dir: &str| Config {
            provider_dir: Some(PathBuf::from(dir)),
            ..Config::default()
        };

        s.begin_transition();
        let next = s.finish_transition(Ok((cfg("/p1"), vec![("a".into(), RootStatus::Synced)])));
        assert_eq!(next, Next::StartRuntime, "no engine yet — start one");
        assert_eq!(s.transition, None);
        assert_eq!(s.cfg.provider_dir.as_deref(), Some(Path::new("/p1")));
        assert_eq!(s.roots.len(), 1);

        // What `Next::StartRuntime` would have installed.
        s.engine = Some(Arc::new(Mutex::new(
            Engine::new(&s.home, &s.home_dir, s.cfg.clone()).expect("a provider is configured"),
        )));

        s.begin_transition();
        let next = s.finish_transition(Ok((cfg("/p2"), Vec::new())));
        assert_eq!(next, Next::Reload, "a second daemon must never be started");
        assert_eq!(s.cfg.provider_dir.as_deref(), Some(Path::new("/p2")));
        assert_eq!(s.roots.len(), 1, "an empty bootstrap says nothing new");
    }

    #[test]
    fn a_config_the_cli_changed_underneath_us_replaces_the_rows() {
        let (mut s, _rx) = state();
        let with = tracking;

        assert!(s.adopt_cfg(with(&["a"])), "a new root is a change");
        assert!(
            !s.adopt_cfg(with(&["a"])),
            "the same config must not notify"
        );
        assert!(
            s.adopt_cfg(with(&[])),
            "a root removed by the CLI is a change"
        );
        assert!(s.cfg.roots.is_empty());
        assert!(
            s.adopt_cfg(Config {
                provider_dir: Some(PathBuf::from("/p")),
                ..Config::default()
            }),
            "a provider changed by the CLI is a change"
        );
    }

    /// A `remove` takes the row off the window at once, because the window
    /// draws from `cfg`. Everything that counts trouble reads the last
    /// cycle's report instead, and that still names the slug until the
    /// `Cmd::Reload` cycle lands.
    #[test]
    fn a_removed_root_stops_counting_before_the_next_cycle_lands() {
        let (mut s, _rx) = state();
        s.adopt_cfg(tracking(&["a", "b"]));
        s.apply(Ok(vec![
            ("a".into(), RootStatus::Conflicts(2)),
            ("b".into(), RootStatus::RootMissing),
        ]));
        assert_eq!(s.conflicts_total, 2);
        // What `refresh_from_engine` left behind for the conflicts section.
        s.conflicts = vec![(
            "a".to_string(),
            ConflictView {
                live: PathBuf::from("todo.md"),
                sibling: PathBuf::from("todo.conflict-abcd1234.md"),
                loser_id8: "abcd1234".into(),
                loser_name: "Air".into(),
                loser_is_me: false,
            },
        )];

        assert!(s.adopt_cfg(tracking(&["b"])), "`remove a` is a change");
        assert_eq!(
            s.roots,
            vec![("b".to_string(), RootStatus::RootMissing)],
            "the tray menu and the footer must not name a removed root"
        );
        assert_eq!(s.conflicts_total, 0, "nor may the badge count one");
        assert!(
            s.conflict_slugs().is_empty(),
            "nor may the window ask the engine about one"
        );
        assert!(
            s.conflicts.is_empty(),
            "nor may the conflicts section still list its files"
        );
    }

    /// The headless half of "a fresh home picks a provider and Add works
    /// without a restart": `start_runtime` must really spawn the daemon, so a
    /// cycle lands on the status channel with nobody touching the UI.
    #[test]
    fn start_runtime_spawns_a_daemon_that_reports_a_cycle() {
        let dir = crate::tmpdir("start-runtime");
        let home = dir.join("home");
        let provider = dir.join("provider");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&provider).unwrap();
        dotlore_core::engine::configure_provider(&home, &dir, &provider).unwrap();

        let (tx, rx) = mpsc::channel();
        let mut s = test_state(tx);
        s.home = home.clone();
        s.home_dir = dir.clone();
        s.cfg = {
            let _g = dotlore_core::config::lock(&home).unwrap();
            Config::load(&home).unwrap()
        };

        s.start_runtime();
        assert!(
            s.engine.is_some() && s.cmd_tx.is_some(),
            "{:?}",
            s.last_error
        );

        let first = rx
            .recv_timeout(Duration::from_secs(20))
            .expect("the daemon must report its first cycle");
        assert_eq!(first, Ok(Vec::new()), "a fresh home tracks nothing");

        s.send(Cmd::Quit);
        std::fs::remove_dir_all(&dir).ok();
    }
}
