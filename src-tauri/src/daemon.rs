//! Filesystem watching and polling.
//!
//! [`run`] is the whole daemon: one thread, one event channel, one wait point.
//! Watcher callbacks and control senders ([`Cmd`]) push onto the same
//! [`Sender<Event>`], so a command wakes an idle wait as fast as a file change
//! does and there is never a second thing to select on.
//!
//! Three deadlines decide when a cycle runs, and the wait is always the
//! nearest of them:
//!
//! * [`POLL`] (30 s) since the last cycle — the floor, so a missed or
//!   unsupported watcher only costs latency;
//! * [`QUIET`] (2 s) since the *last* watch event — collapses the burst of
//!   duplicates a save produces;
//! * [`CAP`] (5 s) since the *first* pending watch event — the two timers
//!   deliberately have different anchors, so a file written continuously
//!   cannot push the cycle out forever.
//!
//! Watch events are filtered against each tracked root's own ignore set
//! before they reach the channel: `~/.claude/projects/*.jsonl` and friends
//! churn constantly during a Claude session and the mirror already excludes
//! them, so waking for them would pin the daemon at one full cycle per
//! [`CAP`] for as long as the user is working. This is not the echo table the
//! plan rules out — it is "do not wake for a path we do not track" — and it
//! fails open: anything that cannot be placed inside a filtered root, and
//! every watcher error or rescan, still counts as a change. One corner is not
//! open: a root whose staging repo cannot be opened is absent from the filter
//! map, so if an *outer* root that ignores the path is in the map while the
//! inner root that tracks it is not, the vote comes out ignored. It lasts
//! until the inner root has a staging repo, and [`POLL`] bounds it to 30 s.
//!
//! The engine lock is never held across a wait. [`Engine::sync_all`] takes the
//! one home lock itself and reloads `config.json` (and with it the provider)
//! under that lock, so a cycle *is* the reload: config changed behind the
//! daemon's back, and a provider transition another process committed, are
//! both picked up by the next cycle. That is why `Reload` and `SyncNow` run
//! the same code — after every cycle the watch set is re-derived from the
//! refreshed config and the watchers are rebuilt when it moved.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use anyhow::Result;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use crate::engine::{Engine, RootStatus};
use crate::project::{EntryList, ProjectFile};
use crate::repo::Repo;

/// Floor: a cycle runs at least this often even with no watch events.
pub const POLL: Duration = Duration::from_secs(30);
/// Quiet period since the last watch event.
pub const QUIET: Duration = Duration::from_secs(2);
/// Hard cap since the first pending watch event.
pub const CAP: Duration = Duration::from_secs(5);

/// An engine shared between the daemon thread and a UI.
pub type SharedEngine = Arc<Mutex<Engine>>;

/// Canonical tracked-root path -> ignore text plus that root's include-list.
///
/// Stored as values, not compiled matchers: [`Gitignore`] is not comparable,
/// and the daemon has to notice an edited `.dotloreignore` or include-list
/// the same way it notices a moved root, or a stale filter would keep
/// swallowing a path the user just started tracking.
#[derive(Clone, PartialEq, Eq, Debug)]
struct RootFilter {
    ignore: String,
    /// `None` is unconstrained (ignore text only). `Some` is the committed
    /// include-list, even when empty — an empty list still has to rebuild
    /// and still has to drop siblings.
    tracked: Option<ProjectFile>,
}

type Filters = HashMap<PathBuf, RootFilter>;

/// Everything one cycle produces: the per-root statuses, the paths to watch,
/// and the filters that decide which events under them matter.
type Cycle = (Result<Vec<(String, RootStatus)>>, Vec<PathBuf>, Filters);

/// Something a UI or a signal handler asks the daemon to do.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cmd {
    /// Run a cycle now.
    SyncNow,
    /// Run a cycle now and re-derive the watch set from disk config.
    Reload,
    /// Stop after the cycle in flight.
    Quit,
}

/// Everything the daemon waits on, on one channel.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Event {
    /// Something under a watched path changed. Carries no detail on purpose:
    /// the debounce collapses a burst into one cycle, and a cycle looks at
    /// every root anyway.
    Watch,
    Control(Cmd),
}

/// Watch, poll and sync until [`Cmd::Quit`] or a dropped channel.
///
/// `tx` must be the sender half of `rx`; the daemon clones it into every
/// watcher callback, and the caller keeps it to send commands. `on_status` is
/// called once per cycle with what [`Engine::sync_all`] returned — including
/// its `Err`, so a config that stopped parsing or a provider that vanished is
/// reported rather than swallowed. A cycle always runs before the first wait.
pub fn run(
    engine: SharedEngine,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    mut on_status: impl FnMut(Result<Vec<(String, RootStatus)>>),
) {
    let mut watcher: Option<RecommendedWatcher> = None;
    let mut watching: Vec<PathBuf> = Vec::new();
    // Anchors of the two debounce deadlines; `None` means nothing is pending.
    let mut first: Option<Instant> = None;
    let mut last: Option<Instant> = None;

    // The `Config` the caller built the engine from may already be stale, so
    // the watch set comes from the cycle rather than the other way round.
    let (status, wanted, filters) = cycle(&engine);
    rebuild(&tx, &wanted, &filters, &mut watcher, &mut watching);
    let mut filtering = filters;
    on_status(status);
    let mut next_poll = Instant::now() + POLL;

    loop {
        let mut deadline = next_poll;
        if let Some(t) = last {
            deadline = deadline.min(t + QUIET);
        }
        if let Some(t) = first {
            deadline = deadline.min(t + CAP);
        }
        let now = Instant::now();
        // Checked before the receive, not only through a timeout: under a
        // continuous event stream `recv_timeout` would keep returning events
        // and the cap would never come due.
        if now < deadline {
            match rx.recv_timeout(deadline - now) {
                Ok(Event::Watch) => {
                    let t = Instant::now();
                    first.get_or_insert(t);
                    last = Some(t);
                    continue;
                }
                // Quit is only ever seen between cycles, because a cycle runs
                // on this thread: the bounded operation has completed.
                Ok(Event::Control(Cmd::Quit)) | Err(RecvTimeoutError::Disconnected) => return,
                Ok(Event::Control(Cmd::SyncNow | Cmd::Reload)) => {}
                Err(RecvTimeoutError::Timeout) => {}
            }
        }

        let (status, wanted, filters) = cycle(&engine);
        if wanted != watching || filters != filtering {
            rebuild(&tx, &wanted, &filters, &mut watcher, &mut watching);
            filtering = filters;
        }
        on_status(status);
        first = None;
        last = None;
        next_poll = Instant::now() + POLL;
    }
}

/// One cycle plus the watch set and event filters the refreshed config asks
/// for.
///
/// The guard is dropped with the returned tuple, before the caller does
/// anything else — the daemon must never wait holding the engine.
fn cycle(engine: &SharedEngine) -> Cycle {
    // A panic elsewhere cannot leave the engine half-written: every entry
    // point reloads config from disk under the home lock, so there is no
    // in-memory invariant a poisoned lock would be protecting.
    let mut e = engine.lock().unwrap_or_else(PoisonError::into_inner);
    let status = e.sync_all();
    let wanted = wanted_paths(&e);
    let filters = watch_filters(&e);
    (status, wanted, filters)
}

/// The ignore text that decides which events under each tracked root matter.
///
/// Keyed by the *canonical* root: FSEvents reports `/private/var/...` where
/// the config holds `/var/...`, and a prefix that never matches would filter
/// nothing. Reuses [`Repo::ignore_text`], so the daemon and the mirror cannot
/// disagree about what a root tracks. The include-list from
/// [`Repo::project_file`] is stored beside it and is strictly narrower.
///
/// A root is simply absent from the map when anything is unavailable — no
/// staging repo yet, an unresolvable path — and an absent root is unfiltered.
/// Missing a real change costs data; an extra cycle costs a few hundred
/// milliseconds.
///
/// A root whose ignore text is empty (every project root) is still entered,
/// with that empty text: nothing stops the user tracking both `~/.claude` and
/// a directory inside it, and the inner root's answer must not be the only one
/// consulted for a path the outer root tracks whole.
fn watch_filters(e: &Engine) -> Filters {
    let mut out = Filters::new();
    for r in &e.cfg.roots {
        let Ok(canon) = r.path.canonicalize() else {
            continue;
        };
        let opened = Repo::open(
            &e.home,
            &r.slug,
            &r.path,
            &e.cfg.device_name,
            &e.cfg.device_id,
        );
        let Ok(repo) = opened else {
            continue;
        };
        let Ok(file) = repo.project_file() else {
            continue;
        };
        out.insert(
            canon,
            RootFilter {
                ignore: repo.ignore_text(&e.home_dir),
                tracked: Some(file),
            },
        );
    }
    out
}

/// Every tracked root plus the transport dir, minus what is not there yet.
///
/// `<provider>/dotlore`, never the provider folder itself: that one may be the
/// user's whole iCloud Drive, and watching it recursively would hold the
/// daemon at one cycle per [`CAP`] forever.
///
/// Dropping paths that do not exist is also how they get picked up later: a
/// deleted root or a transport dir that no device has published into yet is
/// absent from the set, so the set differs again the moment it appears and the
/// next cycle rebuilds.
fn wanted_paths(e: &Engine) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = e.cfg.roots.iter().map(|r| r.path.clone()).collect();
    v.push(e.cloud.base.clone());
    v.retain(|p| p.exists());
    v.sort();
    v.dedup();
    v
}

/// Install a watcher for `wanted`, then drop the previous one.
///
/// In that order: the two overlap for an instant, which costs duplicate events
/// the debounce collapses, where the reverse would leave a window with nothing
/// watching. A path that cannot be watched is left out of `watching` rather
/// than reported — the root's own `RootMissing` status already says it — and
/// is retried next cycle because the sets then differ.
fn rebuild(
    tx: &Sender<Event>,
    wanted: &[PathBuf],
    filters: &Filters,
    watcher: &mut Option<RecommendedWatcher>,
    watching: &mut Vec<PathBuf>,
) {
    let matchers = build_matchers(filters);
    let sender = tx.clone();
    let made = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if !significant(&matchers, &res) {
            return;
        }
        let _ = sender.send(Event::Watch);
    });
    let Ok(mut w) = made else {
        // No watcher at all: the poll still drives the daemon.
        return;
    };
    let mut ok = Vec::new();
    for p in wanted {
        if w.watch(p, RecursiveMode::Recursive).is_ok() {
            ok.push(p.clone());
        }
    }
    *watcher = Some(w);
    *watching = ok;
}

/// Compile each root's ignore text once per watcher, not once per event.
///
/// A text the matcher rejects drops that root from the list, which leaves it
/// unfiltered.
fn build_matchers(filters: &Filters) -> Vec<(PathBuf, Gitignore, Option<EntryList>)> {
    let mut out = Vec::new();
    for (root, filter) in filters {
        let mut b = GitignoreBuilder::new(root);
        if !filter.ignore.lines().all(|l| b.add_line(None, l).is_ok()) {
            continue;
        }
        let Ok(gi) = b.build() else {
            continue;
        };
        out.push((
            root.clone(),
            gi,
            filter.tracked.as_ref().map(ProjectFile::tracked),
        ));
    }
    out
}

/// Whether an event is worth a cycle.
///
/// Every failure mode answers yes: a watcher error, a rescan, an event with no
/// paths, a path under no tracked root. Only an event whose paths are *all*
/// ignored by every tracked root that contains them is dropped.
fn significant(
    matchers: &[(PathBuf, Gitignore, Option<EntryList>)],
    res: &notify::Result<notify::Event>,
) -> bool {
    let Ok(ev) = res else {
        // Errors and overflows are events too: something changed, or the
        // watcher lost track of what did. Either way, cycle.
        return true;
    };
    if ev.need_rescan() || ev.paths.is_empty() {
        return true;
    }
    ev.paths.iter().any(|p| !ignored(matchers, p))
}

/// Whether *every* tracked root containing `p` ignores it.
///
/// Every containing root gets a vote, not just the first: nothing forbids
/// tracking a directory inside another tracked root, and one root ignoring a
/// path says nothing about whether the other mirrors it. A path no root
/// contains is not ignored — the transport dir reaches this and must always
/// count.
fn ignored(matchers: &[(PathBuf, Gitignore, Option<EntryList>)], p: &Path) -> bool {
    // A deleted path has no type left to read, and the answer can differ by
    // type (`!/agents/` re-includes a directory but not a file), so an unknown
    // type is ignored only when both answers agree.
    let dir = fs::symlink_metadata(p).map(|md| md.is_dir());
    let mut contained = false;
    for (root, gi, include) in matchers {
        let Ok(rel) = p.strip_prefix(root) else {
            continue;
        };
        contained = true;
        // The root itself.
        if rel.as_os_str().is_empty() {
            return false;
        }
        // Outside the include-list: this root does not track it. Ancestors of
        // a nested entry still vote "wake" so creating or deleting the parent
        // directory of a tracked file is seen.
        if let Some(include) = include {
            if !include.contains_rel(rel) && !include.has_tracked_descendant(rel) {
                continue;
            }
        }
        // Relative paths only: the matcher panics on an absolute path it
        // cannot strip its own root from.
        let hit = staging_private(rel)
            || match dir {
                Ok(d) => gi.matched_path_or_any_parents(rel, d).is_ignore(),
                Err(_) => {
                    gi.matched_path_or_any_parents(rel, true).is_ignore()
                        && gi.matched_path_or_any_parents(rel, false).is_ignore()
                }
            };
        if !hit {
            return false;
        }
    }
    contained
}

/// A name the mirror never carries either way, for any root.
///
/// [`crate::project::staging_private`] plus `.DS_Store`, which the shared
/// predicate deliberately omits so `delete_stale` can still prune a stray
/// Finder file from the worktree. The ignore text alone does not cover
/// these: a project root's text is empty, so without this a `git` operation
/// anywhere in a tracked project directory, and dotlore's own
/// `.dotlore-tmp` and `.conflict-` writes, each buy a no-op cycle.
fn staging_private(rel: &Path) -> bool {
    rel.components().any(|c| {
        matches!(c, std::path::Component::Normal(n) if {
            let n = n.to_string_lossy();
            crate::project::staging_private(&n) || n == ".DS_Store"
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::thread;
    use std::thread::JoinHandle;

    use tempfile::TempDir;

    use crate::config::{Config, Root};

    type Status = Result<Vec<(String, RootStatus)>>;

    fn engine_at(home: &Path, provider: &Path) -> SharedEngine {
        let mut cfg = Config::load(home).unwrap();
        cfg.provider_dir = Some(provider.to_path_buf());
        cfg.save(home).unwrap();
        Arc::new(Mutex::new(Engine::new(home, home, cfg).unwrap()))
    }

    /// Start the daemon and wait for its initial cycle: once that status is
    /// in, the loop is at the wait point, which is the handshake every timing
    /// assertion below needs.
    fn start(engine: &SharedEngine) -> (Sender<Event>, Receiver<Status>, JoinHandle<()>) {
        let (tx, rx) = mpsc::channel();
        let (stx, srx) = mpsc::channel();
        let e = Arc::clone(engine);
        let t = tx.clone();
        let h = thread::spawn(move || {
            run(e, t, rx, move |s| {
                let _ = stx.send(s);
            })
        });
        srx.recv_timeout(Duration::from_secs(30))
            .expect("initial cycle")
            .expect("initial cycle failed");
        (tx, srx, h)
    }

    /// Wait until the daemon has been quiet for `QUIET + 1 s`, then swallow
    /// whatever it produced getting there.
    ///
    /// Two things need this. A cycle that publishes a bundle writes under the
    /// watched transport dir, so the first cycle echoes into one or two more;
    /// and a freshly installed FSEvents stream drops events for a few hundred
    /// milliseconds, so a write has to come well after the last `rebuild`.
    /// Returning only after a silent window covers both. `POLL` is 30 s, so
    /// the window always exists.
    fn settle(srx: &Receiver<Status>) {
        while srx.recv_timeout(QUIET + Duration::from_secs(1)).is_ok() {}
    }

    fn quit(tx: &Sender<Event>, h: JoinHandle<()>) {
        let _ = tx.send(Event::Control(Cmd::Quit));
        h.join().unwrap();
    }

    #[test]
    fn idle_sync_now_wakes_within_250ms() {
        let home = TempDir::new().unwrap();
        let provider = TempDir::new().unwrap();
        let engine = engine_at(home.path(), provider.path());
        let (tx, srx, h) = start(&engine);

        let t0 = Instant::now();
        tx.send(Event::Control(Cmd::SyncNow)).unwrap();
        srx.recv_timeout(Duration::from_millis(250))
            .expect("SyncNow did not wake the wait")
            .unwrap();
        assert!(t0.elapsed() < Duration::from_millis(250));

        quit(&tx, h);
    }

    #[test]
    fn idle_quit_exits_within_250ms() {
        let home = TempDir::new().unwrap();
        let provider = TempDir::new().unwrap();
        let engine = engine_at(home.path(), provider.path());
        let (tx, _srx, h) = start(&engine);

        let t0 = Instant::now();
        tx.send(Event::Control(Cmd::Quit)).unwrap();
        while !h.is_finished() && t0.elapsed() < Duration::from_millis(250) {
            thread::sleep(Duration::from_millis(2));
        }
        assert!(h.is_finished(), "Quit did not wake the wait");
        h.join().unwrap();
    }

    #[test]
    fn one_watch_event_cycles_after_the_quiet_period() {
        let home = TempDir::new().unwrap();
        let provider = TempDir::new().unwrap();
        let engine = engine_at(home.path(), provider.path());
        let (tx, srx, h) = start(&engine);

        let t0 = Instant::now();
        tx.send(Event::Watch).unwrap();
        srx.recv_timeout(Duration::from_secs(4))
            .expect("no cycle within the quiet period")
            .unwrap();
        let waited = t0.elapsed();
        assert!(
            waited >= Duration::from_millis(1500),
            "cycled per event, after {waited:?}"
        );
        assert!(waited < Duration::from_millis(3500), "cycled at {waited:?}");

        quit(&tx, h);
    }

    #[test]
    fn a_continuous_event_stream_still_cycles_at_the_cap() {
        let home = TempDir::new().unwrap();
        let provider = TempDir::new().unwrap();
        let engine = engine_at(home.path(), provider.path());
        let (tx, srx, h) = start(&engine);

        // 8 s of events, one every 100 ms: the quiet period never comes due,
        // so only the cap anchored on the *first* event can fire. Anchored on
        // the last one it would be 8 s + QUIET, past the bound below.
        let stream = tx.clone();
        let t0 = Instant::now();
        let feeder = thread::spawn(move || {
            for _ in 0..80 {
                if stream.send(Event::Watch).is_err() {
                    return;
                }
                thread::sleep(Duration::from_millis(100));
            }
        });

        srx.recv_timeout(Duration::from_secs(9))
            .expect("no cycle within the 5 s cap")
            .unwrap();
        let waited = t0.elapsed();
        assert!(
            waited >= Duration::from_secs(4),
            "cycled before the cap, after {waited:?}"
        );

        quit(&tx, h);
        feeder.join().unwrap();
    }

    #[test]
    fn a_config_change_behind_the_daemon_is_picked_up_next_cycle() {
        let home = TempDir::new().unwrap();
        let provider = TempDir::new().unwrap();
        let root = TempDir::new().unwrap();
        fs::write(root.path().join("CLAUDE.md"), b"hi\n").unwrap();
        // Created up front, so the event asserted at the end of the test is
        // the write *inside* it and nothing about the directory itself: it is
        // `RecursiveMode::Recursive` that has to carry it.
        fs::create_dir_all(root.path().join("docs")).unwrap();
        let engine = engine_at(home.path(), provider.path());
        let (tx, srx, h) = start(&engine);

        // A second Engine on the same home is exactly what a CLI invocation
        // is: it takes the home lock itself, and the daemon's in-memory config
        // knows nothing about the root it registered.
        let mut cli = {
            let cfg = Config::load(home.path()).unwrap();
            Engine::new(home.path(), home.path(), cfg).unwrap()
        };
        cli.add_root(root.path(), Some("live")).unwrap();

        tx.send(Event::Control(Cmd::SyncNow)).unwrap();
        let status = srx
            .recv_timeout(Duration::from_secs(30))
            .expect("no cycle after SyncNow")
            .unwrap();
        assert_eq!(
            status,
            vec![("live".to_string(), RootStatus::Synced)],
            "the cycle did not reload the config the CLI wrote"
        );

        // Same again for the provider: `dotlore provider <path>` commits the
        // transition under the home lock, and the daemon's next cycle must
        // pick up the new runtime rather than keep publishing to the old
        // folder. Reload needs no separate path because of this.
        let moved = TempDir::new().unwrap();
        crate::engine::configure_provider(home.path(), home.path(), moved.path()).unwrap();
        tx.send(Event::Control(Cmd::Reload)).unwrap();
        let status = srx
            .recv_timeout(Duration::from_secs(30))
            .expect("no cycle after Reload")
            .unwrap();
        assert_eq!(
            status,
            vec![("live".to_string(), RootStatus::Synced)],
            "the cycle did not survive the provider switch"
        );

        // The watcher clause of the same regression bullet: `watching` is
        // local to `run`, so the only way to observe that the rebuild after
        // the transition actually installed a watcher on the newly registered
        // root is to make the filesystem produce the event. A subdirectory,
        // because `RecursiveMode::Recursive` is what is under test.
        settle(&srx);
        fs::write(root.path().join("docs/new.md"), b"real\n").unwrap();
        srx.recv_timeout(Duration::from_secs(6))
            .expect("a real filesystem event did not reach the daemon")
            .unwrap();

        quit(&tx, h);
        let e = engine.lock().unwrap();
        assert_eq!(e.cfg.roots.len(), 1, "the reload lost a root");
        assert_eq!(e.cfg.roots[0].slug, "live");
        assert_eq!(
            e.cloud.base,
            moved.path().canonicalize().unwrap().join("dotlore"),
            "the daemon kept the old provider runtime"
        );
    }

    #[test]
    fn the_watch_set_is_the_existing_roots_plus_the_transport_dir() {
        let home = TempDir::new().unwrap();
        let provider = TempDir::new().unwrap();
        let here = home.path().join("here");
        fs::create_dir_all(&here).unwrap();
        let engine = engine_at(home.path(), provider.path());
        let (transport, paths) = {
            let mut e = engine.lock().unwrap();
            for (slug, path) in [("here", here.clone()), ("gone", home.path().join("gone"))] {
                e.cfg.roots.push(Root {
                    slug: slug.into(),
                    path,
                    initializing: false,
                });
            }
            fs::create_dir_all(&e.cloud.base).unwrap();
            (e.cloud.base.clone(), wanted_paths(&e))
        };

        assert!(paths.contains(&here), "a tracked root is not watched");
        assert!(
            paths.contains(&transport),
            "{} is not watched",
            transport.display()
        );
        assert!(
            !paths.contains(&home.path().join("gone")),
            "a missing root must not be watched"
        );
        assert!(
            !paths.contains(&provider.path().to_path_buf()),
            "the provider folder itself must not be watched"
        );
    }

    /// A project root's ignore text is empty, but it still has to be in the
    /// map: the map is also how [`ignored`] learns which roots contain a path,
    /// and a root that mirrors everything must be able to say so.
    #[test]
    fn watch_filters_holds_every_tracked_directory_root() {
        let home = TempDir::new().unwrap();
        let provider = TempDir::new().unwrap();
        let root = TempDir::new().unwrap();
        fs::write(root.path().join("CLAUDE.md"), b"hi\n").unwrap();
        let engine = engine_at(home.path(), provider.path());
        {
            let cfg = Config::load(home.path()).unwrap();
            let mut cli = Engine::new(home.path(), home.path(), cfg).unwrap();
            cli.add_root(root.path(), Some("proj")).unwrap();
        }

        let mut e = engine.lock().unwrap();
        e.sync_all().unwrap();
        let filters = watch_filters(&e);
        assert_eq!(
            filters
                .get(&root.path().canonicalize().unwrap())
                .map(|f| f.ignore.as_str()),
            Some(crate::project::DEFAULT_NEVER_IGNORE),
            "a project root is missing from the filter map: {filters:?}"
        );
    }

    fn matcher(root: &str, text: &str) -> Vec<(PathBuf, Gitignore, Option<EntryList>)> {
        build_matchers(&Filters::from([(
            PathBuf::from(root),
            RootFilter {
                ignore: text.to_string(),
                tracked: None,
            },
        )]))
    }

    /// Nothing refuses a tracked root inside another tracked root — only
    /// overlap with the state directory and the provider folder is rejected —
    /// so one root's ignore set must never speak for another's. The order here
    /// is the one a first-match-wins filter gets wrong.
    #[test]
    fn a_nested_root_that_tracks_a_path_outvotes_an_outer_root_that_ignores_it() {
        let churn = Path::new("/r/projects/session.jsonl");
        let outer = matcher("/r", crate::project::DEFAULT_NEVER_IGNORE);
        let inner = matcher("/r/projects", "");

        assert!(ignored(&outer, churn), "the outer root does ignore it");
        assert!(!ignored(&inner, churn), "the inner root mirrors it whole");

        let outer_first: Vec<_> = outer.iter().chain(&inner).cloned().collect();
        let inner_first: Vec<_> = inner.iter().chain(&outer).cloned().collect();
        for both in [outer_first, inner_first] {
            assert!(
                !ignored(&both, churn),
                "a root that tracks the path lost its vote"
            );
        }

        // The transport dir is under no tracked root and must always count.
        assert!(!ignored(
            &outer,
            Path::new("/cloud/dotlore/s/devices/d/1.bundle")
        ));

        // A path that no longer exists has no type to read, and here the two
        // answers disagree: `/*` ignores the file, `!/agents/` re-includes the
        // directory. The root tracks it, so it must wake the daemon.
        assert!(
            !ignored(&outer, Path::new("/r/agents")),
            "a deleted tracked directory lost its vote to the file answer"
        );
    }

    /// The names the mirror refuses to carry in either direction. A project
    /// root's ignore text is empty, so nothing else filters them: every `git`
    /// command in a tracked project directory, and every `.dotlore-tmp` or
    /// `.conflict-` file dotlore itself writes into a live root, would
    /// otherwise buy a no-op cycle per `CAP`.
    #[test]
    fn dotlore_and_git_private_names_do_not_wake_the_daemon() {
        let m = matcher("/r", "");
        for p in [
            "/r/.git",
            "/r/.git/index.lock",
            "/r/sub/.git/refs/heads/main",
            "/r/.dotloreignore",
            "/r/.dotloreproject",
            "/r/.DS_Store",
            "/r/CLAUDE.conflict-1234abcd.md",
            "/r/CLAUDE.md.dotlore-tmp",
        ] {
            assert!(ignored(&m, Path::new(p)), "{p} woke the daemon");
        }
        assert!(!ignored(&m, Path::new("/r/CLAUDE.md")));
        assert!(!ignored(&m, Path::new("/r/sub/notes.md")));
    }

    /// `~/.claude` is the headline tracked root and the mirror excludes
    /// `projects/*.jsonl`, `history.jsonl` and `todos/`, which Claude writes
    /// continuously. Waking for them would hold the daemon at one full
    /// ignore-filtered walk plus git per `CAP` for as long as a session runs.
    #[test]
    fn an_ignored_path_does_not_wake_the_daemon_but_a_tracked_one_does() {
        let home = TempDir::new().unwrap();
        // A separate `$HOME`: a tracked root may not sit inside the state
        // directory. `add_root` writes `DEFAULT_NEVER_IGNORE`, which
        // root-anchors `/projects/` and `/history.jsonl`.
        let home_dir = TempDir::new().unwrap();
        let provider = TempDir::new().unwrap();
        let root = home_dir.path().join(".claude");
        fs::create_dir_all(root.join("projects")).unwrap();
        fs::write(root.join("CLAUDE.md"), b"hi\n").unwrap();

        let engine = {
            let mut cfg = Config::load(home.path()).unwrap();
            cfg.provider_dir = Some(provider.path().to_path_buf());
            cfg.save(home.path()).unwrap();
            Arc::new(Mutex::new(
                Engine::new(home.path(), home_dir.path(), cfg).unwrap(),
            ))
        };
        {
            let cfg = Config::load(home.path()).unwrap();
            let mut cli = Engine::new(home.path(), home_dir.path(), cfg).unwrap();
            cli.add_root(&root, Some("hc")).unwrap();
        }

        let (tx, srx, h) = start(&engine);
        settle(&srx);

        fs::write(root.join("projects/a.jsonl"), b"{}\n").unwrap();
        fs::write(root.join("history.jsonl"), b"{}\n").unwrap();
        assert!(
            srx.recv_timeout(Duration::from_secs(7)).is_err(),
            "a session log woke the daemon; CAP is 5 s, so this is one full \
             sync every 5 s for as long as Claude is running"
        );

        // Same root, same watcher, a path the mirror does track.
        fs::write(root.join("CLAUDE.md"), b"hi there\n").unwrap();
        srx.recv_timeout(Duration::from_secs(6))
            .expect("a tracked file did not wake the daemon")
            .unwrap();

        quit(&tx, h);
    }

    /// An include-list is a stricter filter than ignore text: a sibling the
    /// project does not track must not buy a cycle. The matcher is built from
    /// a real `add_root` so `.dotloreproject` is the source of the keys.
    #[test]
    fn an_edit_outside_the_include_list_does_not_wake_the_daemon() {
        let home = TempDir::new().unwrap();
        let provider = TempDir::new().unwrap();
        let root = TempDir::new().unwrap();
        fs::write(root.path().join("CLAUDE.md"), b"hi\n").unwrap();
        fs::create_dir_all(root.path().join("other")).unwrap();
        fs::write(root.path().join("other/notes.md"), b"nope\n").unwrap();

        let engine = engine_at(home.path(), provider.path());
        {
            let mut cfg = Config::load(home.path()).unwrap();
            cfg.default_patterns = Some(vec!["CLAUDE.md".into()]);
            cfg.save(home.path()).unwrap();
            let mut cli = Engine::new(home.path(), home.path(), cfg).unwrap();
            cli.add_root(root.path(), Some("proj")).unwrap();
        }

        let mut e = engine.lock().unwrap();
        e.sync_all().unwrap();
        let filters = watch_filters(&e);
        let m = build_matchers(&filters);
        let canon = root.path().canonicalize().unwrap();
        assert!(
            ignored(&m, &canon.join("other/notes.md")),
            "an untracked sibling woke the daemon"
        );
        assert!(
            !ignored(&m, &canon.join("CLAUDE.md")),
            "a tracked file was filtered out"
        );
    }
}
