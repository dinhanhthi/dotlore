//! The sync cycle: add, link, sync, resolve, recover.
//!
//! This is the orchestration layer the desktop app calls.
//! Nothing below it knows about configuration or locking: [`Engine`] takes the
//! home lock exactly once per public entry point, reloads `config.json` under
//! that lock, and then drives [`Repo`] / [`Cloud`] / [`conflict`] through
//! private `*_locked` helpers that assume the guard. A public method never
//! saves from an in-memory config it did not just reload, and no nested call
//! takes the lock a second time (`std::fs::File::lock` is not reentrant, so a
//! second attempt from the same process deadlocks rather than failing).
//!
//! Every change that reaches a live file goes through the durable transaction
//! in [`crate::repo`]: a pending one is always recovered *after* the root has
//! been checked for existence and *before* any new local commit.

use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;

use crate::cloud::{Cloud, Manifest};
use crate::config::{self, Config, HomeLock, Root};
use crate::conflict;
use crate::git::{self, Git};
use crate::mirror::{self, FileState, Node};
use crate::project::{self, Limits, State};
use crate::repo::{
    provider_key, remote_ref, unique_id, FetchMode, Repo, ResumeOutcome, Transaction,
};

/// Where one tracked root stands after a cycle.
///
/// `Pending` is not an error: a linked root whose staging has no `main` yet
/// because no bundle is readable in the cloud, or an apply that raced a live
/// edit, is retried on the next cycle with its journal intact.
#[derive(Serialize, Clone, PartialEq, Eq, Debug)]
#[serde(tag = "kind", content = "detail")]
pub enum RootStatus {
    Synced,
    Conflicts(usize),
    Pending,
    RootMissing,
    GitMissing,
    Error(String),
}

/// One conflict sibling, ready to display.
#[derive(Serialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ConflictView {
    pub live: PathBuf,
    pub sibling: PathBuf,
    pub loser_id8: String,
    /// From `devices/<id>/device.json`; the id8 itself when unknown.
    pub loser_name: String,
    pub loser_is_me: bool,
}

/// One side of a resolution, pinned by the blob the UI actually showed.
#[derive(Serialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SiblingView {
    pub path: PathBuf,
    pub blob: String,
    pub bytes: Vec<u8>,
    pub loser_id8: String,
    pub loser_name: String,
}

/// Everything a resolver window displayed, exactly as it was displayed.
///
/// [`Engine::resolve_conflict`] re-reads all of it and refuses to touch
/// anything if a single version moved; that is the whole point of the type.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ResolutionSnapshot {
    pub slug: String,
    /// Staging-relative logical path of the live file.
    pub live: PathBuf,
    pub head: String,
    pub live_blob: String,
    pub live_executable: bool,
    pub live_bytes: Vec<u8>,
    /// What the real root holds, or `None` when the file is absent there.
    pub root: Option<FileState>,
    pub siblings: Vec<SiblingView>,
}

/// One explicit include-list entry, for management (missing/empty included).
#[derive(Serialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct EntryView {
    pub key: String,
    pub kind: EntryKind,
    /// Other explicit tracked keys that still cover this path.
    pub covering: Vec<String>,
}

/// File versus directory, taken from the trailing `/` on the key.
#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    File,
    Directory,
}

/// One live or staged file as the tree lists it. Folder sizes are summed
/// later in the frontend from this file list.
#[derive(Serialize, Clone, PartialEq, Eq, Debug)]
pub struct TrackedFile {
    pub rel: String,
    pub bytes: u64,
    pub state: FileSync,
}

/// Whether a listed file is on disk and within the per-file limit.
#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum FileSync {
    Synced,
    TooLarge,
    Pending,
}

/// What [`Engine::add_root`] seeded, plus files the first mirror skipped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddRootReport {
    pub slug: String,
    pub skipped_folders: Vec<project::Skipped>,
    pub skipped_too_large: Vec<(PathBuf, u64)>,
}

/// A new root whose staging repo exists, but whose pattern files are not
/// copied yet. [`AddSeed::materialize`] does that walk without the engine
/// mutex or the home lock; [`Engine::complete_add_root`] registers it after.
pub struct AddSeed {
    pub slug: String,
    path: PathBuf,
    patterns: Vec<String>,
    ignore: String,
    limits: Limits,
    display_name: String,
    is_agent: bool,
    home: PathBuf,
    home_dir: PathBuf,
    device_name: String,
    device_id: String,
    cloud_base: PathBuf,
    /// Exclusive lock held until drop so another process cannot archive this
    /// staging repo while [`AddSeed::materialize`] copies pattern matches.
    _reservation: File,
}

/// First step of [`Engine::add_root`]. `Done` already linked an existing slug.
pub enum AddRootStart {
    Done(AddRootReport),
    Seed(AddSeed),
}

impl AddSeed {
    /// Walk include-list patterns, copy matches into staging, and publish.
    /// Does not register the root and does not take the home lock.
    pub fn materialize(&self) -> Result<AddRootReport> {
        let (file, skipped_folders) =
            project::seed(&self.path, &self.patterns, &self.ignore, self.limits)?;
        let mut repo = Repo::open(
            &self.home,
            &self.slug,
            &self.path,
            &self.device_name,
            &self.device_id,
        )?;
        repo.limits = self.limits;
        project::write(&repo.staging, &file)?;
        fs::write(repo.staging.join(project::IGNORE_FILE), &self.ignore)?;
        // `cloud_base` is already `<provider>/dotlore`. `cloud_at` would append
        // another `dotlore` segment.
        let cloud = Cloud {
            base: self.cloud_base.clone(),
        };
        cloud.write_manifest_once(&Manifest {
            slug: self.slug.clone(),
            display_name: self.display_name.clone(),
            is_agent: self.is_agent,
        })?;
        cloud.write_device_name_once(&self.slug, &self.device_id, &self.device_name)?;
        let (_, report) = repo.commit_local(true, &self.home_dir)?;
        repo.publish(&cloud)?;
        Ok(AddRootReport {
            slug: self.slug.clone(),
            skipped_folders,
            skipped_too_large: report.skipped_too_large,
        })
    }
}

impl fmt::Display for AddRootReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.slug)
    }
}

impl PartialEq<str> for AddRootReport {
    fn eq(&self, other: &str) -> bool {
        self.slug == other
    }
}

impl PartialEq<&str> for AddRootReport {
    fn eq(&self, other: &&str) -> bool {
        self.slug == *other
    }
}

/// What [`Engine::import_installed_agents`] added, and the homes it could not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportAgentsReport {
    pub added: Vec<String>,
    pub failed: Vec<(PathBuf, String)>,
}

/// What [`Engine::wipe_cloud_data`] re-added, and the slugs it could not.
#[derive(Debug)]
pub struct WipeReport {
    pub readded: Vec<String>,
    pub failed: Vec<(String, String)>,
}

/// Preview of a path the user may add. `bytes` excludes ignored, unsafe,
/// and over-file-limit content.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InspectedEntry {
    pub kind: EntryKind,
    pub bytes: u64,
    pub folder_limit: u64,
    pub confirmation_required: bool,
    pub skipped_too_large: Vec<(PathBuf, u64)>,
}

/// Result of [`Engine::track_entry`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrackOutcome {
    Done(RootStatus),
    NeedsConfirmation(InspectedEntry),
}

/// Outcome of a save from the resolver.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ResolveOutcome {
    /// Applied to the live root and published. Only this closes the UI.
    Applied(RootStatus),
    /// Something moved between Open and Save; nothing was deleted or written.
    /// Carries the refreshed snapshot so the UI can redisplay.
    Stale(Box<ResolutionSnapshot>),
    /// Nothing finished: either an earlier apply this root still owes, or
    /// this one could not write every path. The next cycle finishes it only
    /// when nothing is blocked — a blocked path comes back as
    /// `RootStatus::Error` on the root, and that message names the remedy.
    /// Never a completed save.
    Pending,
}

/// One include-list catalog and the lines that apply the next time a folder is added.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatternCatalog {
    pub id: String,
    pub label: String,
    pub lines: Vec<String>,
}

/// Dropdown order: `projects`, each [`project::AGENT_PATTERNS`] key, then `other`.
fn catalog_ids() -> impl Iterator<Item = &'static str> {
    std::iter::once("projects")
        .chain(project::AGENT_PATTERNS.iter().map(|(id, _)| *id))
        .chain(std::iter::once("other"))
}

fn lines_owned(lines: &[&str]) -> Vec<String> {
    lines.iter().copied().map(str::to_string).collect()
}

/// The sync engine for one state directory.
pub struct Engine {
    pub home: PathBuf,
    pub home_dir: PathBuf,
    pub cfg: Config,
    pub cloud: Cloud,
    /// Slugs reserved by [`Engine::begin_add_root`] until register or cancel.
    /// In memory only, so a second add cannot archive the staging repo while
    /// pattern seeding runs without the home lock.
    pending_adds: Vec<(String, PathBuf)>,
}

impl Engine {
    /// Requires a configured provider folder; use [`configure_provider`]
    /// first on a fresh install.
    pub fn new(home: &Path, home_dir: &Path, cfg: Config) -> Result<Engine> {
        let provider = cfg
            .provider_dir
            .clone()
            .ok_or_else(|| anyhow!("no provider folder is configured"))?;
        Ok(Engine {
            home: home.to_path_buf(),
            home_dir: home_dir.to_path_buf(),
            cloud: cloud_at(&provider),
            cfg,
            pending_adds: Vec::new(),
        })
    }

    // --- public entry points ----------------------------------------------

    /// Start tracking `path`, or link to it when the cloud already knows the
    /// slug. Surfaces seed-folder skips and files over the per-file limit.
    ///
    /// Pattern seeding and the first copy run without the home lock so the
    /// rest of the app can keep reading other roots. The lock is taken again
    /// only to register. Those takes are sequential, not nested.
    pub fn add_root(&mut self, path: &Path, slug: Option<&str>) -> Result<AddRootReport> {
        match self.begin_add_root(path, slug)? {
            AddRootStart::Done(report) => Ok(report),
            AddRootStart::Seed(seed) => {
                let report = match seed.materialize() {
                    Ok(report) => report,
                    Err(err) => {
                        self.cancel_add_root(&seed.slug);
                        return Err(err);
                    }
                };
                if let Err(err) = self.complete_add_root(&seed) {
                    self.cancel_add_root(&seed.slug);
                    return Err(err);
                }
                Ok(report)
            }
        }
    }

    /// Validate, init the staging repo, and either link an existing slug or
    /// hand back an [`AddSeed`] the caller materializes without this lock.
    pub fn begin_add_root(&mut self, path: &Path, slug: Option<&str>) -> Result<AddRootStart> {
        let g = config::lock(&self.home)?;
        self.reload(&g)?;
        let cloud = self.cloud();

        // Every rejection happens before the first byte is written anywhere.
        let path = resolve_target(path)?;
        let md = fs::symlink_metadata(&path)
            .with_context(|| format!("{} is unreadable", path.display()))?;
        if !md.is_dir() {
            bail!("{} is not a directory", path.display());
        }
        let slug = match slug {
            Some(s) => s.to_string(),
            None => config::default_slug(&path, &self.home_dir),
        };
        check_slug(&slug)?;
        self.check_placement(&path)?;
        self.check_unregistered(&slug, &path)?;
        self.check_not_pending(&slug, &path)?;

        // The cloud decides first: a slug another device already created is a
        // Link, never a second unrelated history. A manifest we cannot read is
        // not evidence that the slug is free.
        match cloud.read_manifest(&slug) {
            Some(_) => {
                self.link_root_locked(&g, &cloud, &slug, &path)?;
                return Ok(AddRootStart::Done(AddRootReport {
                    slug,
                    skipped_folders: Vec::new(),
                    skipped_too_large: Vec::new(),
                }));
            }
            None if cloud.slug_dir(&slug)?.join("manifest.json").exists() => bail!(
                "the manifest for {slug} exists but could not be read; retry once the \
                 provider has finished downloading it"
            ),
            None => {}
        }

        let reservation = Self::reserve_add(&self.home, &slug)?;
        self.pending_adds.push((slug.clone(), path.clone()));
        let prepared = (|| -> Result<AddSeed> {
            self.archive_stale_staging(&slug)?;
            Repo::init(
                &self.home,
                &slug,
                &path,
                &self.cfg.device_name,
                &self.cfg.device_id,
            )?;
            let display_name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| slug.clone());
            let root = Root {
                slug: slug.clone(),
                path: path.clone(),
                initializing: false,
            };
            // Written before the first commit so both arrive with the history
            // every other device links to. `link_root_locked` writes neither.
            let ignore = self
                .cfg
                .default_ignore
                .as_deref()
                .unwrap_or(project::DEFAULT_NEVER_IGNORE)
                .to_string();
            let patterns = project::patterns_for(&root, &self.home_dir, &self.cfg);
            Ok(AddSeed {
                slug: slug.clone(),
                path: path.clone(),
                patterns,
                ignore,
                limits: Limits::from_config(&self.cfg),
                display_name,
                is_agent: root.is_agent(&self.home_dir),
                home: self.home.clone(),
                home_dir: self.home_dir.clone(),
                device_name: self.cfg.device_name.clone(),
                device_id: self.cfg.device_id.clone(),
                cloud_base: cloud.base.clone(),
                _reservation: reservation,
            })
        })();
        match prepared {
            Ok(seed) => Ok(AddRootStart::Seed(seed)),
            Err(err) => {
                self.cancel_add_root(&slug);
                Err(err)
            }
        }
    }

    /// Register a root whose staging history [`AddSeed::materialize`] already
    /// published. Reloads config under a fresh home lock.
    pub fn complete_add_root(&mut self, seed: &AddSeed) -> Result<()> {
        let g = config::lock(&self.home)?;
        self.reload(&g)?;
        self.check_unregistered(&seed.slug, &seed.path)?;
        self.register(
            &g,
            Root {
                slug: seed.slug.clone(),
                path: seed.path.clone(),
                initializing: false,
            },
        )?;
        self.cancel_add_root(&seed.slug);
        Ok(())
    }

    /// Drop a reservation left by [`Engine::begin_add_root`] when seeding fails.
    pub fn cancel_add_root(&mut self, slug: &str) {
        self.pending_adds.retain(|(pending, _)| pending != slug);
    }

    /// Cross-process hold on `<home>/adding/<slug>`. Released when the file
    /// is dropped. A crash drops it too, so the next add can take the slug.
    fn reserve_add(home: &Path, slug: &str) -> Result<File> {
        let dir = home.join("adding");
        fs::create_dir_all(&dir)?;
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(dir.join(slug))?;
        match file.try_lock() {
            Ok(()) => Ok(file),
            Err(fs::TryLockError::WouldBlock) => bail!("{slug} is already being added"),
            Err(fs::TryLockError::Error(err)) => {
                Err(err).context(format!("locking add reservation for {slug}"))
            }
        }
    }

    fn check_not_pending(&self, slug: &str, path: &Path) -> Result<()> {
        if let Some((other, _)) = self
            .pending_adds
            .iter()
            .find(|(_, pending)| pending == path)
        {
            bail!("{} is already being added as {other}", path.display());
        }
        if self.pending_adds.iter().any(|(pending, _)| pending == slug) {
            bail!("{slug} is already being added");
        }
        Ok(())
    }

    /// Add each installed catalog agent home that is not already tracked or
    /// dismissed. One failure is recorded and the rest still run.
    ///
    /// The home lock is taken only to reload the skip lists. [`add_root`]
    /// locks again itself, and that lock is not reentrant.
    pub fn import_installed_agents(&mut self) -> Result<ImportAgentsReport> {
        let known = {
            let g = config::lock(&self.home)?;
            self.reload(&g)?;
            let mut known: Vec<PathBuf> = self.cfg.roots.iter().map(|r| r.path.clone()).collect();
            known.extend(self.cfg.dismissed_agents.iter().cloned());
            drop(g);
            known
        };

        let mut added = Vec::new();
        let mut failed = Vec::new();
        for path in project::installed_agent_dirs(&self.home_dir) {
            if known.iter().any(|stored| same_agent_path(stored, &path)) {
                continue;
            }
            match self.add_root(&path, None) {
                Ok(report) => added.push(report.slug),
                Err(e) => failed.push((path, e.to_string())),
            }
        }
        Ok(ImportAgentsReport { added, failed })
    }

    /// Adopt an existing cloud slug onto a local path.
    pub fn link_root(&mut self, slug: &str, path: &Path) -> Result<RootStatus> {
        let g = config::lock(&self.home)?;
        self.reload(&g)?;
        let cloud = self.cloud();
        self.link_root_locked(&g, &cloud, slug, path)
    }

    /// One full cycle for one root.
    pub fn sync_root(&mut self, slug: &str) -> Result<RootStatus> {
        let g = config::lock(&self.home)?;
        self.reload(&g)?;
        let cloud = self.cloud();
        self.sync_root_locked(&g, &cloud, slug)
    }

    /// One cycle for every currently registered root, under one lock.
    pub fn sync_all(&mut self) -> Result<Vec<(String, RootStatus)>> {
        let g = config::lock(&self.home)?;
        self.reload(&g)?;
        let cloud = self.cloud();
        let slugs: Vec<String> = self.cfg.roots.iter().map(|r| r.slug.clone()).collect();
        let mut out = Vec::new();
        for slug in slugs {
            let status = self.sync_root_locked(&g, &cloud, &slug)?;
            out.push((slug, status));
        }
        Ok(out)
    }

    /// Stop tracking a root. The staging repo is kept, so re-linking is cheap
    /// and nothing that was never published is thrown away.
    ///
    /// A removed catalog agent home is appended to `dismissed_agents` so a
    /// later import leaves it out. Other roots are not.
    pub fn remove_root(&mut self, slug: &str) -> Result<()> {
        let g = config::lock(&self.home)?;
        self.reload(&g)?;
        let Some(removed) = self.cfg.roots.iter().find(|r| r.slug == slug).cloned() else {
            bail!("no tracked root with slug {slug}");
        };
        self.cfg.roots.retain(|r| r.slug != slug);
        if is_catalog_agent(&self.home_dir, &removed.path)
            && !self
                .cfg
                .dismissed_agents
                .iter()
                .any(|p| same_agent_path(p, &removed.path))
        {
            self.cfg.dismissed_agents.push(removed.path);
        }
        self.save(&g)
    }

    /// Delete `<provider>/dotlore` for every device, then this device's
    /// `repos/`, `recovery/` and `tmp/`, and add every registered root again
    /// so each one re-seeds from the current patterns. `config.json` and the
    /// lock files are kept.
    ///
    /// The one scoped exception to "the cloud is immutable", reached only when
    /// the user asks for it. Never call it from a sync path. Every target is
    /// checked before the first delete: a symlink or a non-directory stops
    /// the wipe with nothing removed. The home lock is dropped before each
    /// re-add because [`add_root`] locks again itself. A root that cannot be
    /// added again is left unregistered and reported in `failed`.
    pub fn wipe_cloud_data(&mut self) -> Result<WipeReport> {
        let g = config::lock(&self.home)?;
        // `reload` already refuses a config without a provider folder.
        self.reload(&g)?;
        if !self.pending_adds.is_empty() {
            bail!("an add is in progress");
        }

        // The cloud goes first: with a manifest left behind, the re-add
        // below would link instead of seeding.
        let mut targets = vec![self.cloud().base];
        targets.extend(["repos", "recovery", "tmp"].map(|d| self.home.join(d)));
        let mut present = Vec::new();
        for t in &targets {
            if removable_dir(t)? {
                present.push(t);
            }
        }
        for t in present {
            fs::remove_dir_all(t).with_context(|| format!("deleting {}", t.display()))?;
        }

        let roots = self.cfg.roots.clone();
        drop(g);

        // Each root leaves the config only right before it is added again, so
        // a crash mid-loop loses at most the root in flight; the rest stay
        // registered and a second wipe picks them up.
        let mut readded = Vec::new();
        let mut failed = Vec::new();
        for root in roots {
            let g = config::lock(&self.home)?;
            self.reload(&g)?;
            self.cfg.roots.retain(|r| r.slug != root.slug);
            self.save(&g)?;
            drop(g);
            match self.add_root(&root.path, Some(&root.slug)) {
                Ok(_) => readded.push(root.slug),
                Err(e) => failed.push((root.slug, format!("{e:#}"))),
            }
        }
        Ok(WipeReport { readded, failed })
    }

    /// Rebuild a damaged staging repo from a verified backup plus the cloud.
    pub fn recover_root(&mut self, slug: &str) -> Result<RootStatus> {
        let g = config::lock(&self.home)?;
        self.reload(&g)?;
        let cloud = self.cloud();
        self.recover_root_locked(&g, &cloud, slug)
    }

    /// Every unresolved conflict in a root's staging repo.
    pub fn conflicts(&mut self, slug: &str) -> Result<Vec<ConflictView>> {
        let g = config::lock(&self.home)?;
        self.reload(&g)?;
        let cloud = self.cloud();
        let repo = self.repo_for(&self.root_cfg(slug)?)?;
        let names = cloud.device_names(slug);
        let mut out = Vec::new();
        for c in open_conflicts(&repo)? {
            out.push(ConflictView {
                loser_name: device_name(&names, &c.loser_id8),
                loser_is_me: c.loser_id8 == repo.id8(),
                live: c.live,
                sibling: c.sibling,
                loser_id8: c.loser_id8,
            });
        }
        drop(g);
        Ok(out)
    }

    /// Root-relative files currently tracked for `slug`, sorted by `rel`.
    ///
    /// Live files come from the same include-list walk as `mirror::walk`.
    /// Over-limit files never reach staging, so they are listed here as
    /// `TooLarge`. Staged paths with no live file (or a missing root) are
    /// `Pending`.
    pub fn tracked_files(&mut self, slug: &str) -> Result<Vec<TrackedFile>> {
        let g = config::lock(&self.home)?;
        self.reload(&g)?;
        let root = self.root_cfg(slug)?;
        let repo = self.repo_for(&root)?;
        let entries = repo.project_file()?.tracked();
        let ignore = repo.ignore_text(&self.home_dir);
        let limits = Limits::from_config(&self.cfg);
        let staged = repo.git.ok(&["ls-files", "-z"])?;
        let live_path = root.path.clone();
        drop(g);

        let staged: Vec<String> = staged
            .split('\0')
            .filter(|s| !s.is_empty())
            .filter(|s| !is_internal(s))
            .filter(|s| entries.contains_rel(Path::new(s)))
            .map(|s| s.to_string())
            .collect();

        let mut by_rel = BTreeMap::new();
        if root_present(&live_path) {
            for (rel, bytes, too_large) in mirror::list_live(&live_path, &entries, &ignore, limits)?
            {
                let rel = rel.to_string_lossy().into_owned();
                if is_internal(&rel) {
                    continue;
                }
                let state = if too_large {
                    FileSync::TooLarge
                } else {
                    FileSync::Synced
                };
                by_rel.insert(rel.clone(), TrackedFile { rel, bytes, state });
            }
        }
        for rel in staged {
            by_rel.entry(rel.clone()).or_insert(TrackedFile {
                rel,
                bytes: 0,
                state: FileSync::Pending,
            });
        }
        Ok(by_rel.into_values().collect())
    }

    /// Explicit include-list keys for `slug`, including missing/empty entries.
    pub fn list_entries(&mut self, slug: &str) -> Result<Vec<EntryView>> {
        let g = config::lock(&self.home)?;
        self.reload(&g)?;
        let repo = self.repo_for(&self.root_cfg(slug)?)?;
        let file = repo.project_file()?;
        drop(g);
        let mut out: Vec<EntryView> = file
            .entries
            .iter()
            .filter(|(_, rec)| rec.state == State::Tracked)
            .map(|(key, _)| EntryView {
                covering: file.covering_keys(key),
                kind: if key.ends_with('/') {
                    EntryKind::Directory
                } else {
                    EntryKind::File
                },
                key: key.clone(),
            })
            .collect();
        out.sort_by(|a, b| a.key.cmp(&b.key));
        Ok(out)
    }

    /// Preview `rel` under `slug`. Read-only: a cancelled add does not mutate.
    pub fn inspect_entry(&mut self, slug: &str, rel: &Path) -> Result<InspectedEntry> {
        let g = config::lock(&self.home)?;
        self.reload(&g)?;
        let root = self.root_cfg(slug)?;
        let repo = self.repo_for(&root)?;
        let limits = Limits::from_config(&self.cfg);
        let ignore = repo.ignore_text(&self.home_dir);
        drop(g);
        inspect_live(&root.path, rel, &ignore, limits)
    }

    /// Track `rel` under `slug`. Uses safe live-path inspection.
    ///
    /// An oversized folder requires `confirmed_folder_bytes` of at least the
    /// newly measured total. Confirmation never overrides the per-file limit.
    pub fn track_entry(
        &mut self,
        slug: &str,
        rel: &Path,
        confirmed_folder_bytes: Option<u64>,
    ) -> Result<TrackOutcome> {
        let g = config::lock(&self.home)?;
        self.reload(&g)?;
        let cloud = self.cloud();
        let root = self.root_cfg(slug)?;
        if !plain_rel(rel) {
            bail!("unsafe path {}", rel.display());
        }
        let limits = Limits::from_config(&self.cfg);
        let repo = self.repo_for(&root)?;
        let ignore = repo.ignore_text(&self.home_dir);
        let preview = inspect_live(&root.path, rel, &ignore, limits)?;
        match preview.kind {
            EntryKind::File => {
                if let Some((_, len)) = preview.skipped_too_large.first() {
                    bail!(
                        "{} is {len} bytes (limit {} bytes)",
                        rel.display(),
                        limits.max_file_bytes
                    );
                }
            }
            EntryKind::Directory => {
                if preview.confirmation_required {
                    let approved = confirmed_folder_bytes.unwrap_or(0);
                    if approved < preview.bytes {
                        return Ok(TrackOutcome::NeedsConfirmation(preview));
                    }
                }
            }
        }
        let key = match preview.kind {
            EntryKind::Directory => format!("{}/", trim_rel(rel).display()),
            EntryKind::File => trim_rel(rel).display().to_string(),
        };
        let mut file = repo.project_file()?;
        let gen = file
            .entries
            .values()
            .map(|e| e.gen)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        file.entries.insert(
            key,
            project::EntryRecord {
                gen,
                state: State::Tracked,
            },
        );
        project::write(&repo.staging, &file)?;
        let status = self.run_cycle(&cloud, &root, FetchMode::Normal);
        self.settle(&g, slug, &status)?;
        Ok(TrackOutcome::Done(status))
    }

    /// Tombstone one include path. Inherited paths under a tracked directory
    /// become a hole in that parent. Lexical when the live path is missing.
    pub fn untrack_entry(&mut self, slug: &str, rel: &Path) -> Result<RootStatus> {
        let g = config::lock(&self.home)?;
        self.reload(&g)?;
        let cloud = self.cloud();
        let root = self.root_cfg(slug)?;
        if !plain_rel(rel) {
            bail!("unsafe path {}", rel.display());
        }
        let repo = self.repo_for(&root)?;
        let as_dir = root.path.join(rel).is_dir() || repo.staging.join(rel).is_dir();
        let mut file = repo.project_file()?;
        file.untrack_as(rel, Some(as_dir))?;
        project::write(&repo.staging, &file)?;
        let status = self.run_cycle(&cloud, &root, FetchMode::Normal);
        self.settle(&g, slug, &status)?;
        Ok(status)
    }

    /// Effective project seed patterns (`None` in config → `DEFAULT_PATTERNS`).
    pub fn default_patterns(&self) -> Vec<String> {
        match &self.cfg.default_patterns {
            Some(p) => p.clone(),
            None => project::DEFAULT_PATTERNS
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
        }
    }

    pub fn set_default_patterns(&mut self, patterns: Vec<String>) -> Result<()> {
        self.set_pattern_catalog("projects", patterns)
    }

    /// Every builtin catalog, in dropdown order, with its effective lines.
    pub fn pattern_catalogs(&self) -> Vec<PatternCatalog> {
        let mut catalogs = Vec::new();
        for id in catalog_ids() {
            let Some(label) = project::catalog_label(id) else {
                continue;
            };
            let lines = if id == "projects" {
                self.default_patterns()
            } else if let Some(over) = self.cfg.agent_patterns.get(id) {
                over.clone()
            } else {
                match project::builtin_lines(id) {
                    Some(builtin) => lines_owned(builtin),
                    None => continue,
                }
            };
            catalogs.push(PatternCatalog {
                id: id.to_string(),
                label: label.to_string(),
                lines,
            });
        }
        catalogs
    }

    /// Store one catalog. A list equal to that catalog's builtin clears the override.
    /// An unknown id returns an error and does not write config.
    pub fn set_pattern_catalog(&mut self, catalog: &str, patterns: Vec<String>) -> Result<()> {
        let g = config::lock(&self.home)?;
        self.reload(&g)?;
        let Some(builtin) = project::builtin_lines(catalog) else {
            bail!("unknown pattern catalog {catalog}");
        };
        if patterns == lines_owned(builtin) {
            if catalog == "projects" {
                self.cfg.default_patterns = None;
            } else {
                self.cfg.agent_patterns.remove(catalog);
            }
        } else if catalog == "projects" {
            self.cfg.default_patterns = Some(patterns);
        } else {
            self.cfg
                .agent_patterns
                .insert(catalog.to_string(), patterns);
        }
        self.save(&g)
    }

    /// Effective ignore text (`None` in config → `DEFAULT_NEVER_IGNORE`).
    pub fn default_ignore(&self) -> String {
        self.cfg
            .default_ignore
            .clone()
            .unwrap_or_else(|| project::DEFAULT_NEVER_IGNORE.to_string())
    }

    pub fn set_default_ignore(&mut self, ignore: String) -> Result<()> {
        let g = config::lock(&self.home)?;
        self.reload(&g)?;
        self.cfg.default_ignore = if ignore == project::DEFAULT_NEVER_IGNORE {
            None
        } else {
            Some(ignore)
        };
        self.save(&g)
    }

    /// Effective per-file ceiling in MiB (`None` in config → 50).
    pub fn max_file_mb(&self) -> u64 {
        self.cfg.max_file_mb.unwrap_or(Limits::DEFAULT_MAX_FILE_MB)
    }

    pub fn set_max_file_mb(&mut self, mb: u64) -> Result<()> {
        let g = config::lock(&self.home)?;
        self.reload(&g)?;
        self.cfg.max_file_mb = Some(mb);
        self.save(&g)
    }

    /// Effective seed/add folder ceiling in MiB (`None` in config → 200).
    pub fn max_seed_folder_mb(&self) -> u64 {
        self.cfg
            .max_seed_folder_mb
            .unwrap_or(Limits::DEFAULT_MAX_SEED_FOLDER_MB)
    }

    pub fn set_max_seed_folder_mb(&mut self, mb: u64) -> Result<()> {
        let g = config::lock(&self.home)?;
        self.reload(&g)?;
        self.cfg.max_seed_folder_mb = Some(mb);
        self.save(&g)
    }

    /// The bytes of one exact `(live, sibling)` pair, as committed.
    pub fn conflict_sides(
        &mut self,
        slug: &str,
        live: &Path,
        sibling: &Path,
    ) -> Result<(Vec<u8>, Vec<u8>)> {
        let g = config::lock(&self.home)?;
        self.reload(&g)?;
        let repo = self.repo_for(&self.root_cfg(slug)?)?;
        let pair = conflict::list(&repo.git)?
            .into_iter()
            .find(|c| c.live == live && c.sibling == sibling)
            .ok_or_else(|| {
                anyhow!(
                    "{} is not a conflict sibling of {}",
                    sibling.display(),
                    live.display()
                )
            })?;
        let live_bytes = blob_bytes(&repo.git, &tree_entry(&repo.git, "HEAD", &pair.live)?.0)?;
        let other = blob_bytes(&repo.git, &tree_entry(&repo.git, "HEAD", &pair.sibling)?.0)?;
        drop(g);
        Ok((live_bytes, other))
    }

    /// Capture every version the resolver is about to display.
    pub fn open_resolution(&mut self, slug: &str, live: &Path) -> Result<ResolutionSnapshot> {
        let g = config::lock(&self.home)?;
        self.reload(&g)?;
        let cloud = self.cloud();
        let root = self.root_cfg(slug)?;
        let repo = self.repo_for(&root)?;
        let snap = self.snapshot(&repo, &cloud, slug, live)?;
        drop(g);
        Ok(snap)
    }

    /// Save a resolution, but only if every version still matches `snapshot`.
    ///
    /// `selected_siblings` must be a subset of the siblings the snapshot
    /// displayed for exactly this live path; nothing else is ever deleted.
    pub fn resolve_conflict(
        &mut self,
        slug: &str,
        snapshot: &ResolutionSnapshot,
        selected_siblings: &[PathBuf],
        content: &[u8],
    ) -> Result<ResolveOutcome> {
        let g = config::lock(&self.home)?;
        self.reload(&g)?;
        let cloud = self.cloud();
        if snapshot.slug != slug {
            bail!("snapshot belongs to {}, not {slug}", snapshot.slug);
        }
        let root = self.root_cfg(slug)?;
        let repo = self.repo_for(&root)?;

        // Finish whatever the last cycle left owing before reading versions.
        if let Some(mut tx) = repo.pending_tx(conflict::resolve_index)? {
            match tx.resume(&repo, &self.home_dir)? {
                ResumeOutcome::Finished => {
                    repo.publish(&cloud)?;
                }
                ResumeOutcome::Pending(_) => return Ok(ResolveOutcome::Pending),
                // A journal is still open, so no new transaction can start.
                // Re-show the refreshed state rather than claim a save.
                ResumeOutcome::Restart => {
                    let fresh = self.snapshot(&repo, &cloud, slug, &snapshot.live)?;
                    return Ok(ResolveOutcome::Stale(Box::new(fresh)));
                }
            }
        }

        let fresh = self.snapshot(&repo, &cloud, slug, &snapshot.live)?;
        if !same_versions(&fresh, snapshot) {
            return Ok(ResolveOutcome::Stale(Box::new(fresh)));
        }
        for s in selected_siblings {
            if !fresh.siblings.iter().any(|v| &v.path == s) {
                bail!(
                    "{} was not one of the siblings shown for {}",
                    s.display(),
                    snapshot.live.display()
                );
            }
        }
        // `conflict::resolve` commits unconditionally, and an empty commit is
        // an error; a no-op save is simply already done.
        if selected_siblings.is_empty() && content == snapshot.live_bytes {
            return Ok(ResolveOutcome::Applied(root_status(&repo)?));
        }

        let key = provider_key(&cloud);
        let mut tx = repo.begin_tx(&key, "HEAD", conflict::resolve_index)?;
        conflict::resolve(&tx, &snapshot.live, selected_siblings, content)?;
        tx.set_target(&repo)?;
        if !tx.apply(&repo, &self.home_dir)?.is_empty() {
            return Ok(ResolveOutcome::Pending);
        }
        tx.finalize(&repo)?;
        repo.publish(&cloud)?;
        let status = root_status(&repo)?;
        self.settle(&g, slug, &status)?;
        Ok(ResolveOutcome::Applied(status))
    }

    // --- locked helpers ---------------------------------------------------

    fn link_root_locked(
        &mut self,
        g: &HomeLock,
        cloud: &Cloud,
        slug: &str,
        path: &Path,
    ) -> Result<RootStatus> {
        check_slug(slug)?;
        cloud.read_manifest(slug).ok_or_else(|| {
            anyhow!(
                "no readable manifest for {slug} in {}",
                cloud.base.display()
            )
        })?;
        let path = resolve_target(path)?;
        match fs::symlink_metadata(&path) {
            Ok(md) if md.is_dir() => {}
            Ok(_) => bail!("{} is not a directory", path.display()),
            Err(e) => return Err(e).with_context(|| format!("{} is missing", path.display())),
        }
        self.check_placement(&path)?;
        self.check_unregistered(slug, &path)?;

        // No `.dotloreignore`, no `.dotloreproject`, and no local first
        // commit: all three arrive with the history the creating device
        // published.
        self.archive_stale_staging(slug)?;
        Repo::init(
            &self.home,
            slug,
            &path,
            &self.cfg.device_name,
            &self.cfg.device_id,
        )?;
        cloud.write_device_name_once(slug, &self.cfg.device_id, &self.cfg.device_name)?;
        let root = Root {
            slug: slug.to_string(),
            path,
            initializing: true,
        };
        self.register(g, root.clone())?;
        let status = self.run_cycle(cloud, &root, FetchMode::Normal);
        self.settle(g, slug, &status)?;
        Ok(status)
    }

    fn sync_root_locked(&mut self, g: &HomeLock, cloud: &Cloud, slug: &str) -> Result<RootStatus> {
        let root = self.root_cfg(slug)?;
        let status = self.run_cycle(cloud, &root, FetchMode::Normal);
        self.settle(g, slug, &status)?;
        Ok(status)
    }

    fn recover_root_locked(
        &mut self,
        g: &HomeLock,
        cloud: &Cloud,
        slug: &str,
    ) -> Result<RootStatus> {
        // The registration is preserved: recovery is not remove + re-link.
        let root = self.root_cfg(slug)?;
        if git::which_git().is_none() {
            return Ok(RootStatus::GitMissing);
        }
        let staging = self.home.join("repos").join(slug);

        if staging.exists() {
            let dst = self
                .home
                .join("recovery")
                .join(format!("{slug}-{}", unique_id()));
            copy_tree(&staging, &dst)?;
            verify_copy(&staging, &dst)
                .with_context(|| format!("verifying the backup at {}", dst.display()))?;
            // Fail closed: unpublished work we cannot read is not work the
            // cloud has. Both the backup and the live root stay untouched.
            for name in ["dotlore-apply.json", "dotlore-state.json"] {
                let p = dst.join(".git").join(name);
                if p.exists()
                    && serde_json::from_slice::<serde_json::Value>(&fs::read(&p)?).is_err()
                {
                    bail!(
                        "{name} could not be read; the staging repo was copied to {} and \
                         nothing was changed",
                        dst.display()
                    );
                }
            }
            let bg = Git::new(dst.clone(), &self.cfg.device_name, &self.cfg.device_id);
            if !bg.run(&["rev-parse", "--git-dir"])?.status.success() {
                bail!(
                    "the staging repo for {slug} is unreadable; it was copied to {} and \
                     nothing was changed. Unpublished commits there cannot be recovered \
                     automatically.",
                    dst.display()
                );
            }
        }

        let usable = Repo::open(
            &self.home,
            slug,
            &root.path,
            &self.cfg.device_name,
            &self.cfg.device_id,
        )
        .map(|r| {
            r.git
                .run(&["rev-parse", "--git-dir"])
                .map(|o| o.status.success())
                .unwrap_or(false)
        })
        .unwrap_or(false);

        // A staging repo that is present but unopenable never reaches here:
        // the backup is a byte copy of it, so the check above already refused.
        // What is left is the staging directory being gone entirely, and then
        // the cloud — this device's own bundles included — is the history.
        if !usable {
            Repo::init(
                &self.home,
                slug,
                &root.path,
                &self.cfg.device_name,
                &self.cfg.device_id,
            )?;
        }

        // Recovery fetches this device's own bundles too; a normal cycle has
        // no use for them, but here they may be the only copy left.
        let status = self.run_cycle(cloud, &root, FetchMode::Recovery);
        self.settle(g, slug, &status)?;
        Ok(status)
    }

    /// The cycle, with any failure turned into `Error` so the journal and the
    /// pinned objects survive for the next attempt.
    fn run_cycle(&self, cloud: &Cloud, root: &Root, mode: FetchMode) -> RootStatus {
        match self.cycle(cloud, root, mode) {
            Ok(s) => s,
            Err(e) => RootStatus::Error(format!("{e:#}")),
        }
    }

    fn cycle(&self, cloud: &Cloud, root: &Root, mode: FetchMode) -> Result<RootStatus> {
        if git::which_git().is_none() {
            return Ok(RootStatus::GitMissing);
        }
        let repo = self.repo_for(root)?;

        // Before resuming anything: `mirror::apply_to_root` creates missing
        // parent directories and treats a missing file as "absent", so
        // resuming a journalled apply against a root the user deleted would
        // quietly recreate it. A project folder must already exist; a missing
        // one is a root the user moved, never a deletion to publish.
        if !root_present(&root.path) {
            return Ok(RootStatus::RootMissing);
        }
        // An unmounted cloud folder (a Google account signed out) must not be
        // recreated as a plain local folder the provider never uploads.
        let provider = cloud.base.parent().unwrap_or(&cloud.base);
        if !provider.is_dir() {
            bail!("provider folder {} is missing", provider.display());
        }
        // The cloud lost this slug under us: list it again so other devices
        // can link it; `publish` then sends the whole history.
        if repo.has_main() && !cloud.has_bundles(&root.slug, &self.cfg.device_id)? {
            cloud.write_manifest_once(&Manifest {
                slug: root.slug.clone(),
                display_name: root
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| root.slug.clone()),
                is_agent: root.is_agent(&self.home_dir),
            })?;
            cloud.write_device_name_once(&root.slug, &self.cfg.device_id, &self.cfg.device_name)?;
        }

        let key = provider_key(cloud);
        let pending = match repo.pending_tx(conflict::resolve_index)? {
            Some(mut tx) => match tx.resume(&repo, &self.home_dir)? {
                // Nothing was applied and `main` never moved: redo the merges
                // on this transaction. No `commit_local` — the journal's
                // baseline is the one the live root still matches, and local
                // edits are picked up by the apply's reconciliation or by the
                // next cycle.
                ResumeOutcome::Restart => Some(tx),
                ResumeOutcome::Finished => {
                    repo.publish(cloud)?;
                    None
                }
                ResumeOutcome::Pending(_) => return Ok(stalled(&root.slug, &tx)),
            },
            None => None,
        };

        if pending.is_none() && root_present(&root.path) {
            repo.commit_local(false, &self.home_dir)?;
        }
        // Decided against durable `main`, before anything is merged, so the
        // answer does not depend on this transaction succeeding or on the
        // order the devices happen to come in.
        let mut ahead = Vec::new();
        for d in repo.fetch_bundles(cloud, mode)? {
            if needs_merge(&repo, &key, &d)? {
                ahead.push(d);
            }
        }

        let (mut tx, to_merge) = match pending {
            Some(tx) => (tx, ahead),
            None if repo.has_main() => {
                if ahead.is_empty() {
                    repo.publish(cloud)?;
                    return root_status(&repo);
                }
                (repo.begin_tx(&key, "HEAD", conflict::resolve_index)?, ahead)
            }
            // Bootstrap: no local history at all. Adopt the first readable
            // remote head and merge the rest onto it. A manifest without a
            // readable bundle yet is normal, not an error.
            None => {
                let Some((first, rest)) = ahead.split_first() else {
                    return Ok(RootStatus::Pending);
                };
                let tx = repo.begin_tx(&key, &remote_ref(&key, first), conflict::resolve_index)?;
                (tx, rest.to_vec())
            }
        };

        for d in &to_merge {
            repo.merge_remote(&tx, d)?;
        }
        tx.set_target(&repo)?;
        if !tx.apply(&repo, &self.home_dir)?.is_empty() {
            // Journal intact; nothing is published until the root agrees.
            return Ok(stalled(&root.slug, &tx));
        }
        tx.finalize(&repo)?;
        repo.publish(cloud)?;
        root_status(&repo)
    }

    // --- small shared pieces ----------------------------------------------

    fn reload(&mut self, _g: &HomeLock) -> Result<()> {
        self.cfg = Config::load(&self.home)?;
        let provider = self
            .cfg
            .provider_dir
            .clone()
            .ok_or_else(|| anyhow!("no provider folder is configured"))?;
        self.cloud = cloud_at(&provider);
        Ok(())
    }

    /// A `Cloud` of our own, so a `&mut self` method can still pass one down.
    fn cloud(&self) -> Cloud {
        Cloud {
            base: self.cloud.base.clone(),
        }
    }

    fn save(&self, _g: &HomeLock) -> Result<()> {
        self.cfg.save(&self.home)
    }

    fn root_cfg(&self, slug: &str) -> Result<Root> {
        self.cfg
            .roots
            .iter()
            .find(|r| r.slug == slug)
            .cloned()
            .ok_or_else(|| anyhow!("no tracked root with slug {slug}"))
    }

    fn repo_for(&self, root: &Root) -> Result<Repo> {
        let mut repo = Repo::open(
            &self.home,
            &root.slug,
            &root.path,
            &self.cfg.device_name,
            &self.cfg.device_id,
        )?;
        repo.limits = Limits::from_config(&self.cfg);
        Ok(repo)
    }

    /// `self.cfg` was loaded under the guard we still hold, so it is the
    /// freshest config there is; no other process can have written since.
    fn register(&mut self, g: &HomeLock, root: Root) -> Result<()> {
        self.cfg
            .roots
            .retain(|r| r.slug != root.slug && r.path != root.path);
        // Adding the path again is the only way back onto the import list.
        // Cleared here so the root and the shorter dismissal list share this save.
        self.cfg
            .dismissed_agents
            .retain(|p| !same_agent_path(p, &root.path));
        self.cfg.roots.push(root);
        self.save(g)
    }

    /// Clear `initializing` once — and only once — a cycle has actually
    /// applied and published.
    fn settle(&mut self, g: &HomeLock, slug: &str, status: &RootStatus) -> Result<()> {
        if !matches!(status, RootStatus::Synced | RootStatus::Conflicts(_)) {
            return Ok(());
        }
        let mut changed = false;
        for r in &mut self.cfg.roots {
            if r.slug == slug && r.initializing {
                r.initializing = false;
                changed = true;
            }
        }
        if changed {
            self.save(g)?;
        }
        Ok(())
    }

    /// Move a staging repo left behind by an earlier `remove_root` out of the
    /// way before Add or Link reuses the slug.
    ///
    /// `Repo::init` adopts an existing `.git`, and the first `commit_local`
    /// against that old `main` would mirror the new (possibly empty) root over
    /// it and commit a mass deletion — then publish it to every device. The
    /// old repo is kept, never deleted: it may hold unpublished commits.
    fn archive_stale_staging(&self, slug: &str) -> Result<()> {
        let staging = self.home.join("repos").join(slug);
        if !staging.exists() {
            return Ok(());
        }
        let to = self
            .home
            .join("recovery")
            .join(format!("{slug}-{}", unique_id()));
        fs::create_dir_all(self.home.join("recovery"))?;
        fs::rename(&staging, &to).with_context(|| format!("moving {} aside", staging.display()))?;
        Ok(())
    }

    fn check_unregistered(&self, slug: &str, path: &Path) -> Result<()> {
        if let Some(r) = self.cfg.roots.iter().find(|r| r.slug == slug) {
            bail!("{slug} is already tracked at {}", r.path.display());
        }
        if let Some(r) = self.cfg.roots.iter().find(|r| r.path == path) {
            bail!("{} is already tracked as {}", path.display(), r.slug);
        }
        Ok(())
    }

    /// A tracked root may not contain, or sit inside, the state home or the
    /// provider folder.
    fn check_placement(&self, path: &Path) -> Result<()> {
        let home = canonical(&self.home);
        if related(path, &home) {
            bail!(
                "{} overlaps the state directory {}",
                path.display(),
                home.display()
            );
        }
        if let Some(p) = &self.cfg.provider_dir {
            let p = canonical(p);
            if related(path, &p) {
                bail!(
                    "{} overlaps the provider folder {}",
                    path.display(),
                    p.display()
                );
            }
        }
        Ok(())
    }

    fn snapshot(
        &self,
        repo: &Repo,
        cloud: &Cloud,
        slug: &str,
        live: &Path,
    ) -> Result<ResolutionSnapshot> {
        let head = repo
            .git
            .rev("HEAD")
            .ok_or_else(|| anyhow!("{slug} has no commit yet"))?;
        let (live_blob, live_executable) = tree_entry(&repo.git, "HEAD", live)?;
        let live_bytes = blob_bytes(&repo.git, &live_blob)?;
        let names = cloud.device_names(slug);
        let mut siblings = Vec::new();
        for c in conflict::list(&repo.git)? {
            if c.live != live {
                continue;
            }
            let (blob, _) = tree_entry(&repo.git, "HEAD", &c.sibling)?;
            siblings.push(SiblingView {
                bytes: blob_bytes(&repo.git, &blob)?,
                blob,
                path: c.sibling,
                loser_name: device_name(&names, &c.loser_id8),
                loser_id8: c.loser_id8,
            });
        }
        siblings.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(ResolutionSnapshot {
            slug: slug.to_string(),
            live: live.to_path_buf(),
            head,
            live_blob,
            live_executable,
            live_bytes,
            root: root_state(&live_root_path(repo, live)?)?,
            siblings,
        })
    }
}

// --- provider configuration -----------------------------------------------

/// Select (or change) the provider folder. Callable before any [`Engine`]
/// exists, which is what first launch needs.
///
/// The transition is resumable by construction rather than by a second
/// journal: the saved provider is the commit point, and every later cycle
/// finishes the bootstrap because the destination namespace has no
/// `refs/dotlore/sent/<key>` yet (so `publish` bundles `main` whole) and no
/// consumed state (so every destination bundle is fetched).
pub fn configure_provider(
    home: &Path,
    home_dir: &Path,
    path: &Path,
) -> Result<Vec<(String, RootStatus)>> {
    let g = config::lock(home)?;
    let mut cfg = Config::load(home)?;
    let dest = path
        .canonicalize()
        .with_context(|| format!("provider folder {} is unreadable", path.display()))?;
    if !dest.is_dir() {
        bail!("provider folder {} is not a directory", dest.display());
    }
    let home_c = canonical(home);
    if related(&dest, &home_c) {
        bail!(
            "provider folder {} overlaps the state directory {}",
            dest.display(),
            home_c.display()
        );
    }
    for r in &cfg.roots {
        if related(&dest, &r.path) {
            bail!(
                "provider folder {} overlaps tracked root {}",
                dest.display(),
                r.path.display()
            );
        }
    }
    if cfg.provider_dir.as_deref() == Some(dest.as_path()) {
        return Ok(Vec::new());
    }

    // Finish anything owed on the source before its state stops being read.
    if let Some(src) = cfg.provider_dir.clone() {
        let mut e = Engine {
            home: home.to_path_buf(),
            home_dir: home_dir.to_path_buf(),
            cloud: cloud_at(&src),
            cfg,
            pending_adds: Vec::new(),
        };
        let src_cloud = e.cloud();
        for slug in e
            .cfg
            .roots
            .iter()
            .map(|r| r.slug.clone())
            .collect::<Vec<_>>()
        {
            match e.sync_root_locked(&g, &src_cloud, &slug)? {
                RootStatus::Synced | RootStatus::Conflicts(_) => {}
                other => bail!(
                    "cannot switch provider: {slug} is {other:?} on the current provider; \
                     the provider folder was left unchanged"
                ),
            }
        }
        cfg = e.cfg;
    }

    cfg.provider_dir = Some(dest.clone());
    cfg.save(home)?;

    let mut e = Engine {
        home: home.to_path_buf(),
        home_dir: home_dir.to_path_buf(),
        cloud: cloud_at(&dest),
        cfg,
        pending_adds: Vec::new(),
    };
    let dest_cloud = e.cloud();
    let mut out = Vec::new();
    for root in e.cfg.roots.clone() {
        dest_cloud.write_manifest_once(&Manifest {
            slug: root.slug.clone(),
            display_name: root
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| root.slug.clone()),
            is_agent: root.is_agent(home_dir),
        })?;
        dest_cloud.write_device_name_once(&root.slug, &e.cfg.device_id, &e.cfg.device_name)?;
        let status = e.sync_root_locked(&g, &dest_cloud, &root.slug)?;
        out.push((root.slug, status));
    }
    Ok(out)
}

// --- free helpers ----------------------------------------------------------

/// The transport root, `<provider_dir>/dotlore`, with `/dotlore` added once.
///
/// The provider path is canonicalized here so that the delivery namespace
/// (`provider_key`, `refs/dotlore/sent/<key>`, `consumed`) is derived from the
/// real directory: a config that records `/var/…` and one that records
/// `/private/var/…` for the same folder must not become two providers.
/// Where this provider's bundles live: `<provider_dir>/dotlore`, canonicalized.
pub fn cloud_folder(provider_dir: &Path) -> PathBuf {
    cloud_at(provider_dir).base
}

fn cloud_at(provider_dir: &Path) -> Cloud {
    Cloud {
        base: canonical(provider_dir).join("dotlore"),
    }
}

/// Why an apply did not finish, as a status a user can act on.
///
/// A path that raced a live edit is genuinely `Pending` — the next cycle picks
/// it up. A path blocked by a symlink in the live root or by a target entry
/// that is not a regular file is re-skipped on every attempt forever: the
/// whole root then stops committing and publishing behind the same `Pending`
/// the UI shows for "no bundles yet", with nothing to act on. That one is
/// `Error` naming the paths.
///
/// The root stays frozen as a whole while any path is blocked: finalizing the
/// unblocked paths would advance `main` past a root that does not match it,
/// which is the invariant the transaction exists to hold.
fn stalled(slug: &str, tx: &Transaction) -> RootStatus {
    match stall_message(slug, tx.blocked()) {
        Some(m) => RootStatus::Error(m),
        None => RootStatus::Pending,
    }
}

/// The blocked-path message: a live path the user can move aside, plus the
/// recover hinge for when that does not apply.
///
/// `cycle` returns `stalled` *before* `fetch_bundles`, so while the journal
/// stands this device never fetches and a peer's corrective commit cannot
/// arrive. Rebuilding the staging repo is then the only exit, so the message
/// names it rather than promising a next sync that cannot happen.
///
/// Separate from [`stalled`] because a `Transaction`'s blocked list can only
/// be produced by a real apply, and this text is the part worth asserting on.
fn stall_message(slug: &str, blocked: &[PathBuf]) -> Option<String> {
    if blocked.is_empty() {
        return None;
    }
    let names = blocked
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        "cannot write {names} in the live root of {slug}: the live path is a symlink or a \
         directory, or the peer's version of it is not a regular file. Nothing was \
         overwritten; if it is a path you put there, move it aside and the next sync \
         continues. If that does not apply or does not help, sync for {slug} stays stopped: \
         while this apply stands the device never fetches, so a peer's correction cannot \
         even arrive. Once one is published, move repos/{slug} aside under the dotlore \
         home and run `dotlore recover {slug}`; recover on its own only re-reads the same \
         stopped apply."
    ))
}

fn root_status(repo: &Repo) -> Result<RootStatus> {
    if !repo.has_main() {
        return Ok(RootStatus::Pending);
    }
    match open_conflicts(repo)?
        .iter()
        .filter(|c| c.live != Path::new(crate::project::IGNORE_FILE))
        .count()
    {
        0 => Ok(RootStatus::Synced),
        n => Ok(RootStatus::Conflicts(n)),
    }
}

/// Conflicts whose live path is still in the include-list, plus the ignore
/// file's. An untracked path keeps its staged blobs, siblings included, so
/// they are left out rather than counted forever.
fn open_conflicts(repo: &Repo) -> Result<Vec<conflict::Conflict>> {
    let entries = repo.project_file()?.tracked();
    Ok(conflict::list(&repo.git)?
        .into_iter()
        .filter(|c| {
            c.live == Path::new(crate::project::IGNORE_FILE) || entries.contains_rel(&c.live)
        })
        .collect())
}

/// Local-only record that a remote commit's content is already in `main`.
/// Keyed by the exact commit, so it lapses the moment that device moves on.
fn settled_ref(provider_key: &str, device_id: &str) -> String {
    format!("refs/dotlore/settled/{provider_key}/{device_id}")
}

/// Is this device's head something `main` does not already contain?
///
/// Two devices can mint equivalent merge commits — same parents, identical
/// trees. The convergence rule keeps the smaller hash, but the discarded one
/// stays published forever and this device's remote ref keeps pointing at it.
/// While the trees match, `merge_remote` answers `AlreadyMerged` and nothing
/// happens; one local commit later the trees differ and that stale head turns
/// into a *real* merge against a base older than everything both sides already
/// resolved, which resurrects deleted conflict siblings. So the equivalence is
/// recorded here, once, by exact commit id.
///
/// Judged against durable `main` only, and before any merge runs: whatever the
/// transaction goes on to do, every future `main` descends from this one, so a
/// settled head stays settled. A head whose hash sorts *below* ours is still
/// merged — that is the adoption case, and it is what keeps both devices on
/// one commit.
///
/// Where the guarantee ends: the marker suppresses that exact commit, here.
/// A third device that merged the discarded commit into a non-identical head
/// before the equivalence was settled still carries it as a real ancestor, so
/// merging *that* device later reaches the old fork point again. Deterministic
/// and non-looping — at worst a resurrected sibling, never data loss.
fn needs_merge(repo: &Repo, key: &str, device: &str) -> Result<bool> {
    let r = remote_ref(key, device);
    let Some(rev) = repo.git.rev(&r) else {
        return Ok(false);
    };
    if repo.git.is_ancestor(&r, "refs/heads/main") {
        return Ok(false);
    }
    if repo.git.rev(&settled_ref(key, device)).as_deref() == Some(rev.as_str()) {
        return Ok(false);
    }
    let Some(main) = repo.git.rev("refs/heads/main") else {
        return Ok(true);
    };
    let identical = repo
        .git
        .run(&["diff", "--quiet", "--end-of-options", &main, &r])?
        .status
        .success();
    if identical && rev > main {
        repo.git
            .ok(&["update-ref", &settled_ref(key, device), &rev])?;
        return Ok(false);
    }
    Ok(true)
}

/// True when `stored` is one of the catalog locations, whether or not that
/// directory still exists. Comparison uses the canonical form [`add_root`]
/// stores (`resolve_target`), including a missing final component.
fn is_catalog_agent(home_dir: &Path, stored: &Path) -> bool {
    project::agent_catalog(home_dir)
        .iter()
        .any(|candidate| same_agent_path(candidate, stored))
}

/// Path equality after the canonicalization [`add_root`] applies.
///
/// The final component is not followed, so a removed directory and a
/// symlink left in its place still match the path that was stored.
fn same_agent_path(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (catalog_stored_path(a), catalog_stored_path(b)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

fn catalog_stored_path(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?;
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty())?;
    Some(parent.canonicalize().ok()?.join(name))
}

fn root_present(path: &Path) -> bool {
    match fs::symlink_metadata(path) {
        Ok(md) => md.is_dir(),
        Err(_) => false,
    }
}

/// `[a-z0-9]+(-[a-z0-9]+)*`: rejected rather than silently sanitized, and the
/// shape that makes a slug safe as a single cloud path component.
fn check_slug(s: &str) -> Result<()> {
    let ok = !s.is_empty()
        && !s.starts_with('-')
        && !s.ends_with('-')
        && !s.contains("--")
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if ok {
        Ok(())
    } else {
        bail!("invalid slug {s:?}: expected [a-z0-9]+(-[a-z0-9]+)*");
    }
}

/// An absolute, symlink-free path for a tracked root.
///
/// An existing path is canonicalized. A Link target that does not exist yet
/// keeps its final component and canonicalizes the parent, which must exist.
fn resolve_target(path: &Path) -> Result<PathBuf> {
    if path.as_os_str().is_empty() {
        bail!("empty path");
    }
    match fs::symlink_metadata(path) {
        Ok(md) if md.file_type().is_symlink() => {
            bail!("{} is a symlink", path.display())
        }
        Ok(_) => path
            .canonicalize()
            .with_context(|| format!("resolving {}", path.display())),
        Err(_) => {
            let name = path
                .file_name()
                .ok_or_else(|| anyhow!("{} has no final path component", path.display()))?;
            let parent = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .ok_or_else(|| anyhow!("{} has no parent directory", path.display()))?;
            if !parent.is_dir() {
                bail!("{} does not exist", parent.display());
            }
            Ok(parent
                .canonicalize()
                .with_context(|| format!("resolving {}", parent.display()))?
                .join(name))
        }
    }
}

/// Staging-private names: never listed as tracked files.
fn is_internal(path: &str) -> bool {
    Path::new(path).components().any(|c| match c {
        Component::Normal(n) => crate::project::staging_private(&n.to_string_lossy()),
        _ => true,
    })
}

fn canonical(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

/// Equal, or one inside the other.
fn related(a: &Path, b: &Path) -> bool {
    a == b || a.starts_with(b) || b.starts_with(a)
}

fn device_name(names: &std::collections::HashMap<String, String>, id8: &str) -> String {
    names
        .iter()
        .find(|(id, _)| id.starts_with(id8))
        .map(|(_, n)| n.clone())
        .unwrap_or_else(|| id8.to_string())
}

/// The real file a staging entry maps to.
fn live_root_path(repo: &Repo, rel: &Path) -> Result<PathBuf> {
    if !plain_rel(rel) {
        bail!("unsafe path {}", rel.display());
    }
    Ok(repo.root.join(rel))
}

fn root_state(path: &Path) -> Result<Option<FileState>> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        Ok(md) if md.is_file() => Ok(Some(FileState {
            bytes: fs::read(path)?,
            executable: md.permissions().mode() & 0o111 != 0,
        })),
        Ok(_) => bail!("{} is not a regular file", path.display()),
    }
}

/// `(blob id, executable)` for one tracked path at a commit.
fn tree_entry(git: &Git, rev: &str, rel: &Path) -> Result<(String, bool)> {
    let spec = rel
        .to_str()
        .filter(|_| plain_rel(rel))
        .ok_or_else(|| anyhow!("unsafe path {}", rel.display()))?;
    let out = git.ok(&[
        "ls-tree",
        "--end-of-options",
        rev,
        "--",
        &format!(":(literal){spec}"),
    ])?;
    // "<mode> <type> <oid>\t<path>"
    let meta = out
        .lines()
        .next()
        .and_then(|l| l.split('\t').next())
        .ok_or_else(|| anyhow!("{} is not tracked at {rev}", rel.display()))?;
    let mut it = meta.split_whitespace();
    let (Some(mode), Some(_ty), Some(oid)) = (it.next(), it.next(), it.next()) else {
        bail!("unparseable ls-tree entry for {}", rel.display());
    };
    if !mode.starts_with("100") {
        bail!("{} is not a regular file at {rev}", rel.display());
    }
    Ok((oid.to_string(), mode == "100755"))
}

/// Raw blob bytes. `Git::ok` trims and lossily decodes, which would corrupt
/// any file we are about to show or write.
fn blob_bytes(git: &Git, oid: &str) -> Result<Vec<u8>> {
    let out = git.run(&["cat-file", "blob", oid])?;
    if !out.status.success() {
        bail!(
            "git cat-file blob {oid} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(out.stdout)
}

fn plain_rel(rel: &Path) -> bool {
    !rel.as_os_str().is_empty() && rel.components().all(|c| matches!(c, Component::Normal(_)))
}

fn trim_rel(rel: &Path) -> PathBuf {
    PathBuf::from(rel.as_os_str().to_string_lossy().trim_end_matches('/'))
}

/// Safe, ignore-aware preview of a live path. Over-limit files are excluded
/// from `bytes` and listed in `skipped_too_large`.
fn inspect_live(
    root: &Path,
    rel: &Path,
    ignore_text: &str,
    limits: Limits,
) -> Result<InspectedEntry> {
    if !plain_rel(rel) {
        bail!("unsafe path {}", rel.display());
    }
    let trimmed = trim_rel(rel);
    if trimmed.as_os_str().is_empty() {
        bail!("unsafe path {}", rel.display());
    }
    match mirror::inspect(root, &trimmed)? {
        Node::Missing => bail!("{} is missing", root.join(&trimmed).display()),
        Node::Opaque => bail!(
            "{} is a symlink or not a regular file or directory",
            root.join(&trimmed).display()
        ),
        Node::File { len } => {
            let over = len > limits.max_file_bytes;
            let measured = mirror::measure_tree(root, &trimmed, ignore_text, limits)?;
            Ok(InspectedEntry {
                kind: EntryKind::File,
                bytes: if over { 0 } else { measured.bytes },
                folder_limit: limits.max_seed_folder_bytes,
                confirmation_required: false,
                skipped_too_large: if over {
                    vec![(trimmed, len)]
                } else {
                    measured.skipped_too_large
                },
            })
        }
        Node::Dir => {
            let measured = mirror::measure_tree(root, &trimmed, ignore_text, limits)?;
            Ok(InspectedEntry {
                kind: EntryKind::Directory,
                bytes: measured.bytes,
                folder_limit: limits.max_seed_folder_bytes,
                confirmation_required: measured.bytes > limits.max_seed_folder_bytes,
                skipped_too_large: measured.skipped_too_large,
            })
        }
    }
}

/// Every version the resolver showed, still exactly as it was shown. Bytes are
/// derived from the blob ids, so comparing ids is comparing content.
fn same_versions(a: &ResolutionSnapshot, b: &ResolutionSnapshot) -> bool {
    a.head == b.head
        && a.live == b.live
        && a.live_blob == b.live_blob
        && a.live_executable == b.live_executable
        && a.root == b.root
        && a.siblings.len() == b.siblings.len()
        && a.siblings
            .iter()
            .zip(&b.siblings)
            .all(|(x, y)| x.path == y.path && x.blob == y.blob)
}

/// Recursive copy, never following a symlink.
fn copy_tree(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src).with_context(|| format!("reading {}", src.display()))? {
        let entry = entry?;
        let md = fs::symlink_metadata(entry.path())?;
        let to = dst.join(entry.file_name());
        if md.file_type().is_symlink() {
            continue;
        }
        if md.is_dir() {
            copy_tree(&entry.path(), &to)?;
        } else if md.is_file() {
            fs::copy(entry.path(), &to)
                .with_context(|| format!("copying {}", entry.path().display()))?;
        }
    }
    Ok(())
}

/// Whether `path` is a real directory [`Engine::wipe_cloud_data`] may
/// delete. Missing is `false`; a symlink or any other entry is refused.
fn removable_dir(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(md) if md.file_type().is_symlink() => {
            bail!("{} is a symlink; refusing to delete it", path.display())
        }
        Ok(md) if md.is_dir() => Ok(true),
        Ok(_) => bail!(
            "{} is not a directory; refusing to delete it",
            path.display()
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Byte-compare a backup against its source before anything is rebuilt: a
/// backup nobody checked is not a backup.
fn verify_copy(src: &Path, dst: &Path) -> Result<()> {
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let md = fs::symlink_metadata(entry.path())?;
        let to = dst.join(entry.file_name());
        if md.file_type().is_symlink() {
            continue;
        }
        if md.is_dir() {
            verify_copy(&entry.path(), &to)?;
        } else if md.is_file() && fs::read(entry.path())? != fs::read(&to)? {
            bail!("backup of {} differs from the original", to.display());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;
    use tempfile::TempDir;

    struct Dev {
        home: TempDir,
        root: TempDir,
        engine: Engine,
    }

    /// A device with its own state home and root, sharing `provider`.
    fn device(provider: &Path, letter: char) -> Dev {
        let home = TempDir::new().unwrap();
        let root = TempDir::new().unwrap();
        // `default_patterns` is hooked here so seeding tests can override it
        // on the hand-built Config before `cfg.save`.
        let cfg = Config {
            device_id: letter.to_string().repeat(32),
            device_name: format!("Mac {letter} Pro"),
            provider_dir: Some(provider.to_path_buf()),
            roots: Vec::new(),
            default_patterns: None,
            default_ignore: None,
            ..Default::default()
        };
        cfg.save(home.path()).unwrap();
        let engine = Engine::new(home.path(), home.path(), cfg).unwrap();
        Dev { home, root, engine }
    }

    fn write(dev: &Dev, rel: &str, body: &[u8]) {
        let p = dev.root.path().join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }

    fn add(dev: &mut Dev) -> String {
        let path = dev.root.path().to_path_buf();
        dev.engine
            .add_root(&path, Some("proj-claude"))
            .unwrap()
            .slug
    }

    fn cloud_of(dev: &Dev) -> Cloud {
        Cloud {
            base: dev.engine.cloud.base.clone(),
        }
    }

    fn staging(dev: &Dev) -> PathBuf {
        dev.home.path().join("repos/proj-claude")
    }

    fn rels(files: &[TrackedFile]) -> Vec<String> {
        files.iter().map(|f| f.rel.clone()).collect()
    }

    #[test]
    fn root_status_serializes_with_a_uniform_shape() {
        assert_eq!(
            serde_json::to_string(&RootStatus::Synced).unwrap(),
            r#"{"kind":"Synced"}"#
        );
        assert_eq!(
            serde_json::to_string(&RootStatus::Conflicts(2)).unwrap(),
            r#"{"kind":"Conflicts","detail":2}"#
        );
        assert_eq!(
            serde_json::to_string(&RootStatus::Error("boom".into())).unwrap(),
            r#"{"kind":"Error","detail":"boom"}"#
        );
    }

    #[test]
    fn a_second_engine_cannot_take_a_slug_while_it_is_seeding() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        let seed = match a
            .engine
            .begin_add_root(a.root.path(), Some("proj-claude"))
            .unwrap()
        {
            AddRootStart::Seed(seed) => seed,
            AddRootStart::Done(_) => panic!("expected a new root to seed"),
        };

        let cfg = Config::load(a.home.path()).unwrap();
        let mut other = Engine::new(a.home.path(), a.home.path(), cfg).unwrap();
        let err = match other.begin_add_root(a.root.path(), Some("proj-claude")) {
            Ok(_) => panic!("a second add took a slug that is still seeding"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("already being added"), "{err:#}");
        assert!(
            a.home.path().join("repos/proj-claude/.git").is_dir(),
            "the in-progress staging repo must stay put"
        );
        drop(seed);
    }

    #[test]
    fn add_root_registers_publishes_and_is_quiescent() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");

        assert_eq!(add(&mut a), "proj-claude");
        let cfg = Config::load(a.home.path()).unwrap();
        assert_eq!(cfg.roots.len(), 1);
        assert!(!cfg.roots[0].initializing);
        assert!(staging(&a).join(".dotloreignore").is_file());
        assert!(!a.root.path().join(".dotloreignore").exists());

        let cloud = cloud_of(&a);
        let manifest = cloud.read_manifest("proj-claude").unwrap();
        assert_eq!(
            manifest.display_name,
            a.root
                .path()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .as_ref()
        );
        assert!(!manifest.is_agent);
        assert_eq!(cloud.list_bundles("proj-claude").len(), 1);

        assert_eq!(
            a.engine.sync_root("proj-claude").unwrap(),
            RootStatus::Synced
        );
        assert_eq!(cloud.list_bundles("proj-claude").len(), 1);
    }

    #[test]
    fn tracked_files_lists_nested_paths_without_internals() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        write(&a, "docs/a.md", b"a\n");
        write(&a, "docs/nested/b.md", b"b\n");
        add(&mut a);
        a.engine.sync_root("proj-claude").unwrap();

        let files = a.engine.tracked_files("proj-claude").unwrap();
        assert_eq!(rels(&files), ["CLAUDE.md", "docs/a.md", "docs/nested/b.md"]);
        assert!(!files.iter().any(|f| f.rel.contains(".dotloreignore")));
        assert!(!files.iter().any(|f| f.rel.contains(".dotloreproject")));
        assert!(files.iter().all(|f| f.state == FileSync::Synced));
    }

    #[test]
    fn add_root_seeds_the_include_list_from_the_default_patterns() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        a.engine.cfg.default_patterns = Some(vec!["CLAUDE.md".into(), "docs/".into()]);
        a.engine.cfg.save(a.home.path()).unwrap();

        write(&a, "CLAUDE.md", b"one\n");
        write(&a, "docs/a.md", b"a\n");
        write(&a, "README.md", b"nope\n");
        add(&mut a);

        let file = crate::project::read(&staging(&a)).unwrap();
        let tracked = file.tracked();
        assert!(tracked.is_explicit(Path::new("CLAUDE.md")));
        assert!(tracked.is_explicit(Path::new("docs")));
        assert!(!tracked.contains_rel(Path::new("README.md")));

        let files = a.engine.tracked_files("proj-claude").unwrap();
        assert_eq!(rels(&files), ["CLAUDE.md", "docs/a.md"]);
        assert!(!files.iter().any(|f| f.rel == "README.md"));
    }

    #[test]
    fn add_root_writes_the_default_never_list_into_dotloreignore() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        add(&mut a);
        let got = fs::read_to_string(staging(&a).join(".dotloreignore")).unwrap();
        assert_eq!(got, crate::project::DEFAULT_NEVER_IGNORE);
    }

    fn builtin_vec(catalog: &str) -> Vec<String> {
        lines_owned(crate::project::builtin_lines(catalog).unwrap())
    }

    #[test]
    fn set_pattern_catalog_equal_to_builtin_clears_the_override() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        a.engine
            .set_pattern_catalog("projects", vec!["CLAUDE.md".into()])
            .unwrap();
        a.engine
            .set_pattern_catalog("claude", vec!["custom.md".into()])
            .unwrap();
        let cfg = Config::load(a.home.path()).unwrap();
        assert_eq!(cfg.default_patterns, Some(vec!["CLAUDE.md".into()]));
        assert_eq!(
            cfg.agent_patterns.get("claude").cloned(),
            Some(vec!["custom.md".into()])
        );

        a.engine
            .set_default_patterns(builtin_vec("projects"))
            .unwrap();
        a.engine
            .set_pattern_catalog("claude", builtin_vec("claude"))
            .unwrap();
        let cfg = Config::load(a.home.path()).unwrap();
        assert_eq!(cfg.default_patterns, None);
        assert!(!cfg.agent_patterns.contains_key("claude"));
    }

    #[test]
    fn pattern_catalogs_returns_a_claude_override() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        let custom = vec!["custom.md".into(), "rules/".into()];
        a.engine
            .set_pattern_catalog("claude", custom.clone())
            .unwrap();

        let catalogs = a.engine.pattern_catalogs();
        let ids: Vec<&str> = catalogs.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids.first().copied(), Some("projects"));
        assert_eq!(ids.last().copied(), Some("other"));
        assert!(
            ids.iter().position(|id| *id == "claude") < ids.iter().position(|id| *id == "codex")
        );

        let claude = catalogs.iter().find(|c| c.id == "claude").unwrap();
        assert_eq!(claude.label, "Claude");
        assert_eq!(claude.lines, custom);
        let projects = catalogs.iter().find(|c| c.id == "projects").unwrap();
        assert_eq!(projects.lines, builtin_vec("projects"));
    }

    #[test]
    fn set_pattern_catalog_rejects_an_unknown_id_without_writing() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        a.engine
            .set_default_patterns(vec!["CLAUDE.md".into()])
            .unwrap();
        let before = fs::read(a.home.path().join("config.json")).unwrap();
        let err = a
            .engine
            .set_pattern_catalog("nope", vec!["x".into()])
            .unwrap_err();
        assert!(err.to_string().contains("unknown pattern catalog"));
        let after = fs::read(a.home.path().join("config.json")).unwrap();
        assert_eq!(before, after);
        let cfg = Config::load(a.home.path()).unwrap();
        assert_eq!(cfg.default_patterns, Some(vec!["CLAUDE.md".into()]));
        assert!(cfg.agent_patterns.is_empty());
    }

    #[test]
    fn set_default_ignore_of_the_builtin_text_stores_none() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        a.engine.set_default_ignore("*.secret\n".into()).unwrap();
        assert_eq!(
            Config::load(a.home.path())
                .unwrap()
                .default_ignore
                .as_deref(),
            Some("*.secret\n")
        );
        a.engine
            .set_default_ignore(crate::project::DEFAULT_NEVER_IGNORE.to_string())
            .unwrap();
        assert_eq!(Config::load(a.home.path()).unwrap().default_ignore, None);
    }

    #[test]
    fn add_root_of_an_agent_folder_uses_the_claude_override() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        let user_home = TempDir::new().unwrap();
        a.engine.home_dir = user_home.path().canonicalize().unwrap();
        let claude = a.engine.home_dir.join(".claude");
        fs::create_dir_all(&claude).unwrap();
        fs::write(claude.join("custom.md"), "hi\n").unwrap();
        fs::write(claude.join("settings.json"), "{}\n").unwrap();
        a.engine
            .set_pattern_catalog("claude", vec!["custom.md".into()])
            .unwrap();

        a.engine.add_root(&claude, Some("claude-agent")).unwrap();

        let staging = a.home.path().join("repos/claude-agent");
        let file = crate::project::read(&staging).unwrap();
        let tracked = file.tracked();
        assert!(tracked.is_explicit(Path::new("custom.md")));
        assert!(!tracked.is_explicit(Path::new("settings.json")));
        assert!(cloud_of(&a).read_manifest("claude-agent").unwrap().is_agent);
    }

    #[test]
    fn set_pattern_catalog_after_add_root_leaves_the_include_list_unchanged() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        a.engine
            .set_pattern_catalog("projects", vec!["CLAUDE.md".into(), "docs/".into()])
            .unwrap();
        write(&a, "CLAUDE.md", b"one\n");
        write(&a, "docs/a.md", b"a\n");
        write(&a, "README.md", b"nope\n");
        add(&mut a);

        let before = fs::read(staging(&a).join(".dotloreproject")).unwrap();
        a.engine
            .set_pattern_catalog("projects", vec!["README.md".into()])
            .unwrap();
        let after = fs::read(staging(&a).join(".dotloreproject")).unwrap();
        assert_eq!(before, after);

        let file = crate::project::read(&staging(&a)).unwrap();
        let tracked = file.tracked();
        assert!(tracked.is_explicit(Path::new("CLAUDE.md")));
        assert!(tracked.is_explicit(Path::new("docs")));
        assert!(!tracked.contains_rel(Path::new("README.md")));
    }

    #[test]
    fn tracked_files_lists_only_include_list_entries() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        write(&a, "docs/a.md", b"a\n");
        write(&a, "README.md", b"nope\n");
        add(&mut a);
        a.engine.sync_root("proj-claude").unwrap();

        let files = a.engine.tracked_files("proj-claude").unwrap();
        assert_eq!(rels(&files), ["CLAUDE.md", "docs/a.md"]);
        assert!(!files.iter().any(|f| f.rel == "README.md"));
    }

    #[test]
    fn tracked_files_reports_a_size_for_every_entry() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        write(&a, "docs/a.md", b"aaaa");
        add(&mut a);
        a.engine.sync_root("proj-claude").unwrap();

        assert_eq!(
            a.engine.tracked_files("proj-claude").unwrap(),
            vec![
                TrackedFile {
                    rel: "CLAUDE.md".into(),
                    bytes: 4,
                    state: FileSync::Synced,
                },
                TrackedFile {
                    rel: "docs/a.md".into(),
                    bytes: 4,
                    state: FileSync::Synced,
                },
            ]
        );
    }

    #[test]
    fn tracked_files_reports_a_staged_path_with_no_live_file_as_pending() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        add(&mut a);
        a.engine.sync_root("proj-claude").unwrap();
        fs::remove_file(a.root.path().join("CLAUDE.md")).unwrap();

        assert_eq!(
            a.engine.tracked_files("proj-claude").unwrap(),
            vec![TrackedFile {
                rel: "CLAUDE.md".into(),
                bytes: 0,
                state: FileSync::Pending,
            }]
        );
    }

    #[test]
    fn tracked_files_on_a_missing_root_lists_staged_paths_as_pending() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        write(&a, "docs/a.md", b"a\n");
        add(&mut a);
        a.engine.sync_root("proj-claude").unwrap();
        fs::remove_dir_all(a.root.path()).unwrap();

        let files = a.engine.tracked_files("proj-claude").unwrap();
        assert_eq!(
            files,
            vec![
                TrackedFile {
                    rel: "CLAUDE.md".into(),
                    bytes: 0,
                    state: FileSync::Pending,
                },
                TrackedFile {
                    rel: "docs/a.md".into(),
                    bytes: 0,
                    state: FileSync::Pending,
                },
            ]
        );
    }

    #[test]
    fn tracked_files_reports_an_over_limit_file_as_too_large() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        write(&a, "docs/a.md", b"a\n");
        add(&mut a);
        a.engine.set_max_file_mb(1).unwrap();
        write(&a, "docs/huge.bin", &vec![b'x'; 1024 * 1024 + 1]);

        let files = a.engine.tracked_files("proj-claude").unwrap();
        let huge = files
            .iter()
            .find(|f| f.rel == "docs/huge.bin")
            .expect("over-limit file must be listed");
        assert_eq!(huge.bytes, 1024 * 1024 + 1);
        assert_eq!(huge.state, FileSync::TooLarge);

        let repo = a
            .engine
            .repo_for(&a.engine.root_cfg("proj-claude").unwrap())
            .unwrap();
        let staged = repo.git.ok(&["ls-files"]).unwrap();
        assert!(
            !staged.lines().any(|l| l == "docs/huge.bin"),
            "too-large file must not be in git ls-files:\n{staged}"
        );
    }

    #[test]
    fn track_entry_publishes_the_entry() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        add(&mut a);
        write(&a, "notes.md", b"notes\n");

        let status = a
            .engine
            .track_entry("proj-claude", Path::new("notes.md"), None)
            .unwrap();
        assert_eq!(status, TrackOutcome::Done(RootStatus::Synced));
        assert_eq!(
            rels(&a.engine.tracked_files("proj-claude").unwrap()),
            ["CLAUDE.md", "notes.md"]
        );
        assert_eq!(cloud_of(&a).list_bundles("proj-claude").len(), 2);

        let listed = a.engine.list_entries("proj-claude").unwrap();
        assert!(listed
            .iter()
            .any(|e| e.key == "notes.md" && e.kind == EntryKind::File));
        assert!(listed.iter().any(|e| e.key == "CLAUDE.md"));
    }

    #[test]
    fn untrack_entry_does_not_commit_a_deletion() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        a.engine.cfg.default_patterns = Some(vec!["CLAUDE.md".into(), "notes.md".into()]);
        a.engine.cfg.save(a.home.path()).unwrap();
        write(&a, "CLAUDE.md", b"one\n");
        write(&a, "notes.md", b"keep me\n");
        add(&mut a);

        let repo = a
            .engine
            .repo_for(&a.engine.root_cfg("proj-claude").unwrap())
            .unwrap();
        let before = repo.git.rev("refs/heads/main").unwrap();
        let blob_before = repo.git.ok(&["rev-parse", "HEAD:notes.md"]).unwrap();

        assert_eq!(
            a.engine
                .untrack_entry("proj-claude", Path::new("notes.md"))
                .unwrap(),
            RootStatus::Synced
        );

        let after = repo.git.rev("refs/heads/main").unwrap();
        assert_ne!(before, after, "untrack must commit the tombstone");
        let log = repo.git.ok(&["log", "--format=%s", "-1"]).unwrap();
        assert!(
            log.starts_with("local aaaaaaaa"),
            "a local commit containing the manifest update is required: {log}"
        );

        let file = crate::project::read(&staging(&a)).unwrap();
        assert_eq!(
            file.entries["notes.md"].state,
            crate::project::State::Removed
        );
        let blob_after = repo.git.ok(&["rev-parse", "HEAD:notes.md"]).unwrap();
        assert_eq!(blob_before, blob_after, "content blob must be unchanged");

        let diff = repo
            .git
            .ok(&["diff", "--name-status", &before, &after])
            .unwrap();
        assert!(
            !diff
                .lines()
                .any(|l| l.starts_with('D') && l.contains("notes.md")),
            "untrack committed a deletion:\n{diff}"
        );
        assert!(
            a.root.path().join("notes.md").is_file(),
            "live file must stay"
        );
        assert_eq!(
            rels(&a.engine.tracked_files("proj-claude").unwrap()),
            ["CLAUDE.md"]
        );
        assert_eq!(cloud_of(&a).list_bundles("proj-claude").len(), 2);
    }

    #[test]
    fn untrack_entry_punches_a_hole_in_an_inherited_only_path() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "docs/a.md", b"a\n");
        write(&a, "docs/b.md", b"b\n");
        add(&mut a);

        assert_eq!(
            a.engine
                .untrack_entry("proj-claude", Path::new("docs/a.md"))
                .unwrap(),
            RootStatus::Synced
        );
        let file = crate::project::read(&staging(&a)).unwrap();
        assert_eq!(file.entries["docs/"].state, crate::project::State::Tracked);
        assert_eq!(
            file.entries["docs/a.md"].state,
            crate::project::State::Removed
        );
        assert!(a.root.path().join("docs/a.md").is_file());
        let rels = rels(&a.engine.tracked_files("proj-claude").unwrap());
        assert!(
            !rels.iter().any(|r| r == "docs/a.md"),
            "inherited untrack must drop the path: {rels:?}"
        );
        assert!(rels.iter().any(|r| r == "docs/b.md"));
    }

    #[test]
    fn untrack_entry_punches_a_hole_in_an_inherited_directory() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "docs/secret/a.md", b"a\n");
        write(&a, "docs/keep.md", b"keep\n");
        add(&mut a);

        assert_eq!(
            a.engine
                .untrack_entry("proj-claude", Path::new("docs/secret"))
                .unwrap(),
            RootStatus::Synced
        );
        let file = crate::project::read(&staging(&a)).unwrap();
        assert_eq!(file.entries["docs/"].state, crate::project::State::Tracked);
        assert_eq!(
            file.entries["docs/secret/"].state,
            crate::project::State::Removed
        );
        assert!(
            !file.entries.contains_key("docs/secret"),
            "live directory must tombstone the dir key so children drop"
        );
        assert!(a.root.path().join("docs/secret/a.md").is_file());
        let rels = rels(&a.engine.tracked_files("proj-claude").unwrap());
        assert!(
            !rels.iter().any(|r| r == "docs/secret/a.md"),
            "inherited directory untrack must drop children: {rels:?}"
        );
        assert!(rels.iter().any(|r| r == "docs/keep.md"));
    }

    #[test]
    fn untrack_entry_reports_remaining_coverage_for_overlapping_entries() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "docs/readme.md", b"hi\n");
        add(&mut a);
        a.engine
            .track_entry("proj-claude", Path::new("docs/readme.md"), None)
            .unwrap();

        let before = a.engine.list_entries("proj-claude").unwrap();
        let child = before.iter().find(|e| e.key == "docs/readme.md").unwrap();
        assert!(
            child.covering.iter().any(|c| c == "docs/"),
            "child must report the parent cover: {child:?}"
        );

        a.engine
            .untrack_entry("proj-claude", Path::new("docs/readme.md"))
            .unwrap();
        let after = a.engine.list_entries("proj-claude").unwrap();
        assert!(!after.iter().any(|e| e.key == "docs/readme.md"));
        let parent = after.iter().find(|e| e.key == "docs/").unwrap();
        assert_eq!(parent.kind, EntryKind::Directory);
        assert!(
            !rels(&a.engine.tracked_files("proj-claude").unwrap())
                .contains(&"docs/readme.md".into()),
            "untracking the child must punch a hole in the parent"
        );

        let mut b = device(provider.path(), 'b');
        let b_root = b.root.path().to_path_buf();
        b.engine.link_root("proj-claude", &b_root).unwrap();
        let b_entries = b.engine.list_entries("proj-claude").unwrap();
        assert!(b_entries.iter().any(|e| e.key == "docs/"));
        assert!(!b_entries.iter().any(|e| e.key == "docs/readme.md"));
        assert!(
            !rels(&b.engine.tracked_files("proj-claude").unwrap())
                .contains(&"docs/readme.md".into()),
            "the hole must sync to a newly linked peer"
        );
    }

    /// Two devices adding the same slug must not mint unrelated histories:
    /// the second Add is a Link. Run on a thread because taking the home lock
    /// twice in one process deadlocks — which is exactly the bug a delegation
    /// to the *public* `link_root` would introduce.
    #[test]
    fn add_root_becomes_link_when_the_slug_already_exists() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        add(&mut a);

        let b = device(provider.path(), 'b');
        let b_root = b.root.path().to_path_buf();
        let mut engine = b.engine;
        let (tx, rx) = mpsc::channel();
        let h = thread::spawn(move || {
            let r = engine.add_root(&b_root, Some("proj-claude"));
            tx.send(()).unwrap();
            (engine, r)
        });
        rx.recv_timeout(Duration::from_secs(30))
            .expect("add_root must not take the home lock twice");
        let (engine, slug) = h.join().unwrap();
        assert_eq!(slug.unwrap(), "proj-claude");

        assert_eq!(fs::read(b.root.path().join("CLAUDE.md")).unwrap(), b"one\n");
        assert_eq!(engine.cfg.roots.len(), 1);
        // The creating device's manifest is the only one.
        let manifest = cloud_of(&a).read_manifest("proj-claude").unwrap();
        assert_eq!(
            manifest.display_name,
            a.root
                .path()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .as_ref()
        );
        assert!(!manifest.is_agent);
    }

    #[test]
    fn an_unreadable_manifest_is_never_evidence_that_a_slug_is_free() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        let dir = provider.path().join("dotlore/proj-claude");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("manifest.json"), b"{ truncated").unwrap();
        write(&a, "CLAUDE.md", b"one\n");

        let path = a.root.path().to_path_buf();
        let err = a
            .engine
            .add_root(&path, Some("proj-claude"))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("unreadable") || err.contains("could not be read"),
            "{err}"
        );
        assert!(Config::load(a.home.path()).unwrap().roots.is_empty());
        assert!(!staging(&a).exists());
    }

    /// The ordering the whole "never lose the user's bytes" rule rests on:
    /// resume first and a journalled apply recreates a root the user deleted.
    #[test]
    fn a_missing_root_is_reported_before_a_journalled_apply_is_resumed() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        add(&mut a);

        // Leave an `Applying` journal with one unapplied addition.
        let root = a.engine.root_cfg("proj-claude").unwrap();
        let repo = a.engine.repo_for(&root).unwrap();
        let key = provider_key(&cloud_of(&a));
        let mut tx = repo
            .begin_tx(&key, "HEAD", conflict::resolve_index)
            .unwrap();
        fs::write(tx.worktree.join("new.md"), b"added\n").unwrap();
        tx.git.ok(&["add", "-A"]).unwrap();
        tx.git.ok(&["commit", "-m", "remote"]).unwrap();
        tx.set_target(&repo).unwrap();
        drop(tx);

        let path = a.root.path().to_path_buf();
        fs::remove_dir_all(&path).unwrap();
        assert_eq!(
            a.engine.sync_root("proj-claude").unwrap(),
            RootStatus::RootMissing
        );
        assert!(!path.exists(), "the deleted root was recreated");
    }

    /// A Google account removed and added again brings the provider folder
    /// back at the same path without our bundles: `sent` must not be trusted.
    #[test]
    fn a_provider_folder_that_lost_our_bundles_is_republished_whole() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        add(&mut a);
        let cloud = cloud_of(&a);
        let a_id = "a".repeat(32);
        let before = cloud.max_seq("proj-claude", &a_id);

        fs::remove_dir_all(&cloud.base).unwrap();
        assert_eq!(
            a.engine.sync_root("proj-claude").unwrap(),
            RootStatus::Synced
        );
        assert!(cloud.read_manifest("proj-claude").is_some());
        assert!(cloud.has_bundles("proj-claude", &a_id).unwrap());

        // Numbering carries on from local state, never reusing a seq.
        let seqs: Vec<u64> = cloud
            .list_bundles("proj-claude")
            .iter()
            .map(|b| b.seq)
            .collect();
        assert!(seqs.iter().all(|s| *s > before), "{seqs:?}");

        let mut b = device(provider.path(), 'b');
        let b_root = b.root.path().to_path_buf();
        b.engine.link_root("proj-claude", &b_root).unwrap();
        assert_eq!(fs::read(b_root.join("CLAUDE.md")).unwrap(), b"one\n");
    }

    #[test]
    fn an_evicted_own_bundle_is_not_republished() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        add(&mut a);
        let dir = cloud_of(&a)
            .slug_dir("proj-claude")
            .unwrap()
            .join("devices")
            .join("a".repeat(32));
        fs::rename(dir.join("000001.bundle"), dir.join(".000001.bundle.icloud")).unwrap();

        assert_eq!(
            a.engine.sync_root("proj-claude").unwrap(),
            RootStatus::Synced
        );
        assert!(cloud_of(&a).list_bundles("proj-claude").is_empty());
    }

    /// A listing that fails is not a listing that came back empty: the
    /// cloud is immutable, so a spurious full republish could never be undone.
    #[test]
    fn an_unreadable_own_device_folder_is_an_error_not_a_republish() {
        use std::os::unix::fs::PermissionsExt;
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        add(&mut a);
        let dir = cloud_of(&a)
            .slug_dir("proj-claude")
            .unwrap()
            .join("devices")
            .join("a".repeat(32));

        fs::set_permissions(&dir, fs::Permissions::from_mode(0o000)).unwrap();
        let status = a.engine.sync_root("proj-claude").unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(matches!(status, RootStatus::Error(_)), "{status:?}");
        let seqs: Vec<u64> = cloud_of(&a)
            .list_bundles("proj-claude")
            .iter()
            .map(|b| b.seq)
            .collect();
        assert_eq!(seqs, vec![1]);
    }

    #[test]
    fn a_missing_provider_folder_is_an_error_and_is_not_recreated() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        add(&mut a);
        let provider_dir = cloud_of(&a).base.parent().unwrap().to_path_buf();

        fs::remove_dir_all(&provider_dir).unwrap();
        write(&a, "CLAUDE.md", b"two\n");
        match a.engine.sync_root("proj-claude").unwrap() {
            RootStatus::Error(e) => assert!(e.contains("is missing"), "{e}"),
            other => panic!("expected an error, got {other:?}"),
        }
        assert!(!provider_dir.exists());
    }

    #[test]
    fn link_is_pending_until_a_bundle_is_readable() {
        let a_provider = TempDir::new().unwrap();
        let b_provider = TempDir::new().unwrap();
        let mut a = device(a_provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        add(&mut a);

        // Only the manifest has propagated so far.
        let dir = b_provider.path().join("dotlore/proj-claude");
        fs::create_dir_all(&dir).unwrap();
        fs::copy(
            a_provider.path().join("dotlore/proj-claude/manifest.json"),
            dir.join("manifest.json"),
        )
        .unwrap();

        let mut b = device(b_provider.path(), 'b');
        let b_root = b.root.path().to_path_buf();
        assert_eq!(
            b.engine.link_root("proj-claude", &b_root).unwrap(),
            RootStatus::Pending
        );
        assert!(fs::read_dir(&b_root).unwrap().next().is_none());
        let cfg = Config::load(b.home.path()).unwrap();
        assert!(
            cfg.roots[0].initializing,
            "a pending link stays initializing"
        );
    }

    /// I4: an Engine holding a config from before another one added a root
    /// must not write that root back out of existence.
    #[test]
    fn a_second_engine_does_not_overwrite_the_first_engines_roots() {
        let provider = TempDir::new().unwrap();
        let mut one = device(provider.path(), 'a');
        let other_root = TempDir::new().unwrap();
        let cfg = Config::load(one.home.path()).unwrap();
        let mut two = Engine::new(one.home.path(), one.home.path(), cfg).unwrap();

        write(&one, "CLAUDE.md", b"one\n");
        let p = one.root.path().to_path_buf();
        one.engine.add_root(&p, Some("one-claude")).unwrap();

        fs::write(other_root.path().join("CLAUDE.md"), b"two\n").unwrap();
        two.add_root(other_root.path(), Some("two-claude")).unwrap();

        let saved = Config::load(one.home.path()).unwrap();
        let mut slugs: Vec<&str> = saved.roots.iter().map(|r| r.slug.as_str()).collect();
        slugs.sort();
        assert_eq!(slugs, vec!["one-claude", "two-claude"]);
        assert_eq!(two.sync_all().unwrap().len(), 2);
    }

    #[test]
    fn remove_root_keeps_the_staging_repo() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        add(&mut a);

        a.engine.remove_root("proj-claude").unwrap();
        assert!(Config::load(a.home.path()).unwrap().roots.is_empty());
        assert!(staging(&a).join(".git").is_dir());
        assert!(a.engine.remove_root("proj-claude").is_err());
    }

    /// `remove_root` keeps the staging repo, so a later Link to the same slug
    /// would otherwise mirror the new (empty) root over the old `main` and
    /// commit — and publish — a deletion of everything.
    #[test]
    fn relinking_a_removed_slug_does_not_commit_a_mass_deletion() {
        let a_provider = TempDir::new().unwrap();
        let b_provider = TempDir::new().unwrap();
        let mut a = device(a_provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        write(&a, "agents/x.md", b"x\n");
        add(&mut a);

        let mut b = device(b_provider.path(), 'b');
        copy_tree(
            &a_provider.path().join("dotlore"),
            &b_provider.path().join("dotlore"),
        )
        .unwrap();
        let b_root = b.root.path().to_path_buf();
        assert_eq!(
            b.engine.link_root("proj-claude", &b_root).unwrap(),
            RootStatus::Synced
        );
        b.engine.remove_root("proj-claude").unwrap();
        for e in fs::read_dir(&b_root).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                fs::remove_dir_all(p).unwrap()
            } else {
                fs::remove_file(p).unwrap()
            }
        }

        assert_eq!(
            b.engine.link_root("proj-claude", &b_root).unwrap(),
            RootStatus::Synced
        );
        assert_eq!(fs::read(b_root.join("CLAUDE.md")).unwrap(), b"one\n");
        let log = Repo::open(
            b.home.path(),
            "proj-claude",
            &b_root,
            "Mac b Pro",
            &"b".repeat(32),
        )
        .unwrap()
        .git
        .ok(&["log", "--format=%s"])
        .unwrap();
        assert!(
            !log.lines().any(|l| l.starts_with("local bbbbbbbb")),
            "a re-link must not commit the empty root:\n{log}"
        );
        // The old staging repo is moved aside, never deleted.
        assert_eq!(
            fs::read_dir(b.home.path().join("recovery"))
                .unwrap()
                .count(),
            1
        );
    }

    #[test]
    fn invalid_slugs_and_overlapping_paths_are_rejected_before_any_mutation() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        let root = a.root.path().to_path_buf();

        for bad in ["Proj_Claude", "-proj", "proj--claude", ""] {
            assert!(
                a.engine.add_root(&root, Some(bad)).is_err(),
                "slug {bad:?} was accepted"
            );
        }
        let home = a.home.path().to_path_buf();
        assert!(a.engine.add_root(&home, Some("state-home")).is_err());
        assert!(a
            .engine
            .add_root(&home.join("repos"), Some("inside-home"))
            .is_err());
        assert!(a
            .engine
            .add_root(provider.path(), Some("the-provider"))
            .is_err());

        // a_file_path_is_refused_as_a_project
        let file = a.root.path().join("CLAUDE.md");
        assert!(a.engine.add_root(&file, Some("as-a-file")).is_err());

        assert!(Config::load(a.home.path()).unwrap().roots.is_empty());
        assert!(!a.home.path().join("repos/state-home").exists());
        assert!(!a.home.path().join("repos/as-a-file").exists());
    }

    /// The resolver contract end to end, with a sibling planted directly in
    /// staging (a real one needs two devices; task 4 covers that).
    #[test]
    fn resolve_conflict_applies_only_when_every_version_still_matches() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"mine\n");
        add(&mut a);

        let cloud = cloud_of(&a);
        cloud
            .write_device_name_once("proj-claude", &"b".repeat(32), "Mac b Pro")
            .unwrap();
        let root = a.engine.root_cfg("proj-claude").unwrap();
        let repo = a.engine.repo_for(&root).unwrap();
        let sibling = PathBuf::from("CLAUDE.conflict-bbbbbbbb-abc1234.md");
        fs::write(repo.staging.join(&sibling), b"theirs\n").unwrap();
        repo.git.ok(&["add", "-A"]).unwrap();
        repo.git.ok(&["commit", "-m", "conflict"]).unwrap();

        let views = a.engine.conflicts("proj-claude").unwrap();
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].live, Path::new("CLAUDE.md"));
        assert_eq!(views[0].loser_id8, "bbbbbbbb");
        assert_eq!(views[0].loser_name, "Mac b Pro");
        assert!(!views[0].loser_is_me);
        assert_eq!(
            a.engine
                .conflict_sides("proj-claude", Path::new("CLAUDE.md"), &sibling)
                .unwrap(),
            (b"mine\n".to_vec(), b"theirs\n".to_vec())
        );

        let snap = a
            .engine
            .open_resolution("proj-claude", Path::new("CLAUDE.md"))
            .unwrap();
        assert_eq!(snap.siblings.len(), 1);
        assert_eq!(snap.siblings[0].bytes, b"theirs\n");
        assert_eq!(snap.live_bytes, b"mine\n");

        // A stale HEAD deletes nothing and saves nothing.
        let mut stale = snap.clone();
        stale.head = "f".repeat(40);
        assert!(matches!(
            a.engine
                .resolve_conflict(
                    "proj-claude",
                    &stale,
                    std::slice::from_ref(&sibling),
                    b"merged\n",
                )
                .unwrap(),
            ResolveOutcome::Stale(_)
        ));
        assert_eq!(a.engine.conflicts("proj-claude").unwrap().len(), 1);
        assert_eq!(
            fs::read(a.root.path().join("CLAUDE.md")).unwrap(),
            b"mine\n"
        );

        // A sibling that was never displayed is refused outright.
        assert!(a
            .engine
            .resolve_conflict(
                "proj-claude",
                &snap,
                &[PathBuf::from("other.conflict-bbbbbbbb-abc1234.md")],
                b"merged\n",
            )
            .is_err());

        let out = a
            .engine
            .resolve_conflict("proj-claude", &snap, &[sibling], b"merged\n")
            .unwrap();
        assert_eq!(out, ResolveOutcome::Applied(RootStatus::Synced));
        assert_eq!(
            fs::read(a.root.path().join("CLAUDE.md")).unwrap(),
            b"merged\n"
        );
        assert!(a.engine.conflicts("proj-claude").unwrap().is_empty());
        // Siblings live in staging only.
        assert!(!a
            .root
            .path()
            .join("CLAUDE.conflict-bbbbbbbb-abc1234.md")
            .exists());
    }

    #[test]
    fn an_ignore_file_conflict_is_not_counted_in_the_badge() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"mine\n");
        add(&mut a);

        let cloud = cloud_of(&a);
        cloud
            .write_device_name_once("proj-claude", &"b".repeat(32), "Mac b Pro")
            .unwrap();
        let root = a.engine.root_cfg("proj-claude").unwrap();
        let repo = a.engine.repo_for(&root).unwrap();
        let sibling = PathBuf::from(format!(
            "{}.conflict-bbbbbbbb-abc1234",
            crate::project::IGNORE_FILE
        ));
        fs::write(repo.staging.join(&sibling), b"theirs-ignore\n").unwrap();
        repo.git.ok(&["add", "-A"]).unwrap();
        repo.git.ok(&["commit", "-m", "ignore conflict"]).unwrap();

        assert_eq!(root_status(&repo).unwrap(), RootStatus::Synced);

        let views = a.engine.conflicts("proj-claude").unwrap();
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].live, Path::new(crate::project::IGNORE_FILE));
        assert_eq!(views[0].sibling, sibling);
        assert_eq!(views[0].loser_id8, "bbbbbbbb");

        let snap = a
            .engine
            .open_resolution("proj-claude", Path::new(crate::project::IGNORE_FILE))
            .unwrap();
        let out = a
            .engine
            .resolve_conflict("proj-claude", &snap, &[sibling], &snap.live_bytes)
            .unwrap();
        assert_eq!(out, ResolveOutcome::Applied(RootStatus::Synced));
        assert!(a.engine.conflicts("proj-claude").unwrap().is_empty());
    }

    #[test]
    fn an_untracked_files_conflict_is_not_counted() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        a.engine.cfg.default_patterns = Some(vec!["CLAUDE.md".into(), "notes.md".into()]);
        a.engine.cfg.save(a.home.path()).unwrap();
        write(&a, "CLAUDE.md", b"one\n");
        write(&a, "notes.md", b"mine\n");
        add(&mut a);

        let root = a.engine.root_cfg("proj-claude").unwrap();
        let repo = a.engine.repo_for(&root).unwrap();
        let sibling = PathBuf::from("notes.conflict-bbbbbbbb-abc1234.md");
        fs::write(repo.staging.join(&sibling), b"theirs\n").unwrap();
        repo.git.ok(&["add", "-A"]).unwrap();
        repo.git.ok(&["commit", "-m", "conflict"]).unwrap();
        assert_eq!(root_status(&repo).unwrap(), RootStatus::Conflicts(1));

        assert_eq!(
            a.engine
                .untrack_entry("proj-claude", Path::new("notes.md"))
                .unwrap(),
            RootStatus::Synced
        );
        assert!(a.engine.conflicts("proj-claude").unwrap().is_empty());
    }

    /// I9: the backup is taken and verified first, the registration survives,
    /// and this device's own bundles are imported (a normal fetch skips them).
    #[test]
    fn recover_rebuilds_from_own_bundles_and_keeps_the_backup() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        add(&mut a);
        let head = Repo::open(
            a.home.path(),
            "proj-claude",
            a.root.path(),
            "Mac a Pro",
            &"a".repeat(32),
        )
        .unwrap()
        .git
        .rev("refs/heads/main")
        .unwrap();

        fs::remove_dir_all(staging(&a)).unwrap();
        assert_eq!(
            a.engine.recover_root("proj-claude").unwrap(),
            RootStatus::Synced
        );
        assert_eq!(Config::load(a.home.path()).unwrap().roots.len(), 1);
        let after = Repo::open(
            a.home.path(),
            "proj-claude",
            a.root.path(),
            "Mac a Pro",
            &"a".repeat(32),
        )
        .unwrap();
        assert!(after.git.is_ancestor(&head, "refs/heads/main"));
        assert_eq!(fs::read(a.root.path().join("CLAUDE.md")).unwrap(), b"one\n");
    }

    #[test]
    fn recovery_fails_closed_on_unreadable_local_state() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        add(&mut a);

        let journal = staging(&a).join(".git/dotlore-apply.json");
        fs::write(&journal, b"{ truncated").unwrap();
        let err = a
            .engine
            .recover_root("proj-claude")
            .unwrap_err()
            .to_string();
        assert!(err.contains("dotlore-apply.json"), "{err}");

        // Backup kept, staging and root untouched.
        let recovery = a.home.path().join("recovery");
        assert_eq!(fs::read_dir(&recovery).unwrap().count(), 1);
        assert_eq!(fs::read(&journal).unwrap(), b"{ truncated");
        assert_eq!(fs::read(a.root.path().join("CLAUDE.md")).unwrap(), b"one\n");
    }

    /// Convergence regression, found with a two-device smoke run.
    ///
    /// Two devices can mint equivalent merge commits (same parents, identical
    /// trees); the one with the larger hash is discarded by adoption, but it
    /// stays published forever and this device's remote ref keeps pointing at
    /// it. `merge_remote` answers `AlreadyMerged` only while the trees are
    /// still identical — one local commit later that stale head becomes a
    /// *real* merge against a base that predates everything both sides
    /// resolved, which resurrects deleted conflict siblings. The engine has to
    /// settle it by commit id.
    #[test]
    fn a_superseded_remote_head_is_never_merged_again() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        add(&mut a);

        let root = a.engine.root_cfg("proj-claude").unwrap();
        let repo = a.engine.repo_for(&root).unwrap();
        let key = provider_key(&cloud_of(&a));
        let main = repo.git.rev("refs/heads/main").unwrap();
        let tree = repo.git.ok(&["rev-parse", "HEAD^{tree}"]).unwrap();
        // An unrelated commit with the same tree, as adoption leaves behind.
        // Its hash must sort *above* ours, so this device keeps its own head.
        let mut msg = String::from("equivalent");
        let other = loop {
            let c = repo.git.ok(&["commit-tree", "-m", &msg, &tree]).unwrap();
            if c > main {
                break c;
            }
            msg.push('x');
        };
        let their_ref = remote_ref(&key, &"b".repeat(32));
        repo.git.ok(&["update-ref", &their_ref, &other]).unwrap();

        assert_eq!(
            a.engine.sync_root("proj-claude").unwrap(),
            RootStatus::Synced
        );
        assert_eq!(repo.git.rev("refs/heads/main").as_deref(), Some(&*main));
        assert_eq!(
            repo.git.rev(&settled_ref(&key, &"b".repeat(32))).as_deref(),
            Some(&*other),
            "an identical-tree head that we kept must be settled"
        );

        // One local commit later the trees differ. Without the settle marker
        // this merges unrelated histories and every file conflicts.
        write(&a, "CLAUDE.md", b"two\n");
        assert_eq!(
            a.engine.sync_root("proj-claude").unwrap(),
            RootStatus::Synced
        );
        assert!(a.engine.conflicts("proj-claude").unwrap().is_empty());
        assert_eq!(fs::read(a.root.path().join("CLAUDE.md")).unwrap(), b"two\n");

        // A genuinely new head from that device is still merged.
        let next = repo
            .git
            .ok(&["commit-tree", "-m", "next", "-p", &other, &tree])
            .unwrap();
        repo.git.ok(&["update-ref", &their_ref, &next]).unwrap();
        assert!(needs_merge(&repo, &key, &"b".repeat(32)).unwrap());
    }

    /// I3: the destination gets a complete bootstrap bundle, the source is
    /// left alone, and switching back reuses the original counters.
    #[test]
    fn switching_provider_bootstraps_the_destination_and_keeps_head() {
        let first = TempDir::new().unwrap();
        let second = TempDir::new().unwrap();
        let mut a = device(first.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        add(&mut a);
        let head = || {
            Repo::open(
                a.home.path(),
                "proj-claude",
                a.root.path(),
                "Mac a Pro",
                &"a".repeat(32),
            )
            .unwrap()
            .git
            .rev("refs/heads/main")
            .unwrap()
        };
        let before = head();

        let out = configure_provider(a.home.path(), a.home.path(), second.path()).unwrap();
        assert_eq!(out, vec![("proj-claude".to_string(), RootStatus::Synced)]);
        assert_eq!(head(), before, "switching provider must not move HEAD");

        let dest = Cloud {
            base: second.path().join("dotlore"),
        };
        let dest_manifest = dest.read_manifest("proj-claude").unwrap();
        assert_eq!(
            dest_manifest.display_name,
            a.root
                .path()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .as_ref()
        );
        assert!(!dest_manifest.is_agent);
        assert_eq!(dest.list_bundles("proj-claude").len(), 1);
        // The source keeps everything it had.
        let src = Cloud {
            base: first.path().join("dotlore"),
        };
        assert_eq!(src.list_bundles("proj-claude").len(), 1);

        // Back again: already published there, so no second bundle.
        configure_provider(a.home.path(), a.home.path(), first.path()).unwrap();
        assert_eq!(src.list_bundles("proj-claude").len(), 1);
        assert_eq!(head(), before);
    }

    /// Every blocked path freezes the root, and `cycle` returns before
    /// `fetch_bundles`, so no peer commit can arrive to unfreeze it. The only
    /// remedy that always exists is rebuilding the staging repo, so the
    /// message must name `dotlore recover <slug>` — and it must not tell the
    /// user to move aside a path no filesystem action can affect.
    #[test]
    fn a_blocked_path_is_told_the_remedy_that_exists() {
        // A live path the user owns is the one case that does clear locally,
        // and it still gets the fallback for when the block is the peer's.
        let live = PathBuf::from("CLAUDE.md");
        let unwritable = stall_message("proj-claude", &[live]).unwrap();
        assert!(unwritable.contains("move it aside"), "{unwritable}");
        assert!(
            unwritable.contains("`dotlore recover proj-claude`"),
            "{unwritable}"
        );

        assert_eq!(stall_message("proj-claude", &[]), None);
    }

    const MIB: usize = 1024 * 1024;

    fn fill(dev: &Dev, rel: &str, n: usize) {
        write(dev, rel, &vec![b'x'; n]);
    }

    #[test]
    fn track_entry_refuses_a_file_over_the_limit() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        add(&mut a);
        a.engine.set_max_file_mb(1).unwrap();
        fill(&a, "huge.bin", MIB + 1);

        let err = a
            .engine
            .track_entry("proj-claude", Path::new("huge.bin"), None)
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains(&(MIB as u64 + 1).to_string()),
            "error must name the size, got {msg}"
        );
        assert!(
            msg.contains(&(MIB as u64).to_string()),
            "error must name the limit, got {msg}"
        );
        assert!(!a
            .engine
            .list_entries("proj-claude")
            .unwrap()
            .iter()
            .any(|e| e.key == "huge.bin"));
    }

    #[test]
    fn manual_add_of_an_oversized_folder_requires_confirmation() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        add(&mut a);
        a.engine.set_max_file_mb(1).unwrap();
        a.engine.set_max_seed_folder_mb(1).unwrap();
        fill(&a, "docs/a.md", 400_000);
        fill(&a, "docs/b.md", 400_000);
        fill(&a, "docs/c.md", 400_000);

        let preview = a
            .engine
            .inspect_entry("proj-claude", Path::new("docs"))
            .unwrap();
        assert_eq!(preview.kind, EntryKind::Directory);
        assert!(preview.confirmation_required);
        assert!(preview.bytes > preview.folder_limit);
        assert_eq!(preview.bytes, 1_200_000);

        let before = a.engine.list_entries("proj-claude").unwrap();
        match a
            .engine
            .track_entry("proj-claude", Path::new("docs"), None)
            .unwrap()
        {
            TrackOutcome::NeedsConfirmation(info) => {
                assert_eq!(info.bytes, preview.bytes);
                assert!(info.confirmation_required);
            }
            TrackOutcome::Done(s) => panic!("expected confirmation, got {s:?}"),
        }
        assert_eq!(a.engine.list_entries("proj-claude").unwrap(), before);
    }

    #[test]
    fn confirmed_folder_add_still_skips_oversized_files() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        add(&mut a);
        a.engine.set_max_file_mb(1).unwrap();
        a.engine.set_max_seed_folder_mb(1).unwrap();
        fill(&a, "docs/a.md", 400_000);
        fill(&a, "docs/b.md", 400_000);
        fill(&a, "docs/c.md", 400_000);
        fill(&a, "docs/huge.bin", 2 * MIB);

        let preview = a
            .engine
            .inspect_entry("proj-claude", Path::new("docs"))
            .unwrap();
        assert!(preview.confirmation_required);
        assert_eq!(preview.bytes, 1_200_000);
        assert_eq!(
            preview.skipped_too_large,
            vec![(PathBuf::from("docs/huge.bin"), 2 * MIB as u64)]
        );

        match a
            .engine
            .track_entry("proj-claude", Path::new("docs"), Some(preview.bytes))
            .unwrap()
        {
            TrackOutcome::Done(RootStatus::Synced) => {}
            other => panic!("expected Done(Synced), got {other:?}"),
        }

        let files = a.engine.tracked_files("proj-claude").unwrap();
        let names = rels(&files);
        assert!(names.contains(&"docs/a.md".into()));
        assert!(names.contains(&"docs/b.md".into()));
        assert!(names.contains(&"docs/c.md".into()));
        let huge = files
            .iter()
            .find(|f| f.rel == "docs/huge.bin")
            .expect("over-limit file is still listed");
        assert_eq!(huge.state, FileSync::TooLarge);
        assert_eq!(huge.bytes, 2 * MIB as u64);
        assert!(
            !staging(&a).join("docs/huge.bin").exists(),
            "per-file limit is not overridden by folder confirmation"
        );
    }

    #[test]
    fn folder_growth_after_preview_requires_new_confirmation() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        add(&mut a);
        a.engine.set_max_file_mb(1).unwrap();
        a.engine.set_max_seed_folder_mb(1).unwrap();
        fill(&a, "docs/a.md", 400_000);
        fill(&a, "docs/b.md", 400_000);
        fill(&a, "docs/c.md", 400_000);

        let preview = a
            .engine
            .inspect_entry("proj-claude", Path::new("docs"))
            .unwrap();
        fill(&a, "docs/d.md", 400_000);

        match a
            .engine
            .track_entry("proj-claude", Path::new("docs"), Some(preview.bytes))
            .unwrap()
        {
            TrackOutcome::NeedsConfirmation(info) => {
                assert_eq!(info.bytes, 1_600_000);
                assert!(info.bytes > preview.bytes);
            }
            TrackOutcome::Done(s) => panic!("growth must require a new confirmation, got {s:?}"),
        }
        assert!(!a
            .engine
            .list_entries("proj-claude")
            .unwrap()
            .iter()
            .any(|e| e.key == "docs/"));
    }

    #[test]
    fn cancelled_folder_add_does_not_mutate_state() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        add(&mut a);
        a.engine.set_max_file_mb(1).unwrap();
        a.engine.set_max_seed_folder_mb(1).unwrap();
        fill(&a, "docs/a.md", 400_000);
        fill(&a, "docs/b.md", 400_000);
        fill(&a, "docs/c.md", 400_000);

        let before_entries = a.engine.list_entries("proj-claude").unwrap();
        let before_files = a.engine.tracked_files("proj-claude").unwrap();
        let preview = a
            .engine
            .inspect_entry("proj-claude", Path::new("docs"))
            .unwrap();
        assert!(preview.confirmation_required);
        assert_eq!(
            a.engine.list_entries("proj-claude").unwrap(),
            before_entries
        );
        assert_eq!(a.engine.tracked_files("proj-claude").unwrap(), before_files);
        assert!(!a
            .engine
            .list_entries("proj-claude")
            .unwrap()
            .iter()
            .any(|e| e.key == "docs/"));
    }

    /// `home_dir` is canonical so `default_slug` sees a parent equal to it
    /// (`~/.claude` → `home-claude`). A TempDir path on macOS is not.
    fn agent_engine(provider: &Path, user_home: &Path) -> (TempDir, Engine) {
        let home_dir = user_home.canonicalize().unwrap();
        let home = TempDir::new().unwrap();
        let cfg = Config {
            device_id: "a".repeat(32),
            device_name: "Mac A Pro".into(),
            provider_dir: Some(provider.to_path_buf()),
            ..Default::default()
        };
        cfg.save(home.path()).unwrap();
        let engine = Engine::new(home.path(), &home_dir, cfg).unwrap();
        (home, engine)
    }

    #[test]
    fn import_installed_agents_adds_an_existing_home_and_is_idempotent() {
        let provider = TempDir::new().unwrap();
        let user = TempDir::new().unwrap();
        fs::create_dir(user.path().join(".claude")).unwrap();
        let (state, mut engine) = agent_engine(provider.path(), user.path());

        let first = engine.import_installed_agents().unwrap();
        assert_eq!(first.added, vec!["home-claude".to_string()]);
        assert!(first.failed.is_empty(), "{:?}", first.failed);

        let second = engine.import_installed_agents().unwrap();
        assert!(second.added.is_empty(), "{:?}", second.added);
        assert!(second.failed.is_empty(), "{:?}", second.failed);

        let cfg = Config::load(state.path()).unwrap();
        assert_eq!(cfg.roots.len(), 1);
        assert_eq!(cfg.roots[0].slug, "home-claude");
    }

    #[test]
    fn import_installed_agents_skips_a_dismissed_path() {
        let provider = TempDir::new().unwrap();
        let user = TempDir::new().unwrap();
        fs::create_dir(user.path().join(".claude")).unwrap();
        let (state, mut engine) = agent_engine(provider.path(), user.path());
        engine.add_root(&user.path().join(".claude"), None).unwrap();
        engine.remove_root("home-claude").unwrap();

        let report = engine.import_installed_agents().unwrap();
        assert!(report.added.is_empty(), "{:?}", report.added);
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        let cfg = Config::load(state.path()).unwrap();
        assert!(cfg.roots.is_empty());
        assert_eq!(cfg.dismissed_agents.len(), 1);
    }

    #[test]
    fn remove_root_dismisses_a_catalog_agent() {
        let provider = TempDir::new().unwrap();
        let user = TempDir::new().unwrap();
        let claude = user.path().join(".claude");
        fs::create_dir(&claude).unwrap();
        let (state, mut engine) = agent_engine(provider.path(), user.path());
        engine.add_root(&claude, None).unwrap();
        let stored = Config::load(state.path()).unwrap().roots[0].path.clone();
        fs::remove_dir_all(&claude).unwrap();

        engine.remove_root("home-claude").unwrap();
        let cfg = Config::load(state.path()).unwrap();
        assert!(cfg.roots.is_empty());
        assert_eq!(cfg.dismissed_agents, vec![stored.clone()]);

        let project = TempDir::new().unwrap();
        fs::write(project.path().join("CLAUDE.md"), b"one\n").unwrap();
        engine.add_root(project.path(), Some("proj")).unwrap();
        engine.remove_root("proj").unwrap();
        let cfg = Config::load(state.path()).unwrap();
        assert!(cfg.roots.is_empty());
        assert_eq!(cfg.dismissed_agents, vec![stored]);
    }

    #[test]
    fn add_root_clears_a_dismissed_catalog_agent() {
        let provider = TempDir::new().unwrap();
        let user = TempDir::new().unwrap();
        let claude = user.path().join(".claude");
        fs::create_dir(&claude).unwrap();
        let (state, mut engine) = agent_engine(provider.path(), user.path());
        engine.add_root(&claude, None).unwrap();
        engine.remove_root("home-claude").unwrap();
        assert_eq!(
            Config::load(state.path()).unwrap().dismissed_agents.len(),
            1
        );

        engine.add_root(&claude, None).unwrap();
        let cfg = Config::load(state.path()).unwrap();
        assert!(cfg.dismissed_agents.is_empty());
        assert_eq!(cfg.roots.len(), 1);
        assert_eq!(cfg.roots[0].slug, "home-claude");
    }

    #[test]
    fn import_installed_agents_continues_after_one_failure() {
        let provider = TempDir::new().unwrap();
        let user = TempDir::new().unwrap();
        let other = TempDir::new().unwrap();
        fs::create_dir(user.path().join(".claude")).unwrap();
        fs::create_dir(user.path().join(".codex")).unwrap();
        let (_state, mut engine) = agent_engine(provider.path(), user.path());
        engine.add_root(other.path(), Some("home-claude")).unwrap();

        let report = engine.import_installed_agents().unwrap();
        assert!(
            report.failed.iter().any(|(path, err)| {
                path.ends_with(".claude") && err.contains("already tracked")
            }),
            "{:?}",
            report.failed
        );
        assert!(
            !report
                .failed
                .iter()
                .any(|(path, _)| path.ends_with(".codex")),
            "{:?}",
            report.failed
        );
        assert_eq!(report.added, vec!["home-codex".to_string()]);
    }

    #[test]
    fn wipe_cloud_data_republishes_every_root_from_scratch() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        add(&mut a);
        let base = a.engine.cloud.base.clone();
        // Leftovers of another device and of a slug nobody tracks any more.
        let other = base.join("proj-claude/devices").join("b".repeat(32));
        fs::create_dir_all(&other).unwrap();
        fs::write(other.join("000001.bundle"), b"old").unwrap();
        fs::create_dir_all(base.join("stale-claude")).unwrap();

        let report = a.engine.wipe_cloud_data().unwrap();
        assert_eq!(report.readded, vec!["proj-claude".to_string()]);
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        assert!(!other.exists());
        assert!(!base.join("stale-claude").exists());
        let bundles = cloud_of(&a).list_bundles("proj-claude");
        assert_eq!(bundles.len(), 1);
        assert_eq!(bundles[0].device, "a".repeat(32));
        assert_eq!(bundles[0].seq, 1);
        assert!(cloud_of(&a).read_manifest("proj-claude").is_some());
    }

    #[test]
    fn wipe_cloud_data_keeps_the_config() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        add(&mut a);
        let before = Config::load(a.home.path()).unwrap();

        a.engine.wipe_cloud_data().unwrap();
        let after = Config::load(a.home.path()).unwrap();
        assert_eq!(after.device_id, before.device_id);
        assert_eq!(after.provider_dir, before.provider_dir);
        assert_eq!(after.roots.len(), 1);
        assert_eq!(after.roots[0].slug, "proj-claude");
        assert_eq!(after.roots[0].path, before.roots[0].path);
    }

    #[test]
    fn wipe_cloud_data_reseeds_from_the_current_patterns() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        a.engine.cfg.default_patterns = Some(vec!["CLAUDE.md".into(), "docs/".into()]);
        a.engine.cfg.save(a.home.path()).unwrap();
        write(&a, "CLAUDE.md", b"one\n");
        write(&a, "docs/a.md", b"a\n");
        add(&mut a);
        assert_eq!(
            rels(&a.engine.tracked_files("proj-claude").unwrap()),
            ["CLAUDE.md", "docs/a.md"]
        );

        a.engine.set_default_patterns(vec!["docs/".into()]).unwrap();
        a.engine.wipe_cloud_data().unwrap();
        assert_eq!(
            rels(&a.engine.tracked_files("proj-claude").unwrap()),
            ["docs/a.md"]
        );
        assert_eq!(fs::read(a.root.path().join("CLAUDE.md")).unwrap(), b"one\n");
    }

    #[test]
    fn wipe_cloud_data_clears_recovery_and_tmp() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        add(&mut a);
        fs::create_dir_all(a.home.path().join("recovery/proj-claude-old")).unwrap();
        fs::write(a.home.path().join("tmp/leftover.bundle"), b"x").unwrap();

        a.engine.wipe_cloud_data().unwrap();
        assert!(!a.home.path().join("recovery").exists());
        // `Config::load` recreates an empty `tmp/` on the next entry point.
        assert!(!a.home.path().join("tmp/leftover.bundle").exists());
        assert!(a.home.path().join("config.json").is_file());
    }

    #[test]
    fn wipe_cloud_data_refuses_a_symlinked_cloud_folder() {
        let provider = TempDir::new().unwrap();
        let elsewhere = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        add(&mut a);
        let base = a.engine.cloud.base.clone();
        let target = elsewhere.path().join("dotlore");
        fs::rename(&base, &target).unwrap();
        std::os::unix::fs::symlink(&target, &base).unwrap();

        assert!(a.engine.wipe_cloud_data().is_err());
        assert!(target.join("proj-claude/manifest.json").is_file());
        assert!(fs::symlink_metadata(&base)
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(staging(&a).join(".git").is_dir());
        assert_eq!(Config::load(a.home.path()).unwrap().roots.len(), 1);
    }

    #[test]
    fn wipe_cloud_data_refuses_a_symlinked_state_folder_before_any_delete() {
        let provider = TempDir::new().unwrap();
        let elsewhere = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        add(&mut a);
        fs::write(elsewhere.path().join("keep"), b"k").unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), a.home.path().join("recovery")).unwrap();

        assert!(a.engine.wipe_cloud_data().is_err());
        assert!(elsewhere.path().join("keep").is_file());
        assert!(staging(&a).join(".git").is_dir());
        assert!(cloud_of(&a).read_manifest("proj-claude").is_some());
    }

    #[test]
    fn wipe_cloud_data_reports_a_root_it_could_not_add_again() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        add(&mut a);
        let gone = TempDir::new().unwrap();
        fs::write(gone.path().join("CLAUDE.md"), b"two\n").unwrap();
        a.engine.add_root(gone.path(), Some("gone")).unwrap();
        let gone_path = gone.path().to_path_buf();
        drop(gone);

        let report = a.engine.wipe_cloud_data().unwrap();
        assert_eq!(report.readded, vec!["proj-claude".to_string()]);
        assert_eq!(report.failed.len(), 1, "{:?}", report.failed);
        assert_eq!(report.failed[0].0, "gone");
        assert!(!report.failed[0].1.is_empty());
        let roots = Config::load(a.home.path()).unwrap().roots;
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].slug, "proj-claude");
        assert!(!roots.iter().any(|r| r.path == gone_path));
    }

    #[test]
    fn wipe_cloud_data_refuses_a_state_entry_that_is_not_a_directory() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        add(&mut a);
        let tmp = a.home.path().join("tmp");
        fs::remove_dir_all(&tmp).unwrap();
        fs::write(&tmp, b"not a dir").unwrap();

        assert!(a.engine.wipe_cloud_data().is_err());
        assert!(tmp.is_file());
        assert!(cloud_of(&a).read_manifest("proj-claude").is_some());
        assert!(staging(&a).join(".git").is_dir());
    }

    #[test]
    fn wipe_cloud_data_refuses_without_a_provider() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        add(&mut a);
        let mut cfg = Config::load(a.home.path()).unwrap();
        cfg.provider_dir = None;
        cfg.save(a.home.path()).unwrap();

        assert!(a.engine.wipe_cloud_data().is_err());
        assert!(cloud_of(&a).read_manifest("proj-claude").is_some());
        assert!(staging(&a).join(".git").is_dir());
    }
}
