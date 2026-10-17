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

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::cloud::{self, Cloud, Manifest};
use crate::config::{self, Config, HomeLock, Root};
use crate::conflict;
use crate::git::{self, Git};
use crate::mirror::{self, FileState, Node};
use crate::project::{self, Limits, SensitiveMatcher, Sensitivity, State};
use crate::repo::{
    self, provider_key, remote_ref, unique_id, FetchMode, Repo, ResumeOutcome, Transaction,
};
use crate::settings;
use crate::trash;

/// Where one tracked root stands after a cycle.
///
/// `Checking`, `Pending` and `Retrying` are not errors. `Checking` is the
/// placeholder a listing shows before a cycle reports the root. `Pending` is
/// a linked root whose staging has no `main` yet because no bundle is readable
/// in the cloud. `Retrying` is an apply that raced a live edit, retried on the
/// next cycle with its journal intact.
#[derive(Serialize, Clone, PartialEq, Eq, Debug)]
#[serde(tag = "kind", content = "detail")]
pub enum RootStatus {
    Synced,
    Conflicts(usize),
    Checking,
    Pending,
    Retrying,
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
    pub sensitivity: Option<Sensitivity>,
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
        cloud.write_device_name(&self.slug, &self.device_id, &self.device_name)?;
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
    pub sensitivity: Option<Sensitivity>,
    pub secret_descendants: Vec<String>,
    pub secret_descendants_more: bool,
}

/// Result of [`Engine::track_entry`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrackOutcome {
    Done(RootStatus),
    NeedsConfirmation(InspectedEntry),
    ConfirmSensitive { paths: Vec<String>, more: bool },
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

/// How [`Engine::restore_from_trash`] treats a live file that differs.
#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum RestoreChoice {
    /// Restore only when no live file differs; otherwise report them.
    Ask,
    /// Keep each differing live file in the trash, then overwrite it.
    Replace,
    /// Leave each differing live file alone and write a `.restored` sibling.
    KeepBoth,
}

/// Outcome of [`Engine::restore_from_trash`].
#[derive(Serialize, Clone, PartialEq, Eq, Debug)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RestoreResult {
    /// `Ask` only: these live files differ, and nothing was written.
    Existing { rels: Vec<String> },
    Done {
        restored: Vec<RestoredFile>,
        /// The live file already held these bytes.
        unchanged: Vec<String>,
        /// Protected names, symlinks on the path, targets that are not files.
        skipped: Vec<String>,
        /// `KeepBoth` copies outside the include-list, now tracked entries.
        /// Empty when adding them failed; see `track_error`.
        tracked: Vec<String>,
        /// `KeepBoth` copies outside the include-list left untracked because
        /// the copy or its original matches a sensitive pattern, or the user
        /// untracked that path before; the user tracks them explicitly.
        kept_local: Vec<String>,
        /// Adding the copies to the include-list failed after the files were
        /// written; nothing was added.
        track_error: Option<String>,
    },
}

/// One restored file: its rel in the trash pass, and the live rel written
/// (a `.restored` sibling for `KeepBoth`).
#[derive(Serialize, Clone, PartialEq, Eq, Debug)]
pub struct RestoredFile {
    pub from: String,
    pub to: String,
}

/// One version of a file in the local staging history, newest first in
/// [`Engine::file_history`].
#[derive(Serialize, Clone, PartialEq, Eq, Debug)]
pub struct HistoryVersion {
    pub commit: String,
    /// Committer time, seconds since the epoch.
    pub time: i64,
    pub device_name: String,
    /// The version deleted the file: nothing to preview or restore.
    pub deleted: bool,
    /// Always `None` in a listing: sizing every version would spawn one git
    /// per version. [`Engine::read_history_file`] reports the size.
    pub bytes_len: Option<u64>,
}

/// A tracked file absent from `main`, with the version that deleted it.
#[derive(Serialize, Clone, PartialEq, Eq, Debug)]
pub struct DeletedFile {
    pub rel: String,
    pub commit: String,
    pub deleted_at: i64,
    pub device_name: String,
}

/// One version's bytes from [`Engine::read_history_file`]: the size always,
/// the bytes only when they fit the caller's cap.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct HistoryFile {
    pub bytes_len: u64,
    pub bytes: Option<Vec<u8>>,
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
    /// Set when a cycle's apply wrote trash copies; the next `settle` takes
    /// it and prunes the trash to its cap under the lock it already holds.
    trash_written: AtomicBool,
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
            trash_written: AtomicBool::new(false),
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

    /// Reload config under the home lock and list the registered roots, so a
    /// caller can sync them one [`Engine::sync_root`] at a time.
    pub fn root_slugs(&mut self) -> Result<Vec<String>> {
        let g = config::lock(&self.home)?;
        self.reload(&g)?;
        Ok(self.cfg.roots.iter().map(|r| r.slug.clone()).collect())
    }

    /// One cycle for every currently registered root, under one lock.
    pub fn sync_all(&mut self) -> Result<Vec<(String, RootStatus)>> {
        let g = config::lock(&self.home)?;
        self.reload(&g)?;
        let cloud = self.cloud();
        // Settings are best-effort: a failure here must not stop root sync.
        if let Err(e) = self.sync_settings_locked(&g, &cloud) {
            eprintln!("dotlore: settings sync: {e:#}");
        }
        let slugs: Vec<String> = self.cfg.roots.iter().map(|r| r.slug.clone()).collect();
        let mut out = Vec::new();
        for slug in slugs {
            let status = self.sync_root_locked(&g, &cloud, &slug)?;
            out.push((slug, status));
        }
        Ok(out)
    }

    /// Import the global settings every other device published, then make
    /// sure this device's own snapshot in the cloud matches its local ones.
    /// Returns whether local config changed. Existing roots are never touched:
    /// imported patterns and never-list only apply to roots added later.
    pub fn sync_settings(&mut self) -> Result<bool> {
        let g = config::lock(&self.home)?;
        self.reload(&g)?;
        let cloud = self.cloud();
        self.sync_settings_locked(&g, &cloud)
    }

    fn sync_settings_locked(&mut self, g: &HomeLock, cloud: &Cloud) -> Result<bool> {
        let trash_cap = self.trash_max_mb();
        let mut changed = false;
        for dev in cloud.settings_devices() {
            if dev == self.cfg.device_id {
                continue;
            }
            // `newest_settings` falls back past a stub or unparseable newest file.
            let Some(snap) = cloud
                .newest_settings(&dev)
                .readable
                .as_deref()
                .and_then(settings::parse)
            else {
                continue;
            };
            changed |= settings::apply(&mut self.cfg, &snap);
        }
        if changed {
            self.save(g)?;
        }
        if self.trash_max_mb() != trash_cap {
            self.prune_trash_locked(g);
        }
        self.republish_settings(cloud)?;
        Ok(changed)
    }

    /// Publish this device's settings snapshot under the next seq unless its
    /// newest readable one already equals it. Nothing while the newest file
    /// is still an iCloud stub, and nothing for an all-builtin device: a key
    /// never goes back to gen 0, so an empty local set has nothing to say.
    fn republish_settings(&self, cloud: &Cloud) -> Result<()> {
        let local = settings::local(&self.cfg);
        if local.fields.is_empty() {
            return Ok(());
        }
        let own = &self.cfg.device_id;
        let newest = cloud.newest_settings(own);
        if newest.newest_is_stub {
            return Ok(());
        }
        if newest
            .readable
            .as_deref()
            .and_then(settings::parse)
            .as_ref()
            == Some(&local)
        {
            return Ok(());
        }
        let seq = cloud.settings_max_seq(own) + 1;
        cloud.publish_settings(own, seq, &local.to_bytes())
    }

    /// Stop tracking a root. The staging repo is kept, so re-linking is cheap
    /// and nothing that was never published is thrown away.
    ///
    /// A removed catalog agent home is appended to `dismissed_agents` so a
    /// later import leaves it out. Other roots are not.
    ///
    /// An untracked slug that still has a cloud manifest is an unlinked row:
    /// it is appended to `dismissed_slugs` so the listing hides it on this
    /// device. The cloud itself is never touched.
    pub fn remove_root(&mut self, slug: &str) -> Result<()> {
        let g = config::lock(&self.home)?;
        self.reload(&g)?;
        let Some(removed) = self.cfg.roots.iter().find(|r| r.slug == slug).cloned() else {
            if !self.cloud().slug_dir(slug)?.join("manifest.json").is_file() {
                bail!("no tracked root with slug {slug}");
            }
            if !self.cfg.dismissed_slugs.iter().any(|s| s == slug) {
                self.cfg.dismissed_slugs.push(slug.to_string());
            }
            return self.save(&g);
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
        let guard = repo.reinclude_guard()?;
        let limits = Limits::from_config(&self.cfg);
        let staged = repo.git.ok(&["ls-files", "-z"])?;
        let live_path = root.path.clone();
        let matcher = self.sensitive_matcher();
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
            for (rel, bytes, too_large) in
                mirror::list_live(&live_path, &entries, &ignore, limits, guard.as_ref())?
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
                by_rel.insert(
                    rel.clone(),
                    TrackedFile {
                        sensitivity: matcher.classify(Path::new(&rel)),
                        rel,
                        bytes,
                        state,
                    },
                );
            }
        }
        for rel in staged {
            by_rel.entry(rel.clone()).or_insert(TrackedFile {
                sensitivity: matcher.classify(Path::new(&rel)),
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
        let matcher = self.sensitive_matcher();
        drop(g);
        inspect_live(&root.path, rel, &ignore, limits, &matcher)
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
        self.track_entry_with_sensitive_confirmation(slug, rel, confirmed_folder_bytes, true)
    }

    /// Track with a sensitive-path confirmation checked before any mutation.
    pub fn track_entry_with_sensitive_confirmation(
        &mut self,
        slug: &str,
        rel: &Path,
        confirmed_folder_bytes: Option<u64>,
        confirmed_sensitive: bool,
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
        let preview = inspect_live(&root.path, rel, &ignore, limits, &self.sensitive_matcher())?;
        if preview.kind == EntryKind::File {
            if let Some((_, len)) = preview.skipped_too_large.first() {
                bail!(
                    "{} is {len} bytes (limit {} bytes)",
                    rel.display(),
                    limits.max_file_bytes
                );
            }
        }
        if !confirmed_sensitive {
            let (paths, more) = if preview.sensitivity == Some(Sensitivity::Secret) {
                (vec![rel.to_string_lossy().into_owned()], false)
            } else {
                (
                    preview.secret_descendants.clone(),
                    preview.secret_descendants_more,
                )
            };
            if !paths.is_empty() {
                return Ok(TrackOutcome::ConfirmSensitive { paths, more });
            }
        }
        match preview.kind {
            EntryKind::File => {}
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
        if project::builtin_lines(catalog).is_none() {
            bail!("unknown pattern catalog {catalog}");
        }
        let key = format!("patterns.{catalog}");
        let before = settings::gen_of(&self.cfg, &key);
        let stored = settings::catalog_override(catalog, patterns);
        let old = if catalog == "projects" {
            self.cfg.default_patterns.clone()
        } else {
            self.cfg.agent_patterns.get(catalog).cloned()
        };
        if stored == old {
            return Ok(());
        }
        match (catalog, stored) {
            ("projects", stored) => self.cfg.default_patterns = stored,
            (_, Some(lines)) => {
                self.cfg.agent_patterns.insert(catalog.to_string(), lines);
            }
            (_, None) => {
                self.cfg.agent_patterns.remove(catalog);
            }
        }
        self.save_setting(&g, &key, before)
    }

    /// Track every path `slug`'s catalog patterns now match and the list does
    /// not already hold. A key the user untracked keeps its tombstone.
    pub fn apply_default_patterns(&mut self, slug: &str) -> Result<RootStatus> {
        let g = config::lock(&self.home)?;
        self.reload(&g)?;
        let cloud = self.cloud();
        let root = self.root_cfg(slug)?;
        let repo = self.repo_for(&root)?;
        let mut file = repo.project_file()?;
        if root_present(&root.path) {
            let patterns = project::patterns_for(&root, &self.home_dir, &self.cfg);
            let ignore = repo.ignore_text(&self.home_dir);
            let limits = Limits::from_config(&self.cfg);
            let (seeded, _) = project::seed(&root.path, &patterns, &ignore, limits)?;
            let tracked = file.tracked();
            let mut gen = file.entries.values().map(|e| e.gen).max().unwrap_or(0);
            for key in seeded.entries.into_keys() {
                if file.entries.contains_key(&key) || tracked.contains_rel(Path::new(&key)) {
                    continue;
                }
                gen = gen.saturating_add(1);
                file.entries.insert(
                    key,
                    project::EntryRecord {
                        gen,
                        state: State::Tracked,
                    },
                );
            }
            project::write(&repo.staging, &file)?;
        }
        let status = self.run_cycle(&cloud, &root, FetchMode::Normal);
        self.settle(&g, slug, &status)?;
        Ok(status)
    }

    /// Effective sensitive patterns (`None` in config → `SECRET_PATTERNS`).
    pub fn sensitive_patterns(&self) -> Vec<String> {
        match &self.cfg.sensitive_patterns {
            Some(p) => p.clone(),
            None => lines_owned(project::SECRET_PATTERNS),
        }
    }

    /// Store the sensitive list: trimmed, empties dropped, deduplicated.
    /// An invalid line is rejected without writing config. A list equal to
    /// the builtin clears the override.
    pub fn set_sensitive_patterns(&mut self, patterns: Vec<String>) -> Result<()> {
        let g = config::lock(&self.home)?;
        self.reload(&g)?;
        let key = "sensitive_patterns";
        let before = settings::gen_of(&self.cfg, key);
        let stored = settings::sensitive_override(&patterns)?;
        if stored == self.cfg.sensitive_patterns {
            return Ok(());
        }
        self.cfg.sensitive_patterns = stored;
        self.save_setting(&g, key, before)
    }

    /// Matcher for the effective sensitive list. A stored list that no longer
    /// builds falls back to the builtin one.
    pub(crate) fn sensitive_matcher(&self) -> SensitiveMatcher {
        match &self.cfg.sensitive_patterns {
            Some(p) => {
                SensitiveMatcher::new(p).unwrap_or_else(|_| SensitiveMatcher::default_secret())
            }
            None => SensitiveMatcher::default_secret(),
        }
    }

    /// `slug`'s include-list and ignore text, the two inputs of the mirror walk.
    pub(crate) fn include_filter(&self, slug: &str) -> Result<(project::EntryList, String)> {
        let repo = self.repo_for(&self.root_cfg(slug)?)?;
        Ok((
            repo.project_file()?.tracked(),
            repo.ignore_text(&self.home_dir),
        ))
    }

    /// `slug`'s capture guard, `None` when its project list has no `!` line.
    pub(crate) fn reinclude_guard(&self, slug: &str) -> Result<Option<mirror::ReincludeGuard>> {
        self.repo_for(&self.root_cfg(slug)?)?.reinclude_guard()
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
        let key = "default_ignore";
        let before = settings::gen_of(&self.cfg, key);
        let stored = settings::ignore_override(ignore);
        if stored == self.cfg.default_ignore {
            return Ok(());
        }
        self.cfg.default_ignore = stored;
        self.save_setting(&g, key, before)
    }

    /// Write the global never-list into `slug`'s `.dotloreignore` and untrack
    /// every tracked path it now matches. Untracking (not deleting) is what
    /// lets peers keep their live copies when the change syncs.
    pub fn apply_default_ignore(&mut self, slug: &str) -> Result<RootStatus> {
        let g = config::lock(&self.home)?;
        self.reload(&g)?;
        let cloud = self.cloud();
        let root = self.root_cfg(slug)?;
        let repo = self.repo_for(&root)?;
        let old = repo.ignore_text(&self.home_dir);
        let new = self.default_ignore();
        // The project list still follows, so its paths stay untracked.
        let effective = repo::join_ignore(&new, &repo.project_ignore_text());
        let ignore = mirror::build_ignore(&root.path, &effective)?;
        let file = self.untrack_newly_ignored(&root, &repo, &old, &ignore)?;
        project::write(&repo.staging, &file)?;
        fs::write(repo.staging.join(project::IGNORE_FILE), &new)?;
        let status = self.run_cycle(&cloud, &root, FetchMode::Normal);
        self.settle(&g, slug, &status)?;
        Ok(status)
    }

    /// `slug`'s own ignore list, on top of the never-list ("" when it has none).
    pub fn project_ignore(&self, slug: &str) -> Result<String> {
        Ok(self.repo_for(&self.root_cfg(slug)?)?.project_ignore_text())
    }

    /// Write `slug`'s own ignore list and untrack every tracked path the
    /// effective list now matches, like `apply_default_ignore`. The text is
    /// validated before anything is written: a committed line that does not
    /// build would fail every later cycle.
    pub fn set_project_ignore(&mut self, slug: &str, text: String) -> Result<RootStatus> {
        let g = config::lock(&self.home)?;
        self.reload(&g)?;
        let cloud = self.cloud();
        let root = self.root_cfg(slug)?;
        let repo = self.repo_for(&root)?;
        let old = repo.ignore_text(&self.home_dir);
        let new = repo::join_ignore(&repo.never_ignore_text(), &text);
        let ignore = mirror::build_ignore(&root.path, &new)?;
        if text == repo.project_ignore_text() {
            return root_status(&repo);
        }
        let file = self.untrack_newly_ignored(&root, &repo, &old, &ignore)?;
        project::write(&repo.staging, &file)?;
        fs::write(repo.staging.join(project::PROJECT_IGNORE_FILE), &text)?;
        let status = self.run_cycle(&cloud, &root, FetchMode::Normal);
        self.settle(&g, slug, &status)?;
        Ok(status)
    }

    /// `root`'s include-list with every path `ignore` matches untracked:
    /// staged paths, plus the live ones the `old` ignore text let through.
    /// Untracking (not deleting) is what lets peers keep their live copies.
    fn untrack_newly_ignored(
        &self,
        root: &Root,
        repo: &Repo,
        old: &str,
        ignore: &ignore::gitignore::Gitignore,
    ) -> Result<project::ProjectFile> {
        let mut file = repo.project_file()?;
        let entries = file.tracked();
        // Staged paths too: a peer may hold them live even when this root does not.
        let mut rels: BTreeSet<PathBuf> = repo
            .git
            .ok(&["ls-files", "-z"])?
            .split('\0')
            .filter(|s| !s.is_empty() && !is_internal(s))
            .map(PathBuf::from)
            .filter(|p| entries.contains_rel(p))
            .collect();
        if root_present(&root.path) {
            let limits = Limits::from_config(&self.cfg);
            for (rel, _, _) in mirror::list_live(&root.path, &entries, old, limits, None)? {
                rels.insert(rel);
            }
        }
        let hits: BTreeSet<(PathBuf, bool)> = rels
            .iter()
            .filter_map(|rel| ignored_prefix(ignore, rel))
            .collect();
        for (rel, is_dir) in &hits {
            file.untrack_as(rel, Some(*is_dir))?;
        }
        Ok(file)
    }

    /// Effective per-file ceiling in MiB (`None` in config → 50).
    pub fn max_file_mb(&self) -> u64 {
        self.cfg.max_file_mb.unwrap_or(Limits::DEFAULT_MAX_FILE_MB)
    }

    pub fn set_max_file_mb(&mut self, mb: u64) -> Result<()> {
        // Peers drop a 0 limit on import, so it must never be stored.
        if mb == 0 {
            bail!("max file size must be at least 1 MB");
        }
        let g = config::lock(&self.home)?;
        self.reload(&g)?;
        let key = "max_file_mb";
        let before = settings::gen_of(&self.cfg, key);
        if self.cfg.max_file_mb == Some(mb) {
            return Ok(());
        }
        self.cfg.max_file_mb = Some(mb);
        self.save_setting(&g, key, before)
    }

    /// Effective seed/add folder ceiling in MiB (`None` in config → 200).
    pub fn max_seed_folder_mb(&self) -> u64 {
        self.cfg
            .max_seed_folder_mb
            .unwrap_or(Limits::DEFAULT_MAX_SEED_FOLDER_MB)
    }

    pub fn set_max_seed_folder_mb(&mut self, mb: u64) -> Result<()> {
        // Peers drop a 0 limit on import, so it must never be stored.
        if mb == 0 {
            bail!("max folder size must be at least 1 MB");
        }
        let g = config::lock(&self.home)?;
        self.reload(&g)?;
        let key = "max_seed_folder_mb";
        let before = settings::gen_of(&self.cfg, key);
        if self.cfg.max_seed_folder_mb == Some(mb) {
            return Ok(());
        }
        self.cfg.max_seed_folder_mb = Some(mb);
        self.save_setting(&g, key, before)
    }

    /// Effective trash size cap in MiB (`None` in config → 500).
    pub fn trash_max_mb(&self) -> u64 {
        self.cfg.trash_max_mb.unwrap_or(trash::DEFAULT_MAX_MB)
    }

    /// Store the trash cap and prune the trash down to it.
    pub fn set_trash_max_mb(&mut self, mb: u64) -> Result<()> {
        // Peers drop a cap under the floor on import, so it must never be stored.
        if mb < trash::MIN_MAX_MB {
            bail!(
                "the trash size limit must be at least {} MB",
                trash::MIN_MAX_MB
            );
        }
        let g = config::lock(&self.home)?;
        self.reload(&g)?;
        let key = "trash_max_mb";
        let before = settings::gen_of(&self.cfg, key);
        if self.cfg.trash_max_mb == Some(mb) {
            return Ok(());
        }
        self.cfg.trash_max_mb = Some(mb);
        self.save_setting(&g, key, before)?;
        self.prune_trash_locked(&g);
        Ok(())
    }

    /// Prune the trash to its cap; the daemon runs this at startup.
    pub fn prune_trash(&mut self) -> Result<()> {
        let g = config::lock(&self.home)?;
        self.reload(&g)?;
        self.prune_trash_locked(&g);
        Ok(())
    }

    pub fn device_name(&self) -> String {
        self.cfg.device_name.clone()
    }

    pub fn device_id(&self) -> String {
        self.cfg.device_id.clone()
    }

    /// Saves this Mac's name and records it under every linked slug. A slug
    /// the cloud cannot take right now gets it on its next sync. Returns the
    /// name as stored, after trimming and `clean_name`.
    pub fn set_device_name(&mut self, name: &str) -> Result<String> {
        let name = cloud::clean_name(name.trim()).trim().to_string();
        if name.is_empty() {
            bail!("the device name cannot be empty");
        }
        let g = config::lock(&self.home)?;
        self.reload(&g)?;
        self.cfg.device_name = name.clone();
        self.save(&g)?;
        let cloud = self.cloud();
        for root in &self.cfg.roots {
            let _ = cloud.write_device_name(&root.slug, &self.cfg.device_id, &self.cfg.device_name);
        }
        Ok(name)
    }

    /// Every device the cloud folder knows of, this one included once it has
    /// a linked project.
    pub fn devices(&mut self) -> Result<Vec<cloud::DeviceInfo>> {
        let g = config::lock(&self.home)?;
        self.reload(&g)?;
        let out = self.cloud().devices();
        drop(g);
        Ok(out)
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
            let outcome = tx.resume(&repo, &self.home_dir);
            // Read before `?`: `resume` runs an apply pass of its own.
            if tx.wrote_trash() {
                self.trash_written.store(true, Ordering::Relaxed);
            }
            match outcome? {
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
        let left = tx.apply(&repo, &self.home_dir);
        // Read before `?`, so a pending or failed apply still counts (the
        // flag is set even when a pass fails part-way).
        if tx.wrote_trash() {
            self.trash_written.store(true, Ordering::Relaxed);
        }
        if !left?.is_empty() {
            return Ok(ResolveOutcome::Pending);
        }
        tx.finalize(&repo)?;
        repo.publish(&cloud)?;
        let status = root_status(&repo)?;
        self.settle(&g, slug, &status)?;
        Ok(ResolveOutcome::Applied(status))
    }

    /// Restore a trash pass, or the file or folder at `rel` inside it, into
    /// the live root of `slug` on the user's request. Restored copies stay in
    /// the trash; the next cycle publishes what was written. A `KeepBoth`
    /// copy the include-list does not cover becomes a tracked entry, and a
    /// cycle runs at once. If that step fails, the result is still `Done`,
    /// with `tracked` empty and the error in `track_error`.
    ///
    /// `Ask` writes nothing at all when any live file differs, and returns
    /// those rels so the user can pick `Replace` or `KeepBoth` for the batch.
    /// A removed project, an initializing root and a missing root folder are
    /// refused. Every write goes through [`mirror::restore_to_root`], and its
    /// first error aborts: a failed trash copy of a displaced file is never
    /// swallowed.
    pub fn restore_from_trash(
        &mut self,
        slug: &str,
        day: &str,
        pass: &str,
        rel: Option<&str>,
        choice: RestoreChoice,
    ) -> Result<RestoreResult> {
        let g = config::lock(&self.home)?;
        self.reload(&g)?;
        let Some(root) = self.cfg.roots.iter().find(|r| r.slug == slug).cloned() else {
            bail!("{slug} was removed from Dotlore; its trash can no longer be restored");
        };
        if root.initializing {
            bail!("cannot restore into {slug} yet; wait until this project finishes linking");
        }
        if !fs::symlink_metadata(&root.path).is_ok_and(|md| md.is_dir()) {
            bail!("the project folder {} is missing", root.path.display());
        }
        let pass_dir = trash::pass_dir(&self.home, slug, day, pass)?;
        if let Some(rel) = rel {
            trash::item_path(&self.home, slug, day, pass, rel)?;
        }
        let rels = trash::files_under(&pass_dir, rel)?;
        if rels.is_empty() {
            bail!("nothing to restore in this trash item");
        }

        if choice == RestoreChoice::Ask {
            let mut existing = Vec::new();
            for rel in &rels {
                let preview =
                    mirror::restore_preview(&root.path, Path::new(rel), &pass_dir.join(rel))?;
                if preview == mirror::Restored::Exists {
                    existing.push(rel.clone());
                }
            }
            if !existing.is_empty() {
                return Ok(RestoreResult::Existing { rels: existing });
            }
        }
        let on_existing = match choice {
            RestoreChoice::Ask => mirror::OnExisting::Ask,
            RestoreChoice::Replace => mirror::OnExisting::Replace,
            RestoreChoice::KeepBoth => mirror::OnExisting::KeepBoth,
        };
        let displaced = repo::trash_dir(&self.home, slug, SystemTime::now());
        let (mut restored, mut unchanged, mut skipped) = (Vec::new(), Vec::new(), Vec::new());
        for rel in rels {
            let src = pass_dir.join(&rel);
            match mirror::restore_to_root(
                &root.path,
                Path::new(&rel),
                &src,
                &displaced,
                on_existing,
            )? {
                mirror::Restored::Written(to) => restored.push(RestoredFile {
                    to: to.to_string_lossy().into_owned(),
                    from: rel,
                }),
                mirror::Restored::Same => unchanged.push(rel),
                // `Exists` only if a live file changed after the `Ask` preview.
                mirror::Restored::Skipped | mirror::Restored::Exists => skipped.push(rel),
            }
        }
        // The files are written: a tracking failure is reported, never
        // returned, or the user would retry and get a second `.restored` copy.
        let (tracked, kept_local, track_error) =
            match self.track_restored_copies(&g, &root, &restored) {
                Ok((tracked, kept_local)) => (tracked, kept_local, None),
                Err(e) => (Vec::new(), Vec::new(), Some(format!("{e:#}"))),
            };
        Ok(RestoreResult::Done {
            restored,
            unchanged,
            skipped,
            tracked,
            kept_local,
            track_error,
        })
    }

    /// Add each `KeepBoth` copy the include-list does not cover as a tracked
    /// file entry, then run a cycle so it syncs. Runs under the caller's
    /// lock; returns the rels it added and the copies it left untracked
    /// (sensitive, or under a `Removed` record), so a secret is never
    /// published without the confirmation [`Engine::track_entry`] asks for.
    ///
    /// Every fallible step runs before or at `project::write`: the cycle
    /// reports through its status, and `settle` can only fail saving the
    /// config for an initializing root, which `restore_from_trash` refuses.
    /// So an error means nothing was added.
    fn track_restored_copies(
        &mut self,
        g: &HomeLock,
        root: &Root,
        restored: &[RestoredFile],
    ) -> Result<(Vec<String>, Vec<String>)> {
        if restored.iter().all(|r| r.to == r.from) {
            return Ok((Vec::new(), Vec::new()));
        }
        let repo = self.repo_for(root)?;
        let mut file = repo.project_file()?;
        let entries = file.tracked();
        let matcher = self.sensitive_matcher();
        let secret = |rel: &str| matcher.classify(Path::new(rel)) == Some(Sensitivity::Secret);
        // A copy of a secret is still one: the `.restored` rename stops
        // exact-name patterns such as `auth.json` from matching the copy.
        // An explicit untrack of the copy's path or a parent folder stands.
        let untracked = |rel: &str| {
            Path::new(rel).ancestors().any(|p| {
                let key = p.to_string_lossy();
                [key.to_string(), format!("{key}/")].iter().any(|k| {
                    file.entries
                        .get(k)
                        .is_some_and(|rec| rec.state == State::Removed)
                })
            })
        };
        let (kept, added): (Vec<&RestoredFile>, Vec<&RestoredFile>) = restored
            .iter()
            .filter(|r| r.to != r.from && !entries.contains_rel(Path::new(&r.to)))
            .partition(|r| secret(&r.from) || secret(&r.to) || untracked(&r.to));
        let kept_local: Vec<String> = kept.into_iter().map(|r| r.to.clone()).collect();
        let tracked: Vec<String> = added.into_iter().map(|r| r.to.clone()).collect();
        if tracked.is_empty() {
            return Ok((tracked, kept_local));
        }
        let gen = file
            .entries
            .values()
            .map(|e| e.gen)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        for rel in &tracked {
            file.entries.insert(
                rel.clone(),
                project::EntryRecord {
                    gen,
                    state: State::Tracked,
                },
            );
        }
        project::write(&repo.staging, &file)?;
        let status = self.run_cycle(&self.cloud(), root, FetchMode::Normal);
        self.settle(g, &root.slug, &status)?;
        Ok((tracked, kept_local))
    }

    /// The versions of `rel` in the local staging history of `main`, newest
    /// first, at most 200. Only history this Mac's staging repo holds.
    pub fn file_history(&mut self, slug: &str, rel: &str) -> Result<Vec<HistoryVersion>> {
        let g = config::lock(&self.home)?;
        self.reload(&g)?;
        let repo = self.repo_for(&self.root_cfg(slug)?)?;
        let rel = Path::new(rel);
        check_history_rel(&repo, rel)?;
        if !repo.has_main() {
            return Ok(Vec::new());
        }
        let names = self.history_names(slug);
        Ok(repo
            .path_log(MAIN, rel, 200)?
            .into_iter()
            .map(|e| HistoryVersion {
                device_name: event_device_name(&names, &e),
                deleted: e.blob.is_none(),
                bytes_len: None,
                time: e.time,
                commit: e.commit,
            })
            .collect())
    }

    /// Tracked files absent from `main`, newest deletion first, at most 500.
    pub fn deleted_files(&mut self, slug: &str) -> Result<Vec<DeletedFile>> {
        let g = config::lock(&self.home)?;
        self.reload(&g)?;
        let repo = self.repo_for(&self.root_cfg(slug)?)?;
        let names = self.history_names(slug);
        Ok(repo
            .deleted_paths(500)?
            .into_iter()
            .map(|(rel, e)| DeletedFile {
                rel: rel.to_string_lossy().into_owned(),
                device_name: event_device_name(&names, &e),
                deleted_at: e.time,
                commit: e.commit,
            })
            .collect())
    }

    /// The bytes of `rel` at `commit`, which must be on `main`. Over
    /// `max_bytes` only the size is read, so a huge blob is never loaded.
    pub fn read_history_file(
        &mut self,
        slug: &str,
        rel: &str,
        commit: &str,
        max_bytes: u64,
    ) -> Result<HistoryFile> {
        let g = config::lock(&self.home)?;
        self.reload(&g)?;
        let repo = self.repo_for(&self.root_cfg(slug)?)?;
        let (oid, _) = history_blob(&repo, Path::new(rel), commit)?;
        let size = repo.git.ok(&["cat-file", "-s", &oid])?;
        let bytes_len: u64 = size
            .parse()
            .with_context(|| format!("unexpected blob size {size:?}"))?;
        let bytes = if bytes_len > max_bytes {
            None
        } else {
            Some(blob_bytes(&repo.git, &oid)?)
        };
        Ok(HistoryFile { bytes_len, bytes })
    }

    /// Write the version of `rel` at `commit` into the live root, with the
    /// guards and choices of [`Engine::restore_from_trash`]. `commit` must be
    /// on `main` and leave a regular file at `rel`; a version that deleted
    /// the file has nothing to restore. The next cycle commits and publishes
    /// the write.
    pub fn restore_from_history(
        &mut self,
        slug: &str,
        rel: &str,
        commit: &str,
        choice: RestoreChoice,
    ) -> Result<RestoreResult> {
        let g = config::lock(&self.home)?;
        self.reload(&g)?;
        let Some(root) = self.cfg.roots.iter().find(|r| r.slug == slug).cloned() else {
            bail!("{slug} was removed from Dotlore; its history can no longer be restored");
        };
        if root.initializing {
            bail!("cannot restore into {slug} yet; wait until this project finishes linking");
        }
        if !fs::symlink_metadata(&root.path).is_ok_and(|md| md.is_dir()) {
            bail!("the project folder {} is missing", root.path.display());
        }
        let repo = self.repo_for(&root)?;
        let rel_path = Path::new(rel);
        let (oid, executable) = history_blob(&repo, rel_path, commit)?;
        let state = FileState {
            bytes: blob_bytes(&repo.git, &oid)?,
            executable,
        };
        let on_existing = match choice {
            RestoreChoice::Ask => mirror::OnExisting::Ask,
            RestoreChoice::Replace => mirror::OnExisting::Replace,
            RestoreChoice::KeepBoth => mirror::OnExisting::KeepBoth,
        };
        let displaced = repo::trash_dir(&self.home, slug, SystemTime::now());
        let out =
            mirror::restore_state_to_root(&root.path, rel_path, &state, &displaced, on_existing)?;
        // Only a `Replace` that kept the live file creates the pass folder;
        // one left by an earlier write this second only costs a prune.
        if displaced.is_dir() {
            self.trash_written.store(true, Ordering::Relaxed);
        }
        let (mut restored, mut unchanged, mut skipped) = (Vec::new(), Vec::new(), Vec::new());
        match out {
            mirror::Restored::Exists if choice == RestoreChoice::Ask => {
                return Ok(RestoreResult::Existing {
                    rels: vec![rel.to_string()],
                });
            }
            mirror::Restored::Written(to) => restored.push(RestoredFile {
                to: to.to_string_lossy().into_owned(),
                from: rel.to_string(),
            }),
            mirror::Restored::Same => unchanged.push(rel.to_string()),
            mirror::Restored::Skipped | mirror::Restored::Exists => skipped.push(rel.to_string()),
        }
        // As in `restore_from_trash`: the file is written, so a tracking
        // failure is reported, never returned.
        let (tracked, kept_local, track_error) =
            match self.track_restored_copies(&g, &root, &restored) {
                Ok((tracked, kept_local)) => (tracked, kept_local, None),
                Err(e) => (Vec::new(), Vec::new(), Some(format!("{e:#}"))),
            };
        Ok(RestoreResult::Done {
            restored,
            unchanged,
            skipped,
            tracked,
            kept_local,
            track_error,
        })
    }

    /// Device id to display name for history rows: the cloud's names, with
    /// this Mac's current name for its own id.
    fn history_names(&self, slug: &str) -> std::collections::HashMap<String, String> {
        let mut names = self.cloud().device_names(slug);
        names.insert(self.cfg.device_id.clone(), self.cfg.device_name.clone());
        names
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
        cloud.write_device_name(slug, &self.cfg.device_id, &self.cfg.device_name)?;
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
        cloud.write_device_name(&root.slug, &self.cfg.device_id, &self.cfg.device_name)?;
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
            cloud.write_device_name(&root.slug, &self.cfg.device_id, &self.cfg.device_name)?;
        }

        let key = provider_key(cloud);
        let pending = match repo.pending_tx(conflict::resolve_index)? {
            Some(mut tx) => {
                let outcome = tx.resume(&repo, &self.home_dir);
                // Read before `?`: `resume` runs an apply pass of its own.
                if tx.wrote_trash() {
                    self.trash_written.store(true, Ordering::Relaxed);
                }
                match outcome? {
                    // Nothing was applied and `main` never moved: redo the
                    // merges on this transaction. No `commit_local` — the
                    // journal's baseline is the one the live root still
                    // matches, and local edits are picked up by the apply's
                    // reconciliation or by the next cycle.
                    ResumeOutcome::Restart => Some(tx),
                    ResumeOutcome::Finished => {
                        repo.publish(cloud)?;
                        None
                    }
                    ResumeOutcome::Pending(_) => return Ok(stalled(&root.slug, &tx)),
                }
            }
            None => None,
        };

        if pending.is_none() && root_present(&root.path) {
            let captured = repo.commit_local(false, &self.home_dir);
            // Read before `?`: the capture may have trashed deletions first.
            if repo.wrote_trash() {
                self.trash_written.store(true, Ordering::Relaxed);
            }
            captured?;
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
        let left = tx.apply(&repo, &self.home_dir);
        // Read before `?`, so a pending or failed apply still counts (the
        // flag is set even when a pass fails part-way).
        if tx.wrote_trash() {
            self.trash_written.store(true, Ordering::Relaxed);
        }
        if !left?.is_empty() {
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

    /// Save a setter's change to synced `key`: bump its gen past `before`
    /// (its gen ahead of the write), save, republish. A publish failure is
    /// logged, never returned; the next settings sync publishes again.
    fn save_setting(&mut self, g: &HomeLock, key: &str, before: u64) -> Result<()> {
        settings::bump(&mut self.cfg, key, before);
        self.save(g)?;
        if let Err(e) = self.republish_settings(&self.cloud()) {
            eprintln!("dotlore: settings publish: {e:#}");
        }
        Ok(())
    }

    /// Remove the oldest trash passes until the trash fits its cap. A failure
    /// is logged, never returned: it must not fail a cycle or a setter.
    fn prune_trash_locked(&self, g: &HomeLock) {
        let cap = self.trash_max_mb().saturating_mul(1024 * 1024);
        if let Err(e) = trash::prune_to_cap(g, &self.home, cap) {
            eprintln!("dotlore: trash prune: {e:#}");
        }
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
        repo.sensitive = self.sensitive_matcher();
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
        self.cfg.dismissed_slugs.retain(|s| *s != root.slug);
        self.cfg.roots.push(root);
        self.save(g)
    }

    /// Clear `initializing` once — and only once — a cycle has actually
    /// applied and published.
    ///
    /// First, whatever the status, prune the trash if a cycle wrote to it.
    fn settle(&mut self, g: &HomeLock, slug: &str, status: &RootStatus) -> Result<()> {
        if self.trash_written.swap(false, Ordering::Relaxed) {
            self.prune_trash_locked(g);
        }
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
            trash_written: AtomicBool::new(false),
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
        trash_written: AtomicBool::new(false),
    };
    let dest_cloud = e.cloud();
    // The provider switch is already committed; settings are best-effort.
    if let Err(err) = e.sync_settings_locked(&g, &dest_cloud) {
        eprintln!("dotlore: settings sync: {err:#}");
    }
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
        dest_cloud.write_device_name(&root.slug, &e.cfg.device_id, &e.cfg.device_name)?;
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
/// A path that raced a live edit is `Retrying` — the next cycle picks it up.
/// A path blocked by a symlink in the live root or by a target entry that is
/// not a regular file is re-skipped on every attempt forever: the whole root
/// then stops committing and publishing behind a status with nothing to act
/// on. That one is `Error` naming the paths.
///
/// The root stays frozen as a whole while any path is blocked: finalizing the
/// unblocked paths would advance `main` past a root that does not match it,
/// which is the invariant the transaction exists to hold.
fn stalled(slug: &str, tx: &Transaction) -> RootStatus {
    match stall_message(slug, tx.blocked()) {
        Some(m) => RootStatus::Error(m),
        None => RootStatus::Retrying,
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
        .filter(|c| !is_ignore_file(&c.live))
        .count()
    {
        0 => Ok(RootStatus::Synced),
        n => Ok(RootStatus::Conflicts(n)),
    }
}

/// Conflicts whose live path is still in the include-list, plus the ignore
/// files'. An untracked path keeps its staged blobs, siblings included, so
/// they are left out rather than counted forever.
fn open_conflicts(repo: &Repo) -> Result<Vec<conflict::Conflict>> {
    let entries = repo.project_file()?.tracked();
    Ok(conflict::list(&repo.git)?
        .into_iter()
        .filter(|c| is_ignore_file(&c.live) || entries.contains_rel(&c.live))
        .collect())
}

/// The never-list or the per-project list: listed, but not in the badge.
fn is_ignore_file(p: &Path) -> bool {
    p == Path::new(crate::project::IGNORE_FILE)
        || p == Path::new(crate::project::PROJECT_IGNORE_FILE)
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
/// Only a head that has diverged from `main` can be settled. A head that
/// descends from `main` is a fast-forward: no three-way merge runs, nothing
/// can resurrect, and its tree matching `main`'s says nothing about the
/// commits in between (a delete followed by a restore). Settling it would
/// drop those commits from this head while a peer still delivers them on
/// their own, so a descendant is always merged, marker or not.
///
/// Judged against durable `main` only, and before any merge runs, so the
/// answer does not depend on the order heads are merged in. A head whose hash
/// sorts *below* ours is still merged — that is the adoption case, and it is
/// what keeps both devices on one commit.
///
/// Identical trees are not enough when each side holds user events the other
/// lacks (two devices deleting the same file): settling would drop the other
/// side's events from every later presence decision, so that head is merged
/// into a real merge with the same tree. Two such merges differ only in merge
/// commits, so the next round settles or adopts as above.
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
    let Some(main) = repo.git.rev("refs/heads/main") else {
        return Ok(true);
    };
    // Before the marker: a fast-forward is always taken, so a marker 0.9.x
    // wrote for a descendant head is ignored and that device heals.
    if repo.git.is_ancestor("refs/heads/main", &r) {
        return Ok(true);
    }
    if repo.git.rev(&settled_ref(key, device)).as_deref() == Some(rev.as_str()) {
        return Ok(false);
    }
    let identical = repo
        .git
        .run(&["diff", "--quiet", "--end-of-options", &main, &r])?
        .status
        .success();
    if identical && rev > main && !crate::repo::histories_differ(&repo.git, &main, &rev)? {
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

/// Shortest prefix of `rel` that `ignore` matches, and whether it is a folder.
fn ignored_prefix(ignore: &ignore::gitignore::Gitignore, rel: &Path) -> Option<(PathBuf, bool)> {
    let comps: Vec<_> = rel.components().collect();
    let mut acc = PathBuf::new();
    for (i, c) in comps.iter().enumerate() {
        acc.push(c);
        let is_dir = i + 1 < comps.len();
        if ignore.matched(&acc, is_dir).is_ignore() {
            return Some((acc, is_dir));
        }
    }
    None
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
pub(crate) fn tree_entry(git: &Git, rev: &str, rel: &Path) -> Result<(String, bool)> {
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
pub(crate) fn blob_bytes(git: &Git, oid: &str) -> Result<Vec<u8>> {
    let out = git.run(&["cat-file", "blob", oid])?;
    if !out.status.success() {
        bail!(
            "git cat-file blob {oid} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(out.stdout)
}

const MAIN: &str = "refs/heads/main";

/// A history rel must be plain and covered by the include-list: an
/// untracked path's history is a tombstone, not something to restore.
fn check_history_rel(repo: &Repo, rel: &Path) -> Result<()> {
    if !plain_rel(rel) {
        bail!("unsafe path {}", rel.display());
    }
    if !repo.project_file()?.tracked().contains_rel(rel) {
        bail!("{} is not tracked in this project", rel.display());
    }
    Ok(())
}

/// `(blob id, executable)` of `rel` at a user-chosen `commit`, which must be
/// a full id on `main` that leaves a regular file at `rel`.
fn history_blob(repo: &Repo, rel: &Path, commit: &str) -> Result<(String, bool)> {
    if !repo::is_commit_id(commit) {
        bail!("invalid commit id {commit:?}");
    }
    check_history_rel(repo, rel)?;
    if !repo.git.is_ancestor(commit, MAIN) {
        bail!("{commit} is not in this project's history");
    }
    tree_entry(&repo.git, commit, rel).map_err(|e| {
        let spec = format!("{commit}:{}", rel.to_string_lossy());
        let present = repo
            .git
            .run(&["cat-file", "-e", "--end-of-options", &spec])
            .is_ok_and(|o| o.status.success());
        if present {
            e
        } else {
            anyhow!(
                "{} is deleted in this version; there is nothing to restore",
                rel.display()
            )
        }
    })
}

/// The display name of the device behind a history event. Every string
/// here was written by some device, so it is display data only.
fn event_device_name(
    names: &std::collections::HashMap<String, String>,
    event: &repo::PathEvent,
) -> String {
    if let Some(name) = event.device_id.as_ref().and_then(|id| names.get(id)) {
        return name.clone();
    }
    let author = cloud::clean_name(event.author.trim());
    if !author.is_empty() {
        return author;
    }
    event
        .device_id
        .as_deref()
        .map(|id| id.chars().take(8).collect())
        .unwrap_or_default()
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
    matcher: &SensitiveMatcher,
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
                sensitivity: matcher.classify(rel),
                secret_descendants: Vec::new(),
                secret_descendants_more: false,
            })
        }
        Node::Dir => {
            let measured = mirror::measure_tree(root, &trimmed, ignore_text, limits)?;
            let (secret_descendants, secret_descendants_more) =
                secret_descendants(root, &trimmed, ignore_text, limits, matcher)?;
            Ok(InspectedEntry {
                kind: EntryKind::Directory,
                bytes: measured.bytes,
                folder_limit: limits.max_seed_folder_bytes,
                confirmation_required: measured.bytes > limits.max_seed_folder_bytes,
                skipped_too_large: measured.skipped_too_large,
                sensitivity: None,
                secret_descendants,
                secret_descendants_more,
            })
        }
    }
}

fn secret_descendants(
    root: &Path,
    rel: &Path,
    ignore_text: &str,
    limits: Limits,
    matcher: &SensitiveMatcher,
) -> Result<(Vec<String>, bool)> {
    let mut entries = project::ProjectFile::default();
    entries.entries.insert(
        format!("{}/", rel.display()),
        project::EntryRecord {
            gen: 1,
            state: State::Tracked,
        },
    );
    let mut paths: Vec<String> =
        mirror::list_live(root, &entries.tracked(), ignore_text, limits, None)?
            .into_iter()
            .filter(|(path, _, _)| matcher.classify(path) == Some(Sensitivity::Secret))
            .map(|(path, _, _)| path.to_string_lossy().into_owned())
            .collect();
    paths.sort();
    let more = paths.len() > 20;
    paths.truncate(20);
    Ok((paths, more))
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
    fn apply_default_patterns_tracks_new_matches_and_keeps_untracked_keys() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        a.engine
            .set_pattern_catalog("projects", vec!["CLAUDE.md".into(), "docs/".into()])
            .unwrap();
        write(&a, "CLAUDE.md", b"one\n");
        write(&a, "docs/a.md", b"a\n");
        write(&a, "README.md", b"readme\n");
        write(&a, "AGENTS.md", b"agents\n");
        add(&mut a);
        a.engine
            .untrack_entry("proj-claude", Path::new("CLAUDE.md"))
            .unwrap();

        a.engine
            .set_pattern_catalog(
                "projects",
                vec![
                    "CLAUDE.md".into(),
                    "docs/".into(),
                    "README.md".into(),
                    "missing.md".into(),
                ],
            )
            .unwrap();
        assert_eq!(
            a.engine.apply_default_patterns("proj-claude").unwrap(),
            RootStatus::Synced
        );

        let file = crate::project::read(&staging(&a)).unwrap();
        assert_eq!(
            file.entries["CLAUDE.md"].state,
            crate::project::State::Removed
        );
        assert!(!file.entries.contains_key("missing.md"));
        assert_eq!(
            rels(&a.engine.tracked_files("proj-claude").unwrap()),
            ["README.md", "docs/a.md"]
        );
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
    fn tracked_files_reports_sensitivity() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        write(&a, "docs/credentials.json", b"{}\n");
        write(&a, "docs/.mcp.json", b"{}\n");
        write(&a, "docs/notes.md", b"notes\n");
        add(&mut a);

        let files = a.engine.tracked_files("proj-claude").unwrap();
        let tier = |rel: &str| files.iter().find(|f| f.rel == rel).unwrap().sensitivity;
        assert_eq!(
            tier("docs/credentials.json"),
            Some(project::Sensitivity::Secret)
        );
        assert_eq!(
            tier("docs/.mcp.json"),
            Some(project::Sensitivity::TokenHint)
        );
        assert_eq!(tier("docs/notes.md"), None);
    }

    #[test]
    fn a_custom_sensitive_list_replaces_the_builtin_secret_patterns() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        write(&a, "docs/x.secret", b"s\n");
        write(&a, "docs/.env", b"K=V\n");
        add(&mut a);
        a.engine
            .set_sensitive_patterns(vec!["  *.secret ".into(), "".into(), "*.secret".into()])
            .unwrap();
        assert_eq!(a.engine.sensitive_patterns(), vec!["*.secret".to_string()]);
        assert_eq!(
            Config::load(a.home.path()).unwrap().sensitive_patterns,
            Some(vec!["*.secret".into()])
        );

        let files = a.engine.tracked_files("proj-claude").unwrap();
        let tier = |rel: &str| files.iter().find(|f| f.rel == rel).unwrap().sensitivity;
        assert_eq!(tier("docs/x.secret"), Some(project::Sensitivity::Secret));
        assert_ne!(tier("docs/.env"), Some(project::Sensitivity::Secret));
    }

    #[test]
    fn set_sensitive_patterns_equal_to_builtin_clears_the_override() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        a.engine
            .set_sensitive_patterns(vec!["*.secret".into()])
            .unwrap();
        a.engine
            .set_sensitive_patterns(lines_owned(project::SECRET_PATTERNS))
            .unwrap();
        assert_eq!(
            Config::load(a.home.path()).unwrap().sensitive_patterns,
            None
        );
        assert_eq!(
            a.engine.sensitive_patterns(),
            lines_owned(project::SECRET_PATTERNS)
        );
    }

    #[test]
    fn set_sensitive_patterns_rejects_an_invalid_line_without_writing() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        a.engine
            .set_sensitive_patterns(vec!["*.secret".into()])
            .unwrap();
        let before = fs::read(a.home.path().join("config.json")).unwrap();
        let err = a
            .engine
            .set_sensitive_patterns(vec!["*.pem".into(), "bad[z-a]".into()])
            .unwrap_err();
        assert!(format!("{err:#}").contains("bad[z-a]"), "{err:#}");
        let after = fs::read(a.home.path().join("config.json")).unwrap();
        assert_eq!(before, after);
        assert_eq!(a.engine.sensitive_patterns(), vec!["*.secret".to_string()]);
    }

    #[test]
    fn track_entry_asks_to_confirm_a_custom_sensitive_match() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        add(&mut a);
        write(&a, "notes/x.secret", b"s\n");
        a.engine
            .set_sensitive_patterns(vec!["*.secret".into()])
            .unwrap();

        let outcome = a
            .engine
            .track_entry_with_sensitive_confirmation(
                "proj-claude",
                Path::new("notes/x.secret"),
                None,
                false,
            )
            .unwrap();
        match outcome {
            TrackOutcome::ConfirmSensitive { paths, more } => {
                assert_eq!(paths, vec!["notes/x.secret".to_string()]);
                assert!(!more);
            }
            _ => panic!("expected ConfirmSensitive"),
        }
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
                    sensitivity: None,
                },
                TrackedFile {
                    rel: "docs/a.md".into(),
                    bytes: 4,
                    state: FileSync::Synced,
                    sensitivity: None,
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
                sensitivity: None,
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
                    sensitivity: None,
                },
                TrackedFile {
                    rel: "docs/a.md".into(),
                    bytes: 0,
                    state: FileSync::Pending,
                    sensitivity: None,
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

    #[test]
    fn apply_default_ignore_untracks_matches_and_keeps_peer_files() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "docs/keep.md", b"keep\n");
        write(&a, "docs/cache/x.md", b"x\n");
        write(&a, "docs/a.draft.md", b"log\n");
        add(&mut a);
        let mut b = device(provider.path(), 'b');
        let b_root = b.root.path().to_path_buf();
        b.engine.link_root("proj-claude", &b_root).unwrap();
        assert!(b.root.path().join("docs/cache/x.md").is_file());

        let ignore = format!("{}/docs/cache/\n*.draft.md\n", a.engine.default_ignore());
        a.engine.set_default_ignore(ignore.clone()).unwrap();
        assert_eq!(
            a.engine.apply_default_ignore("proj-claude").unwrap(),
            RootStatus::Synced
        );
        let file = crate::project::read(&staging(&a)).unwrap();
        assert_eq!(
            file.entries["docs/cache/"].state,
            crate::project::State::Removed
        );
        assert_eq!(
            file.entries["docs/a.draft.md"].state,
            crate::project::State::Removed
        );
        assert_eq!(
            fs::read_to_string(staging(&a).join(crate::project::IGNORE_FILE)).unwrap(),
            ignore
        );
        assert_eq!(
            rels(&a.engine.tracked_files("proj-claude").unwrap()),
            ["docs/keep.md"]
        );

        assert_eq!(
            b.engine.sync_root("proj-claude").unwrap(),
            RootStatus::Synced
        );
        assert_eq!(
            b.engine.sync_root("proj-claude").unwrap(),
            RootStatus::Synced
        );
        for dev in [&a, &b] {
            assert!(dev.root.path().join("docs/cache/x.md").is_file());
            assert!(dev.root.path().join("docs/a.draft.md").is_file());
        }
        assert_eq!(
            rels(&b.engine.tracked_files("proj-claude").unwrap()),
            ["docs/keep.md"]
        );
        assert_eq!(
            a.engine.apply_default_ignore("proj-claude").unwrap(),
            RootStatus::Synced
        );
        // Untracking keeps the live files, so nothing is captured to trash.
        for dev in [&a, &b] {
            assert!(trash_files(dev).is_empty(), "{:?}", trash_files(dev));
        }
    }

    #[test]
    fn set_project_ignore_untracks_matches_and_keeps_live_files() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "docs/keep.md", b"keep\n");
        write(&a, "docs/cache/x.md", b"x\n");
        add(&mut a);
        assert_eq!(a.engine.project_ignore("proj-claude").unwrap(), "");

        assert_eq!(
            a.engine
                .set_project_ignore("proj-claude", "docs/cache/\n".into())
                .unwrap(),
            RootStatus::Synced
        );
        let file = crate::project::read(&staging(&a)).unwrap();
        assert_eq!(
            file.entries["docs/cache/"].state,
            crate::project::State::Removed
        );
        assert_eq!(
            a.engine.project_ignore("proj-claude").unwrap(),
            "docs/cache/\n"
        );
        assert_eq!(
            rels(&a.engine.tracked_files("proj-claude").unwrap()),
            ["docs/keep.md"]
        );
        assert!(a.root.path().join("docs/cache/x.md").is_file());
        let repo = a
            .engine
            .repo_for(&a.engine.root_cfg("proj-claude").unwrap())
            .unwrap();
        assert_eq!(
            repo.git
                .ok(&["ls-files", "--", crate::project::PROJECT_IGNORE_FILE])
                .unwrap()
                .trim(),
            crate::project::PROJECT_IGNORE_FILE
        );
        assert!(repo.git.ok(&["status", "--porcelain"]).unwrap().is_empty());
        assert_eq!(
            a.engine
                .set_project_ignore("proj-claude", "docs/cache/\n".into())
                .unwrap(),
            RootStatus::Synced
        );
        // Untracking keeps the live files, so nothing is captured to trash.
        assert!(trash_files(&a).is_empty(), "{:?}", trash_files(&a));
    }

    #[test]
    fn set_project_ignore_rejects_an_invalid_line_without_writing() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "docs/keep.md", b"keep\n");
        write(&a, "docs/cache/x.md", b"x\n");
        add(&mut a);
        let before = crate::project::read(&staging(&a)).unwrap();

        assert!(a
            .engine
            .set_project_ignore("proj-claude", "docs/cache/\n[z-a].md\n".into())
            .is_err());
        assert!(!staging(&a)
            .join(crate::project::PROJECT_IGNORE_FILE)
            .exists());
        assert_eq!(crate::project::read(&staging(&a)).unwrap(), before);
        assert_eq!(
            rels(&a.engine.tracked_files("proj-claude").unwrap()),
            ["docs/cache/x.md", "docs/keep.md"]
        );
    }

    /// `docs/keep.log` and `docs/credentials.log` behind a never-listed
    /// `*.log`, under a tracked `docs/`.
    fn device_with_never_listed_logs(provider: &Path) -> Dev {
        let mut a = device(provider, 'a');
        let never = format!("{}*.log\n", a.engine.default_ignore());
        a.engine.set_default_ignore(never).unwrap();
        write(&a, "docs/keep.md", b"keep\n");
        write(&a, "docs/keep.log", b"keep\n");
        write(&a, "docs/credentials.log", b"secret\n");
        add(&mut a);
        a
    }

    fn staged_files(dev: &Dev) -> Vec<String> {
        let repo = dev
            .engine
            .repo_for(&dev.engine.root_cfg("proj-claude").unwrap())
            .unwrap();
        repo.git
            .ok(&["ls-files"])
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn a_project_negation_syncs_a_never_listed_file_but_not_a_sensitive_one() {
        let provider = TempDir::new().unwrap();
        let mut a = device_with_never_listed_logs(provider.path());
        assert!(!staged_files(&a).contains(&"docs/keep.log".into()));

        assert_eq!(
            a.engine
                .set_project_ignore("proj-claude", "!*.log\n".into())
                .unwrap(),
            RootStatus::Synced
        );
        let staged = staged_files(&a);
        assert!(staged.contains(&"docs/keep.log".into()), "{staged:?}");
        assert!(
            !staged.contains(&"docs/credentials.log".into()),
            "{staged:?}"
        );
        let tracked = rels(&a.engine.tracked_files("proj-claude").unwrap());
        assert!(tracked.contains(&"docs/keep.log".into()), "{tracked:?}");
        assert!(
            !tracked.contains(&"docs/credentials.log".into()),
            "{tracked:?}"
        );
    }

    #[test]
    fn a_project_negation_is_guarded_by_the_custom_sensitive_patterns() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        let never = format!("{}docs/vault/\n", a.engine.default_ignore());
        a.engine.set_default_ignore(never).unwrap();
        write(&a, "docs/keep.md", b"keep\n");
        write(&a, "docs/vault/ok.md", b"ok\n");
        write(&a, "docs/vault/x.token", b"secret\n");
        add(&mut a);
        a.engine
            .set_sensitive_patterns(vec!["*.token".into()])
            .unwrap();

        assert_eq!(
            a.engine
                .set_project_ignore("proj-claude", "!docs/vault/\n".into())
                .unwrap(),
            RootStatus::Synced
        );
        let staged = staged_files(&a);
        assert!(staged.contains(&"docs/vault/ok.md".into()), "{staged:?}");
        assert!(!staged.contains(&"docs/vault/x.token".into()), "{staged:?}");
    }

    #[test]
    fn a_peer_negation_is_guarded_too() {
        let provider = TempDir::new().unwrap();
        let mut a = device_with_never_listed_logs(provider.path());
        // As a merge lands it: committed straight into staging, never through
        // `set_project_ignore`.
        fs::write(
            staging(&a).join(crate::project::PROJECT_IGNORE_FILE),
            "!*.log\n",
        )
        .unwrap();
        let repo = a
            .engine
            .repo_for(&a.engine.root_cfg("proj-claude").unwrap())
            .unwrap();
        repo.git
            .ok(&["add", "--", crate::project::PROJECT_IGNORE_FILE])
            .unwrap();
        repo.git.ok(&["commit", "-m", "peer list"]).unwrap();

        assert_eq!(
            a.engine.sync_root("proj-claude").unwrap(),
            RootStatus::Synced
        );
        let staged = staged_files(&a);
        assert!(staged.contains(&"docs/keep.log".into()), "{staged:?}");
        assert!(
            !staged.contains(&"docs/credentials.log".into()),
            "{staged:?}"
        );
    }

    #[test]
    fn apply_default_ignore_keeps_the_project_list() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "docs/keep.md", b"keep\n");
        write(&a, "docs/drop.log", b"drop\n");
        write(&a, "docs/cache/x.md", b"x\n");
        add(&mut a);
        a.engine
            .set_project_ignore("proj-claude", "docs/cache/\n".into())
            .unwrap();

        let ignore = format!("{}*.log\n", a.engine.default_ignore());
        a.engine.set_default_ignore(ignore.clone()).unwrap();
        assert_eq!(
            a.engine.apply_default_ignore("proj-claude").unwrap(),
            RootStatus::Synced
        );
        assert_eq!(
            fs::read_to_string(staging(&a).join(crate::project::IGNORE_FILE)).unwrap(),
            ignore
        );
        assert_eq!(
            a.engine.project_ignore("proj-claude").unwrap(),
            "docs/cache/\n"
        );
        assert_eq!(
            rels(&a.engine.tracked_files("proj-claude").unwrap()),
            ["docs/keep.md"]
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
        // Removed, it is an unlinked cloud slug; a second remove dismisses it.
        a.engine.remove_root("proj-claude").unwrap();
        assert_eq!(
            Config::load(a.home.path()).unwrap().dismissed_slugs,
            vec!["proj-claude".to_string()]
        );
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
        // The re-link starts on an unborn HEAD: no deletion to capture.
        assert!(trash_files(&b).is_empty(), "{:?}", trash_files(&b));
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

    #[test]
    fn a_rename_reaches_the_other_device_and_every_device_is_listed() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"x\n");
        add(&mut a);
        let mut b = device(provider.path(), 'b');
        let b_root = b.root.path().to_path_buf();
        b.engine.link_root("proj-claude", &b_root).unwrap();

        assert_eq!(
            a.engine.set_device_name("  Work\u{202e} Mac \n").unwrap(),
            "Work Mac"
        );
        assert_eq!(a.engine.device_name(), "Work Mac");
        assert_eq!(Config::load(a.home.path()).unwrap().device_name, "Work Mac");

        let devices = b.engine.devices().unwrap();
        assert_eq!(devices.len(), 2, "{devices:?}");
        let da = devices.iter().find(|d| d.id == "a".repeat(32)).unwrap();
        assert_eq!(da.name, "Work Mac");
        assert_eq!(da.projects, 1);
        assert!(a.engine.set_device_name(" \u{1b} ").is_err());
    }

    #[test]
    fn a_sync_publishes_a_name_changed_while_offline() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"x\n");
        add(&mut a);
        let mut cfg = Config::load(a.home.path()).unwrap();
        cfg.device_name = "Renamed".into();
        cfg.save(a.home.path()).unwrap();

        a.engine.sync_all().unwrap();

        assert_eq!(
            cloud_of(&a)
                .device_name("proj-claude", &"a".repeat(32))
                .as_deref(),
            Some("Renamed")
        );
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
            .write_device_name("proj-claude", &"b".repeat(32), "Mac b Pro")
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
            .write_device_name("proj-claude", &"b".repeat(32), "Mac b Pro")
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
    fn a_project_ignore_file_conflict_is_not_counted_in_the_badge() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"mine\n");
        add(&mut a);
        a.engine
            .set_project_ignore("proj-claude", "mine.tmp\n".into())
            .unwrap();

        let cloud = cloud_of(&a);
        cloud
            .write_device_name("proj-claude", &"b".repeat(32), "Mac b Pro")
            .unwrap();
        let root = a.engine.root_cfg("proj-claude").unwrap();
        let repo = a.engine.repo_for(&root).unwrap();
        let sibling = PathBuf::from(format!(
            "{}.conflict-bbbbbbbb-abc1234",
            crate::project::PROJECT_IGNORE_FILE
        ));
        fs::write(repo.staging.join(&sibling), b"theirs.tmp\n").unwrap();
        repo.git.ok(&["add", "-A"]).unwrap();
        repo.git
            .ok(&["commit", "-m", "project ignore conflict"])
            .unwrap();

        assert_eq!(root_status(&repo).unwrap(), RootStatus::Synced);

        let views = a.engine.conflicts("proj-claude").unwrap();
        assert_eq!(views.len(), 1);
        assert_eq!(
            views[0].live,
            Path::new(crate::project::PROJECT_IGNORE_FILE)
        );
        assert_eq!(views[0].sibling, sibling);
        assert_eq!(views[0].loser_id8, "bbbbbbbb");

        let snap = a
            .engine
            .open_resolution(
                "proj-claude",
                Path::new(crate::project::PROJECT_IGNORE_FILE),
            )
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

    /// Lost-restore regression (bug memo 2026-10-16): this device is offline
    /// while one peer deletes F (D, a child of `main`) and another restores it
    /// (R, a child of D whose tree equals `main`'s). R contains D and fast-
    /// forwards `main`; settling it as an identical-tree head instead left D
    /// to be merged on its own, and F was gone for good. A settled marker that
    /// 0.9.x already wrote for R must not keep it out either.
    #[test]
    fn a_descendant_head_with_mains_tree_is_fast_forwarded_not_settled() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        write(&a, "docs/a.md", b"a\n");
        add(&mut a);

        let root = a.engine.root_cfg("proj-claude").unwrap();
        let repo = a.engine.repo_for(&root).unwrap();
        let key = provider_key(&cloud_of(&a));
        let m0 = repo.git.rev("refs/heads/main").unwrap();
        let tree_m0 = repo.git.ok(&["rev-parse", "HEAD^{tree}"]).unwrap();
        assert!(repo
            .git
            .ok(&["ls-tree", "-r", "--name-only", "HEAD"])
            .unwrap()
            .contains("docs/a.md"));

        // D: M0 without docs/a.md, everything else kept.
        repo.git
            .ok(&["rm", "--cached", "-q", "--", "docs/a.md"])
            .unwrap();
        let tree_d = repo.git.ok(&["write-tree"]).unwrap();
        repo.git.ok(&["read-tree", "HEAD"]).unwrap();
        assert_ne!(tree_d, tree_m0);
        let d = repo
            .git
            .ok(&["commit-tree", "-m", "delete", "-p", &m0, &tree_d])
            .unwrap();
        // R: restores M0's tree on top of D. Its hash must sort above M0's,
        // so the old identical-tree shortcut would fire.
        let mut msg = String::from("restore");
        let r = loop {
            let c = repo
                .git
                .ok(&["commit-tree", "-m", &msg, "-p", &d, &tree_m0])
                .unwrap();
            if c > m0 {
                break c;
            }
            msg.push('x');
        };

        let (b, c) = ("b".repeat(32), "c".repeat(32));
        repo.git
            .ok(&["update-ref", &remote_ref(&key, &b), &r])
            .unwrap();
        repo.git
            .ok(&["update-ref", &remote_ref(&key, &c), &d])
            .unwrap();

        assert!(needs_merge(&repo, &key, &b).unwrap());
        // A stale marker from 0.9.x is ignored for a fast-forward.
        repo.git
            .ok(&["update-ref", &settled_ref(&key, &b), &r])
            .unwrap();
        assert!(needs_merge(&repo, &key, &b).unwrap());

        assert_eq!(
            a.engine.sync_root("proj-claude").unwrap(),
            RootStatus::Synced
        );
        assert_eq!(repo.git.rev("refs/heads/main").as_deref(), Some(&*r));
        assert_eq!(fs::read(a.root.path().join("docs/a.md")).unwrap(), b"a\n");
    }

    /// Two devices delete the same file from `main`'s parent: identical
    /// trees, but each head carries a user event the other lacks. Settling
    /// the larger hash would drop the other delete from every later presence
    /// decision, so it is merged; once both sides hold equivalent merges
    /// (only merge commits differ), the larger one is settled as before.
    #[test]
    fn needs_merge_keeps_identical_heads_with_differing_user_events() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        write(&a, "docs/a.md", b"a\n");
        add(&mut a);

        let root = a.engine.root_cfg("proj-claude").unwrap();
        let repo = a.engine.repo_for(&root).unwrap();
        let key = provider_key(&cloud_of(&a));
        let b = "b".repeat(32);
        let m0 = repo.git.rev("refs/heads/main").unwrap();
        repo.git
            .ok(&["rm", "--cached", "-q", "--", "docs/a.md"])
            .unwrap();
        let tree_d = repo.git.ok(&["write-tree"]).unwrap();
        repo.git.ok(&["read-tree", "HEAD"]).unwrap();
        let ordered = |mut pair: [String; 2]| {
            pair.sort();
            pair
        };
        let [d_small, d_large] = ordered(["local aaaaaaaa", "local bbbbbbbb"].map(|msg| {
            repo.git
                .ok(&["commit-tree", "-m", msg, "-p", &m0, &tree_d])
                .unwrap()
        }));

        repo.git
            .ok(&["update-ref", "refs/heads/main", &d_small])
            .unwrap();
        repo.git
            .ok(&["update-ref", &remote_ref(&key, &b), &d_large])
            .unwrap();
        assert!(needs_merge(&repo, &key, &b).unwrap());
        assert_eq!(repo.git.rev(&settled_ref(&key, &b)), None);

        let [m_small, m_large] = ordered([(&d_small, &d_large), (&d_large, &d_small)].map(
            |(p1, p2)| {
                repo.git
                    .ok(&["commit-tree", "-m", "Merge", "-p", p1, "-p", p2, &tree_d])
                    .unwrap()
            },
        ));
        repo.git
            .ok(&["update-ref", "refs/heads/main", &m_small])
            .unwrap();
        repo.git
            .ok(&["update-ref", &remote_ref(&key, &b), &m_large])
            .unwrap();
        assert!(!needs_merge(&repo, &key, &b).unwrap());
        assert_eq!(repo.git.rev(&settled_ref(&key, &b)), Some(m_large));
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
            other => panic!("expected confirmation, got {other:?}"),
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
            other => panic!("growth must require a new confirmation, got {other:?}"),
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
    fn remove_root_dismisses_an_unlinked_cloud_slug_until_it_is_linked() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        add(&mut a);
        let mut b = device(provider.path(), 'b');

        b.engine.remove_root("proj-claude").unwrap();
        b.engine.remove_root("proj-claude").unwrap();
        let cfg = Config::load(b.home.path()).unwrap();
        assert!(cfg.roots.is_empty());
        assert_eq!(cfg.dismissed_slugs, vec!["proj-claude".to_string()]);
        assert!(cloud_of(&b).read_manifest("proj-claude").is_some());

        let err = b.engine.remove_root("nowhere").unwrap_err().to_string();
        assert!(err.contains("no tracked root with slug nowhere"), "{err}");

        let b_root = b.root.path().to_path_buf();
        b.engine.link_root("proj-claude", &b_root).unwrap();
        let cfg = Config::load(b.home.path()).unwrap();
        assert!(cfg.dismissed_slugs.is_empty());
        assert_eq!(cfg.roots.len(), 1);
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

    // --- settings sync ------------------------------------------------------

    /// Edit a synced setting the way a setter will: change the field, bump
    /// its gen, save.
    fn customize(dev: &mut Dev, key: &str, edit: impl FnOnce(&mut Config)) {
        let cfg = &mut dev.engine.cfg;
        let before = settings::gen_of(cfg, key);
        edit(cfg);
        settings::bump(cfg, key, before);
        cfg.save(dev.home.path()).unwrap();
    }

    fn cfg_of(dev: &Dev) -> Config {
        Config::load(dev.home.path()).unwrap()
    }

    #[test]
    fn a_changed_setting_reaches_the_other_device() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        let mut b = device(provider.path(), 'b');
        customize(&mut a, "max_file_mb", |c| c.max_file_mb = Some(7));

        assert!(!a.engine.sync_settings().unwrap());
        assert!(b.engine.sync_settings().unwrap());
        assert_eq!(cfg_of(&b).max_file_mb, Some(7));
        assert_eq!(b.engine.cfg.max_file_mb, Some(7));
    }

    #[test]
    fn a_fresh_device_does_not_override_peer_settings() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        customize(&mut a, "max_file_mb", |c| c.max_file_mb = Some(7));
        a.engine.sync_settings().unwrap();

        let mut b = device(provider.path(), 'b');
        b.engine.sync_settings().unwrap();
        assert!(!a.engine.sync_settings().unwrap());

        assert_eq!(cfg_of(&a).max_file_mb, Some(7));
        assert_eq!(cfg_of(&b).max_file_mb, Some(7));
    }

    #[test]
    fn resetting_to_builtin_syncs_as_a_clear() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        let mut b = device(provider.path(), 'b');
        customize(&mut a, "max_file_mb", |c| c.max_file_mb = Some(7));
        a.engine.sync_settings().unwrap();
        b.engine.sync_settings().unwrap();
        assert_eq!(cfg_of(&b).max_file_mb, Some(7));

        customize(&mut a, "max_file_mb", |c| c.max_file_mb = None);
        a.engine.sync_settings().unwrap();
        assert!(b.engine.sync_settings().unwrap());
        assert_eq!(cfg_of(&b).max_file_mb, None);
        assert_eq!(cfg_of(&a).max_file_mb, None);
    }

    #[test]
    fn edits_to_different_catalogs_both_survive() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        let mut b = device(provider.path(), 'b');
        let projects = vec!["from-a/**".to_string()];
        let claude = vec!["from-b.json".to_string()];
        let p = projects.clone();
        customize(&mut a, "patterns.projects", |c| {
            c.default_patterns = Some(p)
        });
        let cl = claude.clone();
        customize(&mut b, "patterns.claude", |c| {
            c.agent_patterns.insert("claude".into(), cl);
        });

        for _ in 0..2 {
            a.engine.sync_settings().unwrap();
            b.engine.sync_settings().unwrap();
        }
        for dev in [&a, &b] {
            let cfg = cfg_of(dev);
            assert_eq!(cfg.default_patterns.as_ref(), Some(&projects));
            assert_eq!(cfg.agent_patterns.get("claude"), Some(&claude));
        }
    }

    #[test]
    fn concurrent_edits_to_the_same_key_converge() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        let mut b = device(provider.path(), 'b');
        customize(&mut a, "max_file_mb", |c| c.max_file_mb = Some(7));
        customize(&mut b, "max_file_mb", |c| c.max_file_mb = Some(9));

        for _ in 0..2 {
            a.engine.sync_settings().unwrap();
            b.engine.sync_settings().unwrap();
        }
        let (ca, cb) = (cfg_of(&a), cfg_of(&b));
        assert_eq!(ca.max_file_mb, cb.max_file_mb);
        assert_eq!(ca.settings_gens, cb.settings_gens);
        assert!(ca.max_file_mb.is_some());
    }

    #[test]
    fn an_import_never_touches_existing_roots() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        let mut b = device(provider.path(), 'b');
        write(&b, "CLAUDE.md", b"one\n");
        add(&mut b);
        let project_file = staging(&b).join(project::PROJECT_FILE);
        let ignore_file = staging(&b).join(project::IGNORE_FILE);
        let project_before = fs::read(&project_file).unwrap();
        let ignore_before = fs::read(&ignore_file).unwrap();
        let roots_before = cfg_of(&b).roots;

        customize(&mut a, "patterns.projects", |c| {
            c.default_patterns = Some(vec!["other/**".into()])
        });
        customize(&mut a, "default_ignore", |c| {
            c.default_ignore = Some("*.never\n".into())
        });
        a.engine.sync_settings().unwrap();
        assert!(b.engine.sync_settings().unwrap());

        let cfg = cfg_of(&b);
        assert_eq!(cfg.default_ignore.as_deref(), Some("*.never\n"));
        assert_eq!(cfg.roots, roots_before);
        assert_eq!(fs::read(&project_file).unwrap(), project_before);
        assert_eq!(fs::read(&ignore_file).unwrap(), ignore_before);
        assert!(!b.root.path().join(project::IGNORE_FILE).exists());
    }

    #[test]
    fn an_unchanged_state_publishes_nothing() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        let mut b = device(provider.path(), 'b');
        let a_id = a.engine.cfg.device_id.clone();
        let b_id = b.engine.cfg.device_id.clone();

        // An all-builtin device has nothing to publish.
        b.engine.sync_settings().unwrap();
        assert_eq!(cloud_of(&b).settings_max_seq(&b_id), 0);

        customize(&mut a, "max_file_mb", |c| c.max_file_mb = Some(7));
        a.engine.sync_settings().unwrap();
        assert_eq!(cloud_of(&a).settings_max_seq(&a_id), 1);
        a.engine.sync_settings().unwrap();
        assert_eq!(cloud_of(&a).settings_max_seq(&a_id), 1);
    }

    #[test]
    fn sync_settings_republishes_after_a_wipe() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        let a_id = a.engine.cfg.device_id.clone();
        customize(&mut a, "max_file_mb", |c| c.max_file_mb = Some(7));
        a.engine.sync_settings().unwrap();

        fs::remove_dir_all(provider.path().join("dotlore/_settings")).unwrap();
        a.engine.sync_settings().unwrap();

        let cloud = cloud_of(&a);
        assert_eq!(cloud.settings_max_seq(&a_id), 1);
        let bytes = cloud.newest_settings(&a_id).readable.unwrap();
        let snap = settings::parse(&bytes).unwrap();
        assert_eq!(snap, settings::local(&cfg_of(&a)));
    }

    #[test]
    fn setting_the_same_value_does_not_bump_the_gen() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        let a_id = a.engine.cfg.device_id.clone();
        let claude = lines_owned(project::builtin_lines("claude").unwrap());
        a.engine.set_pattern_catalog("claude", claude).unwrap();
        a.engine
            .set_default_ignore(project::DEFAULT_NEVER_IGNORE.to_string())
            .unwrap();
        a.engine
            .set_sensitive_patterns(lines_owned(project::SECRET_PATTERNS))
            .unwrap();
        assert!(cfg_of(&a).settings_gens.is_empty());

        a.engine.set_max_file_mb(7).unwrap();
        a.engine
            .set_sensitive_patterns(vec![" *.pem ".into(), "*.pem".into()])
            .unwrap();
        let gens = cfg_of(&a).settings_gens;
        assert_eq!(gens.len(), 2);
        assert_eq!(cloud_of(&a).settings_max_seq(&a_id), 2);

        a.engine.set_max_file_mb(7).unwrap();
        a.engine
            .set_sensitive_patterns(vec!["*.pem".into()])
            .unwrap();
        assert_eq!(cfg_of(&a).settings_gens, gens);
        assert_eq!(cloud_of(&a).settings_max_seq(&a_id), 2);
    }

    #[test]
    fn each_setter_bumps_only_its_own_key() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        let mut prev = BTreeMap::new();
        let mut check = |a: &Dev, key: &str| {
            let now = cfg_of(a).settings_gens;
            assert!(now[key] > prev.get(key).copied().unwrap_or(0), "{key}");
            prev.insert(key.to_string(), now[key]);
            assert_eq!(now, prev, "after {key}");
        };
        a.engine
            .set_pattern_catalog("projects", vec!["x/**".into()])
            .unwrap();
        check(&a, "patterns.projects");
        a.engine.set_default_patterns(vec!["y/**".into()]).unwrap();
        check(&a, "patterns.projects");
        a.engine
            .set_pattern_catalog("claude", vec!["c.json".into()])
            .unwrap();
        check(&a, "patterns.claude");
        a.engine
            .set_sensitive_patterns(vec!["*.pem".into()])
            .unwrap();
        check(&a, "sensitive_patterns");
        a.engine.set_default_ignore("*.never\n".into()).unwrap();
        check(&a, "default_ignore");
        a.engine.set_max_file_mb(3).unwrap();
        check(&a, "max_file_mb");
        a.engine.set_max_seed_folder_mb(4).unwrap();
        check(&a, "max_seed_folder_mb");
        a.engine.set_trash_max_mb(300).unwrap();
        check(&a, "trash_max_mb");
    }

    #[test]
    fn a_setter_publishes_a_new_snapshot() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        let a_id = a.engine.cfg.device_id.clone();
        let newest = |a: &Dev| {
            let bytes = cloud_of(a).newest_settings(&a_id).readable.unwrap();
            settings::parse(&bytes).unwrap()
        };

        a.engine.set_max_file_mb(7).unwrap();
        assert_eq!(cloud_of(&a).settings_max_seq(&a_id), 1);
        assert_eq!(newest(&a), settings::local(&cfg_of(&a)));

        a.engine.set_default_ignore("*.never\n".into()).unwrap();
        assert_eq!(cloud_of(&a).settings_max_seq(&a_id), 2);
        let snap = newest(&a);
        assert_eq!(snap, settings::local(&cfg_of(&a)));
        assert_eq!(snap.fields.len(), 2);
    }

    #[test]
    fn a_setter_edit_beats_an_older_peer_value() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        let mut b = device(provider.path(), 'b');
        a.engine.set_max_file_mb(7).unwrap();
        assert!(b.engine.sync_settings().unwrap());
        assert_eq!(cfg_of(&b).max_file_mb, Some(7));

        b.engine.set_max_file_mb(9).unwrap();
        assert!(a.engine.sync_settings().unwrap());
        assert_eq!(cfg_of(&a).max_file_mb, Some(9));
        assert_eq!(cfg_of(&a).settings_gens, cfg_of(&b).settings_gens);
    }

    #[test]
    fn sync_all_imports_peer_settings() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        let mut b = device(provider.path(), 'b');
        customize(&mut a, "max_file_mb", |c| c.max_file_mb = Some(7));
        a.engine.sync_settings().unwrap();

        b.engine.sync_all().unwrap();
        assert_eq!(cfg_of(&b).max_file_mb, Some(7));
    }

    #[test]
    fn configure_provider_imports_settings_from_the_new_folder() {
        let first = TempDir::new().unwrap();
        let second = TempDir::new().unwrap();
        let a = device(first.path(), 'a');
        let mut b = device(second.path(), 'b');
        customize(&mut b, "max_file_mb", |c| c.max_file_mb = Some(7));
        b.engine.sync_settings().unwrap();

        configure_provider(a.home.path(), a.home.path(), second.path()).unwrap();
        assert_eq!(cfg_of(&a).max_file_mb, Some(7));
    }

    #[test]
    fn configure_provider_publishes_local_settings_to_a_fresh_folder() {
        let first = TempDir::new().unwrap();
        let second = TempDir::new().unwrap();
        let mut a = device(first.path(), 'a');
        let a_id = a.engine.cfg.device_id.clone();
        customize(&mut a, "max_file_mb", |c| c.max_file_mb = Some(7));

        configure_provider(a.home.path(), a.home.path(), second.path()).unwrap();
        let dest = Cloud {
            base: second.path().join("dotlore"),
        };
        let bytes = dest.newest_settings(&a_id).readable.unwrap();
        let snap = settings::parse(&bytes).unwrap();
        assert_eq!(snap, settings::local(&cfg_of(&a)));
    }

    #[test]
    fn a_zero_limit_setter_is_rejected() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        assert!(a.engine.set_max_file_mb(0).is_err());
        assert!(a.engine.set_max_seed_folder_mb(0).is_err());
        assert!(a.engine.set_trash_max_mb(0).is_err());
        assert!(a.engine.set_trash_max_mb(trash::MIN_MAX_MB - 1).is_err());
        let cfg = cfg_of(&a);
        assert_eq!(cfg.max_file_mb, None);
        assert_eq!(cfg.max_seed_folder_mb, None);
        assert_eq!(cfg.trash_max_mb, None);
        assert!(cfg.settings_gens.is_empty());
    }

    #[test]
    fn a_reset_to_builtin_via_setter_reaches_the_peer() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        let mut b = device(provider.path(), 'b');
        // A customization from before settings synced: no stored gen.
        a.engine
            .cfg
            .agent_patterns
            .insert("claude".into(), vec!["c.json".into()]);
        a.engine.cfg.save(a.home.path()).unwrap();
        a.engine.sync_settings().unwrap();
        assert!(b.engine.sync_settings().unwrap());
        assert!(cfg_of(&b).agent_patterns.contains_key("claude"));

        let builtin = lines_owned(project::builtin_lines("claude").unwrap());
        a.engine.set_pattern_catalog("claude", builtin).unwrap();
        assert!(b.engine.sync_settings().unwrap());
        assert!(!cfg_of(&b).agent_patterns.contains_key("claude"));
    }

    #[test]
    fn a_publish_failure_does_not_fail_the_setter() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        fs::create_dir_all(provider.path().join("dotlore")).unwrap();
        fs::write(provider.path().join("dotlore/_settings"), b"not a dir").unwrap();

        a.engine.set_max_file_mb(7).unwrap();
        let cfg = cfg_of(&a);
        assert_eq!(cfg.max_file_mb, Some(7));
        assert!(cfg.settings_gens["max_file_mb"] > 0);
    }

    // --- trash size cap -----------------------------------------------------

    /// A sparse `mib` MiB file in trash pass `<day>/<pass>` of proj-claude:
    /// its length counts toward the cap without using the disk.
    fn big_pass(dev: &Dev, day: &str, pass: &str, mib: u64) -> PathBuf {
        let dir = dev
            .home
            .path()
            .join("trash/proj-claude")
            .join(day)
            .join(pass);
        fs::create_dir_all(&dir).unwrap();
        File::create(dir.join("big.bin"))
            .unwrap()
            .set_len(mib * 1024 * 1024)
            .unwrap();
        dir
    }

    /// Three 60 MiB passes, oldest first: 180 MiB, under the 500 MB default.
    fn three_passes(dev: &Dev) -> [PathBuf; 3] {
        [
            big_pass(dev, "2026-01-01", "100000", 60),
            big_pass(dev, "2026-01-02", "100000", 60),
            big_pass(dev, "2026-01-03", "100000", 60),
        ]
    }

    /// Set the cap in config.json only, bypassing the setter's own prune.
    fn cap_in_config(dev: &mut Dev, mb: u64) {
        let mut cfg = cfg_of(dev);
        cfg.trash_max_mb = Some(mb);
        cfg.save(dev.home.path()).unwrap();
    }

    #[test]
    fn lowering_the_trash_cap_prunes_the_oldest_passes() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        let [p1, p2, p3] = three_passes(&a);
        assert_eq!(a.engine.trash_max_mb(), trash::DEFAULT_MAX_MB);

        a.engine.set_trash_max_mb(100).unwrap();
        assert_eq!(a.engine.trash_max_mb(), 100);
        // 2 x 60 MiB is still over 100 MiB, so only the newest pass stays.
        assert!(!p1.exists());
        assert!(!p2.exists());
        assert!(p3.join("big.bin").is_file());
    }

    #[test]
    fn an_imported_trash_cap_prunes_the_local_trash() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        let mut b = device(provider.path(), 'b');
        a.engine.set_trash_max_mb(100).unwrap();
        let [p1, p2, p3] = three_passes(&b);

        assert!(b.engine.sync_settings().unwrap());
        assert_eq!(cfg_of(&b).trash_max_mb, Some(100));
        assert!(!p1.exists());
        assert!(!p2.exists());
        assert!(p3.join("big.bin").is_file());
    }

    /// A on CLAUDE.md "one", B linked and in sync, both old 60 MiB passes
    /// planted in B's trash and B's cap at 100 MB.
    fn linked_pair_with_old_trash(provider: &Path) -> (Dev, Dev, [PathBuf; 2]) {
        let mut a = device(provider, 'a');
        write(&a, "CLAUDE.md", b"one\n");
        add(&mut a);
        let mut b = device(provider, 'b');
        let b_root = b.root.path().to_path_buf();
        b.engine.link_root("proj-claude", &b_root).unwrap();
        let old = [
            big_pass(&b, "2026-01-01", "100000", 60),
            big_pass(&b, "2026-01-02", "100000", 60),
        ];
        cap_in_config(&mut b, 100);
        (a, b, old)
    }

    #[test]
    fn a_cycle_that_writes_trash_prunes_to_the_cap() {
        let provider = TempDir::new().unwrap();
        let (mut a, mut b, [p1, p2]) = linked_pair_with_old_trash(provider.path());
        write(&a, "CLAUDE.md", b"two\n");
        assert_eq!(
            a.engine.sync_root("proj-claude").unwrap(),
            RootStatus::Synced
        );

        // The overwrite keeps "one" in a new pass, putting B over the cap.
        assert_eq!(
            b.engine.sync_root("proj-claude").unwrap(),
            RootStatus::Synced
        );
        assert_eq!(fs::read(b.root.path().join("CLAUDE.md")).unwrap(), b"two\n");
        let trash = b.home.path().join("trash/proj-claude");
        // 120 MiB plus the new pass: the oldest goes, then it fits.
        assert!(!p1.exists());
        assert!(p2.join("big.bin").is_file());
        let kept = files_in(&trash);
        assert_eq!(kept.len(), 2, "{kept:?}");
        assert!(kept.iter().any(|f| f.ends_with("/CLAUDE.md")), "{kept:?}");
    }

    #[test]
    fn a_captured_local_deletion_prunes_to_the_cap() {
        let provider = TempDir::new().unwrap();
        let (_a, mut b, [p1, p2]) = linked_pair_with_old_trash(provider.path());
        fs::remove_file(b.root.path().join("CLAUDE.md")).unwrap();

        // The capture keeps "one" in a new pass, putting B over the cap.
        assert_eq!(
            b.engine.sync_root("proj-claude").unwrap(),
            RootStatus::Synced
        );
        assert!(!b.root.path().join("CLAUDE.md").exists());
        // 120 MiB plus the new pass: the oldest goes, then it fits.
        assert!(!p1.exists());
        assert!(p2.join("big.bin").is_file());
        let kept = files_in(&b.home.path().join("trash/proj-claude"));
        assert_eq!(kept.len(), 2, "{kept:?}");
        assert!(kept.iter().any(|f| f.ends_with("/CLAUDE.md")), "{kept:?}");
    }

    #[test]
    fn a_cycle_without_trash_writes_does_not_prune() {
        let provider = TempDir::new().unwrap();
        let (mut a, mut b, [p1, p2]) = linked_pair_with_old_trash(provider.path());
        // An add-only change: nothing on B is overwritten or deleted.
        write(&a, "notes.md", b"new\n");
        a.engine
            .track_entry("proj-claude", Path::new("notes.md"), None)
            .unwrap();

        assert_eq!(
            b.engine.sync_root("proj-claude").unwrap(),
            RootStatus::Synced
        );
        assert_eq!(fs::read(b.root.path().join("notes.md")).unwrap(), b"new\n");
        // Over the cap, but no cycle wrote trash.
        assert!(p1.join("big.bin").is_file());
        assert!(p2.join("big.bin").is_file());
    }

    #[test]
    fn a_resumed_apply_that_writes_trash_prunes_to_the_cap() {
        let provider = TempDir::new().unwrap();
        let (mut a, mut b, [p1, p2]) = linked_pair_with_old_trash(provider.path());
        write(&a, "CLAUDE.md", b"two\n");
        assert_eq!(
            a.engine.sync_root("proj-claude").unwrap(),
            RootStatus::Synced
        );

        // B's CLAUDE.md is a symlink for now: the apply stalls on it.
        let live = b.root.path().join("CLAUDE.md");
        let elsewhere = b.home.path().join("elsewhere.md");
        fs::write(&elsewhere, b"one\n").unwrap();
        fs::remove_file(&live).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &live).unwrap();
        let status = b.engine.sync_root("proj-claude").unwrap();
        assert_ne!(status, RootStatus::Synced);
        assert!(p1.join("big.bin").is_file(), "nothing was trashed yet");
        assert!(p2.join("big.bin").is_file());

        // The real file is back: the next cycle resumes and overwrites it.
        fs::remove_file(&live).unwrap();
        fs::write(&live, b"one\n").unwrap();
        assert_eq!(
            b.engine.sync_root("proj-claude").unwrap(),
            RootStatus::Synced
        );
        assert_eq!(fs::read(&live).unwrap(), b"two\n");
        assert!(!p1.exists());
        assert!(p2.join("big.bin").is_file());
        let kept = files_in(&b.home.path().join("trash/proj-claude"));
        assert_eq!(kept.len(), 2, "{kept:?}");
        assert!(kept.iter().any(|f| f.ends_with("/CLAUDE.md")), "{kept:?}");
    }

    // --- restore_from_trash -------------------------------------------------

    const TRASH_PASS: &str = "trash/proj-claude/2026-10-13/120000";

    /// Plant a 0600 trash copy, as `mirror::keep_in_trash` writes them.
    fn plant(dev: &Dev, rel: &str, body: &[u8]) -> PathBuf {
        let p = dev.home.path().join(TRASH_PASS).join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, body).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
        p
    }

    fn restore(dev: &mut Dev, rel: Option<&str>, choice: RestoreChoice) -> Result<RestoreResult> {
        dev.engine
            .restore_from_trash("proj-claude", "2026-10-13", "120000", rel, choice)
    }

    /// Every plain file in `dev`'s trash; empty when there is no trash yet.
    fn trash_files(dev: &Dev) -> Vec<String> {
        let trash = dev.home.path().join("trash");
        if trash.exists() {
            files_in(&trash)
        } else {
            Vec::new()
        }
    }

    /// Every plain file under `dir`, `/`-joined and sorted.
    fn files_in(dir: &Path) -> Vec<String> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in fs::read_dir(&d).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    let rel = p.strip_prefix(dir).unwrap();
                    out.push(rel.to_string_lossy().into_owned());
                }
            }
        }
        out.sort();
        out
    }

    #[test]
    fn restore_from_trash_restores_a_pass_into_the_live_root() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        write(&a, "docs/same.md", b"same\n");
        add(&mut a);
        plant(&a, "docs/gone.md", b"gone\n");
        plant(&a, "docs/deep/x.md", b"x\n");
        plant(&a, "docs/same.md", b"same\n");
        plant(&a, "other.md", b"other\n");

        // A folder inside the pass restores only what is under it.
        let got = restore(&mut a, Some("docs/deep"), RestoreChoice::Ask).unwrap();
        assert_eq!(
            got,
            RestoreResult::Done {
                restored: vec![RestoredFile {
                    from: "docs/deep/x.md".into(),
                    to: "docs/deep/x.md".into(),
                }],
                unchanged: vec![],
                skipped: vec![],
                tracked: vec![],
                kept_local: Vec::new(),
                track_error: None,
            }
        );
        assert!(!a.root.path().join("docs/gone.md").exists());

        // The whole pass; restored copies stay in the trash.
        let got = restore(&mut a, None, RestoreChoice::Ask).unwrap();
        let RestoreResult::Done {
            restored,
            unchanged,
            skipped,
            tracked,
            kept_local,
            track_error,
        } = got
        else {
            panic!("expected Done, got {got:?}");
        };
        assert!(kept_local.is_empty());
        let tos: Vec<&str> = restored.iter().map(|r| r.to.as_str()).collect();
        assert_eq!(tos, ["docs/gone.md", "other.md"]);
        assert_eq!(unchanged, ["docs/deep/x.md", "docs/same.md"]);
        assert!(skipped.is_empty());
        assert!(tracked.is_empty());
        assert_eq!(track_error, None);
        let root = a.root.path();
        assert_eq!(fs::read(root.join("docs/gone.md")).unwrap(), b"gone\n");
        assert_eq!(fs::read(root.join("other.md")).unwrap(), b"other\n");
        assert_eq!(
            files_in(&a.home.path().join(TRASH_PASS)),
            ["docs/deep/x.md", "docs/gone.md", "docs/same.md", "other.md"]
        );

        // An unknown item, or an empty pass, is an error.
        assert!(restore(&mut a, Some("nope.md"), RestoreChoice::Ask).is_err());
        assert!(restore(&mut a, Some("../x"), RestoreChoice::Ask).is_err());
        fs::create_dir_all(a.home.path().join(TRASH_PASS).join("empty")).unwrap();
        assert!(restore(&mut a, Some("empty"), RestoreChoice::Ask).is_err());
    }

    #[test]
    fn restore_from_trash_ask_reports_existing_and_writes_nothing() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        write(&a, "docs/differs.md", b"live\n");
        add(&mut a);
        plant(&a, "docs/differs.md", b"old\n");
        plant(&a, "docs/missing.md", b"missing\n");

        let got = restore(&mut a, None, RestoreChoice::Ask).unwrap();

        assert_eq!(
            got,
            RestoreResult::Existing {
                rels: vec!["docs/differs.md".into()],
            }
        );
        assert!(!a.root.path().join("docs/missing.md").exists());
        assert_eq!(
            fs::read(a.root.path().join("docs/differs.md")).unwrap(),
            b"live\n"
        );
        assert_eq!(
            files_in(&a.home.path().join("trash")),
            [
                "proj-claude/2026-10-13/120000/docs/differs.md",
                "proj-claude/2026-10-13/120000/docs/missing.md",
            ]
        );
    }

    #[test]
    fn restore_from_trash_replace_keeps_the_displaced_live_file() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        write(&a, "docs/a.md", b"live\n");
        add(&mut a);
        plant(&a, "docs/a.md", b"old\n");

        let got = restore(&mut a, Some("docs/a.md"), RestoreChoice::Replace).unwrap();

        assert_eq!(
            got,
            RestoreResult::Done {
                restored: vec![RestoredFile {
                    from: "docs/a.md".into(),
                    to: "docs/a.md".into(),
                }],
                unchanged: vec![],
                skipped: vec![],
                tracked: vec![],
                kept_local: Vec::new(),
                track_error: None,
            }
        );
        assert_eq!(fs::read(a.root.path().join("docs/a.md")).unwrap(), b"old\n");
        // The displaced live file is in a new pass; the source stays too.
        let trash = a.home.path().join("trash/proj-claude");
        let kept: Vec<String> = files_in(&trash)
            .into_iter()
            .filter(|f| !f.starts_with("2026-10-13/120000/"))
            .collect();
        assert_eq!(kept.len(), 1, "{kept:?}");
        assert!(kept[0].ends_with("/docs/a.md"), "{kept:?}");
        let kept = trash.join(&kept[0]);
        assert_eq!(fs::read(&kept).unwrap(), b"live\n");
        assert_eq!(
            fs::metadata(&kept).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(trash.join("2026-10-13/120000/docs/a.md").is_file());

        // Keep both leaves the live file and writes a sibling.
        let got = restore(&mut a, Some("docs/a.md"), RestoreChoice::Ask).unwrap();
        assert!(matches!(got, RestoreResult::Done { ref unchanged, .. } if unchanged.len() == 1));
        write(&a, "docs/a.md", b"newer\n");
        let got = restore(&mut a, Some("docs/a.md"), RestoreChoice::KeepBoth).unwrap();
        let RestoreResult::Done { restored, .. } = got else {
            panic!("expected Done, got {got:?}");
        };
        assert_eq!(
            restored,
            [RestoredFile {
                from: "docs/a.md".into(),
                to: "docs/a.restored.md".into(),
            }]
        );
        assert_eq!(
            fs::read(a.root.path().join("docs/a.md")).unwrap(),
            b"newer\n"
        );
    }

    #[test]
    fn keep_both_tracks_a_restored_copy_outside_the_include_list() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"live\n");
        write(&a, "docs/a.md", b"live a\n");
        add(&mut a);
        plant(&a, "CLAUDE.md", b"old\n");

        let got = restore(&mut a, Some("CLAUDE.md"), RestoreChoice::KeepBoth).unwrap();

        let RestoreResult::Done {
            restored, tracked, ..
        } = got
        else {
            panic!("expected Done, got {got:?}");
        };
        assert_eq!(
            restored,
            [RestoredFile {
                from: "CLAUDE.md".into(),
                to: "CLAUDE.restored.md".into(),
            }]
        );
        assert_eq!(tracked, ["CLAUDE.restored.md"]);
        let repo = a
            .engine
            .repo_for(&a.engine.root_cfg("proj-claude").unwrap())
            .unwrap();
        let file = repo.project_file().unwrap();
        assert_eq!(file.entries["CLAUDE.restored.md"].state, State::Tracked);
        // The restore ran a cycle, so the copy is already staged.
        assert_eq!(
            repo.git
                .ok(&["show", "HEAD:CLAUDE.restored.md"])
                .unwrap()
                .trim(),
            "old"
        );
        assert!(rels(&a.engine.tracked_files("proj-claude").unwrap())
            .contains(&"CLAUDE.restored.md".to_string()));
    }

    #[test]
    fn keep_both_reports_a_tracking_failure_and_keeps_the_written_copy() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"live\n");
        add(&mut a);
        plant(&a, "CLAUDE.md", b"old\n");
        // An unreadable staging `.dotloreproject` breaks only the tracking step.
        let project = a.home.path().join("repos/proj-claude/.dotloreproject");
        fs::remove_file(&project).unwrap();
        fs::create_dir(&project).unwrap();

        let got = restore(&mut a, Some("CLAUDE.md"), RestoreChoice::KeepBoth).unwrap();

        let RestoreResult::Done {
            restored,
            tracked,
            track_error,
            ..
        } = got
        else {
            panic!("expected Done, got {got:?}");
        };
        assert_eq!(
            restored,
            [RestoredFile {
                from: "CLAUDE.md".into(),
                to: "CLAUDE.restored.md".into(),
            }]
        );
        assert!(tracked.is_empty(), "{tracked:?}");
        assert!(track_error.is_some());
        let root = a.root.path();
        assert_eq!(fs::read(root.join("CLAUDE.restored.md")).unwrap(), b"old\n");
        assert_eq!(fs::read(root.join("CLAUDE.md")).unwrap(), b"live\n");
    }

    #[test]
    fn keep_both_never_tracks_a_sensitive_copy() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        write(&a, "credentials.json", b"live\n");
        add(&mut a);
        plant(&a, "credentials.json", b"old\n");

        let got = restore(&mut a, Some("credentials.json"), RestoreChoice::KeepBoth).unwrap();

        let RestoreResult::Done {
            tracked,
            kept_local,
            track_error,
            ..
        } = got
        else {
            panic!("expected Done, got {got:?}");
        };
        assert!(tracked.is_empty(), "{tracked:?}");
        assert_eq!(kept_local, ["credentials.restored.json"]);
        assert_eq!(track_error, None);
        let root = a.root.path();
        assert_eq!(
            fs::read(root.join("credentials.restored.json")).unwrap(),
            b"old\n"
        );
        let repo = a
            .engine
            .repo_for(&a.engine.root_cfg("proj-claude").unwrap())
            .unwrap();
        let entries = repo.project_file().unwrap().tracked();
        assert!(!entries.contains_rel(Path::new("credentials.restored.json")));
    }

    #[test]
    fn keep_both_never_tracks_a_copy_of_an_exact_name_secret() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        write(&a, "auth.json", b"live\n");
        add(&mut a);
        plant(&a, "auth.json", b"old\n");

        let got = restore(&mut a, Some("auth.json"), RestoreChoice::KeepBoth).unwrap();

        let RestoreResult::Done {
            tracked,
            kept_local,
            ..
        } = got
        else {
            panic!("expected Done, got {got:?}");
        };
        assert!(tracked.is_empty(), "{tracked:?}");
        assert_eq!(kept_local, ["auth.restored.json"]);
        let repo = a
            .engine
            .repo_for(&a.engine.root_cfg("proj-claude").unwrap())
            .unwrap();
        let entries = repo.project_file().unwrap().tracked();
        assert!(!entries.contains_rel(Path::new("auth.restored.json")));
    }

    #[test]
    fn keep_both_never_tracks_a_copy_the_user_untracked() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"live\n");
        add(&mut a);
        plant(&a, "CLAUDE.md", b"old\n");
        restore(&mut a, Some("CLAUDE.md"), RestoreChoice::KeepBoth).unwrap();
        a.engine
            .untrack_entry("proj-claude", Path::new("CLAUDE.restored.md"))
            .unwrap();
        fs::remove_file(a.root.path().join("CLAUDE.restored.md")).unwrap();

        let got = restore(&mut a, Some("CLAUDE.md"), RestoreChoice::KeepBoth).unwrap();

        let RestoreResult::Done {
            restored,
            tracked,
            kept_local,
            ..
        } = got
        else {
            panic!("expected Done, got {got:?}");
        };
        assert_eq!(restored[0].to, "CLAUDE.restored.md");
        assert!(tracked.is_empty(), "{tracked:?}");
        assert_eq!(kept_local, ["CLAUDE.restored.md"]);
        let repo = a
            .engine
            .repo_for(&a.engine.root_cfg("proj-claude").unwrap())
            .unwrap();
        let file = repo.project_file().unwrap();
        assert_eq!(file.entries["CLAUDE.restored.md"].state, State::Removed);
    }

    fn kept_local_after_untrack(untracked: &str, rel: &str) -> (Vec<String>, Vec<String>, State) {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        write(&a, rel, b"live\n");
        add(&mut a);
        a.engine
            .untrack_entry("proj-claude", Path::new(untracked))
            .unwrap();
        plant(&a, rel, b"old\n");

        let got = restore(&mut a, Some(rel), RestoreChoice::KeepBoth).unwrap();

        let RestoreResult::Done {
            tracked,
            kept_local,
            ..
        } = got
        else {
            panic!("expected Done, got {got:?}");
        };
        let repo = a
            .engine
            .repo_for(&a.engine.root_cfg("proj-claude").unwrap())
            .unwrap();
        let state = repo.project_file().unwrap().entries[&format!("{untracked}/")].state;
        (tracked, kept_local, state)
    }

    #[test]
    fn keep_both_never_tracks_a_copy_inside_an_untracked_folder() {
        let (tracked, kept_local, state) = kept_local_after_untrack("docs", "docs/a.md");
        assert!(tracked.is_empty(), "{tracked:?}");
        assert_eq!(kept_local, ["docs/a.restored.md"]);
        assert_eq!(state, State::Removed);
    }

    #[test]
    fn keep_both_never_tracks_a_copy_inside_a_hole_of_a_tracked_folder() {
        let (tracked, kept_local, state) =
            kept_local_after_untrack("docs/private", "docs/private/x.md");
        assert!(tracked.is_empty(), "{tracked:?}");
        assert_eq!(kept_local, ["docs/private/x.restored.md"]);
        assert_eq!(state, State::Removed);
    }

    #[test]
    fn a_restore_without_a_restored_copy_never_reads_the_project_file() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"live\n");
        add(&mut a);
        plant(&a, "CLAUDE.md", b"old\n");
        let project = a.home.path().join("repos/proj-claude/.dotloreproject");
        fs::remove_file(&project).unwrap();
        fs::create_dir(&project).unwrap();

        let got = restore(&mut a, Some("CLAUDE.md"), RestoreChoice::Replace).unwrap();

        let RestoreResult::Done { track_error, .. } = got else {
            panic!("expected Done, got {got:?}");
        };
        assert_eq!(track_error, None);
        assert_eq!(fs::read(a.root.path().join("CLAUDE.md")).unwrap(), b"old\n");
    }

    #[test]
    fn keep_both_under_a_tracked_folder_adds_no_entry() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        write(&a, "docs/a.md", b"live\n");
        add(&mut a);
        plant(&a, "docs/a.md", b"old\n");
        let repo = a
            .engine
            .repo_for(&a.engine.root_cfg("proj-claude").unwrap())
            .unwrap();
        let before = repo.project_file().unwrap();

        let got = restore(&mut a, Some("docs/a.md"), RestoreChoice::KeepBoth).unwrap();

        assert!(
            matches!(got, RestoreResult::Done { ref tracked, ref restored, .. }
                if tracked.is_empty() && restored[0].to == "docs/a.restored.md"),
            "{got:?}"
        );
        assert_eq!(repo.project_file().unwrap(), before);
    }

    #[test]
    fn restore_without_a_restored_copy_adds_no_entry() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        add(&mut a);
        plant(&a, "other.md", b"other\n");
        let repo = a
            .engine
            .repo_for(&a.engine.root_cfg("proj-claude").unwrap())
            .unwrap();
        let before = repo.project_file().unwrap();

        let got = restore(&mut a, None, RestoreChoice::KeepBoth).unwrap();

        assert!(
            matches!(got, RestoreResult::Done { ref tracked, ref restored, .. }
                if tracked.is_empty() && restored[0].to == "other.md"),
            "{got:?}"
        );
        assert_eq!(repo.project_file().unwrap(), before);
    }

    #[test]
    fn restore_from_trash_refuses_a_removed_project() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        add(&mut a);
        plant(&a, "gone.md", b"gone\n");
        a.engine.remove_root("proj-claude").unwrap();

        let err = restore(&mut a, None, RestoreChoice::Replace).unwrap_err();

        assert!(err.to_string().contains("removed"), "{err:#}");
        assert!(!a.root.path().join("gone.md").exists());
    }

    #[test]
    fn restore_from_trash_refuses_a_missing_root_folder() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        add(&mut a);
        plant(&a, "gone.md", b"gone\n");
        let root = a.root.path().to_path_buf();
        fs::remove_dir_all(&root).unwrap();

        let err = restore(&mut a, None, RestoreChoice::Replace).unwrap_err();

        assert!(err.to_string().contains("missing"), "{err:#}");
        assert!(!root.exists(), "the project folder must not be recreated");
    }

    #[test]
    fn restore_from_trash_refuses_an_initializing_root() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        add(&mut a);
        plant(&a, "gone.md", b"gone\n");
        let mut cfg = Config::load(a.home.path()).unwrap();
        cfg.roots[0].initializing = true;
        cfg.save(a.home.path()).unwrap();

        let err = restore(&mut a, None, RestoreChoice::Replace).unwrap_err();

        assert!(err.to_string().contains("linking"), "{err:#}");
        assert!(!a.root.path().join("gone.md").exists());
    }

    #[test]
    fn restored_file_is_published_on_the_next_cycle() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        write(&a, "docs/a.md", b"a\n");
        add(&mut a);
        plant(&a, "docs/notes.md", b"notes\n");

        restore(&mut a, None, RestoreChoice::Ask).unwrap();
        assert_eq!(
            a.engine.sync_root("proj-claude").unwrap(),
            RootStatus::Synced
        );

        let repo = a
            .engine
            .repo_for(&a.engine.root_cfg("proj-claude").unwrap())
            .unwrap();
        assert_eq!(
            repo.git.ok(&["show", "HEAD:docs/notes.md"]).unwrap().trim(),
            "notes"
        );
        assert_eq!(cloud_of(&a).list_bundles("proj-claude").len(), 2);
    }

    #[test]
    fn restore_types_serialize_for_the_webview() {
        let choice: RestoreChoice = serde_json::from_str(r#""keep_both""#).unwrap();
        assert_eq!(choice, RestoreChoice::KeepBoth);
        assert_eq!(
            serde_json::to_string(&RestoreResult::Existing {
                rels: vec!["a.md".into()]
            })
            .unwrap(),
            r#"{"kind":"existing","rels":["a.md"]}"#
        );
        assert_eq!(
            serde_json::to_string(&RestoreResult::Done {
                restored: vec![RestoredFile {
                    from: "a.md".into(),
                    to: "a.restored.md".into()
                }],
                unchanged: vec![],
                skipped: vec![],
                tracked: vec![],
                kept_local: Vec::new(),
                track_error: None,
            })
            .unwrap(),
            r#"{"kind":"done","restored":[{"from":"a.md","to":"a.restored.md"}],"unchanged":[],"skipped":[],"tracked":[],"kept_local":[],"track_error":null}"#
        );
    }

    // --- history ------------------------------------------------------------

    fn history(dev: &mut Dev, rel: &str) -> Vec<HistoryVersion> {
        dev.engine.file_history("proj-claude", rel).unwrap()
    }

    fn restore_version(
        dev: &mut Dev,
        rel: &str,
        commit: &str,
        choice: RestoreChoice,
    ) -> Result<RestoreResult> {
        dev.engine
            .restore_from_history("proj-claude", rel, commit, choice)
            .map_err(|e| anyhow!("{e:#}"))
    }

    fn done(rel: &str, to: &str) -> RestoreResult {
        RestoreResult::Done {
            restored: vec![RestoredFile {
                from: rel.into(),
                to: to.into(),
            }],
            unchanged: vec![],
            skipped: vec![],
            tracked: vec![],
            kept_local: Vec::new(),
            track_error: None,
        }
    }

    /// A on `docs/x.md` "v1", then the user deletes it and A syncs.
    fn deleted_x(provider: &Path) -> Dev {
        let mut a = device(provider, 'a');
        write(&a, "CLAUDE.md", b"one\n");
        write(&a, "docs/x.md", b"v1\n");
        add(&mut a);
        fs::remove_file(a.root.path().join("docs/x.md")).unwrap();
        assert_eq!(
            a.engine.sync_root("proj-claude").unwrap(),
            RootStatus::Synced
        );
        a
    }

    #[test]
    fn file_history_lists_versions_newest_first_with_device_names() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        write(&a, "docs/x.md", b"v1\n");
        add(&mut a);
        let mut b = device(provider.path(), 'b');
        let b_root = b.root.path().to_path_buf();
        b.engine.link_root("proj-claude", &b_root).unwrap();
        write(&b, "docs/x.md", b"v2\n");
        assert_eq!(
            b.engine.sync_root("proj-claude").unwrap(),
            RootStatus::Synced
        );
        assert_eq!(
            a.engine.sync_root("proj-claude").unwrap(),
            RootStatus::Synced
        );

        let got = history(&mut a, "docs/x.md");

        let names: Vec<&str> = got.iter().map(|v| v.device_name.as_str()).collect();
        assert_eq!(names, ["Mac b Pro", "Mac a Pro"], "{got:?}");
        assert!(got.iter().all(|v| !v.deleted && v.bytes_len.is_none()));
        assert!(got.iter().all(|v| repo::is_commit_id(&v.commit)));
        assert!(got[0].time >= got[1].time && got[1].time > 0);
        // Untracked or unsafe rels have no history to show.
        assert!(a.engine.file_history("proj-claude", "../x").is_err());
        assert!(a.engine.file_history("nope", "docs/x.md").is_err());
    }

    #[test]
    fn history_deleted_files_lists_a_file_the_user_deleted() {
        let provider = TempDir::new().unwrap();
        let mut a = deleted_x(provider.path());

        let versions = history(&mut a, "docs/x.md");
        let got = a.engine.deleted_files("proj-claude").unwrap();

        assert_eq!(versions.len(), 2, "{versions:?}");
        assert!(versions[0].deleted && !versions[1].deleted);
        assert_eq!(
            got,
            [DeletedFile {
                rel: "docs/x.md".into(),
                commit: versions[0].commit.clone(),
                deleted_at: versions[0].time,
                device_name: "Mac a Pro".into(),
            }]
        );
    }

    #[test]
    fn restore_from_history_writes_a_deleted_file_and_the_next_sync_publishes_it() {
        let provider = TempDir::new().unwrap();
        let mut a = deleted_x(provider.path());
        let versions = history(&mut a, "docs/x.md");
        let bundles = cloud_of(&a).list_bundles("proj-claude").len();

        let got = restore_version(&mut a, "docs/x.md", &versions[1].commit, RestoreChoice::Ask);

        assert_eq!(got.unwrap(), done("docs/x.md", "docs/x.md"));
        assert_eq!(fs::read(a.root.path().join("docs/x.md")).unwrap(), b"v1\n");
        let statuses = a.engine.sync_all().unwrap();
        assert_eq!(statuses, [("proj-claude".into(), RootStatus::Synced)]);
        let repo = a
            .engine
            .repo_for(&a.engine.root_cfg("proj-claude").unwrap())
            .unwrap();
        assert_eq!(
            repo.git.ok(&["show", "refs/heads/main:docs/x.md"]).unwrap(),
            "v1"
        );
        assert_eq!(cloud_of(&a).list_bundles("proj-claude").len(), bundles + 1);
        assert!(a.engine.deleted_files("proj-claude").unwrap().is_empty());
    }

    #[test]
    fn restore_from_history_ask_replace_and_keep_both() {
        let provider = TempDir::new().unwrap();
        let mut a = device(provider.path(), 'a');
        write(&a, "CLAUDE.md", b"one\n");
        write(&a, "docs/x.md", b"v1\n");
        add(&mut a);
        write(&a, "docs/x.md", b"v2\n");
        a.engine.sync_root("proj-claude").unwrap();
        let versions = history(&mut a, "docs/x.md");
        let v1 = versions[1].commit.clone();
        let live = a.root.path().join("docs/x.md");

        // Ask: the live file differs, nothing is written.
        let got = restore_version(&mut a, "docs/x.md", &v1, RestoreChoice::Ask).unwrap();
        assert_eq!(
            got,
            RestoreResult::Existing {
                rels: vec!["docs/x.md".into()]
            }
        );
        assert_eq!(fs::read(&live).unwrap(), b"v2\n");
        assert!(trash_files(&a).is_empty());

        // Replace: the live file goes to the trash first.
        let got = restore_version(&mut a, "docs/x.md", &v1, RestoreChoice::Replace).unwrap();
        assert_eq!(got, done("docs/x.md", "docs/x.md"));
        assert_eq!(fs::read(&live).unwrap(), b"v1\n");
        let kept = trash_files(&a);
        assert_eq!(kept.len(), 1, "{kept:?}");
        assert!(kept[0].ends_with("/docs/x.md"), "{kept:?}");
        assert_eq!(
            fs::read(a.home.path().join("trash").join(&kept[0])).unwrap(),
            b"v2\n"
        );
        assert!(a.engine.trash_written.load(Ordering::Relaxed));

        // Keep both: the live file stays, a sibling gets the old bytes.
        write(&a, "docs/x.md", b"v3\n");
        let got = restore_version(&mut a, "docs/x.md", &v1, RestoreChoice::KeepBoth).unwrap();
        assert_eq!(got, done("docs/x.md", "docs/x.restored.md"));
        assert_eq!(fs::read(&live).unwrap(), b"v3\n");
        assert_eq!(
            fs::read(a.root.path().join("docs/x.restored.md")).unwrap(),
            b"v1\n"
        );
    }

    #[test]
    fn restore_from_history_refuses_bad_commits_rels_and_deleted_versions() {
        let provider = TempDir::new().unwrap();
        let mut a = deleted_x(provider.path());
        write(&a, "README.md", b"readme\n");
        let versions = history(&mut a, "docs/x.md");
        let v1 = versions[1].commit.clone();
        let repo = a
            .engine
            .repo_for(&a.engine.root_cfg("proj-claude").unwrap())
            .unwrap();
        let tree = repo.git.ok(&["rev-parse", "HEAD^{tree}"]).unwrap();
        let foreign = repo
            .git
            .ok(&["commit-tree", "-m", "foreign", &tree])
            .unwrap();

        let refused = |a: &mut Dev, rel: &str, commit: &str, want: &str| {
            let err = restore_version(a, rel, commit, RestoreChoice::Replace).unwrap_err();
            assert!(err.to_string().contains(want), "{rel} {commit}: {err:#}");
        };
        refused(&mut a, "docs/x.md", "HEAD", "commit");
        refused(&mut a, "docs/x.md", &v1[..8], "commit");
        refused(&mut a, "docs/x.md", &"z".repeat(40), "commit");
        refused(&mut a, "docs/x.md", &"0".repeat(40), "history");
        refused(&mut a, "docs/x.md", &foreign, "history");
        refused(&mut a, "README.md", &v1, "not tracked");
        refused(&mut a, "../x.md", &v1, "unsafe");
        refused(&mut a, "/etc/hosts", &v1, "unsafe");
        refused(&mut a, "docs/x.md", &versions[0].commit, "deleted");

        assert!(!a.root.path().join("docs/x.md").exists());
        assert!(!a.root.path().join("x.md").exists());
        assert!(trash_files(&a).iter().all(|f| !f.ends_with("README.md")));
    }

    #[test]
    fn restore_from_history_refuses_a_removed_project() {
        let provider = TempDir::new().unwrap();
        let mut a = deleted_x(provider.path());
        let v1 = history(&mut a, "docs/x.md")[1].commit.clone();
        a.engine.remove_root("proj-claude").unwrap();

        let err = restore_version(&mut a, "docs/x.md", &v1, RestoreChoice::Replace).unwrap_err();

        assert!(err.to_string().contains("removed"), "{err:#}");
        assert!(!a.root.path().join("docs/x.md").exists());
    }

    #[test]
    fn read_history_file_respects_the_cap_and_refuses_a_deleted_version() {
        let provider = TempDir::new().unwrap();
        let mut a = deleted_x(provider.path());
        let versions = history(&mut a, "docs/x.md");
        let v1 = versions[1].commit.clone();

        let read = |a: &mut Dev, commit: &str, cap: u64| {
            a.engine
                .read_history_file("proj-claude", "docs/x.md", commit, cap)
        };
        assert_eq!(
            read(&mut a, &v1, 1024).unwrap(),
            HistoryFile {
                bytes_len: 3,
                bytes: Some(b"v1\n".to_vec()),
            }
        );
        assert_eq!(
            read(&mut a, &v1, 2).unwrap(),
            HistoryFile {
                bytes_len: 3,
                bytes: None,
            }
        );
        let err = read(&mut a, &versions[0].commit, 1024).unwrap_err();
        assert!(format!("{err:#}").contains("deleted"), "{err:#}");
        assert!(read(&mut a, &"0".repeat(40), 1024).is_err());
        assert!(a
            .engine
            .read_history_file("proj-claude", "../x", &v1, 1024)
            .is_err());
    }
}
