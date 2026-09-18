//! The sync cycle: add, link, sync, resolve, recover.
//!
//! This is the orchestration layer the CLI and the menu-bar app both call.
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

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};

use crate::cloud::{Cloud, Kind, Manifest};
use crate::config::{self, Config, HomeLock, Root};
use crate::conflict;
use crate::git::{self, Git};
use crate::mirror::FileState;
use crate::repo::{
    provider_key, remote_ref, unique_id, FetchMode, Repo, ResumeOutcome, Transaction,
};

/// Where one tracked root stands after a cycle.
///
/// `Pending` is not an error: a linked root whose staging has no `main` yet
/// because no bundle is readable in the cloud, or an apply that raced a live
/// edit, is retried on the next cycle with its journal intact.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum RootStatus {
    Synced,
    Conflicts(usize),
    Pending,
    RootMissing,
    GitMissing,
    Error(String),
}

/// One conflict sibling, ready to display.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ConflictView {
    pub live: PathBuf,
    pub sibling: PathBuf,
    pub loser_id8: String,
    /// From `devices/<id>/device.json`; the id8 itself when unknown.
    pub loser_name: String,
    pub loser_is_me: bool,
}

/// One side of a resolution, pinned by the blob the UI actually showed.
#[derive(Clone, PartialEq, Eq, Debug)]
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

/// Outcome of a save from the resolver.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ResolveOutcome {
    /// Applied to the live root and published. Only this closes the UI.
    Applied(RootStatus),
    /// Something moved between Open and Save; nothing was deleted or written.
    /// Carries the refreshed snapshot so the UI can redisplay.
    Stale(Box<ResolutionSnapshot>),
    /// The resolution is recorded but not yet applied to the live root; the
    /// next cycle finishes it. Never a completed save.
    Pending,
}

/// The sync engine for one state directory.
pub struct Engine {
    pub home: PathBuf,
    pub home_dir: PathBuf,
    pub cfg: Config,
    pub cloud: Cloud,
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
        })
    }

    // --- public entry points ----------------------------------------------

    /// Start tracking `path`, or link to it when the cloud already knows the
    /// slug. Returns the slug.
    pub fn add_root(&mut self, path: &Path, slug: Option<&str>) -> Result<String> {
        let g = config::lock(&self.home)?;
        self.reload(&g)?;
        let cloud = self.cloud();

        // Every rejection happens before the first byte is written anywhere.
        let path = resolve_target(path)?;
        let kind = kind_of(&path)?;
        let slug = match slug {
            Some(s) => s.to_string(),
            None => config::default_slug(&path, &self.home_dir),
        };
        check_slug(&slug)?;
        self.check_placement(&path)?;
        self.check_unregistered(&slug, &path)?;

        // The cloud decides first: a slug another device already created is a
        // Link, never a second unrelated history. A manifest we cannot read is
        // not evidence that the slug is free.
        match cloud.read_manifest(&slug) {
            Some(_) => {
                self.link_root_locked(&g, &cloud, &slug, &path)?;
                return Ok(slug);
            }
            None if cloud.slug_dir(&slug)?.join("manifest.json").exists() => bail!(
                "the manifest for {slug} exists but could not be read; retry once the \
                 provider has finished downloading it"
            ),
            None => {}
        }

        self.archive_stale_staging(&slug)?;
        let repo = Repo::init(
            &self.home,
            &slug,
            &path,
            &self.cfg.device_name,
            &self.cfg.device_id,
            kind,
        )?;
        // Written before the first commit so it is part of the history every
        // other device links to, and so `ignore_text` filters this mirror.
        let ignore = repo.ignore_text(&self.home_dir);
        fs::write(repo.staging.join(".dotloreignore"), ignore)?;
        cloud.write_manifest_once(&Manifest {
            slug: slug.clone(),
            kind,
        })?;
        cloud.write_device_name_once(&slug, &self.cfg.device_id, &self.cfg.device_name)?;
        repo.commit_local(true, &self.home_dir)?;
        repo.publish(&cloud)?;
        self.register(
            &g,
            Root {
                slug: slug.clone(),
                path,
                kind,
                initializing: false,
            },
        )?;
        Ok(slug)
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
    pub fn remove_root(&mut self, slug: &str) -> Result<()> {
        let g = config::lock(&self.home)?;
        self.reload(&g)?;
        let before = self.cfg.roots.len();
        self.cfg.roots.retain(|r| r.slug != slug);
        if self.cfg.roots.len() == before {
            bail!("no tracked root with slug {slug}");
        }
        self.save(&g)
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
        for c in conflict::list(&repo.git)? {
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
        let manifest = cloud.read_manifest(slug).ok_or_else(|| {
            anyhow!(
                "no readable manifest for {slug} in {}",
                cloud.base.display()
            )
        })?;
        let kind = manifest.kind;
        let path = resolve_target(path)?;
        match fs::symlink_metadata(&path) {
            Ok(_) if kind_of(&path)? != kind => {
                bail!("{} does not match the {kind:?} slug {slug}", path.display())
            }
            Ok(_) => {}
            // A file root may be created by the first sync; a directory root
            // must already be there.
            Err(_) if kind == Kind::File => {}
            Err(e) => return Err(e).with_context(|| format!("{} is missing", path.display())),
        }
        self.check_placement(&path)?;
        self.check_unregistered(slug, &path)?;

        // No `.dotloreignore` and no local first commit: both arrive with the
        // history the creating device published.
        self.archive_stale_staging(slug)?;
        Repo::init(
            &self.home,
            slug,
            &path,
            &self.cfg.device_name,
            &self.cfg.device_id,
            kind,
        )?;
        cloud.write_device_name_once(slug, &self.cfg.device_id, &self.cfg.device_name)?;
        let root = Root {
            slug: slug.to_string(),
            path,
            kind,
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
            root.kind,
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
                root.kind,
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
        // quietly recreate it. Only a not-yet-initialized *file* root may
        // legitimately be absent (I8: its Link creates it); anything else that
        // is gone is a root the user moved, never a deletion to publish.
        let may_be_absent = root.initializing && root.kind == Kind::File;
        if !may_be_absent && !root_present(&root.path, root.kind) {
            return Ok(RootStatus::RootMissing);
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
                ResumeOutcome::Pending(_) => return Ok(stalled(&tx)),
            },
            None => None,
        };

        if pending.is_none() && root_present(&root.path, root.kind) {
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
            return Ok(stalled(&tx));
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
        Repo::open(
            &self.home,
            &root.slug,
            &root.path,
            &self.cfg.device_name,
            &self.cfg.device_id,
            root.kind,
        )
    }

    /// `self.cfg` was loaded under the guard we still hold, so it is the
    /// freshest config there is; no other process can have written since.
    fn register(&mut self, g: &HomeLock, root: Root) -> Result<()> {
        self.cfg
            .roots
            .retain(|r| r.slug != root.slug && r.path != root.path);
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
    };
    let dest_cloud = e.cloud();
    let mut out = Vec::new();
    for root in e.cfg.roots.clone() {
        dest_cloud.write_manifest_once(&Manifest {
            slug: root.slug.clone(),
            kind: root.kind,
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
fn cloud_at(provider_dir: &Path) -> Cloud {
    Cloud {
        base: canonical(provider_dir).join("dotlore"),
    }
}

/// Why an apply did not finish, as a status a user can act on.
///
/// A path that raced a live edit is genuinely `Pending` — the next cycle picks
/// it up. A path blocked by a symlink in the live root, or by a target entry
/// that is not a regular file, is re-skipped on every attempt forever: the
/// whole root then stops committing and publishing behind the same `Pending`
/// the UI shows for "no bundles yet", with nothing to act on. That one is
/// `Error` naming the paths. It is still fail-closed and still retried, so
/// clearing the path lets the very next cycle finish.
///
/// The root stays frozen as a whole while any path is blocked: finalizing the
/// unblocked paths would advance `main` past a root that does not match it,
/// which is the invariant the transaction exists to hold.
fn stalled(tx: &Transaction) -> RootStatus {
    let blocked = tx.blocked();
    if blocked.is_empty() {
        return RootStatus::Pending;
    }
    let names: Vec<String> = blocked.iter().map(|p| p.display().to_string()).collect();
    RootStatus::Error(format!(
        "cannot write {} in the live root: the path is a symlink, or is not a regular file \
         on one side. Nothing was overwritten; move it aside and the next sync continues.",
        names.join(", ")
    ))
}

fn root_status(repo: &Repo) -> Result<RootStatus> {
    if !repo.has_main() {
        return Ok(RootStatus::Pending);
    }
    match conflict::list(&repo.git)?.len() {
        0 => Ok(RootStatus::Synced),
        n => Ok(RootStatus::Conflicts(n)),
    }
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

fn root_present(path: &Path, kind: Kind) -> bool {
    match fs::symlink_metadata(path) {
        Ok(md) => match kind {
            Kind::Dir => md.is_dir(),
            Kind::File => md.is_file(),
        },
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

fn kind_of(path: &Path) -> Result<Kind> {
    let md =
        fs::symlink_metadata(path).with_context(|| format!("{} is unreadable", path.display()))?;
    if md.is_dir() {
        Ok(Kind::Dir)
    } else if md.is_file() {
        Ok(Kind::File)
    } else {
        bail!(
            "{} is neither a regular file nor a directory",
            path.display()
        );
    }
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

/// The real file a staging entry maps to. A single-file root has exactly one
/// logical entry, `content`, whatever the local file is called.
fn live_root_path(repo: &Repo, rel: &Path) -> Result<PathBuf> {
    match repo.kind {
        Kind::File => {
            if rel != Path::new("content") {
                bail!("file root: unexpected staging entry {}", rel.display());
            }
            Ok(repo.root.clone())
        }
        Kind::Dir => {
            if !plain_rel(rel) {
                bail!("unsafe path {}", rel.display());
            }
            Ok(repo.root.join(rel))
        }
    }
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
        let cfg = Config {
            device_id: letter.to_string().repeat(32),
            device_name: format!("Mac {letter} Pro"),
            provider_dir: Some(provider.to_path_buf()),
            roots: Vec::new(),
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
        dev.engine.add_root(&path, Some("proj-claude")).unwrap()
    }

    fn cloud_of(dev: &Dev) -> Cloud {
        Cloud {
            base: dev.engine.cloud.base.clone(),
        }
    }

    fn staging(dev: &Dev) -> PathBuf {
        dev.home.path().join("repos/proj-claude")
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
        assert_eq!(cfg.roots[0].kind, Kind::Dir);
        assert!(staging(&a).join(".dotloreignore").is_file());
        assert!(!a.root.path().join(".dotloreignore").exists());

        let cloud = cloud_of(&a);
        assert_eq!(cloud.read_manifest("proj-claude").unwrap().kind, Kind::Dir);
        assert_eq!(cloud.list_bundles("proj-claude").len(), 1);

        assert_eq!(
            a.engine.sync_root("proj-claude").unwrap(),
            RootStatus::Synced
        );
        assert_eq!(cloud.list_bundles("proj-claude").len(), 1);
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
        assert_eq!(
            cloud_of(&a).read_manifest("proj-claude").unwrap().kind,
            Kind::Dir
        );
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
            Kind::Dir,
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

        assert!(Config::load(a.home.path()).unwrap().roots.is_empty());
        assert!(!a.home.path().join("repos/state-home").exists());
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
            Kind::Dir,
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
            Kind::Dir,
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
                Kind::Dir,
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
        assert_eq!(dest.read_manifest("proj-claude").unwrap().kind, Kind::Dir);
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
}
