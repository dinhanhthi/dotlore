//! The per-slug staging git repository.
//!
//! One [`Repo`] owns `<home>/repos/<slug>`: a private git repo whose worktree
//! is a filtered copy of the tracked root. It mirrors and commits the root,
//! fetches other devices' bundles out of the provider folder, merges them, and
//! publishes its own bundle back.
//!
//! Nothing here writes to the live root directly. Every change that reaches a
//! real file goes through a [`Transaction`]: a detached worktree under
//! `<home>/tmp/` plus a fsynced journal at `.git/dotlore-apply.json`, so a
//! crash at any point resumes instead of re-mirroring a half-applied root onto
//! a merged head.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::cloud::Cloud;
use crate::git::Git;
use crate::mirror::{self, FileState, Snapshot, Status};
use crate::project::{self, EntryList, Limits, ProjectFile};

/// Journal format; a file written by a newer build is refused, not guessed at.
const JOURNAL_VERSION: u32 = 1;

/// How often a concurrent root edit may restart guarded apply before the cycle
/// gives up and returns the unapplied paths (the journal survives, so the next
/// cycle picks the work up again).
const MAX_RECONCILE: u32 = 3;

/// Pathspecs per `git add` invocation, to stay well inside the argv limit.
const ADD_CHUNK: usize = 256;

/// `crate::conflict::resolve_index`, passed in rather than called directly.
///
/// A merge that leaves an unmerged index has to be resolved by the conflict
/// policy. Passing the function keeps that dependency visible in the signature
/// of [`Repo::begin_tx`] / [`Repo::pending_tx`] instead of hidden in a default
/// a caller can silently forget, and it is the seam the unit tests below hang
/// `no_resolver` on to assert that nothing they exercise ever reaches an
/// unmerged index.
pub type IndexResolver = fn(&Git, &str, &str, &str) -> Result<Vec<PathBuf>>;

/// Whether a fetch pass includes this device's own bundles. Recovery does;
/// a normal cycle has no use for its own history.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FetchMode {
    Normal,
    Recovery,
}

/// What merging one remote head did to the transaction worktree.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum MergeOutcome {
    AlreadyMerged,
    FastForward,
    /// Two diverged heads with identical trees; this device took theirs
    /// because their commit hash sorts first. No merge commit is minted and
    /// no bundle is needed — the adopted commit is already in the cloud.
    Adopted,
    Clean,
    /// Merged after the conflict policy resolved the index; the paths are the
    /// live files that gained a `.conflict-` sibling.
    Resolved(Vec<PathBuf>),
}

/// Where a journalled transaction had got to.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    /// Worktree created, target not computed yet. `main` is untouched, so the
    /// merges can simply be redone.
    Preparing,
    /// Target pinned; paths are being written to the live root one at a time.
    Applying,
    /// A root file changed under us; the merge is being rebuilt from a
    /// baseline that preserves the original branch relationship.
    Reconciling,
    /// Every path is applied and acknowledged; `main` may advance.
    Finalizing,
}

/// What [`Transaction::resume`] did with a journal found at startup.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ResumeOutcome {
    /// Phase `Preparing`: nothing was ever applied and `main` never moved. The
    /// worktree is back at its start commit; redo the merges on this same
    /// transaction and carry on as usual.
    Restart,
    /// Apply and finalization completed; `main` is at the target and the
    /// journal is gone. The caller should publish.
    Finished,
    /// Some paths could not be applied yet (drift, a symlink, an unreadable
    /// source). The journal is intact for the next cycle.
    Pending(Vec<PathBuf>),
}

/// A staging repo for one tracked root.
///
/// `home` and `device_name` are private: they are needed to place temporary
/// files and to build the transaction worktree's [`Git`], and keeping them
/// private forces construction through [`Repo::init`] / [`Repo::open`].
pub struct Repo {
    pub slug: String,
    pub root: PathBuf,
    pub staging: PathBuf,
    pub git: Git,
    pub my_id: String,
    home: PathBuf,
    device_name: String,
    /// Per-sync file/folder ceilings. Engine overwrites from config.
    pub limits: Limits,
}

impl Repo {
    /// Create `<home>/repos/<slug>` as a git repo on `main`.
    ///
    /// Does not write `.dotloreignore`; the engine owns that file.
    pub fn init(
        home: &Path,
        slug: &str,
        root: &Path,
        device_name: &str,
        device_id: &str,
    ) -> Result<Repo> {
        let r = Repo::at(home, slug, root, device_name, device_id)?;
        fs::create_dir_all(&r.staging)?;
        if !r.staging.join(".git").exists() {
            r.git.ok(&["init", "-b", "main"])?;
        }
        // Everything a user's global config could otherwise decide for us.
        // `Git::command` already keeps that config out; these are the repo's
        // own answers, so a future reader of the repo sees them too.
        for (k, v) in [
            ("core.autocrlf", "false"),
            ("merge.conflictstyle", "merge"),
            ("gc.auto", "0"),
            ("commit.gpgsign", "false"),
        ] {
            r.git.ok(&["config", k, v])?;
        }
        r.ensure_attributes()?;
        Ok(r)
    }

    /// Write `.git/info/attributes` unless it is already there.
    ///
    /// A `.gitattributes` mirrored in from the tracked root is a *user* file:
    /// it governs their project, never our staging copy. Left to act,
    /// `* text=auto` makes `git add` normalise CRLF on check-in, so the
    /// publishing device keeps its CRLF bytes (its next mirror re-normalises
    /// to the same blob, so no diff and no commit) while every linking device
    /// writes LF into its live root — a permanent byte divergence between two
    /// roots, and a silent rewrite of line endings in a live file.
    /// `info/attributes` outranks every in-tree `.gitattributes` and lives in
    /// the common dir, so the transaction worktrees are covered too.
    /// Deliberately not `* binary`: that implies `-merge`, which turns every
    /// non-overlapping concurrent edit into a `UU` conflict.
    ///
    /// [`Repo::open`] calls it too — that is the path every ordinary cycle
    /// takes — so a staging repo created by an older build, or one whose
    /// `.git/info` an external `git gc` or a manual cleanup removed, is
    /// repaired instead of carrying the hazard forever.
    fn ensure_attributes(&self) -> Result<()> {
        let info = self.staging.join(".git").join("info");
        let attrs = info.join("attributes");
        if attrs.is_file() {
            return Ok(());
        }
        fs::create_dir_all(&info)?;
        fs::write(attrs, "* -text -ident -filter -working-tree-encoding\n")?;
        Ok(())
    }

    /// Open an existing staging repo; an error when it has no `.git`.
    pub fn open(
        home: &Path,
        slug: &str,
        root: &Path,
        device_name: &str,
        device_id: &str,
    ) -> Result<Repo> {
        let r = Repo::at(home, slug, root, device_name, device_id)?;
        if !r.staging.join(".git").exists() {
            bail!("no staging repo at {}", r.staging.display());
        }
        r.ensure_attributes()?;
        Ok(r)
    }

    fn at(
        home: &Path,
        slug: &str,
        root: &Path,
        device_name: &str,
        device_id: &str,
    ) -> Result<Repo> {
        if !valid_slug(slug) {
            bail!("invalid slug {slug:?}");
        }
        if !valid_device(device_id) {
            bail!("invalid device id {device_id:?}");
        }
        let staging = home.join("repos").join(slug);
        Ok(Repo {
            slug: slug.to_string(),
            root: root.to_path_buf(),
            staging: staging.clone(),
            git: Git::new(staging, device_name, device_id),
            my_id: device_id.to_string(),
            home: home.to_path_buf(),
            device_name: device_name.to_string(),
            limits: Limits::default(),
        })
    }

    pub fn has_main(&self) -> bool {
        self.git.rev("refs/heads/main").is_some()
    }

    /// Short device id — the only device token that appears in file names.
    pub fn id8(&self) -> &str {
        short8(&self.my_id)
    }

    /// The committed `.dotloreproject`, or an empty include-list when absent.
    pub fn project_file(&self) -> Result<ProjectFile> {
        project::read(&self.staging)
    }

    /// The committed `.dotloreignore`, or the default never-list when absent
    /// (a linked root whose history has not arrived yet).
    pub fn ignore_text(&self, _home_dir: &Path) -> String {
        fs::read_to_string(self.staging.join(crate::project::IGNORE_FILE))
            .unwrap_or_else(|_| project::DEFAULT_NEVER_IGNORE.to_string())
    }

    /// Mirror the root into staging and commit whatever changed.
    ///
    /// Returns whether a commit was made. `allow_empty_first` mints an empty
    /// commit when `main` does not exist yet, so an Add on an empty root still
    /// has a head to publish.
    pub fn commit_local(
        &self,
        allow_empty_first: bool,
        home_dir: &Path,
    ) -> Result<(bool, mirror::MirrorReport)> {
        let entries = project::read(&self.staging)?.tracked();
        let report = mirror::root_to_staging(
            &self.root,
            &self.staging,
            &entries,
            &self.ignore_text(home_dir),
            self.limits,
        )?;
        stage_worktree(&self.git, &self.staging)?;

        let msg = format!("local {}", self.id8());
        // `--quiet` exits 1 when the index differs from HEAD; on an unborn
        // HEAD it compares against the empty tree, which is what we want.
        let staged = !self
            .git
            .run(&["diff", "--cached", "--quiet"])?
            .status
            .success();
        if staged {
            self.git.ok(&["commit", "-m", &msg])?;
            Ok((true, report))
        } else if allow_empty_first && !self.has_main() {
            self.git.ok(&["commit", "--allow-empty", "-m", &msg])?;
            Ok((true, report))
        } else {
            Ok((false, report))
        }
    }

    // --- bundle delivery --------------------------------------------------

    /// Fetch every not-yet-consumed bundle into
    /// `refs/remotes/<provider-key>/<device-id>/main`.
    ///
    /// Passes repeat until one fetches nothing: a device's bundle can need a
    /// prerequisite that only another device's later bundle carries. Within a
    /// pass a device's chain stops at its first unreadable bundle, leaving
    /// `consumed` where it was so the next cycle retries from there (an iCloud
    /// stub that materialises later must not be skipped over).
    ///
    /// Returns the device ids that have a remote ref afterwards.
    pub fn fetch_bundles(&self, cloud: &Cloud, mode: FetchMode) -> Result<Vec<String>> {
        let key = provider_key(cloud);
        let mut by_device: BTreeMap<String, Vec<(u64, PathBuf)>> = BTreeMap::new();
        for b in cloud.list_bundles(&self.slug) {
            if !valid_device(&b.device) {
                continue;
            }
            if mode == FetchMode::Normal && b.device == self.my_id {
                continue;
            }
            by_device.entry(b.device).or_default().push((b.seq, b.path));
        }
        for list in by_device.values_mut() {
            list.sort_by_key(|(seq, _)| *seq);
        }

        let mut state = self.load_state()?;
        loop {
            let mut fetched_any = false;
            for (device, list) in &by_device {
                let refspec = format!("+main:{}", remote_ref(&key, device));
                for (seq, path) in list {
                    let consumed = provider_slot(&mut state, &key, &cloud.base)?
                        .consumed
                        .get(device)
                        .copied()
                        .unwrap_or(0);
                    if *seq <= consumed {
                        continue;
                    }
                    let Some(path) = path.to_str() else { break };
                    if !self.git.run(&["bundle", "verify", path])?.status.success() {
                        break;
                    }
                    if !self.git.run(&["fetch", path, &refspec])?.status.success() {
                        break;
                    }
                    provider_slot(&mut state, &key, &cloud.base)?
                        .consumed
                        .insert(device.clone(), *seq);
                    self.save_state(&state)?;
                    fetched_any = true;
                }
            }
            if !fetched_any {
                break;
            }
        }

        // Every device with a remote ref, not just the ones seen in this
        // listing: a device whose bundles are all dataless stubs right now
        // still has a head this cycle may need to merge.
        let prefix = format!("refs/remotes/{key}/");
        let refs = self
            .git
            .ok(&["for-each-ref", "--format=%(refname)", &prefix])?;
        let mut devices: Vec<String> = refs
            .lines()
            .filter_map(|r| r.strip_prefix(&prefix)?.strip_suffix("/main"))
            .filter(|d| valid_device(d) && (mode == FetchMode::Recovery || *d != self.my_id))
            .map(str::to_string)
            .collect();
        devices.sort();
        devices.dedup();
        Ok(devices)
    }

    /// Whether a bundle still in the cloud carries `commit`: one of ours, or
    /// one from a device whose fetched head contains it.
    fn cloud_carries(&self, cloud: &Cloud, key: &str, commit: &str) -> Result<bool> {
        if cloud.has_bundles(&self.slug, &self.my_id)? {
            return Ok(true);
        }
        let prefix = format!("refs/remotes/{key}/");
        let refs = self
            .git
            .ok(&["for-each-ref", "--format=%(refname) %(objectname)", &prefix])?;
        for (r, head) in refs.lines().filter_map(|l| l.split_once(' ')) {
            let Some(d) = r
                .strip_prefix(&prefix)
                .and_then(|d| d.strip_suffix("/main"))
            else {
                continue;
            };
            if self.git.is_ancestor(commit, head) && cloud.has_bundles(&self.slug, d)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Bundle `main` into the provider folder, unless it is already there.
    ///
    /// The sequence number is reserved durably *before* the cloud is touched,
    /// so a crash mid-publish burns a number rather than risking a second,
    /// different bundle at one that is already taken.
    pub fn publish(&self, cloud: &Cloud) -> Result<Option<u64>> {
        let Some(main) = self.git.rev("refs/heads/main") else {
            return Ok(None);
        };
        let key = provider_key(cloud);
        let sent_ref = format!("refs/dotlore/sent/{key}");
        // A provider folder that lost our bundles (a Google account removed
        // and added again comes back at the same path) must get the whole
        // history again, not an increment on bundles nobody can read.
        let sent = match self.git.rev(&sent_ref) {
            Some(s) if self.cloud_carries(cloud, &key, &s)? => Some(s),
            _ => None,
        };
        if sent.as_deref() == Some(main.as_str()) {
            return Ok(None);
        }
        // Adoption can move `main` to an ancestor of what we last sent; the
        // cloud already has it and `<sent>..main` would be an empty range.
        if let Some(s) = &sent {
            if self.git.is_ancestor(&main, s) {
                self.git.ok(&["update-ref", &sent_ref, &main])?;
                return Ok(None);
            }
        }

        let mut state = self.load_state()?;
        let seq = {
            let slot = provider_slot(&mut state, &key, &cloud.base)?;
            // The cloud listing is only a floor: an evicted own bundle must
            // never hand its number back out.
            let seq = slot
                .own_next_seq
                .max(cloud.max_seq(&self.slug, &self.my_id) + 1);
            slot.own_next_seq = seq + 1;
            seq
        };
        self.save_state(&state)?;

        let tmp_dir = home_tmp_dir(&self.home)?;
        // Unpredictable, not just unused. `git bundle create` makes the file
        // itself: verified against git 2.50.1, it unlinks and recreates the
        // path (the inode changes and a pre-created 0600 file comes back
        // umask-derived 0644), and it follows a symlink planted there. So a
        // pre-opened descriptor is not an option and a fixed name lets any
        // same-user process that can write into `<home>/tmp` redirect every
        // tracked byte — secrets included — somewhere of its choosing. Two
        // threats, two guards: `unique_id` is 16 bytes of `/dev/urandom`, so
        // no same-uid process can guess the name to plant a symlink at, and
        // `home_tmp_dir` keeps the directory 0700, so the window between
        // git's umask-derived creation mode and the `set_permissions` below
        // is not visible to any other local account.
        let tmp = tmp_dir.join(format!("{}-{seq}-{}.bundle", self.slug, unique_id()));
        let tmp_str = tmp
            .to_str()
            .ok_or_else(|| anyhow!("non-utf8 temp path {}", tmp.display()))?
            .to_string();

        let range = match &sent {
            Some(s) => match self.git.ok(&["merge-base", s, &main]) {
                Ok(base) if !base.is_empty() => format!("{base}..main"),
                // No merge base (unrelated histories after a recovery or an
                // Add race): the full history is the only correct bundle.
                _ => "main".to_string(),
            },
            None => "main".to_string(),
        };

        let res = (|| -> Result<()> {
            self.git.ok(&["bundle", "create", "-q", &tmp_str, &range])?;
            // The bundle carries every tracked byte; it is 0600 in the cloud
            // and has no business being looser while it waits in tmp.
            fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
            self.git.ok(&["bundle", "verify", &tmp_str])?;
            cloud.publish_bundle(&self.slug, &self.my_id, seq, &tmp)?;
            self.git.ok(&["update-ref", &sent_ref, &main])?;
            Ok(())
        })();
        let _ = fs::remove_file(&tmp);
        res?;
        Ok(Some(seq))
    }

    // --- merging ----------------------------------------------------------

    /// Merge one device's remote head into the transaction worktree.
    ///
    /// Every git call below runs in `tx`, never in the main staging worktree:
    /// `main` stays exactly where it was until the apply completes.
    pub fn merge_remote(&self, tx: &Transaction, their_id: &str) -> Result<MergeOutcome> {
        if !valid_device(their_id) {
            bail!("invalid device id {their_id:?}");
        }
        let r = remote_ref(&tx.provider_key, their_id);
        let g = &tx.git;
        if g.rev(&r).is_none() {
            bail!("no remote ref {r}");
        }
        if g.is_ancestor(&r, "HEAD") {
            return Ok(MergeOutcome::AlreadyMerged);
        }
        if g.is_ancestor("HEAD", &r) {
            g.ok(&["merge", "--ff-only", &r])?;
            return Ok(MergeOutcome::FastForward);
        }
        // Diverged heads that happen to hold the same tree: minting a merge
        // commit here is what makes two devices trade bundles forever. Both
        // sides pick the smaller hash, so both reach the same commit.
        if g.run(&["diff", "--quiet", "HEAD", &r])?.status.success() {
            let theirs = g.rev(&r).unwrap_or_default();
            let ours = g.rev("HEAD").unwrap_or_default();
            return if theirs < ours {
                g.ok(&["reset", "--hard", &r])?;
                Ok(MergeOutcome::Adopted)
            } else {
                Ok(MergeOutcome::AlreadyMerged)
            };
        }

        let mut out = g.run(&["merge", "--no-edit", &r])?;
        if !out.status.success()
            && String::from_utf8_lossy(&out.stderr).contains("unrelated histories")
        {
            out = g.run(&["merge", "--no-edit", "--allow-unrelated-histories", &r])?;
        }
        if out.status.success() {
            return Ok(MergeOutcome::Clean);
        }
        let paths = (tx.resolve_index)(g, &r, &self.my_id, their_id)?;
        g.ok(&[
            "commit",
            "--no-edit",
            "-m",
            &format!("merge {}", short8(their_id)),
        ])?;
        Ok(MergeOutcome::Resolved(paths))
    }

    // --- reading trees ----------------------------------------------------

    /// The logical entries that differ between two pinned commits.
    ///
    /// `pre == None` means there was no local history: every regular file in
    /// `target` is an addition. Staging-private entries are excluded from both
    /// forms — they must never reach a live root. Paths outside `entries` are
    /// dropped after that, so an untracked blob never enters a change set.
    pub fn changes_since(
        &self,
        pre: Option<&str>,
        target: &str,
        entries: &EntryList,
    ) -> Result<Vec<(Status, PathBuf)>> {
        let mut out = Vec::new();
        match pre {
            Some(pre) => {
                let o = self.git.run(&[
                    "diff",
                    "--name-status",
                    "-z",
                    "--no-renames",
                    "--end-of-options",
                    pre,
                    target,
                ])?;
                if !o.status.success() {
                    bail!(
                        "git diff {pre}..{target} failed: {}",
                        String::from_utf8_lossy(&o.stderr).trim()
                    );
                }
                let fields: Vec<&[u8]> = o.stdout.split(|b| *b == 0).collect();
                let mut i = 0;
                while i + 1 < fields.len() {
                    let (code, path) = (fields[i], fields[i + 1]);
                    i += 2;
                    let status = match code.first() {
                        Some(b'A') => Status::Added,
                        Some(b'M') | Some(b'T') => Status::Modified,
                        Some(b'D') => Status::Deleted,
                        _ => bail!(
                            "unhandled diff status {:?} for {:?}",
                            String::from_utf8_lossy(code),
                            String::from_utf8_lossy(path)
                        ),
                    };
                    if let Some(p) = usable_entry(path) {
                        if entries.contains_rel(&p) {
                            out.push((status, p));
                        }
                    }
                }
            }
            None => {
                for (mode, path) in self.ls_tree(target)? {
                    // Regular blobs only: a mode-120000 symlink or a 160000
                    // gitlink is not something we write into a live root.
                    if mode.starts_with("100") {
                        if let Some(p) = usable_entry(path.as_os_str().as_bytes()) {
                            if entries.contains_rel(&p) {
                                out.push((Status::Added, p));
                            }
                        }
                    }
                }
            }
        }
        Ok(out)
    }

    /// What each path is expected to hold in the live root right now, read
    /// from the pinned pre-merge commit.
    ///
    /// `None` for a path means "expected absent". A git failure that is not
    /// specifically "this path is not in that commit" is propagated: a
    /// transient error that looked like an absence would feed
    /// [`mirror::apply_to_root`] a wrong expectation, and it writes on a match.
    pub fn expected_bytes(&self, pre: Option<&str>, paths: &[PathBuf]) -> Result<Snapshot> {
        let mut snap = Snapshot::new();
        let Some(pre) = pre else {
            for p in paths {
                snap.insert(p.clone(), None);
            }
            return Ok(snap);
        };
        let modes: BTreeMap<PathBuf, String> = self
            .ls_tree(pre)?
            .into_iter()
            .map(|(mode, path)| (path, mode))
            .collect();
        for p in paths {
            let rel = p
                .to_str()
                .ok_or_else(|| anyhow!("non-utf8 path {}", p.display()))?;
            let out = self.git.run(&["show", &format!("{pre}:{rel}")])?;
            if out.status.success() {
                snap.insert(
                    p.clone(),
                    Some(FileState {
                        bytes: out.stdout,
                        executable: modes.get(p).map(|m| m == "100755").unwrap_or(false),
                    }),
                );
                continue;
            }
            let err = String::from_utf8_lossy(&out.stderr);
            let absent = out.status.code() == Some(128)
                && (err.contains("does not exist in")
                    || err.contains("exists on disk, but not in"));
            if absent {
                snap.insert(p.clone(), None);
            } else {
                bail!("git show {pre}:{rel} failed: {}", err.trim());
            }
        }
        Ok(snap)
    }

    /// `(mode, path)` for every entry of a commit's tree, recursively.
    fn ls_tree(&self, rev: &str) -> Result<Vec<(String, PathBuf)>> {
        let o = self
            .git
            .run(&["ls-tree", "-r", "-z", "--end-of-options", rev])?;
        if !o.status.success() {
            bail!(
                "git ls-tree {rev} failed: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            );
        }
        let mut out = Vec::new();
        for entry in o.stdout.split(|b| *b == 0).filter(|e| !e.is_empty()) {
            // "<mode> SP <type> SP <oid> TAB <path>"
            let Some(tab) = entry.iter().position(|b| *b == b'\t') else {
                continue;
            };
            let meta = String::from_utf8_lossy(&entry[..tab]);
            let Some(mode) = meta.split(' ').next() else {
                continue;
            };
            out.push((
                mode.to_string(),
                PathBuf::from(OsStr::from_bytes(&entry[tab + 1..])),
            ));
        }
        Ok(out)
    }

    // --- transactions -----------------------------------------------------

    /// Begin a durable apply transaction detached at `start`.
    ///
    /// Pins the pre-merge head, creates `<home>/tmp/<slug>-tx-<id>` and fsyncs
    /// the journal in phase `Preparing` before anything is merged. `start` is
    /// `"HEAD"` for a normal cycle and the adopted remote ref when this device
    /// has no `main` yet.
    pub fn begin_tx(
        &self,
        provider_key: &str,
        start: &str,
        resolve_index: IndexResolver,
    ) -> Result<Transaction> {
        if self.journal_path().exists() {
            bail!(
                "a pending transaction is journalled for {}; recover it first",
                self.slug
            );
        }
        let id = unique_id();
        let start_rev = self
            .git
            .rev(start)
            .ok_or_else(|| anyhow!("cannot start a transaction at {start}: no such commit"))?;
        let pre = self.git.rev("refs/heads/main");

        self.git
            .ok(&["update-ref", &tx_ref(&id, "start"), &start_rev])?;
        if let Some(pre) = &pre {
            self.git.ok(&["update-ref", &tx_ref(&id, "pre"), pre])?;
        }

        let worktree = home_tmp_dir(&self.home)?.join(format!("{}-tx-{id}", self.slug));
        let _ = fs::remove_dir_all(&worktree);
        self.git.ok(&["worktree", "prune"])?;
        self.git.ok(&[
            "worktree",
            "add",
            "-q",
            "--detach",
            worktree
                .to_str()
                .ok_or_else(|| anyhow!("non-utf8 worktree path"))?,
            &start_rev,
        ])?;

        let journal = Journal {
            version: JOURNAL_VERSION,
            slug: self.slug.clone(),
            id: id.clone(),
            worktree: worktree.clone(),
            provider_key: provider_key.to_string(),
            phase: Phase::Preparing,
            pre,
            start: start_rev,
            target: None,
            changes: Vec::new(),
            applied: Vec::new(),
            intent: None,
            attempts: 0,
        };
        let tx = self.tx_from(journal, resolve_index);
        tx.write_journal()?;
        Ok(tx)
    }

    /// The journalled transaction, if one was left behind by an earlier run.
    pub fn pending_tx(&self, resolve_index: IndexResolver) -> Result<Option<Transaction>> {
        let path = self.journal_path();
        let bytes = match fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        let journal: Journal = serde_json::from_slice(&bytes)
            .with_context(|| format!("parsing {}", path.display()))?;
        if journal.version != JOURNAL_VERSION {
            bail!(
                "{} has journal version {}, expected {JOURNAL_VERSION}",
                path.display(),
                journal.version
            );
        }
        let tx = self.tx_from(journal, resolve_index);
        tx.ensure_worktree(self)?;
        Ok(Some(tx))
    }

    fn tx_from(&self, journal: Journal, resolve_index: IndexResolver) -> Transaction {
        Transaction {
            git: Git::new(journal.worktree.clone(), &self.device_name, &self.my_id),
            worktree: journal.worktree.clone(),
            id: journal.id.clone(),
            provider_key: journal.provider_key.clone(),
            journal_path: self.journal_path(),
            resolve_index,
            j: journal,
            blocked: Vec::new(),
        }
    }

    fn journal_path(&self) -> PathBuf {
        self.staging.join(".git").join("dotlore-apply.json")
    }

    /// The live file a staging entry maps to.
    fn live_path(&self, rel: &Path) -> Result<PathBuf> {
        if !plain_rel(rel) {
            bail!("unsafe path {}", rel.display());
        }
        Ok(self.root.join(rel))
    }

    // --- delivery state ---------------------------------------------------

    fn state_path(&self) -> PathBuf {
        self.staging.join(".git").join("dotlore-state.json")
    }

    fn load_state(&self) -> Result<State> {
        let path = self.state_path();
        match fs::read(&path) {
            Ok(b) => {
                serde_json::from_slice(&b).with_context(|| format!("parsing {}", path.display()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    fn save_state(&self, state: &State) -> Result<()> {
        write_durable(&self.state_path(), &serde_json::to_vec(state)?)
    }
}

// --- transaction ----------------------------------------------------------

/// One durable apply: an isolated detached worktree plus its journal.
///
/// `git` is bound to `worktree`, not to the staging repo, and is the handle
/// the conflict policy uses (`conflict::resolve_index(&tx.git, ..)` and
/// `conflict::resolve(tx, ..)`).
pub struct Transaction {
    pub git: Git,
    pub worktree: PathBuf,
    pub id: String,
    pub provider_key: String,
    journal_path: PathBuf,
    resolve_index: IndexResolver,
    j: Journal,
    /// The subset of the last [`Transaction::apply`] pass's skips that will
    /// not clear on their own. Not journalled: it is re-derived from the
    /// filesystem and the target on every pass.
    blocked: Vec<PathBuf>,
}

impl Transaction {
    pub fn phase(&self) -> Phase {
        self.j.phase
    }

    /// The pinned pre-merge head; `None` when this device had no history.
    pub fn pre(&self) -> Option<&str> {
        self.j.pre.as_deref()
    }

    /// The pinned merged commit, once [`Transaction::set_target`] has run.
    pub fn target(&self) -> Option<&str> {
        self.j.target.as_deref()
    }

    /// Paths the last [`Transaction::apply`] could not write and that no
    /// retry will fix: the live path is (or sits under) a symlink, or holds a
    /// directory, or the target's entry is not a regular file, or the entry
    /// maps to no live path at all ([`Repo::live_path`]). A caller that
    /// reports "skipped" as "pending" would retry these forever and freeze the
    /// root behind a status indistinguishable from "no bundles yet"; these are
    /// the ones a human has to clear.
    pub fn blocked(&self) -> &[PathBuf] {
        &self.blocked
    }

    /// The entries this transaction will write to the live root.
    pub fn changes(&self) -> Vec<(Status, PathBuf)> {
        self.j
            .changes
            .iter()
            .map(|(s, p)| ((*s).into(), p.clone()))
            .collect()
    }

    /// Pin the merged worktree head as the target and move to `Applying`.
    ///
    /// Call once the merges are done; `main` is still untouched at this point.
    pub fn set_target(&mut self, repo: &Repo) -> Result<()> {
        if self.j.phase != Phase::Preparing {
            bail!("set_target in phase {:?}", self.j.phase);
        }
        let target = self
            .git
            .rev("HEAD")
            .ok_or_else(|| anyhow!("transaction worktree has no HEAD"))?;
        repo.git
            .ok(&["update-ref", &tx_ref(&self.id, "target"), &target])?;
        // Governing list is the merged TARGET's, not staging's: on a
        // bootstrap Link staging is empty and the peer's list arrives here.
        let entries = project::read(&self.worktree)?.tracked();
        self.j.changes = repo
            .changes_since(self.j.pre.as_deref(), &target, &entries)?
            .into_iter()
            .map(|(s, p)| (s.into(), p))
            .collect();
        self.j.target = Some(target);
        self.j.applied.clear();
        self.j.phase = Phase::Applying;
        self.write_journal()
    }

    /// Write the target to the live root, one path at a time.
    ///
    /// Returns the paths that could not be applied; an empty result means the
    /// transaction is ready to [`Transaction::finalize`]. A path whose live
    /// value matches neither the expectation nor the target is a concurrent
    /// edit: apply stops and rebuilds the merge from a baseline that keeps
    /// both versions, then starts over.
    pub fn apply(&mut self, repo: &Repo, home_dir: &Path) -> Result<Vec<PathBuf>> {
        if self.j.phase == Phase::Finalizing {
            return Ok(Vec::new());
        }
        if self.j.phase == Phase::Preparing {
            bail!("apply before set_target");
        }
        loop {
            let mut skipped = Vec::new();
            let mut drifted = false;
            self.blocked.clear();
            // One `ls-tree` plus one `git show` per path, hoisted: the
            // expectations cannot change during a pass, and per-path they made
            // a bootstrap Link of a large root an O(n²) tree read. Recomputed
            // on every pass because `reconcile` moves `self.j.pre`.
            let mut expectations = repo.expected_bytes(self.j.pre.as_deref(), &self.unapplied())?;
            for (status, rel) in self.changes() {
                if self.j.applied.contains(&rel) {
                    continue;
                }
                let expected = expectations
                    .remove(&rel)
                    .ok_or_else(|| anyhow!("no expectation for {}", rel.display()))?;
                let Some(desired) = desired_state(&self.worktree, &rel, status)? else {
                    // The target's source is missing or not a regular file:
                    // nothing safe to write, and re-reading the same pinned
                    // target will say the same thing next cycle.
                    self.blocked.push(rel.clone());
                    skipped.push(rel);
                    continue;
                };
                let Ok(live) = repo.live_path(&rel) else {
                    // The entry maps to no live path at all: an unsafe
                    // relative path (`..`, absolute, empty). Reported per
                    // path like every other unwritable one; a hard `Err`
                    // here would abandon the journal mid-`Applying` and take
                    // the rest of the root down with the one bad entry.
                    self.blocked.push(rel.clone());
                    skipped.push(rel);
                    continue;
                };
                let Some(current) = live_state(&repo.root, &live)? else {
                    // The live path is not ours to own — a symlink on it, or a
                    // directory where a file belongs. Fail closed, but say so:
                    // nothing here clears without the user moving it.
                    self.blocked.push(rel.clone());
                    skipped.push(rel);
                    continue;
                };

                if current == desired {
                    // Including after a crash between the write and its
                    // acknowledgement: record it, do not write again.
                    self.acknowledge(&rel)?;
                } else if current == expected {
                    self.j.intent = Some((status.into(), rel.clone()));
                    self.write_journal()?;
                    let mut snap = Snapshot::new();
                    snap.insert(rel.clone(), expected);
                    let left = mirror::apply_to_root(
                        &self.worktree,
                        &repo.root,
                        &[(status, rel.clone())],
                        &snap,
                    )?;
                    self.j.intent = None;
                    if left.is_empty() {
                        self.acknowledge(&rel)?;
                    } else {
                        skipped.push(rel);
                        self.write_journal()?;
                    }
                } else {
                    drifted = true;
                    break;
                }
            }

            if drifted {
                if self.reconcile(repo, home_dir)? {
                    continue;
                }
                return Ok(self.unapplied());
            }
            if skipped.is_empty() {
                self.j.phase = Phase::Finalizing;
                self.write_journal()?;
            }
            return Ok(skipped);
        }
    }

    /// Advance `main` to the pinned target and clear the transaction.
    ///
    /// Idempotent from phase `Finalizing` on, so a crash between the ref
    /// update and the staging reset repeats finalization instead of falling
    /// back to a mirror of a root that is already at the target.
    pub fn finalize(&mut self, repo: &Repo) -> Result<()> {
        let target = self
            .j
            .target
            .clone()
            .ok_or_else(|| anyhow!("finalize before set_target"))?;
        if self.j.phase != Phase::Finalizing {
            if !self.unapplied().is_empty() {
                bail!("finalize with unapplied paths");
            }
            self.j.phase = Phase::Finalizing;
            self.write_journal()?;
        }

        repo.git.ok(&["update-ref", "refs/heads/main", &target])?;
        repo.git.ok(&["reset", "--hard", "refs/heads/main"])?;

        // A target that some device already advertises is, by construction,
        // already in the cloud along with everything it reaches: adoption and
        // fast-forward both land here and need no bundle of their own.
        let sent_ref = format!("refs/dotlore/sent/{}", self.provider_key);
        if repo.git.rev(&sent_ref).as_deref() != Some(target.as_str())
            && remote_heads(repo, &self.provider_key)?.contains(&target)
        {
            repo.git.ok(&["update-ref", &sent_ref, &target])?;
        }

        self.cleanup(repo)
    }

    /// Resume a journalled transaction found at startup.
    pub fn resume(&mut self, repo: &Repo, home_dir: &Path) -> Result<ResumeOutcome> {
        match self.j.phase {
            Phase::Preparing => {
                // Nothing was applied and `main` never moved, so the merges
                // are simply redone. Clear a half-finished merge first, or the
                // worktree index stays unmerged forever.
                let _ = self.git.run(&["merge", "--abort"]);
                self.git.ok(&["reset", "--hard", &self.j.start])?;
                self.j.attempts = 0;
                self.write_journal()?;
                Ok(ResumeOutcome::Restart)
            }
            Phase::Applying | Phase::Reconciling => {
                // A crash during reconciliation left the journal on the old
                // target; the worktree goes back to it and apply hits the same
                // concurrent edit again, which is exactly the retry we want.
                let target = self
                    .j
                    .target
                    .clone()
                    .ok_or_else(|| anyhow!("journal in {:?} with no target", self.j.phase))?;
                let _ = self.git.run(&["merge", "--abort"]);
                self.git.ok(&["reset", "--hard", &target])?;
                self.j.phase = Phase::Applying;
                if let Some((_, rel)) = self.j.intent.take() {
                    sweep_tmp_litter(repo, &rel);
                }
                // The reconciliation budget bounds one cycle, not the journal:
                // a transaction that ran out of attempts while the user was
                // typing must get a fresh budget next cycle, or a root that
                // has gone quiet stays Pending forever.
                self.j.attempts = 0;
                self.write_journal()?;
                let skipped = self.apply(repo, home_dir)?;
                if skipped.is_empty() {
                    self.finalize(repo)?;
                    Ok(ResumeOutcome::Finished)
                } else {
                    Ok(ResumeOutcome::Pending(skipped))
                }
            }
            Phase::Finalizing => {
                self.finalize(repo)?;
                Ok(ResumeOutcome::Finished)
            }
        }
    }

    // --- internals --------------------------------------------------------

    fn unapplied(&self) -> Vec<PathBuf> {
        self.j
            .changes
            .iter()
            .map(|(_, p)| p.clone())
            .filter(|p| !self.j.applied.contains(p))
            .collect()
    }

    fn acknowledge(&mut self, rel: &Path) -> Result<()> {
        if !self.j.applied.contains(&rel.to_path_buf()) {
            self.j.applied.push(rel.to_path_buf());
        }
        self.j.intent = None;
        self.write_journal()
    }

    /// Rebuild the merge around a root file that changed under us.
    ///
    /// Baseline `B` = the pre-merge values for paths not yet applied plus the
    /// acknowledged target values for the ones that are — that is exactly what
    /// the root holds now. The freshly captured root becomes `L` (a child of
    /// `B` only), the desired tree becomes `T` (a child of `B` *and* of the
    /// old target, so the remote history it carries stays an ancestor and is
    /// never merged again), and the two are merged by the usual policy.
    ///
    /// Returns `false` when the attempt budget is spent; the caller reports
    /// the remaining paths as pending and the journal survives for next cycle.
    fn reconcile(&mut self, repo: &Repo, home_dir: &Path) -> Result<bool> {
        self.j.attempts += 1;
        if self.j.attempts > MAX_RECONCILE {
            return Ok(false);
        }
        self.j.phase = Phase::Reconciling;
        self.j.intent = None;
        self.write_journal()?;

        // Worktree still holds the merged target here; reset below would
        // drop `.dotloreproject`. Staging's list is the old one and would
        // skip a peer-just-tracked entry (see the named test).
        let entries = project::read(&self.worktree)?.tracked();

        let target = self
            .j
            .target
            .clone()
            .ok_or_else(|| anyhow!("reconcile with no target"))?;
        let g = &self.git;

        // B: the root as it stood when apply started, path by path.
        match &self.j.pre {
            Some(pre) => g.ok(&["reset", "--hard", pre])?,
            None => {
                let empty = g.ok(&["hash-object", "-w", "-t", "tree", "/dev/null"])?;
                let base = g.ok(&["commit-tree", "-m", "empty baseline", &empty])?;
                g.ok(&["reset", "--hard", &base])?
            }
        };
        for rel in self.j.applied.clone() {
            let p = rel
                .to_str()
                .ok_or_else(|| anyhow!("non-utf8 path {}", rel.display()))?;
            let spec = literal(p);
            if g.run(&["checkout", &target, "--", &spec])?.status.success() {
                continue;
            }
            // Absent in the target: the acknowledged desired value is deletion.
            g.ok(&["rm", "-q", "-f", "--ignore-unmatch", "--", &spec])?;
        }
        stage_worktree(g, &self.worktree)?;
        g.ok(&["commit", "--allow-empty", "-m", "reconcile baseline"])?;
        let base = g
            .rev("HEAD")
            .ok_or_else(|| anyhow!("reconcile baseline has no commit"))?;

        // L: the same baseline carrying whatever the root holds right now.
        mirror::root_to_staging(
            &repo.root,
            &self.worktree,
            &entries,
            &repo.ignore_text(home_dir),
            repo.limits,
        )?;
        stage_worktree(g, &self.worktree)?;
        g.ok(&[
            "commit",
            "--allow-empty",
            "-m",
            &format!("local {}", repo.id8()),
        ])?;
        let local = g
            .rev("HEAD")
            .ok_or_else(|| anyhow!("reconcile local has no commit"))?;

        // T: the desired tree, still descended from the merged remote history.
        let t = g.ok(&[
            "commit-tree",
            "-m",
            "merged",
            "-p",
            &base,
            "-p",
            &target,
            &format!("{target}^{{tree}}"),
        ])?;
        let out = g.run(&["merge", "--no-edit", &t])?;
        if !out.status.success() {
            // Both sides are this device's own view of the root here, so the
            // loser sibling carries our id8 either way.
            (self.resolve_index)(g, &t, &repo.my_id, &repo.my_id)?;
            g.ok(&["commit", "--no-edit", "-m", "reconcile"])?;
        }

        let new_target = g
            .rev("HEAD")
            .ok_or_else(|| anyhow!("reconcile produced no commit"))?;
        repo.git
            .ok(&["update-ref", &tx_ref(&self.id, "base"), &base])?;
        repo.git
            .ok(&["update-ref", &tx_ref(&self.id, "local"), &local])?;
        repo.git
            .ok(&["update-ref", &tx_ref(&self.id, "target"), &new_target])?;

        // `local` is what the root holds right now — baseline values for the
        // paths this transaction never reached, acknowledged target values for
        // the ones it did, and the user's own edits on top — so it is the
        // expectation the retry must check against, and the applied set starts
        // over from it.
        self.j.changes = repo
            .changes_since(Some(&local), &new_target, &entries)?
            .into_iter()
            .map(|(s, p)| (s.into(), p))
            .collect();
        self.j.pre = Some(local);
        self.j.target = Some(new_target);
        self.j.applied.clear();
        self.j.phase = Phase::Applying;
        self.write_journal()?;
        Ok(true)
    }

    /// Recreate the scratch worktree when a temp sweep removed it.
    fn ensure_worktree(&self, repo: &Repo) -> Result<()> {
        if self.worktree.join(".git").exists() {
            return Ok(());
        }
        let at = self
            .j
            .target
            .clone()
            .unwrap_or_else(|| self.j.start.clone());
        home_tmp_dir(&repo.home)?;
        let _ = fs::remove_dir_all(&self.worktree);
        repo.git.ok(&["worktree", "prune"])?;
        repo.git.ok(&[
            "worktree",
            "add",
            "-q",
            "--detach",
            self.worktree
                .to_str()
                .ok_or_else(|| anyhow!("non-utf8 worktree path"))?,
            &at,
        ])?;
        Ok(())
    }

    fn cleanup(&self, repo: &Repo) -> Result<()> {
        if let Some(w) = self.worktree.to_str() {
            let _ = repo.git.run(&["worktree", "remove", "--force", w]);
        }
        let _ = fs::remove_dir_all(&self.worktree);
        let _ = repo.git.run(&["worktree", "prune"]);
        for name in ["start", "pre", "target", "base", "local"] {
            let _ = repo.git.run(&["update-ref", "-d", &tx_ref(&self.id, name)]);
        }
        // Last: while it exists, the transaction is still owed.
        match fs::remove_file(&self.journal_path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).with_context(|| format!("removing {}", self.journal_path.display())),
        }
    }

    fn write_journal(&self) -> Result<()> {
        write_durable(&self.journal_path, &serde_json::to_vec(&self.j)?)
    }
}

// --- journal and delivery state -------------------------------------------

/// `Status` as stored in the journal; `mirror::Status` is not serializable and
/// is not ours to change.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
enum JStatus {
    Added,
    Modified,
    Deleted,
}

impl From<Status> for JStatus {
    fn from(s: Status) -> JStatus {
        match s {
            Status::Added => JStatus::Added,
            Status::Modified => JStatus::Modified,
            Status::Deleted => JStatus::Deleted,
        }
    }
}

impl From<JStatus> for Status {
    fn from(s: JStatus) -> Status {
        match s {
            JStatus::Added => Status::Added,
            JStatus::Modified => Status::Modified,
            JStatus::Deleted => Status::Deleted,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
struct Journal {
    version: u32,
    slug: String,
    id: String,
    worktree: PathBuf,
    provider_key: String,
    phase: Phase,
    /// Pre-merge head: what the live root is expected to match.
    pre: Option<String>,
    /// Where the scratch worktree was detached.
    start: String,
    target: Option<String>,
    changes: Vec<(JStatus, PathBuf)>,
    applied: Vec<PathBuf>,
    /// The path a write was started for but not yet acknowledged.
    intent: Option<(JStatus, PathBuf)>,
    attempts: u32,
}

/// Per-provider delivery state, kept out of the cloud and out of config.
#[derive(Serialize, Deserialize, Default, Clone, PartialEq, Eq, Debug)]
struct State {
    #[serde(default)]
    providers: BTreeMap<String, ProviderState>,
}

#[derive(Serialize, Deserialize, Default, Clone, PartialEq, Eq, Debug)]
struct ProviderState {
    /// The path the key was derived from, so a hash collision is an error
    /// rather than two providers quietly sharing one counter.
    path: PathBuf,
    #[serde(default)]
    consumed: BTreeMap<String, u64>,
    #[serde(default)]
    own_next_seq: u64,
}

fn provider_slot<'a>(
    state: &'a mut State,
    key: &str,
    base: &Path,
) -> Result<&'a mut ProviderState> {
    let slot = state
        .providers
        .entry(key.to_string())
        .or_insert_with(|| ProviderState {
            path: base.to_path_buf(),
            ..ProviderState::default()
        });
    if slot.path != base {
        bail!(
            "provider key {key} already belongs to {}, not {}",
            slot.path.display(),
            base.display()
        );
    }
    Ok(slot)
}

// --- free helpers ---------------------------------------------------------

/// The delivery-state namespace for a provider: the git blob hash of its
/// canonical base path.
///
/// Switching provider must not reuse another provider's acknowledgements, and
/// the key ends up inside ref names, so it has to be short and ref-safe.
pub fn provider_key(cloud: &Cloud) -> String {
    let path = cloud.base.as_os_str().as_bytes();
    let mut msg = format!("blob {}\0", path.len()).into_bytes();
    msg.extend_from_slice(path);
    hex(&sha1(&msg))
}

/// `refs/remotes/<provider-key>/<device-id>/main` — the only place another
/// device's head is recorded, namespaced so a provider switch starts clean.
pub fn remote_ref(provider_key: &str, device_id: &str) -> String {
    format!("refs/remotes/{provider_key}/{device_id}/main")
}

fn tx_ref(id: &str, name: &str) -> String {
    format!("refs/dotlore/tx/{id}/{name}")
}

fn remote_heads(repo: &Repo, provider_key: &str) -> Result<Vec<String>> {
    let prefix = format!("refs/remotes/{provider_key}/");
    let out = repo
        .git
        .ok(&["for-each-ref", "--format=%(objectname)", &prefix])?;
    Ok(out.lines().map(|l| l.trim().to_string()).collect())
}

fn short8(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

/// A relative path made only of plain names: no `..`, no root, no prefix.
fn plain_rel(rel: &Path) -> bool {
    !rel.as_os_str().is_empty() && rel.components().all(|c| matches!(c, Component::Normal(_)))
}

/// `[a-z0-9]+(-[a-z0-9]+)*` — also the shape that makes a slug safe as a
/// single path component.
fn valid_slug(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && !s.ends_with('-')
        && !s.contains("--")
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Device ids come out of cloud directory names and go into ref names.
fn valid_device(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// Staging-private entries, and names git can carry but we cannot pass to it.
fn usable_entry(path: &[u8]) -> Option<PathBuf> {
    // `Git::run` takes `&str`, so a name we cannot decode could never be
    // handed back to git. Staging never creates one; another device might.
    let p = PathBuf::from(OsStr::from_bytes(path));
    if p.as_os_str().is_empty() || p.to_str().is_none() {
        return None;
    }
    let private = p.components().any(|c| match c {
        Component::Normal(n) => crate::project::staging_private(&n.to_string_lossy()),
        _ => true,
    });
    if private {
        None
    } else {
        Some(p)
    }
}

/// Stage exactly what the mirror left in the staging worktree.
///
/// Literal pathspecs with `-f`: a `.gitignore` that happens to have been
/// copied in from the tracked root governs that project's repo, never ours,
/// and must not be able to drop a file out of the synced set. `git add -u`
/// then picks up tracked files the mirror deleted.
fn stage_worktree(git: &Git, worktree: &Path) -> Result<()> {
    let mut files = Vec::new();
    collect_files(worktree, Path::new(""), &mut files)?;
    files.sort();
    for chunk in files.chunks(ADD_CHUNK) {
        let mut args = vec!["add", "-f", "--"];
        let specs: Vec<String> = chunk.iter().map(|f| literal(f)).collect();
        args.extend(specs.iter().map(String::as_str));
        git.ok(&args)?;
    }
    git.ok(&["add", "-u"])?;
    Ok(())
}

/// `:(literal)` disables pathspec globbing, so a file really called `a[b].md`
/// is staged as itself.
fn literal(rel: &str) -> String {
    format!(":(literal){rel}")
}

fn collect_files(base: &Path, rel: &Path, out: &mut Vec<String>) -> Result<()> {
    let dir = base.join(rel);
    for entry in fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if name == ".git" {
            continue;
        }
        let child = rel.join(&name);
        let md = fs::symlink_metadata(entry.path())?;
        if md.is_dir() {
            collect_files(base, &child, out)?;
        } else if let Some(s) = child.to_str() {
            out.push(s.to_string());
        }
    }
    Ok(())
}

/// What the target wants this path to hold.
///
/// Outer `None` means the source is unusable (missing, or not a regular file —
/// git stores a symlink as a mode-120000 blob), which is drift to report, not
/// an absence to act on.
fn desired_state(worktree: &Path, rel: &Path, status: Status) -> Result<Option<Option<FileState>>> {
    if status == Status::Deleted {
        return Ok(Some(None));
    }
    let src = worktree.join(rel);
    let md = match fs::symlink_metadata(&src) {
        Ok(md) => md,
        Err(_) => return Ok(None),
    };
    if !md.file_type().is_file() {
        return Ok(None);
    }
    Ok(Some(Some(FileState {
        bytes: fs::read(&src).with_context(|| format!("reading {}", src.display()))?,
        executable: md.permissions().mode() & 0o111 != 0,
    })))
}

/// What the live root holds. Outer `None` means "not something we may own
/// here": a symlink on the path, a directory where a file belongs, or a path
/// that cannot be stat'ed at all.
fn live_state(root: &Path, target: &Path) -> Result<Option<Option<FileState>>> {
    let mut at = Some(target);
    while let Some(p) = at {
        if !p.starts_with(root) {
            break;
        }
        if fs::symlink_metadata(p)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            return Ok(None);
        }
        at = p.parent();
    }
    match fs::symlink_metadata(target) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Some(None)),
        Err(_) => Ok(None),
        Ok(md) if md.is_file() => Ok(Some(Some(FileState {
            bytes: fs::read(target)?,
            executable: md.permissions().mode() & 0o111 != 0,
        }))),
        Ok(_) => Ok(None),
    }
}

/// Remove the temp files a crashed write of `rel` left in the live root.
///
/// The only reader of `Journal::intent`, and the reason it is written: a crash
/// inside `mirror::write_atomic` between its write and its rename leaves a
/// full copy of the tracked bytes beside the target, and `mirror`'s
/// `protected_name` makes every later mirror pass ignore that name forever —
/// so without this the copy stays in the user's real root for good. Best
/// effort throughout: an unwritable or vanished parent is litter left behind,
/// never a failed resume.
///
/// Matches `write_atomic`'s `.<name>.<pid>.dotlore-tmp` by prefix and suffix,
/// because the pid in the name belongs to the process that died. That is a
/// coupling to a private name in `mirror`; the two must change together.
fn sweep_tmp_litter(repo: &Repo, rel: &Path) {
    let Ok(live) = repo.live_path(rel) else {
        return;
    };
    let (Some(dir), Some(name)) = (live.parent(), live.file_name()) else {
        return;
    };
    let prefix = format!(".{}.", name.to_string_lossy());
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let found = entry.file_name().to_string_lossy().into_owned();
        // `remove_file` unlinks a symlink rather than following it, so a
        // planted link of this name costs its target nothing.
        if found.starts_with(&prefix) && found.ends_with(".dotlore-tmp") {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// `<home>/tmp`, created and kept at 0700.
///
/// Everything staged here — the outgoing bundle, a transaction worktree — is a
/// copy of every tracked byte. `git bundle create` makes its file itself at a
/// umask-derived mode, so dotlore cannot close that window on the file; a 0700
/// parent closes it on the directory instead, since no other local account can
/// open, or even stat, an entry in a directory it cannot search. Chmodded on
/// every call rather than only on creation: that is self-healing, and it needs
/// no race-free "did I create it" check.
///
/// `<home>` itself is the wider perimeter — `<home>/repos/<slug>` holds the
/// same bytes at rest, permanently — and it is created by `config::lock`,
/// which every operation calls first. That is where its mode belongs.
fn home_tmp_dir(home: &Path) -> Result<PathBuf> {
    let dir = home.join("tmp");
    fs::create_dir_all(&dir)?;
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("securing {}", dir.display()))?;
    Ok(dir)
}

/// A name no other process can predict: 16 bytes of `/dev/urandom` as hex, the
/// same source `config::random_id` uses.
///
/// Names built from this are the only thing standing between a temp path and a
/// symlink pre-planted at it by another process of the same user, so the pid +
/// wall-clock nanos this used to be was not enough: the pid is readable and
/// the nanos are enumerable by anyone willing to plant a dense set of links
/// over a future window. That pair is kept only as the fallback for an
/// unreadable `/dev/urandom` (no `/dev` in a sandbox, descriptors exhausted) —
/// unique, so nothing collides or corrupts, merely guessable. Failing a whole
/// sync over it would be the worse trade.
pub(crate) fn unique_id() -> String {
    let mut buf = [0u8; 16];
    if fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut buf))
        .is_ok()
    {
        return buf.iter().map(|b| format!("{b:02x}")).collect();
    }
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:x}-{nanos:x}", std::process::id())
}

/// Atomically replace `path` and make it durable before the rename is visible.
/// Same shape as `mirror::write_atomic`: `O_EXCL` refuses a pre-planted
/// symlink, the mode is restored on the handle, and `sync_all` runs before the
/// rename so a crash cannot leave a journal that is visible but truncated.
fn write_durable(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| anyhow!("no parent directory for {}", path.display()))?;
    fs::create_dir_all(dir)?;
    let name = path
        .file_name()
        .ok_or_else(|| anyhow!("bad file name for {}", path.display()))?
        .to_string_lossy()
        .into_owned();
    let tmp = dir.join(format!(".{name}.{}.tmp", unique_id()));
    let res = (|| -> Result<()> {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(bytes)?;
        f.set_permissions(fs::Permissions::from_mode(0o600))?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, path)?;
        Ok(())
    })();
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    res.with_context(|| format!("writing {}", path.display()))
}

// --- sha1 -----------------------------------------------------------------

/// SHA-1 of `data`. Used only to name a provider's delivery namespace, so that
/// `provider_key` is a real `git hash-object` id (pinned by a test) without a
/// hash crate; the locked dependency set has none and stdlib's hasher is not
/// stable across toolchains.
fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let bits = (data.len() as u64).wrapping_mul(8);
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bits.to_be_bytes());

    for block in msg.chunks_exact(64) {
        let mut w = [0u32; 80];
        for (i, c) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([c[0], c[1], c[2], c[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A827999),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let t = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }
    let mut out = [0u8; 20];
    for (i, v) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const ID_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const ID_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    /// Task 2 owns the real policy; nothing here reaches an unmerged index.
    fn no_resolver(_: &Git, _: &str, _: &str, _: &str) -> Result<Vec<PathBuf>> {
        bail!("index resolver not expected in this test")
    }

    struct Fx {
        home: TempDir,
        root: TempDir,
        provider: TempDir,
        repo: Repo,
    }

    fn fixture() -> Fx {
        let home = TempDir::new().unwrap();
        let root = TempDir::new().unwrap();
        let provider = TempDir::new().unwrap();
        let repo = Repo::init(home.path(), "proj-claude", root.path(), "Mac A Pro", ID_A).unwrap();
        Fx {
            home,
            root,
            provider,
            repo,
        }
    }

    impl Fx {
        fn cloud(&self) -> Cloud {
            Cloud {
                base: self.provider.path().join("dotlore"),
            }
        }
        fn write(&self, rel: &str, body: &[u8]) {
            let p = self.root.path().join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, body).unwrap();
            self.track(&[rel]);
        }
        fn track(&self, keys: &[&str]) {
            let mut file = project::read(&self.repo.staging).unwrap();
            for k in keys {
                file.entries.insert(
                    (*k).to_string(),
                    project::EntryRecord {
                        gen: 1,
                        state: project::State::Tracked,
                    },
                );
            }
            project::write(&self.repo.staging, &file).unwrap();
        }
        fn commit(&self) -> bool {
            self.repo.commit_local(false, self.home.path()).unwrap().0
        }
    }

    // --- provider key -----------------------------------------------------

    /// `provider_key` claims to be a git blob id; if the hand-rolled SHA-1
    /// drifts from that, this fails.
    #[test]
    fn provider_key_is_the_git_hash_of_the_base_path() {
        let fx = fixture();
        let cloud = fx.cloud();
        let path_bytes = cloud.base.as_os_str().as_bytes().to_vec();
        let f = fx.home.path().join("keysrc");
        fs::write(&f, &path_bytes).unwrap();
        let expect = fx
            .repo
            .git
            .ok(&["hash-object", "--", f.to_str().unwrap()])
            .unwrap();

        assert_eq!(provider_key(&cloud), expect);
        assert_eq!(provider_key(&cloud).len(), 40);
    }

    #[test]
    fn provider_key_separates_two_providers() {
        let a = Cloud {
            base: PathBuf::from("/tmp/one/dotlore"),
        };
        let b = Cloud {
            base: PathBuf::from("/tmp/two/dotlore"),
        };
        assert_ne!(provider_key(&a), provider_key(&b));
    }

    // --- commit_local -----------------------------------------------------

    /// I6: a `.gitignore` copied in from the tracked root governs that
    /// project's repo, not ours. Drop `-f`/literal pathspecs and `docs/x.md`
    /// silently stops syncing.
    #[test]
    fn a_copied_gitignore_cannot_exclude_a_mirrored_file() {
        let fx = fixture();
        fx.write(".gitignore", b"docs/\n*.md\n");
        fx.write("docs/x.md", b"x");
        fx.write("docs/nested/.gitignore", b"*\n");
        fx.write("docs/nested/y.md", b"y");
        fx.write("we[i]rd/z.md", b"z");

        assert!(fx.commit());
        let tracked = fx.repo.git.ok(&["ls-files"]).unwrap();
        for want in [
            ".gitignore",
            "docs/x.md",
            "docs/nested/.gitignore",
            "docs/nested/y.md",
            "we[i]rd/z.md",
        ] {
            assert!(
                tracked.lines().any(|l| l == want),
                "missing {want}:\n{tracked}"
            );
        }
    }

    #[test]
    fn commit_local_is_idempotent_and_records_deletions() {
        let fx = fixture();
        fx.write("CLAUDE.md", b"one\n");
        assert!(fx.commit());
        assert!(!fx.commit(), "a quiet root must not mint a commit");

        fs::remove_file(fx.root.path().join("CLAUDE.md")).unwrap();
        assert!(fx.commit());
        let tracked = fx.repo.git.ok(&["ls-files"]).unwrap();
        assert!(
            !tracked.lines().any(|l| l == "CLAUDE.md"),
            "deleted live file must leave the index: {tracked}"
        );
        assert_eq!(tracked, ".dotloreproject");
    }

    #[test]
    fn allow_empty_first_only_applies_to_a_repo_without_main() {
        let fx = fixture();
        assert!(!fx.repo.has_main());
        assert!(fx.repo.commit_local(true, fx.home.path()).unwrap().0);
        assert!(fx.repo.has_main());
        assert!(!fx.repo.commit_local(true, fx.home.path()).unwrap().0);
    }

    // --- changes_since ----------------------------------------------------

    #[test]
    fn changes_since_parses_name_status_and_drops_private_entries() {
        let fx = fixture();
        fx.write("CLAUDE.md", b"one\n");
        fx.write("gone.md", b"g");
        fx.commit();
        let pre = fx.repo.git.rev("HEAD").unwrap();

        fx.write("CLAUDE.md", b"two\n");
        fx.write("new file.md", b"n");
        fs::remove_file(fx.root.path().join("gone.md")).unwrap();
        // Staging-private entries: committed here, never applied to a root.
        fs::write(fx.repo.staging.join(".dotloreignore"), "x\n").unwrap();
        project::write(&fx.repo.staging, &fx.repo.project_file().unwrap()).unwrap();
        fs::write(
            fx.repo.staging.join("CLAUDE.conflict-bbbbbbbb-1234567.md"),
            "loser",
        )
        .unwrap();
        fs::write(fx.repo.staging.join("a.dotlore-tmp"), "tmp").unwrap();
        fx.commit();
        // Re-add the project file so `usable_entry` is what drops it.
        project::write(&fx.repo.staging, &fx.repo.project_file().unwrap()).unwrap();
        fx.repo
            .git
            .ok(&["add", "-f", "--", ":(literal).dotloreproject"])
            .unwrap();
        if !fx
            .repo
            .git
            .run(&["diff", "--cached", "--quiet"])
            .unwrap()
            .status
            .success()
        {
            fx.repo
                .git
                .ok(&["commit", "-m", "private peer paths"])
                .unwrap();
        }
        let target = fx.repo.git.rev("HEAD").unwrap();

        let mut got = fx
            .repo
            .changes_since(
                Some(&pre),
                &target,
                &fx.repo.project_file().unwrap().tracked(),
            )
            .unwrap();
        got.sort_by(|a, b| a.1.cmp(&b.1));
        assert_eq!(
            got,
            vec![
                (Status::Modified, PathBuf::from("CLAUDE.md")),
                (Status::Deleted, PathBuf::from("gone.md")),
                (Status::Added, PathBuf::from("new file.md")),
            ]
        );
        // Git refuses to store `.git/x` in a tree, so pin the predicate
        // `changes_since` uses. `a.dotlore-tmp` is also in the peer tree
        // above; this names the path the assert_eq would miss if it leaked.
        assert!(usable_entry(b".git/x").is_none());
        assert!(usable_entry(b"a.dotlore-tmp").is_none());
    }

    #[test]
    fn changes_since_none_lists_regular_files_only() {
        let fx = fixture();
        fx.write("CLAUDE.md", b"one\n");
        fx.commit();
        // A symlink can only reach staging from another device's commit.
        std::os::unix::fs::symlink("CLAUDE.md", fx.repo.staging.join("link.md")).unwrap();
        fx.repo
            .git
            .ok(&["add", "-f", "--", ":(literal)link.md"])
            .unwrap();
        fx.repo.git.ok(&["commit", "-m", "link"]).unwrap();
        let target = fx.repo.git.rev("HEAD").unwrap();

        let got = fx
            .repo
            .changes_since(None, &target, &fx.repo.project_file().unwrap().tracked())
            .unwrap();
        assert_eq!(got, vec![(Status::Added, PathBuf::from("CLAUDE.md"))]);
    }

    fn tracked_list(keys: &[&str]) -> EntryList {
        let mut file = ProjectFile::default();
        for k in keys {
            file.entries.insert(
                (*k).to_string(),
                project::EntryRecord {
                    gen: 1,
                    state: project::State::Tracked,
                },
            );
        }
        file.tracked()
    }

    #[test]
    fn changes_since_drops_entries_outside_the_include_list() {
        let fx = fixture();
        assert!(fx.repo.commit_local(true, fx.home.path()).unwrap().0);
        let pre = fx.repo.git.rev("HEAD").unwrap();

        fx.write("in.md", b"in");
        fx.write("out.md", b"out");
        fx.commit();
        let target = fx.repo.git.rev("HEAD").unwrap();
        let only_in = tracked_list(&["in.md"]);

        let mut got = fx
            .repo
            .changes_since(Some(&pre), &target, &only_in)
            .unwrap();
        got.sort_by(|a, b| a.1.cmp(&b.1));
        assert_eq!(got, vec![(Status::Added, PathBuf::from("in.md"))]);

        let got = fx.repo.changes_since(None, &target, &only_in).unwrap();
        assert_eq!(got, vec![(Status::Added, PathBuf::from("in.md"))]);
    }

    #[test]
    fn a_bootstrap_link_change_set_is_scoped_to_the_target_include_list() {
        let fx = fixture();
        fx.write("keep.md", b"keep");
        fx.write("secret.md", b"secret");
        fx.commit();

        let mut file = fx.repo.project_file().unwrap();
        file.untrack(Path::new("secret.md")).unwrap();
        project::write(&fx.repo.staging, &file).unwrap();
        fx.repo
            .git
            .ok(&["add", "-f", "--", ":(literal).dotloreproject"])
            .unwrap();
        fx.repo
            .git
            .ok(&["commit", "-m", "tombstone secret.md"])
            .unwrap();

        let target = fx.repo.git.rev("HEAD").unwrap();
        let tree = fx.repo.ls_tree(&target).unwrap();
        assert!(
            tree.iter().any(|(_, p)| p == Path::new("secret.md")),
            "tombstoned blob must remain in the tree: {tree:?}"
        );

        let entries = fx.repo.project_file().unwrap().tracked();
        let got = fx.repo.changes_since(None, &target, &entries).unwrap();
        assert!(
            !got.iter().any(|(_, p)| p == Path::new("secret.md")),
            "tombstoned blob must not enter a bootstrap change set: {got:?}"
        );
        assert_eq!(got, vec![(Status::Added, PathBuf::from("keep.md"))]);
    }

    // --- expected_bytes ---------------------------------------------------

    /// The discrimination the fail-closed apply rests on: "not in that commit"
    /// is an absence, anything else is an error. Collapse the two and a
    /// transient git failure starts authorising writes over live files.
    #[test]
    fn expected_bytes_separates_an_absent_path_from_a_git_error() {
        let fx = fixture();
        fx.write("CLAUDE.md", b"one\n");
        fx.write("hooks/run.sh", b"#!/bin/sh\n");
        fs::set_permissions(
            fx.root.path().join("hooks/run.sh"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        fx.commit();
        let pre = fx.repo.git.rev("HEAD").unwrap();

        let snap = fx
            .repo
            .expected_bytes(
                Some(&pre),
                &[
                    PathBuf::from("CLAUDE.md"),
                    PathBuf::from("hooks/run.sh"),
                    PathBuf::from("nope.md"),
                ],
            )
            .unwrap();
        assert_eq!(
            snap[Path::new("CLAUDE.md")],
            Some(FileState {
                bytes: b"one\n".to_vec(),
                executable: false
            })
        );
        assert!(snap[Path::new("hooks/run.sh")].as_ref().unwrap().executable);
        assert_eq!(snap[Path::new("nope.md")], None);

        // A git failure that is not "this path is not in that commit" must
        // stay an error. `git show` exits 128 for both, so only the message
        // tells them apart — and treating the wrong one as an absence hands
        // `apply_to_root` an expectation of "absent" that authorises a write.
        let err = fx
            .repo
            .expected_bytes(Some(&pre), &[PathBuf::from("../outside.md")])
            .unwrap_err();
        assert!(err.to_string().contains("outside"), "{err}");

        let err = fx
            .repo
            .expected_bytes(Some("no-such-rev"), &[PathBuf::from("CLAUDE.md")])
            .unwrap_err();
        assert!(err.to_string().contains("no-such-rev"), "{err}");
    }

    #[test]
    fn expected_bytes_without_a_pre_commit_expects_absence() {
        let fx = fixture();
        let snap = fx
            .repo
            .expected_bytes(None, &[PathBuf::from("CLAUDE.md")])
            .unwrap();
        assert_eq!(snap[Path::new("CLAUDE.md")], None);
    }

    // --- delivery state ---------------------------------------------------

    /// A device's sequence is local state; the cloud listing is only a floor.
    /// Read the next seq from the cloud alone and an evicted bundle hands its
    /// number back out, which `publish_bundle` then refuses forever.
    #[test]
    fn own_sequence_survives_an_evicted_own_bundle() {
        let fx = fixture();
        let cloud = fx.cloud();
        fx.write("CLAUDE.md", b"one\n");
        fx.commit();
        assert_eq!(fx.repo.publish(&cloud).unwrap(), Some(1));
        fx.write("CLAUDE.md", b"two\n");
        fx.commit();
        assert_eq!(fx.repo.publish(&cloud).unwrap(), Some(2));

        let dev = cloud
            .slug_dir("proj-claude")
            .unwrap()
            .join("devices")
            .join(ID_A);
        fs::rename(dev.join("000002.bundle"), dev.join(".000002.bundle.icloud")).unwrap();

        fx.write("CLAUDE.md", b"three\n");
        fx.commit();
        assert_eq!(fx.repo.publish(&cloud).unwrap(), Some(3));
        assert!(dev.join("000003.bundle").is_file());
    }

    #[test]
    fn publish_is_skipped_when_main_is_already_sent() {
        let fx = fixture();
        let cloud = fx.cloud();
        fx.write("CLAUDE.md", b"one\n");
        fx.commit();
        assert_eq!(fx.repo.publish(&cloud).unwrap(), Some(1));
        assert_eq!(fx.repo.publish(&cloud).unwrap(), None);
    }

    /// Delivery state is per provider: a switch must not inherit the other
    /// provider's acknowledgements or counter.
    #[test]
    fn a_second_provider_gets_its_own_sequence() {
        let fx = fixture();
        fx.write("CLAUDE.md", b"one\n");
        fx.commit();
        assert_eq!(fx.repo.publish(&fx.cloud()).unwrap(), Some(1));

        let other = TempDir::new().unwrap();
        let cloud2 = Cloud {
            base: other.path().join("dotlore"),
        };
        assert_eq!(fx.repo.publish(&cloud2).unwrap(), Some(1));
        assert!(other
            .path()
            .join("dotlore/proj-claude/devices")
            .join(ID_A)
            .join("000001.bundle")
            .is_file());
    }

    // --- fetch_bundles ----------------------------------------------------

    /// Two devices publishing into one provider folder.
    fn two_devices() -> (Fx, Repo, TempDir) {
        let fx = fixture();
        fx.write("CLAUDE.md", b"one\n");
        fx.commit();
        fx.repo.publish(&fx.cloud()).unwrap();

        let b_home = TempDir::new().unwrap();
        let b = Repo::init(
            b_home.path(),
            "proj-claude",
            fx.root.path(),
            "Mac B Pro",
            ID_B,
        )
        .unwrap();
        (fx, b, b_home)
    }

    #[test]
    fn fetch_creates_the_remote_ref_and_records_consumed() {
        let (fx, b, _bh) = two_devices();
        let cloud = fx.cloud();
        let key = provider_key(&cloud);

        let devs = b.fetch_bundles(&cloud, FetchMode::Normal).unwrap();
        assert_eq!(devs, vec![ID_A.to_string()]);
        assert!(b.git.rev(&remote_ref(&key, ID_A)).is_some());
        assert_eq!(
            provider_slot(&mut b.load_state().unwrap(), &key, &cloud.base)
                .unwrap()
                .consumed
                .get(ID_A),
            Some(&1)
        );
    }

    /// An unreadable bundle stops that device's chain for the pass without
    /// touching `consumed`, so the later bundle is not skipped over once the
    /// missing one materialises. Bump `consumed` on failure and the device
    /// loses that commit permanently.
    #[test]
    fn a_corrupt_bundle_stops_the_chain_and_keeps_consumed() {
        let (fx, b, _bh) = two_devices();
        let cloud = fx.cloud();
        let key = provider_key(&cloud);
        fx.write("CLAUDE.md", b"two\n");
        fx.commit();
        fx.repo.publish(&cloud).unwrap();
        fx.write("CLAUDE.md", b"three\n");
        fx.commit();
        fx.repo.publish(&cloud).unwrap();

        let dev = cloud
            .slug_dir("proj-claude")
            .unwrap()
            .join("devices")
            .join(ID_A);
        let good = fs::read(dev.join("000002.bundle")).unwrap();
        fs::write(dev.join("000002.bundle"), b"not a bundle").unwrap();

        b.fetch_bundles(&cloud, FetchMode::Normal).unwrap();
        assert_eq!(
            provider_slot(&mut b.load_state().unwrap(), &key, &cloud.base)
                .unwrap()
                .consumed
                .get(ID_A),
            Some(&1),
            "a failed verify must not acknowledge the bundle"
        );
        let after_stop = b.git.rev(&remote_ref(&key, ID_A));

        fs::write(dev.join("000002.bundle"), &good).unwrap();
        b.fetch_bundles(&cloud, FetchMode::Normal).unwrap();
        assert_eq!(
            provider_slot(&mut b.load_state().unwrap(), &key, &cloud.base)
                .unwrap()
                .consumed
                .get(ID_A),
            Some(&3),
            "the rest of the chain must follow once the gap is filled"
        );
        assert_ne!(after_stop, b.git.rev(&remote_ref(&key, ID_A)));
    }

    #[test]
    fn normal_mode_skips_own_bundles_and_recovery_does_not() {
        let fx = fixture();
        let cloud = fx.cloud();
        fx.write("CLAUDE.md", b"one\n");
        fx.commit();
        fx.repo.publish(&cloud).unwrap();
        let key = provider_key(&cloud);

        assert!(fx
            .repo
            .fetch_bundles(&cloud, FetchMode::Normal)
            .unwrap()
            .is_empty());
        assert!(fx.repo.git.rev(&remote_ref(&key, ID_A)).is_none());

        assert_eq!(
            fx.repo.fetch_bundles(&cloud, FetchMode::Recovery).unwrap(),
            vec![ID_A.to_string()]
        );
        assert!(fx.repo.git.rev(&remote_ref(&key, ID_A)).is_some());
    }

    // --- transaction ------------------------------------------------------

    #[test]
    fn a_journalled_transaction_round_trips_and_blocks_a_second_one() {
        let fx = fixture();
        let key = provider_key(&fx.cloud());
        fx.write("CLAUDE.md", b"one\n");
        fx.commit();
        let head = fx.repo.git.rev("HEAD").unwrap();

        let tx = fx.repo.begin_tx(&key, "HEAD", no_resolver).unwrap();
        assert_eq!(tx.phase(), Phase::Preparing);
        assert_eq!(tx.pre(), Some(head.as_str()));
        assert!(tx.worktree.join("CLAUDE.md").is_file());
        assert!(fx.repo.begin_tx(&key, "HEAD", no_resolver).is_err());

        let again = fx.repo.pending_tx(no_resolver).unwrap().unwrap();
        assert_eq!(again.phase(), Phase::Preparing);
        assert_eq!(again.pre(), Some(head.as_str()));
        assert_eq!(again.worktree, tx.worktree);
        assert_eq!(again.provider_key, key);
    }

    /// The whole point of the transaction: merging must not move `main` or the
    /// live root until the apply says so.
    #[test]
    fn merging_leaves_main_and_the_root_untouched_until_finalize() {
        let (fx, _b, bh) = two_devices();
        let cloud = fx.cloud();
        let key = provider_key(&cloud);
        // B links the same cloud slug onto its own empty root.
        let b_root = TempDir::new().unwrap();
        let b = Repo::open(bh.path(), "proj-claude", b_root.path(), "Mac B Pro", ID_B).unwrap();
        b.fetch_bundles(&cloud, FetchMode::Normal).unwrap();

        let mut tx = b
            .begin_tx(&key, &remote_ref(&key, ID_A), no_resolver)
            .unwrap();
        assert!(!b.has_main(), "main must not exist yet");
        tx.set_target(&b).unwrap();
        assert_eq!(
            tx.changes(),
            vec![(Status::Added, PathBuf::from("CLAUDE.md"))]
        );
        assert!(!b.has_main(), "set_target must not advance main");
        assert!(!b_root.path().join("CLAUDE.md").exists());

        assert!(tx.apply(&b, bh.path()).unwrap().is_empty());
        assert_eq!(
            fs::read(b_root.path().join("CLAUDE.md")).unwrap(),
            b"one\n",
            "apply writes the live root"
        );
        assert!(!b.has_main(), "apply must not advance main either");
        assert_eq!(tx.phase(), Phase::Finalizing);

        tx.finalize(&b).unwrap();
        assert!(b.has_main());
        assert_eq!(
            b.git.rev("refs/heads/main"),
            fx.repo.git.rev("refs/heads/main")
        );
        assert!(b.pending_tx(no_resolver).unwrap().is_none());
        assert!(!tx.worktree.exists());
        // A's head came in over a bundle, so it is already in the cloud.
        assert_eq!(b.publish(&cloud).unwrap(), None);
    }

    /// A crash mid-apply must resume, not re-mirror a half-applied root onto
    /// the merged head.
    #[test]
    fn a_pending_apply_resumes_from_its_journal() {
        let (fx, _b, bh) = two_devices();
        let cloud = fx.cloud();
        let key = provider_key(&cloud);
        let b_root = TempDir::new().unwrap();
        let b = Repo::open(bh.path(), "proj-claude", b_root.path(), "Mac B Pro", ID_B).unwrap();
        b.fetch_bundles(&cloud, FetchMode::Normal).unwrap();

        let mut tx = b
            .begin_tx(&key, &remote_ref(&key, ID_A), no_resolver)
            .unwrap();
        tx.set_target(&b).unwrap();
        drop(tx); // crash before apply

        let mut tx = b.pending_tx(no_resolver).unwrap().unwrap();
        assert_eq!(tx.phase(), Phase::Applying);
        assert_eq!(tx.resume(&b, bh.path()).unwrap(), ResumeOutcome::Finished);
        assert_eq!(fs::read(b_root.path().join("CLAUDE.md")).unwrap(), b"one\n");
        assert!(b.pending_tx(no_resolver).unwrap().is_none());
    }

    /// A partial apply, reachable without forging a journal: one live path is
    /// blocked by a symlink the user put there, the other is written and
    /// acknowledged. The blocked one is reported as *blocked*, not as a race
    /// to retry forever, and removing the symlink lets the journalled
    /// transaction finish on the next resume — the "crash between two applies"
    /// case, with the fail-closed guarantee checked on the symlink's target.
    #[test]
    fn one_blocked_path_leaves_a_partial_apply_that_resumes_once_it_is_cleared() {
        let fx = fixture();
        let cloud = fx.cloud();
        let key = provider_key(&cloud);
        fx.write("CLAUDE.md", b"one\n");
        fx.write("agents/x.md", b"agent x\n");
        fx.commit();
        fx.repo.publish(&cloud).unwrap();

        let bh = TempDir::new().unwrap();
        let b_root = TempDir::new().unwrap();
        let b = Repo::init(bh.path(), "proj-claude", b_root.path(), "Mac B Pro", ID_B).unwrap();
        b.fetch_bundles(&cloud, FetchMode::Normal).unwrap();

        // The user has symlinked one of the incoming files out to a dotfiles
        // repo of their own — a very ordinary setup, and not ours to own.
        let elsewhere = bh.path().join("elsewhere.md");
        fs::write(&elsewhere, b"not ours\n").unwrap();
        fs::create_dir_all(b_root.path().join("agents")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, b_root.path().join("agents/x.md")).unwrap();

        let mut tx = b
            .begin_tx(&key, &remote_ref(&key, ID_A), no_resolver)
            .unwrap();
        tx.set_target(&b).unwrap();
        let skipped = tx.apply(&b, bh.path()).unwrap();
        assert_eq!(skipped, vec![PathBuf::from("agents/x.md")]);
        assert_eq!(
            tx.blocked(),
            skipped,
            "a symlinked live path never clears on its own; calling it a race \
             freezes the root behind an indistinguishable Pending"
        );
        assert_eq!(
            fs::read(b_root.path().join("CLAUDE.md")).unwrap(),
            b"one\n",
            "the paths that are ours are still applied"
        );
        assert_eq!(
            fs::read(&elsewhere).unwrap(),
            b"not ours\n",
            "fail closed: nothing is written through the symlink"
        );
        assert!(b_root
            .path()
            .join("agents/x.md")
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(!b.has_main(), "nothing finalizes while a path is blocked");

        // Same answer on every retry, not a drifting one.
        let mut tx = b.pending_tx(no_resolver).unwrap().unwrap();
        assert_eq!(
            tx.resume(&b, bh.path()).unwrap(),
            ResumeOutcome::Pending(vec![PathBuf::from("agents/x.md")])
        );

        // The user moves their symlink out of the way.
        fs::remove_file(b_root.path().join("agents/x.md")).unwrap();
        let mut tx = b.pending_tx(no_resolver).unwrap().unwrap();
        assert_eq!(tx.resume(&b, bh.path()).unwrap(), ResumeOutcome::Finished);
        assert!(tx.blocked().is_empty());
        assert_eq!(
            fs::read(b_root.path().join("agents/x.md")).unwrap(),
            b"agent x\n"
        );
        assert!(b.pending_tx(no_resolver).unwrap().is_none());
        assert_eq!(
            b.git.rev("refs/heads/main"),
            fx.repo.git.rev("refs/heads/main")
        );
    }

    /// A peer-chosen path outside this device's include-list must never reach
    /// the live root, and must not leak into an error message (the name is an
    /// OSC-52 clipboard write — the error must not echo that path).
    #[test]
    fn an_out_of_list_entry_from_a_peer_never_reaches_the_live_root() {
        const POISON: &str = "\u{1b}]52;c;ZXZpbA==\u{7}x";

        let fx = fixture();
        fx.write("CLAUDE.md", b"one\n");
        fx.commit();
        fs::write(fx.repo.staging.join(POISON), b"peer secret\n").unwrap();
        fx.repo.git.ok(&["add", "-A"]).unwrap();
        fx.repo
            .git
            .ok(&["commit", "-m", "peer planted poison"])
            .unwrap();
        fx.repo.publish(&fx.cloud()).unwrap();

        let bh = TempDir::new().unwrap();
        let b_root = TempDir::new().unwrap();
        let b = Repo::init(bh.path(), "proj-claude", b_root.path(), "Mac B Pro", ID_B).unwrap();
        b.fetch_bundles(&fx.cloud(), FetchMode::Normal).unwrap();

        let key = provider_key(&fx.cloud());
        let mut tx = b
            .begin_tx(&key, &remote_ref(&key, ID_A), no_resolver)
            .unwrap();
        tx.set_target(&b).unwrap();

        let entries = project::read(&tx.worktree).unwrap().tracked();
        let target = tx.git.rev("HEAD").unwrap();
        let changes = match b.changes_since(None, &target, &entries) {
            Ok(c) => c,
            Err(e) => {
                let msg = format!("{e:?}");
                assert!(
                    !msg.contains(POISON) && !msg.contains("\u{1b}"),
                    "error carries peer bytes: {msg:?}"
                );
                panic!("changes_since failed: {e}");
            }
        };
        assert!(
            !changes
                .iter()
                .any(|(_, p)| p.to_string_lossy().contains(POISON)
                    || p.as_os_str().to_string_lossy().contains('\u{1b}')),
            "poison path entered the change set: {changes:?}"
        );
        assert_eq!(changes, vec![(Status::Added, PathBuf::from("CLAUDE.md"))]);

        match tx.apply(&b, bh.path()) {
            Ok(skipped) => {
                assert!(
                    skipped.iter().all(|p| {
                        let s = p.to_string_lossy();
                        !s.contains(POISON) && !s.contains('\u{1b}')
                    }),
                    "blocked paths carry peer bytes: {skipped:?}"
                );
            }
            Err(e) => {
                let msg = format!("{e:?}");
                assert!(
                    !msg.contains(POISON) && !msg.contains("\u{1b}"),
                    "error carries peer bytes: {msg:?}"
                );
                panic!("apply failed: {e}");
            }
        }
        assert!(!b_root.path().join(POISON).exists());
        assert_eq!(fs::read(b_root.path().join("CLAUDE.md")).unwrap(), b"one\n");
        let names: Vec<_> = fs::read_dir(b_root.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert!(
            names
                .iter()
                .all(|n| !n.to_string_lossy().contains('\u{1b}')),
            "poison name written under the live root: {names:?}"
        );
    }

    /// Phase `Preparing` never touched anything, so recovery just redoes the
    /// merges rather than trying to apply a target that was never computed.
    #[test]
    fn resuming_a_preparing_transaction_restarts_it() {
        let fx = fixture();
        let key = provider_key(&fx.cloud());
        fx.write("CLAUDE.md", b"one\n");
        fx.commit();
        let head = fx.repo.git.rev("HEAD").unwrap();
        let tx = fx.repo.begin_tx(&key, "HEAD", no_resolver).unwrap();
        // Simulate a merge that got part-way before the crash.
        fs::write(tx.worktree.join("CLAUDE.md"), "half merged").unwrap();
        drop(tx);

        let mut tx = fx.repo.pending_tx(no_resolver).unwrap().unwrap();
        assert_eq!(
            tx.resume(&fx.repo, fx.home.path()).unwrap(),
            ResumeOutcome::Restart
        );
        assert_eq!(tx.git.rev("HEAD"), Some(head.clone()));
        assert_eq!(fs::read(tx.worktree.join("CLAUDE.md")).unwrap(), b"one\n");
        assert_eq!(fx.repo.git.rev("refs/heads/main"), Some(head));
    }

    /// A live file that changed under the transaction is never overwritten
    /// from the stale expectation: the merge is rebuilt from a baseline that
    /// keeps both sides. Skip reconciliation and the hand edit is lost.
    #[test]
    fn a_concurrent_root_edit_is_reconciled_without_losing_either_side() {
        let lines = |first: &str, twentieth: &str| -> Vec<u8> {
            (1..=30)
                .map(|i| match i {
                    1 => format!("{first}\n"),
                    20 => format!("{twentieth}\n"),
                    n => format!("line {n}\n"),
                })
                .collect::<String>()
                .into_bytes()
        };

        let fx = fixture();
        let cloud = fx.cloud();
        let key = provider_key(&cloud);
        fx.write("CLAUDE.md", &lines("line 1", "line 20"));
        fx.commit();
        fx.repo.publish(&cloud).unwrap();

        // B adopts A's history onto an empty root, so the two share a base.
        let bh = TempDir::new().unwrap();
        let b_root = TempDir::new().unwrap();
        let b = Repo::init(bh.path(), "proj-claude", b_root.path(), "Mac B Pro", ID_B).unwrap();
        b.fetch_bundles(&cloud, FetchMode::Normal).unwrap();
        let mut tx = b
            .begin_tx(&key, &remote_ref(&key, ID_A), no_resolver)
            .unwrap();
        tx.set_target(&b).unwrap();
        assert!(tx.apply(&b, bh.path()).unwrap().is_empty());
        tx.finalize(&b).unwrap();

        // A edits line 1 and publishes; B starts the merge for it.
        fx.write("CLAUDE.md", &lines("A EDIT", "line 20"));
        fx.commit();
        fx.repo.publish(&cloud).unwrap();
        b.fetch_bundles(&cloud, FetchMode::Normal).unwrap();
        let mut tx = b.begin_tx(&key, "HEAD", no_resolver).unwrap();
        assert_eq!(
            b.merge_remote(&tx, ID_A).unwrap(),
            MergeOutcome::FastForward
        );
        tx.set_target(&b).unwrap();
        assert_eq!(
            tx.changes(),
            vec![(Status::Modified, PathBuf::from("CLAUDE.md"))]
        );

        // The user edits line 20 of that very file after the target was
        // pinned but before it was written.
        let hand = lines("line 1", "B EDIT");
        fs::write(b_root.path().join("CLAUDE.md"), &hand).unwrap();

        assert!(tx.apply(&b, bh.path()).unwrap().is_empty());
        tx.finalize(&b).unwrap();

        let got = fs::read(b_root.path().join("CLAUDE.md")).unwrap();
        assert_eq!(got, lines("A EDIT", "B EDIT"), "both edits must survive");
        assert!(b.pending_tx(no_resolver).unwrap().is_none());
        // The merged remote head stays an ancestor, so A is never re-merged.
        let theirs = b.git.rev(&remote_ref(&key, ID_A)).unwrap();
        assert!(b.git.is_ancestor(&theirs, "refs/heads/main"));
    }

    /// Reconcile must mirror with the merged target's include-list, not
    /// staging's. A peer that just tracked `b` would otherwise be missing
    /// from `local` and show up as `Added` against a live root that already
    /// has it.
    #[test]
    fn reconcile_uses_the_merged_targets_include_list() {
        let lines = |first: &str, twentieth: &str| -> Vec<u8> {
            (1..=30)
                .map(|i| match i {
                    1 => format!("{first}\n"),
                    20 => format!("{twentieth}\n"),
                    n => format!("line {n}\n"),
                })
                .collect::<String>()
                .into_bytes()
        };

        let fx = fixture();
        let cloud = fx.cloud();
        let key = provider_key(&cloud);
        fx.write("a", &lines("line 1", "line 20"));
        fx.commit();
        fx.repo.publish(&cloud).unwrap();

        let bh = TempDir::new().unwrap();
        let b_root = TempDir::new().unwrap();
        let b = Repo::init(bh.path(), "proj-claude", b_root.path(), "Mac B Pro", ID_B).unwrap();
        b.fetch_bundles(&cloud, FetchMode::Normal).unwrap();
        let mut tx = b
            .begin_tx(&key, &remote_ref(&key, ID_A), no_resolver)
            .unwrap();
        tx.set_target(&b).unwrap();
        assert!(tx.apply(&b, bh.path()).unwrap().is_empty());
        tx.finalize(&b).unwrap();

        fx.write("a", &lines("A EDIT", "line 20"));
        fx.write("b", b"peer-b\n");
        fx.commit();
        fx.repo.publish(&cloud).unwrap();

        fs::write(b_root.path().join("b"), b"peer-b\n").unwrap();

        b.fetch_bundles(&cloud, FetchMode::Normal).unwrap();
        let mut tx = b.begin_tx(&key, "HEAD", no_resolver).unwrap();
        assert_eq!(
            b.merge_remote(&tx, ID_A).unwrap(),
            MergeOutcome::FastForward
        );
        tx.set_target(&b).unwrap();

        let staging_list = project::read(&b.staging).unwrap();
        assert!(
            staging_list.entries.contains_key("a") && !staging_list.entries.contains_key("b"),
            "staging list must still be [a]: {:?}",
            staging_list.entries.keys().collect::<Vec<_>>()
        );
        let target_list = project::read(&tx.worktree).unwrap();
        assert!(
            target_list.entries.contains_key("b"),
            "target must track [a, b]: {:?}",
            target_list.entries.keys().collect::<Vec<_>>()
        );

        fs::write(b_root.path().join("a"), &lines("line 1", "B EDIT")).unwrap();
        let skipped = tx.apply(&b, bh.path()).unwrap();
        assert!(skipped.is_empty(), "nothing blocked: {skipped:?}");
        assert!(tx.blocked().is_empty());
        assert!(
            tx.worktree.join("b").is_file(),
            "b must be mirrored into the tx worktree"
        );
        assert!(
            !tx.changes()
                .iter()
                .any(|(s, p)| *s == Status::Added && p == Path::new("b")),
            "change set must not carry Added b: {:?}",
            tx.changes()
        );

        let local = b.git.rev(&tx_ref(&tx.id, "local")).unwrap();
        let local_tree = b.ls_tree(&local).unwrap();
        assert!(
            local_tree.iter().any(|(_, p)| p == Path::new("b")),
            "b must be in the mirrored local tree: {local_tree:?}"
        );
    }

    // --- merge_remote -----------------------------------------------------

    /// Identical trees on diverged heads: both devices pick the same side, so
    /// no merge commit is minted and the pair stops trading bundles.
    #[test]
    fn identical_trees_on_diverged_heads_adopt_the_smaller_hash() {
        let fx = fixture();
        let key = provider_key(&fx.cloud());
        fx.write("CLAUDE.md", b"one\n");
        fx.commit();
        let base = fx.repo.git.rev("HEAD").unwrap();

        // Two commits with the same tree on top of the same base.
        let tree = fx.repo.git.ok(&["rev-parse", "HEAD^{tree}"]).unwrap();
        let mut heads = Vec::new();
        for msg in ["left", "right"] {
            heads.push(
                fx.repo
                    .git
                    .ok(&["commit-tree", "-m", msg, "-p", &base, &tree])
                    .unwrap(),
            );
        }
        heads.sort();
        let (small, large) = (heads[0].clone(), heads[1].clone());

        fx.repo
            .git
            .ok(&["update-ref", "refs/heads/main", &large])
            .unwrap();
        fx.repo
            .git
            .ok(&["update-ref", &remote_ref(&key, ID_B), &small])
            .unwrap();
        let tx = fx
            .repo
            .begin_tx(&key, "refs/heads/main", no_resolver)
            .unwrap();
        assert_eq!(
            fx.repo.merge_remote(&tx, ID_B).unwrap(),
            MergeOutcome::Adopted
        );
        assert_eq!(tx.git.rev("HEAD"), Some(small.clone()));

        // The mirror image: the device already holding the smaller hash keeps
        // it, so both end on the same commit and neither mints a merge.
        fx.repo
            .git
            .ok(&["update-ref", "refs/heads/main", &small])
            .unwrap();
        fx.repo
            .git
            .ok(&["update-ref", &remote_ref(&key, ID_B), &large])
            .unwrap();
        assert_eq!(
            fx.repo.merge_remote(&tx, ID_B).unwrap(),
            MergeOutcome::AlreadyMerged
        );
    }

    // --- input boundaries -------------------------------------------------

    /// `engine::live_root_path` already refused `..` / empty / absolute; this
    /// one used to `join` them. The stricter guard wins.
    #[test]
    fn live_path_rejects_a_relative_path_that_is_not_plain() {
        let fx = fixture();
        assert!(
            fx.repo.live_path(Path::new("../escape")).is_err(),
            "parent component must not map onto a live path"
        );
    }

    #[test]
    fn unsafe_slugs_and_device_ids_are_refused() {
        let home = TempDir::new().unwrap();
        let root = TempDir::new().unwrap();
        for slug in ["../escape", "", "Proj", "a--b", "-a", "a/b"] {
            assert!(
                Repo::init(home.path(), slug, root.path(), "n", ID_A).is_err(),
                "accepted slug {slug:?}"
            );
        }
        assert!(Repo::init(home.path(), "ok", root.path(), "n", "../x").is_err());
        assert!(Repo::open(home.path(), "ok", root.path(), "n", ID_A).is_err());
    }

    /// A tracked root's own `.gitattributes` is mirrored into staging and
    /// staged like any other file (`add -f` with a literal pathspec disables
    /// *pathspecs*, not attributes). With `* text=auto` in it, `git add` would
    /// normalise CRLF on check-in: this device keeps its CRLF bytes forever
    /// while every linking device writes LF into its live root. That is a
    /// permanent byte divergence between two roots, so `Repo::init` neutralises
    /// attributes in `info/attributes`, which outranks the copied file.
    #[test]
    fn a_mirrored_gitattributes_cannot_rewrite_the_bytes_we_publish() {
        let fx = fixture();
        fx.write(".gitattributes", b"* text=auto\n* eol=lf\n");
        fx.write("crlf.md", b"a\r\nb\r\n");
        assert!(fx.commit());

        // It really is tracked; otherwise this test would pass vacuously.
        assert!(fx
            .repo
            .git
            .ok(&["ls-files"])
            .unwrap()
            .lines()
            .any(|l| l == ".gitattributes"));
        assert_eq!(
            fx.repo
                .git
                .ok(&["check-attr", "text", "--", "crlf.md"])
                .unwrap(),
            "crlf.md: text: unset"
        );

        let blob = fx
            .repo
            .git
            .run(&["cat-file", "blob", "HEAD:crlf.md"])
            .unwrap();
        assert_eq!(
            blob.stdout, b"a\r\nb\r\n",
            "the committed blob is not the user's bytes"
        );

        // The main worktree is not where merges happen: every merge and every
        // `stage_worktree` runs in the transaction worktree. If the override
        // did not reach there, `git add` would normalise a CRLF root into the
        // `local` commit, `expected_bytes` would hand back LF, the live root
        // would never match it, and the path would loop through
        // `MAX_RECONCILE` into a permanent `Pending`.
        let tx = fx
            .repo
            .begin_tx(&provider_key(&fx.cloud()), "HEAD", no_resolver)
            .unwrap();
        assert!(
            tx.worktree.join(".gitattributes").is_file(),
            "the tx worktree has no .gitattributes, so this would pass vacuously"
        );
        assert_eq!(
            tx.git.ok(&["check-attr", "text", "--", "crlf.md"]).unwrap(),
            "crlf.md: text: unset"
        );

        // And the bytes a linking device would write back are the same ones.
        let snap = fx
            .repo
            .expected_bytes(
                Some(&fx.repo.git.rev("HEAD").unwrap()),
                &[PathBuf::from("crlf.md")],
            )
            .unwrap();
        assert_eq!(
            snap[Path::new("crlf.md")].as_ref().unwrap().bytes,
            b"a\r\nb\r\n"
        );
    }

    /// Everything in `<home>/tmp` is a copy of every tracked byte: the
    /// outgoing bundle, which `git bundle create` creates itself at a
    /// umask-derived mode before `publish` can tighten it, and a transaction
    /// worktree for as long as an apply runs. The directory is the only guard
    /// that covers the window git owns.
    #[test]
    fn the_home_tmp_directory_is_kept_private() {
        let fx = fixture();
        let cloud = fx.cloud();
        let key = provider_key(&cloud);
        fx.write("CLAUDE.md", b"one\n");
        fx.commit();
        fx.repo.publish(&cloud).unwrap();

        let tmp = fx.home.path().join("tmp");
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&tmp), 0o700, "publish left {:o}", mode(&tmp));

        // Repaired, not merely set at creation: a directory left loose by an
        // older build or a stray chmod is tightened on the next use.
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755)).unwrap();
        fx.repo.begin_tx(&key, "HEAD", no_resolver).unwrap();
        assert_eq!(mode(&tmp), 0o700, "begin_tx left {:o}", mode(&tmp));
    }

    /// Temp names are the whole defence against a symlink pre-planted by
    /// another process of the same user, so they must be unguessable, not just
    /// unused: the pid-and-nanos format this replaced was neither.
    #[test]
    fn unique_id_is_unguessable_hex() {
        let id = unique_id();
        assert_eq!(id.len(), 32, "{id}");
        assert!(
            id.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "{id} is not lowercase hex"
        );
        assert_ne!(id, unique_id());
    }

    /// `Journal::intent` exists to be read. A crash inside
    /// `mirror::write_atomic` leaves `.<name>.<pid>.dotlore-tmp` beside the
    /// target holding a full copy of the tracked bytes, and `mirror` protects
    /// that name from every later pass — so unless the resume sweeps it, it
    /// stays in the user's real root forever. Only the intent's own path is
    /// swept: over-matching here would delete live files.
    #[test]
    fn a_resume_sweeps_the_temp_file_a_crashed_write_left_in_the_root() {
        let (fx, _b, bh) = two_devices();
        let cloud = fx.cloud();
        let key = provider_key(&cloud);
        let b_root = TempDir::new().unwrap();
        let b = Repo::open(bh.path(), "proj-claude", b_root.path(), "Mac B Pro", ID_B).unwrap();
        b.fetch_bundles(&cloud, FetchMode::Normal).unwrap();

        let mut tx = b
            .begin_tx(&key, &remote_ref(&key, ID_A), no_resolver)
            .unwrap();
        tx.set_target(&b).unwrap();
        // Exactly the state a crash between `write_atomic`'s write and its
        // rename leaves: the intent journalled, the copy on disk under a pid
        // that is no longer running.
        tx.j.intent = Some((JStatus::Added, PathBuf::from("CLAUDE.md")));
        tx.write_journal().unwrap();
        let litter = b_root.path().join(".CLAUDE.md.99999.dotlore-tmp");
        fs::write(&litter, b"one\n").unwrap();
        let bystander = b_root.path().join(".other.md.99999.dotlore-tmp");
        fs::write(&bystander, b"another path's business\n").unwrap();
        drop(tx); // crash

        let mut tx = b.pending_tx(no_resolver).unwrap().unwrap();
        assert_eq!(tx.resume(&b, bh.path()).unwrap(), ResumeOutcome::Finished);
        assert_eq!(fs::read(b_root.path().join("CLAUDE.md")).unwrap(), b"one\n");
        assert!(
            !litter.exists(),
            "the crashed write's copy is still in the live root"
        );
        assert!(
            bystander.is_file(),
            "the sweep removed a file that was not the intent's"
        );
    }

    #[test]
    fn ignore_text_prefers_the_committed_file() {
        let fx = fixture();
        assert_eq!(
            fx.repo.ignore_text(fx.home.path()),
            project::DEFAULT_NEVER_IGNORE
        );
        fs::write(fx.repo.staging.join(".dotloreignore"), "custom\n").unwrap();
        assert_eq!(fx.repo.ignore_text(fx.home.path()), "custom\n");
    }

    #[test]
    fn ignore_text_falls_back_to_the_default_never_list_when_staging_has_none() {
        let home = TempDir::new().unwrap();
        let home_dir = TempDir::new().unwrap();
        let claude = home_dir.path().join(".claude");
        fs::create_dir_all(&claude).unwrap();
        let repo = Repo::init(home.path(), "home-claude", &claude, "n", ID_A).unwrap();
        assert!(
            !repo.staging.join(project::IGNORE_FILE).exists(),
            "a freshly inited repo has no committed ignore file"
        );
        assert_eq!(
            repo.ignore_text(home_dir.path()),
            project::DEFAULT_NEVER_IGNORE
        );
    }
}
