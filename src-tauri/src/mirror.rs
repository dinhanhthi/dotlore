//! Copying a tracked root into and out of its staging worktree.
//!
//! This is the only place that moves bytes between a live tracked root and its
//! staging git worktree, and the only one that writes into a live root at all
//! (a user restore from the trash included). Two rules shape everything here:
//! nothing git-private (`.git`, `.dotloreignore`, `*.conflict-*`) ever reaches
//! the root, and a root file is only overwritten when its current bytes and
//! mode are exactly what the caller expected them to be.

use std::collections::{HashMap, HashSet};
use std::ffi::{CString, OsStr, OsString};
use std::fs::{self, File};
use std::io::{self, ErrorKind, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd};
use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use ignore::gitignore::{Gitignore, GitignoreBuilder};

use crate::project::{EntryList, Limits, SensitiveMatcher, Sensitivity, NEVER_REINCLUDE};

/// What happened to one logical entry between two staging commits.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Added,
    Modified,
    Deleted,
}

/// The content of one file as far as dotlore cares: bytes plus the only mode
/// bit git records.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FileState {
    pub bytes: Vec<u8>,
    pub executable: bool,
}

/// Expected current root content per logical relative path; `None` means the
/// path is expected to be absent.
pub type Snapshot = HashMap<PathBuf, Option<FileState>>;

/// Result of one root → staging pass. Paths are relative to the root.
#[derive(Debug, Default)]
pub struct MirrorReport {
    pub changed: bool,
    pub skipped_symlinks: Vec<PathBuf>,
    /// Regular files over `Limits.max_file_bytes`. They are opaque: not
    /// copied, and not pruned from staging.
    pub skipped_too_large: Vec<(PathBuf, u64)>,
}

/// Keeps sensitive files out of capture when a project `!` line re-included
/// them past the never-list.
pub struct ReincludeGuard {
    never: Gitignore,
    builtin: Gitignore,
    sensitive: SensitiveMatcher,
}

impl ReincludeGuard {
    /// `never_text` is the never-list alone, without the project list.
    pub fn new(root: &Path, never_text: &str, sensitive: SensitiveMatcher) -> Result<Self> {
        Ok(ReincludeGuard {
            never: build_ignore(root, never_text)?,
            builtin: build_ignore(root, &NEVER_REINCLUDE.join("\n"))?,
            sensitive,
        })
    }

    /// True when the never-list ignores `rel` (or a parent) and `rel` is in
    /// `NEVER_REINCLUDE` or Secret-sensitive. Only files the effective ignore
    /// text let through reach this, so a match here means a `!` line
    /// re-included it.
    pub fn blocks(&self, rel: &Path) -> bool {
        self.never
            .matched_path_or_any_parents(rel, false)
            .is_ignore()
            && (self
                .builtin
                .matched_path_or_any_parents(rel, false)
                .is_ignore()
                || self.sensitive.classify(rel) == Some(Sensitivity::Secret))
    }
}

/// Copy a tracked root into its staging worktree, scoped by `entries` and
/// filtered by `ignore_text` (gitignore syntax; the caller reads
/// `<staging>/.dotloreignore` or passes a default). Files `guard` blocks are
/// left out like ignored ones, so staging drops any copy it already has.
/// Byte- and mode-identical files are left untouched, so a quiet root reports
/// `changed == false`.
pub fn root_to_staging(
    root: &Path,
    staging: &Path,
    entries: &EntryList,
    ignore_text: &str,
    limits: Limits,
    guard: Option<&ReincludeGuard>,
) -> Result<MirrorReport> {
    let md = fs::symlink_metadata(root)
        .with_context(|| format!("tracked root {} is unreadable", root.display()))?;
    if md.file_type().is_symlink() {
        bail!("tracked root {} is a symlink", root.display());
    }
    if !md.is_dir() {
        bail!("tracked root {} is not a directory", root.display());
    }

    // Rel paths of regular files the include-list wants copied into staging.
    let mut want: HashSet<PathBuf> = HashSet::new();
    // Paths the walk could not look inside (symlinks, undecodable names). We
    // know nothing about what the root holds there, so staging keeps whatever
    // it has at or under them: deleting it would publish a deletion of data
    // another device legitimately owns.
    let mut opaque: HashSet<PathBuf> = HashSet::new();
    let mut report = MirrorReport::default();

    let ignore = build_ignore(root, ignore_text)?;
    walk(
        root,
        Path::new(""),
        entries,
        &ignore,
        limits,
        &mut want,
        &mut opaque,
        &mut report,
    )?;
    want.retain(|rel| !guard.is_some_and(|g| g.blocks(rel)));

    fs::create_dir_all(staging)?;
    let keep: HashSet<&Path> = want.iter().map(PathBuf::as_path).collect();
    report.changed = delete_stale(staging, Path::new(""), &keep, &opaque, entries)?;
    for rel in &want {
        // `delete_stale` ran above with `keep` built from `want`, which holds
        // `rel`, so a skipped entry keeps whatever staging already has.
        match copy_if_changed(root, rel, &staging.join(rel))? {
            Some(changed) => report.changed |= changed,
            None => report.skipped_symlinks.push(rel.clone()),
        }
    }
    Ok(report)
}

/// Apply staging changes back to the live root.
///
/// `snap` holds the expected current root state per logical path; any entry
/// whose real state differs is left alone and returned, as is any path that is
/// (or sits under) a symlink and any whose staging source is missing or not a
/// regular file. A missing expectation is an error: every write must be
/// justified by one.
///
/// Every live file this deletes or overwrites is first copied to
/// `<trash>/<rel>`, or `<trash>-N/<rel>` when that is taken (see
/// [`keep_in_trash`]), and a failed copy aborts before the live file is
/// touched: a sync never destroys the only local copy of a file.
pub fn apply_to_root(
    staging: &Path,
    root: &Path,
    changes: &[(Status, PathBuf)],
    snap: &Snapshot,
    trash: &Path,
) -> Result<Vec<PathBuf>> {
    let mut skipped = Vec::new();
    for (status, rel) in changes {
        if !plain_rel(rel) {
            bail!("unsafe path in change set: {}", rel.display());
        }
        if protected(rel) {
            continue;
        }
        let target = root.join(rel);

        if symlink_on_path(root, &target) {
            skipped.push(rel.clone());
            continue;
        }
        let expected = snap
            .get(rel)
            .ok_or_else(|| anyhow!("no apply expectation for {}", rel.display()))?;
        let current = match current_state(&target)? {
            Some(state) => state,
            // A directory where a file belongs, or a file where a parent
            // directory belongs: never ours to replace.
            None => {
                skipped.push(rel.clone());
                continue;
            }
        };

        if &current != expected {
            skipped.push(rel.clone());
            continue;
        }

        match status {
            Status::Added | Status::Modified => {
                let src = staging.join(rel);
                // A staging entry that is missing or not a regular file (git
                // stores symlinks as ordinary blobs) is drift like any other:
                // skipped and reported, never an abort that would leave the
                // root half-applied and re-fail on every retry.
                match fs::symlink_metadata(&src) {
                    Ok(md) if md.file_type().is_file() => {
                        if let Some(old) = &current {
                            keep_in_trash(trash, rel, old)?;
                        }
                        write_atomic(&src, &target)?
                    }
                    _ => skipped.push(rel.clone()),
                }
            }
            Status::Deleted => {
                if let Some(old) = &current {
                    keep_in_trash(trash, rel, old)?;
                }
                match fs::remove_file(&target) {
                    Ok(()) => {}
                    Err(e) if e.kind() == ErrorKind::NotFound => {}
                    Err(e) => {
                        return Err(e).with_context(|| format!("removing {}", target.display()))
                    }
                }
            }
        }
    }
    Ok(skipped)
}

/// Copy the live bytes about to be deleted or overwritten to `<trash>/<rel>`.
///
/// The bytes are the ones the expectation check just read, so the copy is
/// exactly what the apply displaces. An existing copy is never replaced: a
/// taken name goes to the next sibling pass folder, `<trash>-1/<rel>`,
/// `<trash>-2/<rel>`, ..., so every copy keeps its exact rel. Copies are
/// private (0600, or 0700 when executable), since roots hold secrets.
pub(crate) fn keep_in_trash(trash: &Path, rel: &Path, old: &FileState) -> Result<()> {
    let mode = if old.executable { 0o700 } else { 0o600 };
    let mut n = 0u32;
    loop {
        let pass = if n == 0 {
            trash.to_path_buf()
        } else {
            let mut name = trash.file_name().unwrap_or_default().to_os_string();
            name.push(format!("-{n}"));
            trash.with_file_name(name)
        };
        let path = pass.join(rel);
        let dir = path.parent().unwrap_or(&pass);
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&path)
        {
            Ok(mut f) => {
                f.write_all(&old.bytes)
                    .and_then(|()| f.sync_all())
                    .with_context(|| format!("writing {}", path.display()))?;
                return Ok(());
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => n += 1,
            Err(e) => return Err(e).with_context(|| format!("creating {}", path.display())),
        }
    }
}

/// What [`restore_to_root`] does when a different live file is in the way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OnExisting {
    /// Write nothing and report [`Restored::Exists`], so the user can choose.
    Ask,
    /// Keep the live file in the trash first, then overwrite it.
    Replace,
    /// Leave the live file alone and write a `.restored` sibling.
    KeepBoth,
}

/// The outcome of one [`restore_to_root`].
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Restored {
    /// The rel actually written, relative to the root.
    Written(PathBuf),
    /// The live file already holds these bytes and exec bit.
    Same,
    /// A different live file exists and `on_existing` was `Ask`.
    Exists,
    /// A protected name, a symlink on the path, or a target that is not ours.
    Skipped,
}

/// Restore one trash copy `src` into the live root at `rel`, on the user's
/// request: reads `src`, which must be a regular file so a symlink planted in
/// the trash is never read through, and hands its bytes and exec bit to
/// [`restore_state_to_root`]. Trash copies are 0600/0700, so only their exec
/// bit carries over: a new file gets the same 0644/0755 a sync-created one
/// would.
pub(crate) fn restore_to_root(
    root: &Path,
    rel: &Path,
    src: &Path,
    displaced: &Path,
    on_existing: OnExisting,
) -> Result<Restored> {
    let state = read_restore_source(src)?;
    restore_state_to_root(root, rel, &state, displaced, on_existing)
}

/// Restore `state` into the live root at `rel`, on the user's request.
///
/// Together with [`restore_to_root`] this is the only write into a live root
/// that is not a sync apply, so it keeps every guard [`apply_to_root`] has:
/// an unsafe `rel` is an error, a staging-private name is never written (it
/// would be mirrored on towards the cloud), and a symlink on the path, a
/// directory, or an unstatable target is skipped rather than written through.
///
/// A live file holding the same bytes is left alone. A different one is
/// never lost: `Ask` writes nothing, `Replace` first keeps it in `displaced`
/// (see [`keep_in_trash`]; a failed copy aborts before the live file is
/// touched), and `KeepBoth` writes a new `.restored` sibling instead.
pub(crate) fn restore_state_to_root(
    root: &Path,
    rel: &Path,
    state: &FileState,
    displaced: &Path,
    on_existing: OnExisting,
) -> Result<Restored> {
    let current = match restore_check_state(root, rel, state)? {
        Check::Done(out) => return Ok(out),
        Check::Missing => {
            write_atomic_bytes(&state.bytes, state.executable, &root.join(rel))?;
            return Ok(Restored::Written(rel.to_path_buf()));
        }
        Check::Differs { current } => current,
    };
    match on_existing {
        OnExisting::Ask => Ok(Restored::Exists),
        OnExisting::Replace => {
            keep_in_trash(displaced, rel, &current)?;
            write_atomic_bytes(&state.bytes, state.executable, &root.join(rel))?;
            Ok(Restored::Written(rel.to_path_buf()))
        }
        OnExisting::KeepBoth => write_restored_sibling(root, rel, state),
    }
}

/// What [`restore_to_root`] with `OnExisting::Ask` would return, writing
/// nothing: `Written(rel)` here means "would be written". Lets a batch
/// restore find every `Exists` before it writes a single file.
pub(crate) fn restore_preview(root: &Path, rel: &Path, src: &Path) -> Result<Restored> {
    Ok(match restore_check(root, rel, src)? {
        Check::Done(out) => out,
        Check::Missing => Restored::Written(rel.to_path_buf()),
        Check::Differs { .. } => Restored::Exists,
    })
}

/// Where a restore stands before anything is written.
enum Check {
    /// Nothing to write: `Skipped` or `Same`.
    Done(Restored),
    /// No live file at `rel`.
    Missing,
    /// A live file with other bytes or exec bit.
    Differs { current: FileState },
}

/// The bytes and exec bit of a restore source, which must be a regular file.
fn read_restore_source(src: &Path) -> Result<FileState> {
    let md = fs::symlink_metadata(src).with_context(|| format!("reading {}", src.display()))?;
    if !md.file_type().is_file() {
        bail!("{} is not a regular file", src.display());
    }
    Ok(FileState {
        bytes: fs::read(src).with_context(|| format!("reading {}", src.display()))?,
        executable: is_exec(md.permissions().mode()),
    })
}

/// Every guard of [`restore_to_root`], none of its writes.
fn restore_check(root: &Path, rel: &Path, src: &Path) -> Result<Check> {
    restore_check_state(root, rel, &read_restore_source(src)?)
}

/// Every guard of [`restore_state_to_root`], none of its writes.
fn restore_check_state(root: &Path, rel: &Path, state: &FileState) -> Result<Check> {
    if !plain_rel(rel) {
        bail!("unsafe restore path: {}", rel.display());
    }
    if protected(rel) {
        return Ok(Check::Done(Restored::Skipped));
    }
    let target = root.join(rel);
    if symlink_on_path(root, &target) {
        return Ok(Check::Done(Restored::Skipped));
    }
    Ok(match current_state(&target)? {
        None => Check::Done(Restored::Skipped),
        Some(None) => Check::Missing,
        Some(Some(current)) if current == *state => Check::Done(Restored::Same),
        Some(Some(current)) => Check::Differs { current },
    })
}

/// Write `state` to a new `<stem>.restored.<ext>`, `<stem>.restored.1.<ext>`,
/// ... next to `rel`, never touching `rel` itself. `create_new` (`O_EXCL`)
/// treats any existing name, a planted symlink included, as taken.
fn write_restored_sibling(root: &Path, rel: &Path, state: &FileState) -> Result<Restored> {
    let name = Path::new(rel.file_name().unwrap_or_default());
    let stem = name.file_stem().unwrap_or_default().to_string_lossy();
    let ext = name
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let mode = mode_like(&root.join(rel), state.executable);
    let mut n = 0u32;
    loop {
        let num = if n == 0 {
            String::new()
        } else {
            format!(".{n}")
        };
        let sibling = rel.with_file_name(format!("{stem}.restored{num}{ext}"));
        // Every numbered name shares the stem and extension, so a protected
        // first candidate means they all are.
        if protected(&sibling) {
            return Ok(Restored::Skipped);
        }
        let path = root.join(&sibling);
        if symlink_on_path(root, path.parent().unwrap_or(root)) {
            return Ok(Restored::Skipped);
        }
        let mut f = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&path)
        {
            Ok(f) => f,
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                n += 1;
                continue;
            }
            Err(e) => return Err(e).with_context(|| format!("creating {}", path.display())),
        };
        // `mode` was masked by the umask at open; `fchmod` on the handle.
        let res = f
            .set_permissions(fs::Permissions::from_mode(mode))
            .and_then(|()| f.write_all(&state.bytes))
            .and_then(|()| f.sync_all());
        if let Err(e) = res {
            drop(f);
            let _ = fs::remove_file(&path);
            return Err(e).with_context(|| format!("writing {}", path.display()));
        }
        return Ok(Restored::Written(sibling));
    }
}

/// A NUL byte in the first 8000 bytes, the same heuristic git uses.
pub fn is_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8000).any(|&b| b == 0)
}

// --- root -> staging ------------------------------------------------------

pub(crate) fn build_ignore(root: &Path, ignore_text: &str) -> Result<Gitignore> {
    let mut b = GitignoreBuilder::new(root);
    for line in ignore_text.lines() {
        b.add_line(None, line)?;
    }
    Ok(b.build()?)
}

fn walk(
    root: &Path,
    rel: &Path,
    entries: &EntryList,
    ignore: &Gitignore,
    limits: Limits,
    want: &mut HashSet<PathBuf>,
    opaque: &mut HashSet<PathBuf>,
    report: &mut MirrorReport,
) -> Result<()> {
    // Missing tracked directories are ordinary deletions: do not read_dir.
    match inspect(root, rel)? {
        Node::Missing => return Ok(()),
        Node::Opaque => {
            if !rel.as_os_str().is_empty() {
                mark_opaque(rel, opaque, report);
            }
            return Ok(());
        }
        Node::File { .. } if !rel.as_os_str().is_empty() => {
            // Kind mismatch: we only walk directories.
            mark_opaque(rel, opaque, report);
            return Ok(());
        }
        Node::File { .. } | Node::Dir => {}
    }

    let names = match list_dir(root, rel)? {
        None => return Ok(()),
        Some(Err(())) => {
            if !rel.as_os_str().is_empty() {
                mark_opaque(rel, opaque, report);
            }
            return Ok(());
        }
        Some(Ok(names)) => names,
    };
    for raw in names {
        let name = match raw.into_string() {
            Ok(n) => n,
            Err(raw) => {
                opaque.insert(rel.join(raw));
                continue;
            }
        };
        if protected_name(&name) || name == ".DS_Store" {
            continue;
        }
        let child = rel.join(&name);
        match inspect(root, &child)? {
            Node::Missing => {}
            Node::Opaque => mark_opaque(&child, opaque, report),
            Node::File { len } => {
                if expects_dir(entries, &child) {
                    mark_opaque(&child, opaque, report);
                    continue;
                }
                if ignore
                    .matched_path_or_any_parents(&child, false)
                    .is_ignore()
                {
                    continue;
                }
                if skip_too_large(&child, len, limits, opaque, report) {
                    continue;
                }
                if entries.contains_rel(&child) {
                    want.insert(child);
                }
            }
            Node::Dir => {
                if expects_file(entries, &child) {
                    mark_opaque(&child, opaque, report);
                    continue;
                }
                if ignore.matched_path_or_any_parents(&child, true).is_ignore() {
                    continue;
                }
                if entries.contains_rel(&child) || entries.has_tracked_descendant(&child) {
                    walk(root, &child, entries, ignore, limits, want, opaque, report)?;
                }
            }
        }
    }
    Ok(())
}

fn mark_opaque(rel: &Path, opaque: &mut HashSet<PathBuf>, report: &mut MirrorReport) {
    opaque.insert(rel.to_path_buf());
    report.skipped_symlinks.push(rel.to_path_buf());
}

/// Live include-list files with sizes, using the same walk and `guard` as
/// `root_to_staging`. `too_large` is true when the file exceeds
/// `limits.max_file_bytes` and was therefore not copied into staging.
pub(crate) fn list_live(
    root: &Path,
    entries: &EntryList,
    ignore_text: &str,
    limits: Limits,
    guard: Option<&ReincludeGuard>,
) -> Result<Vec<(PathBuf, u64, bool)>> {
    let mut want: HashSet<PathBuf> = HashSet::new();
    let mut opaque: HashSet<PathBuf> = HashSet::new();
    let mut report = MirrorReport::default();
    let ignore = build_ignore(root, ignore_text)?;
    walk(
        root,
        Path::new(""),
        entries,
        &ignore,
        limits,
        &mut want,
        &mut opaque,
        &mut report,
    )?;
    want.retain(|rel| !guard.is_some_and(|g| g.blocks(rel)));
    let mut out = Vec::new();
    for rel in want {
        match inspect(root, &rel)? {
            Node::File { len } => out.push((rel, len, false)),
            _ => {}
        }
    }
    for (rel, len) in report.skipped_too_large {
        if entries.contains_rel(&rel) && !guard.is_some_and(|g| g.blocks(&rel)) {
            out.push((rel, len, true));
        }
    }
    Ok(out)
}

/// Strict `>`: a file exactly at the limit still syncs. Opaque so a later
/// growth does not publish a deletion of whatever staging already holds.
fn skip_too_large(
    rel: &Path,
    len: u64,
    limits: Limits,
    opaque: &mut HashSet<PathBuf>,
    report: &mut MirrorReport,
) -> bool {
    if len > limits.max_file_bytes {
        opaque.insert(rel.to_path_buf());
        report.skipped_too_large.push((rel.to_path_buf(), len));
        true
    } else {
        false
    }
}

/// Directory include-list keys cover any child; a file key does not.
fn expects_dir(entries: &EntryList, rel: &Path) -> bool {
    entries.is_explicit(rel) && entries.contains_rel(&rel.join("_"))
}

fn expects_file(entries: &EntryList, rel: &Path) -> bool {
    entries.is_explicit(rel) && !entries.contains_rel(&rel.join("_"))
}

/// Remove staging files that the root no longer has, pruning directories left
/// empty. Returns whether anything was removed.
///
/// `opaque` holds root paths the walk refused to look inside; staging keeps
/// everything at or under them, recursion included.
fn delete_stale(
    staging: &Path,
    rel: &Path,
    keep: &HashSet<&Path>,
    opaque: &HashSet<PathBuf>,
    entries: &EntryList,
) -> Result<bool> {
    if under_opaque(rel, opaque) {
        return Ok(false);
    }
    let dir = staging.join(rel);
    let mut changed = false;
    for entry in fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        let name = match entry.file_name().into_string() {
            Ok(n) => n,
            Err(_) => continue,
        };
        if protected_name(&name) {
            continue;
        }
        let child = rel.join(&name);
        if under_opaque(&child, opaque) {
            continue;
        }
        let path = entry.path();
        if fs::symlink_metadata(&path)?.is_dir() {
            if entries.contains_rel(&child) || entries.has_tracked_descendant(&child) {
                changed |= delete_stale(staging, &child, keep, opaque, entries)?;
                if fs::read_dir(&path)?.next().is_none() {
                    fs::remove_dir(&path)?;
                }
            }
        } else if name == ".DS_Store" {
            fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
            changed = true;
        } else if entries.contains_rel(&child) && !keep.contains(child.as_path()) {
            fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
            changed = true;
        }
    }
    Ok(changed)
}

fn under_opaque(rel: &Path, opaque: &HashSet<PathBuf>) -> bool {
    rel.ancestors()
        .any(|a| !a.as_os_str().is_empty() && opaque.contains(a))
}

/// `None` means the source was not a regular file when we got to it and was
/// left alone. `walk` filters symlinks out of `want`, but it stats the whole
/// tree before this loop runs: an entry swapped for a symlink inside that
/// window would otherwise be read through and its target's bytes published.
fn copy_if_changed(root: &Path, rel: &Path, dst: &Path) -> Result<Option<bool>> {
    let mut src = match open_chain(root, rel, false) {
        Ok(f) => f,
        Err(_) => return Ok(None),
    };
    let md = src.metadata()?;
    if !md.is_file() {
        return Ok(None);
    }
    let executable = is_exec(md.permissions().mode());
    let mut bytes = Vec::new();
    src.read_to_end(&mut bytes)
        .with_context(|| format!("reading {}", root.join(rel).display()))?;

    match fs::symlink_metadata(dst) {
        Ok(dmd) if dmd.is_file() => {
            if is_exec(dmd.permissions().mode()) == executable && fs::read(dst)? == bytes {
                return Ok(Some(false));
            }
        }
        Ok(dmd) if dmd.is_dir() => fs::remove_dir_all(dst)?,
        Ok(_) => fs::remove_file(dst)?,
        Err(_) => {}
    }
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(dst, &bytes).with_context(|| format!("writing {}", dst.display()))?;
    fs::set_permissions(dst, fs::Permissions::from_mode(mode_for(executable)))?;
    Ok(Some(true))
}

#[derive(Debug)]
pub(crate) enum Node {
    Missing,
    Opaque,
    File { len: u64 },
    Dir,
}

/// Measured size of one entry. Over-limit files are excluded from `bytes`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TreeMeasure {
    pub bytes: u64,
    pub skipped_too_large: Vec<(PathBuf, u64)>,
}

/// Ignore-aware, no-follow size of `rel` (file or folder). Unsafe, ignored,
/// and over-`max_file_bytes` content is left out of `bytes`.
pub fn measure_tree(
    root: &Path,
    rel: &Path,
    ignore_text: &str,
    limits: Limits,
) -> Result<TreeMeasure> {
    let ignore = build_ignore(root, ignore_text)?;
    let mut out = TreeMeasure::default();
    measure_walk(root, rel, &ignore, limits, &mut out)?;
    Ok(out)
}

fn measure_walk(
    root: &Path,
    rel: &Path,
    ignore: &Gitignore,
    limits: Limits,
    out: &mut TreeMeasure,
) -> Result<()> {
    match inspect(root, rel)? {
        Node::Missing | Node::Opaque => Ok(()),
        Node::File { len } => {
            if rel.as_os_str().is_empty() {
                return Ok(());
            }
            if ignore.matched_path_or_any_parents(rel, false).is_ignore() {
                return Ok(());
            }
            if len > limits.max_file_bytes {
                out.skipped_too_large.push((rel.to_path_buf(), len));
                return Ok(());
            }
            out.bytes = out.bytes.saturating_add(len);
            Ok(())
        }
        Node::Dir => {
            if !rel.as_os_str().is_empty()
                && ignore.matched_path_or_any_parents(rel, true).is_ignore()
            {
                return Ok(());
            }
            let names = match list_dir(root, rel)? {
                None => return Ok(()),
                Some(Err(())) => return Ok(()),
                Some(Ok(names)) => names,
            };
            for raw in names {
                let name = match raw.into_string() {
                    Ok(n) => n,
                    Err(_) => continue,
                };
                if protected_name(&name) || name == ".DS_Store" {
                    continue;
                }
                measure_walk(root, &rel.join(&name), ignore, limits, out)?;
            }
            Ok(())
        }
    }
}

/// Inspect `rel` from `root` with no-follow semantics on every component.
/// The test seam fires before each component is opened so a replacement
/// cannot be missed by a preflight-only canonicalize.
pub(crate) fn inspect(root: &Path, rel: &Path) -> Result<Node> {
    if rel.as_os_str().is_empty() {
        return match open_root_dir(root) {
            Ok(f) => {
                let md = f.metadata()?;
                if md.is_dir() {
                    Ok(Node::Dir)
                } else {
                    Ok(Node::Opaque)
                }
            }
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(Node::Missing),
            Err(e) if is_unsafe_open(&e) => Ok(Node::Opaque),
            Err(e) => Err(e).with_context(|| format!("inspecting {}", root.display())),
        };
    }
    match open_chain(root, rel, false) {
        Ok(f) => {
            let md = f.metadata()?;
            if md.is_dir() {
                Ok(Node::Dir)
            } else if md.is_file() {
                Ok(Node::File { len: md.len() })
            } else {
                Ok(Node::Opaque)
            }
        }
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(Node::Missing),
        Err(e) if is_unsafe_open(&e) => Ok(Node::Opaque),
        Err(e) => Err(e).with_context(|| format!("inspecting {}", root.join(rel).display())),
    }
}

// Darwin / Linux open(2) flags. Dotlore is Mac-only; Linux values keep
// the helper honest if someone builds the crate elsewhere.
#[cfg(target_os = "macos")]
const O_NOFOLLOW: i32 = 0x0100;
#[cfg(target_os = "macos")]
const O_DIRECTORY: i32 = 0x0010_0000;
#[cfg(target_os = "macos")]
const O_CLOEXEC: i32 = 0x0100_0000;
#[cfg(target_os = "linux")]
const O_NOFOLLOW: i32 = 0x20000;
#[cfg(target_os = "linux")]
const O_DIRECTORY: i32 = 0x10000;
#[cfg(target_os = "linux")]
const O_CLOEXEC: i32 = 0o2000000;
const O_RDONLY: i32 = 0;

#[cfg(target_os = "macos")]
const ELOOP: i32 = 62;
#[cfg(target_os = "linux")]
const ELOOP: i32 = 40;
const ENOTDIR: i32 = 20;

extern "C" {
    fn openat(dirfd: i32, pathname: *const libc_c_char, flags: i32) -> i32;
}

#[cfg(target_os = "macos")]
enum DIR {}

#[cfg(target_os = "macos")]
#[repr(C)]
struct Dirent {
    d_ino: u64,
    d_seekoff: u64,
    d_reclen: u16,
    d_namlen: u16,
    d_type: u8,
    d_name: [i8; 1024],
}

#[cfg(target_os = "macos")]
extern "C" {
    fn close(fd: i32) -> i32;
    fn fdopendir(fd: i32) -> *mut DIR;
    fn readdir(dirp: *mut DIR) -> *mut Dirent;
    fn closedir(dirp: *mut DIR) -> i32;
}

use std::os::raw::c_char as libc_c_char;

fn is_unsafe_open(e: &io::Error) -> bool {
    matches!(e.raw_os_error(), Some(ELOOP) | Some(ENOTDIR)) || e.kind() == ErrorKind::InvalidInput
}

fn open_root_dir(root: &Path) -> io::Result<File> {
    let mut opts = fs::OpenOptions::new();
    opts.read(true)
        .custom_flags(O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
    opts.open(root)
}

fn openat_child(parent: &File, name: &OsStr, flags: i32) -> io::Result<File> {
    let c_name = CString::new(name.as_bytes())
        .map_err(|_| io::Error::new(ErrorKind::InvalidInput, "path component contains NUL"))?;
    let fd = unsafe { openat(parent.as_raw_fd(), c_name.as_ptr(), flags) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

/// Open `rel` from `root` one component at a time, never following a symlink.
fn open_chain(root: &Path, rel: &Path, directory: bool) -> io::Result<File> {
    let mut fd = open_root_dir(root)?;
    let mut acc = PathBuf::new();
    let comps: Vec<_> = rel.components().collect();
    for (i, component) in comps.iter().enumerate() {
        let name = match component {
            Component::Normal(n) => *n,
            _ => {
                return Err(io::Error::new(
                    ErrorKind::InvalidInput,
                    "path is not a plain relative path",
                ))
            }
        };
        acc.push(name);
        replace_hook(&root.join(&acc));
        let last = i + 1 == comps.len();
        let mut flags = O_RDONLY | O_CLOEXEC | O_NOFOLLOW;
        if !last || directory {
            flags |= O_DIRECTORY;
        }
        fd = openat_child(&fd, name, flags)?;
    }
    Ok(fd)
}

fn open_dir_nofollow(root: &Path, rel: &Path) -> io::Result<File> {
    if rel.as_os_str().is_empty() {
        open_root_dir(root)
    } else {
        open_chain(root, rel, true)
    }
}

/// List `rel` through a no-follow directory fd. `None` is missing.
/// `Some(Err(()))` is a symlink replacement — the caller treats the
/// prefix as opaque and must not prune its staging subtree.
fn list_dir(root: &Path, rel: &Path) -> Result<Option<Result<Vec<OsString>, ()>>> {
    let path = if rel.as_os_str().is_empty() {
        root.to_path_buf()
    } else {
        root.join(rel)
    };
    listing_replace_hook(&path);
    let fd = match open_dir_nofollow(root, rel) {
        Ok(f) => f,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) if is_unsafe_open(&e) => return Ok(Some(Err(()))),
        Err(e) => return Err(e).with_context(|| format!("opening {}", path.display())),
    };
    match read_dir_fd(fd) {
        Ok(names) => Ok(Some(Ok(names))),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) if is_unsafe_open(&e) => Ok(Some(Err(()))),
        Err(e) => Err(e).with_context(|| format!("listing {}", path.display())),
    }
}

/// List through an already-open no-follow directory fd (`fdopendir`).
#[cfg(target_os = "macos")]
fn read_dir_fd(dir: File) -> io::Result<Vec<OsString>> {
    let raw = dir.into_raw_fd();
    let dirp = unsafe { fdopendir(raw) };
    if dirp.is_null() {
        let err = io::Error::last_os_error();
        unsafe { close(raw) };
        return Err(err);
    }
    let mut names = Vec::new();
    loop {
        let ent = unsafe { readdir(dirp) };
        if ent.is_null() {
            break;
        }
        let namlen = unsafe { (*ent).d_namlen as usize };
        let bytes =
            unsafe { std::slice::from_raw_parts((*ent).d_name.as_ptr().cast::<u8>(), namlen) };
        if bytes == b"." || bytes == b".." {
            continue;
        }
        names.push(OsStr::from_bytes(bytes).to_os_string());
    }
    unsafe { closedir(dirp) };
    Ok(names)
}

#[cfg(not(target_os = "macos"))]
fn read_dir_fd(dir: File) -> io::Result<Vec<OsString>> {
    let listing = fs::read_dir(format!("/proc/self/fd/{}", dir.as_raw_fd()))?;
    let mut names = Vec::new();
    for entry in listing {
        names.push(entry?.file_name());
    }
    Ok(names)
}

fn replace_hook(path: &Path) {
    #[cfg(test)]
    REPLACE_AT.with(|c| {
        if let Some(f) = c.borrow().as_ref() {
            f(path);
        }
    });
    let _ = path;
}

fn listing_replace_hook(path: &Path) {
    #[cfg(test)]
    LISTING_REPLACE_AT.with(|c| {
        if let Some(f) = c.borrow().as_ref() {
            f(path);
        }
    });
    let _ = path;
}

// --- staging -> root ------------------------------------------------------

/// `None` means "the real state is not something we can own here" — a
/// directory where a file belongs, or a path we cannot even stat (`ENOTDIR`:
/// a regular file sits where a parent directory belongs). Both are drift, to
/// be skipped and reported, never a hard error that would abort a
/// half-finished apply and recur on every retry.
fn current_state(path: &Path) -> Result<Option<Option<FileState>>> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(Some(None)),
        Err(_) => Ok(None),
        Ok(md) if md.is_file() => Ok(Some(Some(FileState {
            bytes: fs::read(path)?,
            executable: is_exec(md.permissions().mode()),
        }))),
        Ok(_) => Ok(None),
    }
}

/// True when `target`, the tracked root itself, or any directory between
/// them, is a symlink. The root is included on purpose: writing through a
/// symlinked root lands the bytes in whatever it points at.
fn symlink_on_path(root: &Path, target: &Path) -> bool {
    let mut at = Some(target);
    while let Some(p) = at {
        if !p.starts_with(root) {
            break;
        }
        if is_symlink(p) {
            return true;
        }
        at = p.parent();
    }
    false
}

fn is_symlink(p: &Path) -> bool {
    fs::symlink_metadata(p)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

fn write_atomic(src: &Path, dst: &Path) -> Result<()> {
    let md = fs::symlink_metadata(src)
        .with_context(|| format!("staging file {} is missing", src.display()))?;
    // `fs::read` follows symlinks, and staging holds whatever another device
    // committed: a mode-120000 blob pointing at ~/.ssh/id_rsa would otherwise
    // be read through and republished as ordinary content. `apply_to_root`
    // already skips these; this is the guard nearest the `fs::read`.
    if !md.file_type().is_file() {
        bail!("staging file {} is not a regular file", src.display());
    }
    let bytes = fs::read(src)?;
    write_atomic_bytes(&bytes, is_exec(md.permissions().mode()), dst)
}

/// Write `bytes` to `dst` atomically: a private temp file beside it, synced,
/// then renamed over it. An existing regular `dst` keeps its permissions;
/// only the exec bit follows `executable` (see [`mode_like`]).
fn write_atomic_bytes(bytes: &[u8], executable: bool, dst: &Path) -> Result<()> {
    let mode = mode_like(dst, executable);

    let dir = dst
        .parent()
        .ok_or_else(|| anyhow!("no parent directory for {}", dst.display()))?;
    fs::create_dir_all(dir)?;
    let name = dst
        .file_name()
        .ok_or_else(|| anyhow!("bad target {}", dst.display()))?
        .to_string_lossy()
        .into_owned();
    let tmp = dir.join(format!(".{name}.{}.dotlore-tmp", std::process::id()));

    let res = (|| -> Result<()> {
        // `create_new` + `mode`: the temp file never exists at the default
        // 0644 holding secret bytes, and `O_EXCL` refuses to follow a symlink
        // pre-planted at the predictable pid-based temp path. A leftover from
        // a crashed run of the same pid is removed once, then retried.
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create_new(true).mode(mode);
        let mut f = match opts.open(&tmp) {
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                fs::remove_file(&tmp)?;
                opts.open(&tmp)?
            }
            other => other?,
        };
        f.write_all(bytes)?;
        // `mode` was masked by the umask at open; restore it exactly. On the
        // open handle: `File::set_permissions` is `fchmod`, so it cannot be
        // redirected by swapping the predictable temp path for a symlink.
        f.set_permissions(fs::Permissions::from_mode(mode))?;
        // Without this the rename can be durable while the bytes are not: a
        // crash would leave a truncated live file that the next mirror pass
        // reads as a user edit and publishes to every other device.
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, dst)?;
        Ok(())
    })();
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    res.with_context(|| format!("writing {}", dst.display()))
}

// --- shared ---------------------------------------------------------------

fn is_exec(mode: u32) -> bool {
    mode & 0o111 != 0
}

/// The mode for bytes written at or beside `template`. Only the executable bit
/// is ours to set. An existing regular file keeps its own permissions:
/// settings.json holds API keys and is routinely 0600, and a fixed 0644 would
/// silently make it (or a copy next to it) world-readable.
fn mode_like(template: &Path, exec: bool) -> u32 {
    match fs::symlink_metadata(template) {
        Ok(md) if md.file_type().is_file() => {
            let cur = md.permissions().mode() & 0o777;
            if exec {
                cur | 0o111
            } else {
                cur & !0o111
            }
        }
        _ => mode_for(exec),
    }
}

fn mode_for(executable: bool) -> u32 {
    if executable {
        0o755
    } else {
        0o644
    }
}

/// Names that are staging-private and must never be mirrored either way.
/// `.dotlore-tmp` is included because a crash between `write_atomic`'s write
/// and its rename leaves a full copy of the file in the real root; mirroring
/// it would carry that copy on towards the shared cloud folder.
fn protected_name(name: &str) -> bool {
    crate::project::staging_private(name)
}

fn protected(rel: &Path) -> bool {
    rel.components()
        .any(|c| matches!(c, Component::Normal(n) if protected_name(&n.to_string_lossy())))
}

/// A relative path made only of plain names: no `..`, no root, no prefix.
fn plain_rel(rel: &Path) -> bool {
    !rel.as_os_str().is_empty() && rel.components().all(|c| matches!(c, Component::Normal(_)))
}

#[cfg(test)]
thread_local! {
    static REPLACE_AT: std::cell::RefCell<Option<Box<dyn Fn(&Path)>>> =
        const { std::cell::RefCell::new(None) };
    static LISTING_REPLACE_AT: std::cell::RefCell<Option<Box<dyn Fn(&Path)>>> =
        const { std::cell::RefCell::new(None) };
}

/// Test seam: run `hook` between inspecting a path component and opening it.
#[cfg(test)]
fn with_replace_hook<F, T>(hook: F, body: impl FnOnce() -> T) -> T
where
    F: Fn(&Path) + 'static,
{
    REPLACE_AT.with(|c| *c.borrow_mut() = Some(Box::new(hook)));
    let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));
    REPLACE_AT.with(|c| *c.borrow_mut() = None);
    match out {
        Ok(v) => v,
        Err(e) => std::panic::resume_unwind(e),
    }
}

/// Test seam: run `hook` after inspect classified a directory and before
/// the no-follow listing open.
#[cfg(test)]
fn with_listing_hook<F, T>(hook: F, body: impl FnOnce() -> T) -> T
where
    F: Fn(&Path) + 'static,
{
    LISTING_REPLACE_AT.with(|c| *c.borrow_mut() = Some(Box::new(hook)));
    let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));
    LISTING_REPLACE_AT.with(|c| *c.borrow_mut() = None);
    match out {
        Ok(v) => v,
        Err(e) => std::panic::resume_unwind(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use tempfile::TempDir;

    fn put(path: &Path, body: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    fn chmod(path: &Path, mode: u32) {
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    fn mode_of(path: &Path) -> u32 {
        fs::symlink_metadata(path).unwrap().permissions().mode() & 0o777
    }

    /// Every file under `dir`, relative and sorted.
    fn files(dir: &Path) -> Vec<String> {
        fn go(base: &Path, dir: &Path, out: &mut Vec<String>) {
            for e in fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if fs::symlink_metadata(&p).unwrap().is_dir() {
                    go(base, &p, out);
                } else {
                    out.push(p.strip_prefix(base).unwrap().to_string_lossy().into_owned());
                }
            }
        }
        let mut out = Vec::new();
        go(dir, dir, &mut out);
        out.sort();
        out
    }

    fn dirs(td: &TempDir) -> (PathBuf, PathBuf) {
        let root = td.path().join("root");
        let staging = td.path().join("staging");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&staging).unwrap();
        (root, staging)
    }

    fn tracked(keys: &[&str]) -> EntryList {
        let mut file = crate::project::ProjectFile::default();
        for k in keys {
            file.entries.insert(
                (*k).to_string(),
                crate::project::EntryRecord {
                    gen: 1,
                    state: crate::project::State::Tracked,
                },
            );
        }
        file.tracked()
    }

    fn to_staging(root: &Path, staging: &Path, keys: &[&str], ignore: &str) -> MirrorReport {
        to_staging_with(root, staging, keys, ignore, Limits::default())
    }

    fn to_staging_with(
        root: &Path,
        staging: &Path,
        keys: &[&str],
        ignore: &str,
        limits: Limits,
    ) -> MirrorReport {
        root_to_staging(root, staging, &tracked(keys), ignore, limits, None).unwrap()
    }

    fn tiny_limits(max_file_bytes: u64) -> Limits {
        Limits {
            max_file_bytes,
            max_seed_folder_bytes: u64::MAX,
        }
    }

    #[test]
    fn home_claude_ignore_keeps_the_whitelist_only() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        put(&root.join("agents/x.md"), "a");
        put(&root.join("settings.json"), "{}");
        put(&root.join("plugins/installed_plugins.json"), "{}");
        put(&root.join("projects/a.jsonl"), "{}");
        put(&root.join("plugins/marketplaces/m/README.md"), "m");
        put(&root.join("cache/x"), "c");

        let rep = to_staging(
            &root,
            &staging,
            &[
                "agents/",
                "settings.json",
                "plugins/",
                "projects/",
                "cache/",
            ],
            crate::project::DEFAULT_NEVER_IGNORE,
        );

        assert!(rep.changed);
        assert_eq!(
            files(&staging),
            vec![
                "agents/x.md".to_string(),
                "plugins/installed_plugins.json".to_string(),
                "plugins/marketplaces/m/README.md".to_string(),
                "settings.json".to_string(),
            ]
        );
    }

    #[test]
    fn nested_git_directory_is_pruned() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        put(&root.join("sub/file.txt"), "hi");
        put(&root.join("sub/.git/HEAD"), "ref: refs/heads/main\n");
        put(&root.join("sub/.git/config"), "[core]\n");

        to_staging(&root, &staging, &["sub/"], "");

        assert_eq!(files(&staging), vec!["sub/file.txt".to_string()]);
    }

    #[test]
    fn second_unchanged_run_reports_no_change() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        put(&root.join("a.md"), "one");
        put(&root.join("deep/b.md"), "two");

        assert!(to_staging(&root, &staging, &["a.md", "deep/"], "").changed);
        assert!(!to_staging(&root, &staging, &["a.md", "deep/"], "").changed);

        fs::remove_file(root.join("deep/b.md")).unwrap();
        assert!(to_staging(&root, &staging, &["a.md", "deep/"], "").changed);
        assert_eq!(files(&staging), vec!["a.md".to_string()]);
    }

    #[test]
    fn a_file_path_is_rejected_as_a_root() {
        let td = TempDir::new().unwrap();
        let staging = td.path().join("staging");
        let file = td.path().join("CLAUDE.md");
        put(&file, "guidance");
        fs::create_dir_all(&staging).unwrap();

        let err = root_to_staging(
            &file,
            &staging,
            &tracked(&["CLAUDE.md"]),
            "",
            Limits::default(),
            None,
        )
        .unwrap_err();
        assert!(err.to_string().contains("is not a directory"));
        assert_eq!(files(&staging), Vec::<String>::new());
    }

    /// Capture with the never-list `never` plus the project list `project`,
    /// guarded by the builtin sensitive patterns.
    fn to_staging_guarded(
        root: &Path,
        staging: &Path,
        keys: &[&str],
        never: &str,
        project: &str,
    ) -> MirrorReport {
        let guard = ReincludeGuard::new(
            root,
            never,
            crate::project::SensitiveMatcher::default_secret(),
        )
        .unwrap();
        root_to_staging(
            root,
            staging,
            &tracked(keys),
            &format!("{never}{project}"),
            Limits::default(),
            Some(&guard),
        )
        .unwrap()
    }

    #[test]
    fn a_reincluded_sensitive_file_is_never_staged() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        for rel in [
            "projects/a.md",
            "projects/x/.env",
            ".credentials.json",
            ".claude.json",
            "keep.log",
            "credentials.log",
        ] {
            put(&root.join(rel), "x");
        }

        to_staging_guarded(
            &root,
            &staging,
            &[
                "projects/",
                ".credentials.json",
                ".claude.json",
                "keep.log",
                "credentials.log",
            ],
            "/.credentials.json\n/.claude.json\n/projects/\n*.log\n",
            "!/projects/\n!/.credentials.json\n!/.claude.json\n!*.log\n",
        );

        assert_eq!(
            files(&staging),
            vec!["keep.log".to_string(), "projects/a.md".to_string()]
        );
    }

    #[test]
    fn a_reincluded_folder_never_stages_claude_json_copies() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        for rel in [
            "backups/.claude.json.backup.1",
            "backups/x/.claude.json",
            "backups/ok.md",
        ] {
            put(&root.join(rel), "x");
        }

        to_staging_guarded(
            &root,
            &staging,
            &["backups/"],
            "/backups/\n",
            "!/backups/\n",
        );

        assert_eq!(files(&staging), vec!["backups/ok.md".to_string()]);
    }

    #[test]
    fn list_live_leaves_out_a_blocked_oversized_file() {
        let td = TempDir::new().unwrap();
        let (root, _) = dirs(&td);
        put(&root.join("docs/credentials.log"), "too large");
        put(&root.join("docs/big.log"), "too large");
        let guard = ReincludeGuard::new(
            &root,
            "*.log\n",
            crate::project::SensitiveMatcher::default_secret(),
        )
        .unwrap();

        let mut live = list_live(
            &root,
            &tracked(&["docs/"]),
            "*.log\n!*.log\n",
            tiny_limits(4),
            Some(&guard),
        )
        .unwrap();
        live.sort();

        assert_eq!(live, vec![(PathBuf::from("docs/big.log"), 9, true)]);
    }

    #[test]
    fn a_sensitive_file_outside_the_never_list_is_not_blocked() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        put(&root.join("docs/.env"), "TOKEN=x");

        to_staging_guarded(&root, &staging, &["docs/"], "/projects/\n", "!/projects/\n");

        assert_eq!(files(&staging), vec!["docs/.env".to_string()]);
    }

    #[test]
    fn a_blocked_file_already_in_staging_is_pruned() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        put(&root.join(".credentials.json"), "{}");
        put(&staging.join(".credentials.json"), "{}");

        let rep = to_staging_guarded(
            &root,
            &staging,
            &[".credentials.json"],
            "/.credentials.json\n",
            "!/.credentials.json\n",
        );

        assert!(rep.changed);
        assert_eq!(files(&staging), Vec::<String>::new());
    }

    #[test]
    fn apply_skips_paths_that_drifted_from_the_snapshot() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        put(&staging.join("drifted.md"), "fresh");
        put(&staging.join("clean.md"), "fresh");
        put(&root.join("drifted.md"), "new");
        put(&root.join("clean.md"), "old");

        let old = || {
            Some(FileState {
                bytes: b"old".to_vec(),
                executable: false,
            })
        };
        let mut snap = Snapshot::new();
        snap.insert(PathBuf::from("drifted.md"), old());
        snap.insert(PathBuf::from("clean.md"), old());

        let skipped = apply_to_root(
            &staging,
            &root,
            &[
                (Status::Modified, PathBuf::from("drifted.md")),
                (Status::Modified, PathBuf::from("clean.md")),
            ],
            &snap,
            &td.path().join("trash"),
        )
        .unwrap();

        assert_eq!(skipped, vec![PathBuf::from("drifted.md")]);
        assert_eq!(fs::read(root.join("drifted.md")).unwrap(), b"new");
        assert_eq!(fs::read(root.join("clean.md")).unwrap(), b"fresh");
    }

    #[test]
    fn apply_without_an_expectation_is_an_error() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        put(&staging.join("a.md"), "fresh");

        let err = apply_to_root(
            &staging,
            &root,
            &[(Status::Added, PathBuf::from("a.md"))],
            &Snapshot::new(),
            &td.path().join("trash"),
        )
        .unwrap_err();

        assert!(err.to_string().contains("no apply expectation"));
        assert!(!root.join("a.md").exists());
    }

    #[test]
    fn binary_detection() {
        assert!(is_binary(b"a\0b"));
        assert!(!is_binary(b"hello"));
    }

    #[test]
    fn executable_bit_survives_both_directions() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        put(&root.join("hooks/run.sh"), "#!/bin/sh\n");
        chmod(&root.join("hooks/run.sh"), 0o755);
        put(&root.join("hooks/notes.md"), "plain");

        to_staging(&root, &staging, &["hooks/"], "");
        assert_eq!(mode_of(&staging.join("hooks/run.sh")), 0o755);
        assert_eq!(mode_of(&staging.join("hooks/notes.md")), 0o644);

        let other = td.path().join("other");
        let mut snap = Snapshot::new();
        snap.insert(PathBuf::from("hooks/run.sh"), None);
        let skipped = apply_to_root(
            &staging,
            &other,
            &[(Status::Added, PathBuf::from("hooks/run.sh"))],
            &snap,
            &td.path().join("trash"),
        )
        .unwrap();

        assert!(skipped.is_empty());
        assert_eq!(mode_of(&other.join("hooks/run.sh")), 0o755);
    }

    #[test]
    fn symlinks_are_never_followed_in_either_direction() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        put(&root.join("agents/x.md"), "a");
        fs::create_dir_all(root.join("skills")).unwrap();
        symlink("../agents", root.join("skills/link")).unwrap();

        let rep = to_staging(&root, &staging, &["agents/", "skills/"], "");
        assert_eq!(rep.skipped_symlinks, vec![PathBuf::from("skills/link")]);
        assert_eq!(files(&staging), vec!["agents/x.md".to_string()]);

        // Applying onto a root path that is a symlink leaves it alone.
        let other = td.path().join("other");
        fs::create_dir_all(&other).unwrap();
        put(&other.join("real.md"), "user data");
        symlink("real.md", other.join("x.md")).unwrap();
        put(&staging.join("x.md"), "incoming");

        let mut snap = Snapshot::new();
        snap.insert(PathBuf::from("x.md"), None);
        let skipped = apply_to_root(
            &staging,
            &other,
            &[(Status::Modified, PathBuf::from("x.md"))],
            &snap,
            &td.path().join("trash"),
        )
        .unwrap();

        assert_eq!(skipped, vec![PathBuf::from("x.md")]);
        assert!(fs::symlink_metadata(other.join("x.md"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read(other.join("real.md")).unwrap(), b"user data");
    }

    #[test]
    fn staging_private_files_never_reach_the_root() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        put(&staging.join(".dotloreignore"), "*.log\n");
        put(&staging.join(".dotloreignore-project"), "*.tmp\n");
        put(&staging.join(".dotloreproject"), "{}\n");
        put(&staging.join("a.conflict-dev2.md"), "loser");
        put(&root.join("keep.md"), "keep");

        let skipped = apply_to_root(
            &staging,
            &root,
            &[
                (Status::Added, PathBuf::from(".dotloreignore")),
                (Status::Added, PathBuf::from(".dotloreignore-project")),
                (Status::Added, PathBuf::from(".dotloreproject")),
                (Status::Added, PathBuf::from("a.conflict-dev2.md")),
            ],
            &Snapshot::new(),
            &td.path().join("trash"),
        )
        .unwrap();

        assert!(skipped.is_empty());
        assert_eq!(files(&root), vec!["keep.md".to_string()]);

        // ...and the mirror leaves them in staging instead of deleting them.
        let rep = to_staging(&root, &staging, &["keep.md"], "");
        assert!(rep.changed);
        assert_eq!(
            files(&staging),
            vec![
                ".dotloreignore".to_string(),
                ".dotloreignore-project".to_string(),
                ".dotloreproject".to_string(),
                "a.conflict-dev2.md".to_string(),
                "keep.md".to_string(),
            ]
        );
    }

    /// `delete_stale` must still prune a Finder file the walk never copies.
    /// Collapsing `.DS_Store` into the shared staging-private predicate
    /// would skip it here and leave the stray in the worktree forever.
    #[test]
    fn a_stray_ds_store_is_still_pruned_from_staging() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        put(&root.join("keep.md"), "keep");
        put(&staging.join(".DS_Store"), "finder");
        put(&staging.join("keep.md"), "keep");

        let rep = to_staging(&root, &staging, &["keep.md"], "");
        assert!(rep.changed);
        assert_eq!(files(&staging), vec!["keep.md".to_string()]);
    }

    /// C1: a symlink in the root hides a whole subtree from the walk. Staging
    /// must keep what another device published under it, or the next commit
    /// would publish a deletion of that device's own files.
    #[test]
    fn a_symlinked_subtree_is_never_deleted_from_staging() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        put(&root.join("a.md"), "mine");
        fs::create_dir_all(root.join("skills")).unwrap();
        symlink("../elsewhere", root.join("skills/link")).unwrap();
        // What device B published and the merge brought into staging.
        put(&staging.join("skills/link/a.md"), "device B data");
        put(&staging.join("a.md"), "mine");

        let rep = to_staging(&root, &staging, &["a.md", "skills/"], "");

        assert!(!rep.changed);
        assert_eq!(rep.skipped_symlinks, vec![PathBuf::from("skills/link")]);
        assert_eq!(
            fs::read(staging.join("skills/link/a.md")).unwrap(),
            b"device B data"
        );
    }

    /// I1: a tracked root that is itself a symlink must never be written
    /// through.
    #[test]
    fn a_symlinked_directory_root_is_never_written_through() {
        let td = TempDir::new().unwrap();
        let staging = td.path().join("staging");
        let real = td.path().join("real");
        fs::create_dir_all(&real).unwrap();
        put(&real.join("keep.md"), "user data");
        let link = td.path().join("link");
        symlink(&real, &link).unwrap();
        put(&staging.join("a.md"), "incoming");

        let mut snap = Snapshot::new();
        snap.insert(PathBuf::from("a.md"), None);
        let skipped = apply_to_root(
            &staging,
            &link,
            &[(Status::Added, PathBuf::from("a.md"))],
            &snap,
            &td.path().join("trash"),
        )
        .unwrap();

        assert_eq!(skipped, vec![PathBuf::from("a.md")]);
        assert_eq!(files(&real), vec!["keep.md".to_string()]);
    }

    /// I2: a regular file where a parent directory belongs is drift, not a
    /// hard error — the rest of the change set must still be applied.
    #[test]
    fn an_unstattable_path_is_drift_and_does_not_abort_the_apply() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        put(&staging.join("agents/x.md"), "incoming");
        put(&staging.join("b.md"), "incoming");
        // A regular file where the `agents` directory belongs: stat of
        // `agents/x.md` fails with ENOTDIR.
        put(&root.join("agents"), "not a directory");

        let mut snap = Snapshot::new();
        snap.insert(PathBuf::from("agents/x.md"), None);
        snap.insert(PathBuf::from("b.md"), None);
        let skipped = apply_to_root(
            &staging,
            &root,
            &[
                (Status::Added, PathBuf::from("agents/x.md")),
                (Status::Added, PathBuf::from("b.md")),
            ],
            &snap,
            &td.path().join("trash"),
        )
        .unwrap();

        assert_eq!(skipped, vec![PathBuf::from("agents/x.md")]);
        assert_eq!(fs::read(root.join("agents")).unwrap(), b"not a directory");
        assert_eq!(fs::read(root.join("b.md")).unwrap(), b"incoming");
    }

    /// I6: applying to an existing file keeps its permissions; only the
    /// executable bit follows the staging file.
    #[test]
    fn apply_keeps_the_existing_targets_permissions() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        put(&staging.join("settings.json"), r#"{"key":"new"}"#);
        put(&root.join("settings.json"), r#"{"key":"old"}"#);
        chmod(&root.join("settings.json"), 0o600);

        let mut snap = Snapshot::new();
        snap.insert(
            PathBuf::from("settings.json"),
            Some(FileState {
                bytes: br#"{"key":"old"}"#.to_vec(),
                executable: false,
            }),
        );
        let skipped = apply_to_root(
            &staging,
            &root,
            &[(Status::Modified, PathBuf::from("settings.json"))],
            &snap,
            &td.path().join("trash"),
        )
        .unwrap();

        assert!(skipped.is_empty());
        assert_eq!(
            fs::read(root.join("settings.json")).unwrap(),
            br#"{"key":"new"}"#
        );
        assert_eq!(mode_of(&root.join("settings.json")), 0o600);
    }

    /// Pins `write_atomic`'s `set_permissions` on the open handle: 0o666 is a
    /// mode the umask actually masks, so without that call the temp file stays
    /// at 0o644 and the rename carries 0o644 onto the target. Discriminates
    /// under umask 022, 002 and 077; under umask 000 it passes either way
    /// (vacuous, never flaky).
    #[test]
    fn apply_restores_a_mode_the_umask_would_have_masked() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        put(&staging.join("notes.md"), "new");
        put(&root.join("notes.md"), "old");
        chmod(&root.join("notes.md"), 0o666);

        let mut snap = Snapshot::new();
        snap.insert(
            PathBuf::from("notes.md"),
            Some(FileState {
                bytes: b"old".to_vec(),
                executable: false,
            }),
        );
        let skipped = apply_to_root(
            &staging,
            &root,
            &[(Status::Modified, PathBuf::from("notes.md"))],
            &snap,
            &td.path().join("trash"),
        )
        .unwrap();

        assert!(skipped.is_empty());
        assert_eq!(fs::read(root.join("notes.md")).unwrap(), b"new");
        assert_eq!(mode_of(&root.join("notes.md")), 0o666);
    }

    /// I6, both directions: the executable bit follows staging, every other
    /// permission bit of an existing target is left alone.
    #[test]
    fn apply_toggles_only_the_exec_bit_on_an_existing_target() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        // 0o600 target + executable staging file -> 0o711.
        put(&staging.join("run.sh"), "new");
        chmod(&staging.join("run.sh"), 0o755);
        put(&root.join("run.sh"), "old");
        chmod(&root.join("run.sh"), 0o600);
        // 0o755 target + plain staging file -> 0o644.
        put(&staging.join("notes.md"), "new");
        chmod(&staging.join("notes.md"), 0o644);
        put(&root.join("notes.md"), "old");
        chmod(&root.join("notes.md"), 0o755);

        let state = |exec| {
            Some(FileState {
                bytes: b"old".to_vec(),
                executable: exec,
            })
        };
        let mut snap = Snapshot::new();
        snap.insert(PathBuf::from("run.sh"), state(false));
        snap.insert(PathBuf::from("notes.md"), state(true));
        let skipped = apply_to_root(
            &staging,
            &root,
            &[
                (Status::Modified, PathBuf::from("run.sh")),
                (Status::Modified, PathBuf::from("notes.md")),
            ],
            &snap,
            &td.path().join("trash"),
        )
        .unwrap();

        assert!(skipped.is_empty());
        assert_eq!(mode_of(&root.join("run.sh")), 0o711);
        assert_eq!(mode_of(&root.join("notes.md")), 0o644);
        assert_eq!(fs::read(root.join("run.sh")).unwrap(), b"new");
        assert_eq!(fs::read(root.join("notes.md")).unwrap(), b"new");
    }

    /// R2/F1: staging is filled by another device's commit, and git stores
    /// symlinks as ordinary blobs. Reading through one would republish the
    /// target's bytes to the shared cloud folder. It is reported in `skipped`
    /// rather than aborting: an abort would leave the root half-applied and
    /// re-fail identically on every retry, wedging the slug for good.
    #[test]
    fn a_symlink_in_staging_is_never_read_through() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        let secret = td.path().join("id_rsa");
        put(&secret, "PRIVATE KEY");
        symlink(&secret, staging.join("settings.json")).unwrap();
        put(&staging.join("b.md"), "incoming");

        let mut snap = Snapshot::new();
        snap.insert(PathBuf::from("settings.json"), None);
        snap.insert(PathBuf::from("b.md"), None);
        let skipped = apply_to_root(
            &staging,
            &root,
            &[
                (Status::Added, PathBuf::from("settings.json")),
                (Status::Added, PathBuf::from("b.md")),
            ],
            &snap,
            &td.path().join("trash"),
        )
        .unwrap();

        assert_eq!(skipped, vec![PathBuf::from("settings.json")]);
        assert_eq!(fs::read(&secret).unwrap(), b"PRIVATE KEY");
        // The entry after the hostile one is still applied.
        assert_eq!(files(&root), vec!["b.md".to_string()]);
    }

    /// F1: a staging entry that vanished between the diff and the apply is
    /// skipped too, and does not abort the entries behind it.
    #[test]
    fn a_missing_staging_entry_is_skipped_not_fatal() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        put(&staging.join("b.md"), "incoming");

        let mut snap = Snapshot::new();
        snap.insert(PathBuf::from("gone.md"), None);
        snap.insert(PathBuf::from("b.md"), None);
        let skipped = apply_to_root(
            &staging,
            &root,
            &[
                (Status::Added, PathBuf::from("gone.md")),
                (Status::Added, PathBuf::from("b.md")),
            ],
            &snap,
            &td.path().join("trash"),
        )
        .unwrap();

        assert_eq!(skipped, vec![PathBuf::from("gone.md")]);
        assert_eq!(files(&root), vec!["b.md".to_string()]);
    }

    /// F2: `walk` stats the whole tree before the copy loop runs, so a root
    /// entry swapped for a symlink inside that window reaches
    /// `copy_if_changed` as a symlink. The window is not deterministically
    /// drivable through `root_to_staging`, so the guard is exercised directly.
    #[test]
    fn a_root_entry_swapped_for_a_symlink_is_not_copied_into_staging() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        let secret = td.path().join("id_rsa");
        put(&secret, "PRIVATE KEY");
        symlink(&secret, root.join("a.md")).unwrap();
        put(&staging.join("a.md"), "device B data");

        assert_eq!(
            copy_if_changed(&root, Path::new("a.md"), &staging.join("a.md")).unwrap(),
            None
        );
        assert_eq!(fs::read(staging.join("a.md")).unwrap(), b"device B data");
    }

    /// F4: the delete arm — a clean delete, a drifted path that must survive,
    /// and an already-absent path that must not error.
    #[test]
    fn deletes_respect_the_snapshot_and_are_idempotent() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        put(&root.join("clean.md"), "old");
        put(&root.join("drifted.md"), "locally edited");

        let old = || {
            Some(FileState {
                bytes: b"old".to_vec(),
                executable: false,
            })
        };
        let mut snap = Snapshot::new();
        snap.insert(PathBuf::from("clean.md"), old());
        snap.insert(PathBuf::from("drifted.md"), old());
        snap.insert(PathBuf::from("absent.md"), None);

        let skipped = apply_to_root(
            &staging,
            &root,
            &[
                (Status::Deleted, PathBuf::from("clean.md")),
                (Status::Deleted, PathBuf::from("drifted.md")),
                (Status::Deleted, PathBuf::from("absent.md")),
            ],
            &snap,
            &td.path().join("trash"),
        )
        .unwrap();

        assert_eq!(skipped, vec![PathBuf::from("drifted.md")]);
        assert!(!root.join("clean.md").exists());
        assert_eq!(
            fs::read(root.join("drifted.md")).unwrap(),
            b"locally edited"
        );
        assert_eq!(files(&root), vec!["drifted.md".to_string()]);
        assert_eq!(
            files(&td.path().join("trash")),
            vec!["clean.md".to_string()]
        );
    }

    /// Every live file an apply deletes or overwrites is copied to the trash
    /// first, and a second copy of the same path never replaces the first.
    /// Without this a wrong merge would leave no local copy to restore.
    #[test]
    fn displaced_live_files_are_kept_in_the_trash() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        let trash = td.path().join("trash");
        put(&root.join("docs/gone.md"), "old gone");
        put(&root.join("docs/edit.md"), "old edit");
        put(&root.join("run.sh"), "old run");
        chmod(&root.join("run.sh"), 0o755);
        put(&staging.join("docs/edit.md"), "new edit");
        put(&staging.join("run.sh"), "new run");
        put(&staging.join("new.md"), "new");

        let state = |b: &str, executable| {
            Some(FileState {
                bytes: b.as_bytes().to_vec(),
                executable,
            })
        };
        let mut snap = Snapshot::new();
        snap.insert(PathBuf::from("docs/gone.md"), state("old gone", false));
        snap.insert(PathBuf::from("docs/edit.md"), state("old edit", false));
        snap.insert(PathBuf::from("run.sh"), state("old run", true));
        snap.insert(PathBuf::from("new.md"), None);

        let skipped = apply_to_root(
            &staging,
            &root,
            &[
                (Status::Deleted, PathBuf::from("docs/gone.md")),
                (Status::Modified, PathBuf::from("docs/edit.md")),
                (Status::Modified, PathBuf::from("run.sh")),
                (Status::Added, PathBuf::from("new.md")),
            ],
            &snap,
            &trash,
        )
        .unwrap();

        assert!(skipped.is_empty());
        assert!(!root.join("docs/gone.md").exists());
        assert_eq!(fs::read(root.join("docs/edit.md")).unwrap(), b"new edit");
        assert_eq!(fs::read(trash.join("docs/gone.md")).unwrap(), b"old gone");
        assert_eq!(fs::read(trash.join("docs/edit.md")).unwrap(), b"old edit");
        assert_eq!(fs::read(trash.join("run.sh")).unwrap(), b"old run");
        assert_eq!(mode_of(&trash.join("run.sh")), 0o700);
        assert_eq!(mode_of(&trash.join("docs/edit.md")), 0o600);
        // `new.md` was absent, so there was nothing to keep.
        assert_eq!(files(&trash).len(), 3);

        put(&staging.join("docs/edit.md"), "newer edit");
        snap.insert(PathBuf::from("docs/edit.md"), state("new edit", false));
        let skipped = apply_to_root(
            &staging,
            &root,
            &[(Status::Modified, PathBuf::from("docs/edit.md"))],
            &snap,
            &trash,
        )
        .unwrap();

        assert!(skipped.is_empty());
        assert_eq!(fs::read(trash.join("docs/edit.md")).unwrap(), b"old edit");
        // The second copy keeps its exact rel, in a sibling pass folder.
        assert_eq!(
            fs::read(td.path().join("trash-1/docs/edit.md")).unwrap(),
            b"new edit"
        );
        assert!(!trash.join("docs/edit.1.md").exists());
    }

    /// Each further copy of a path in one pass goes to the next free sibling
    /// pass folder, and the earlier copies are never replaced.
    #[test]
    fn a_third_copy_in_the_same_pass_goes_to_dash_two() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        let trash = td.path().join("trash");
        put(&root.join("docs/edit.md"), "v0");
        let mut snap = Snapshot::new();
        for (old, new) in [("v0", "v1"), ("v1", "v2"), ("v2", "v3")] {
            put(&staging.join("docs/edit.md"), new);
            snap.insert(
                PathBuf::from("docs/edit.md"),
                Some(FileState {
                    bytes: old.as_bytes().to_vec(),
                    executable: false,
                }),
            );
            let skipped = apply_to_root(
                &staging,
                &root,
                &[(Status::Modified, PathBuf::from("docs/edit.md"))],
                &snap,
                &trash,
            )
            .unwrap();
            assert!(skipped.is_empty());
        }

        assert_eq!(fs::read(trash.join("docs/edit.md")).unwrap(), b"v0");
        assert_eq!(
            fs::read(td.path().join("trash-1/docs/edit.md")).unwrap(),
            b"v1"
        );
        assert_eq!(
            fs::read(td.path().join("trash-2/docs/edit.md")).unwrap(),
            b"v2"
        );
        assert_eq!(mode_of(&td.path().join("trash-2/docs/edit.md")), 0o600);
        assert_eq!(fs::read(root.join("docs/edit.md")).unwrap(), b"v3");
    }

    /// A copy that cannot be kept aborts before the live file is touched:
    /// deleting or overwriting it anyway could destroy the only local copy.
    #[test]
    fn a_failed_trash_copy_leaves_the_live_file_alone() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        let trash = td.path().join("trash");
        // A regular file where the copy's parent directory belongs.
        put(&trash.join("docs"), "in the way");
        put(&root.join("docs/gone.md"), "old gone");
        put(&root.join("docs/edit.md"), "old edit");
        put(&staging.join("docs/edit.md"), "new edit");
        let state = |b: &str| {
            Some(FileState {
                bytes: b.as_bytes().to_vec(),
                executable: false,
            })
        };
        let mut snap = Snapshot::new();
        snap.insert(PathBuf::from("docs/gone.md"), state("old gone"));
        snap.insert(PathBuf::from("docs/edit.md"), state("old edit"));

        for change in [
            (Status::Deleted, PathBuf::from("docs/gone.md")),
            (Status::Modified, PathBuf::from("docs/edit.md")),
        ] {
            assert!(apply_to_root(&staging, &root, &[change], &snap, &trash).is_err());
        }
        assert_eq!(fs::read(root.join("docs/gone.md")).unwrap(), b"old gone");
        assert_eq!(fs::read(root.join("docs/edit.md")).unwrap(), b"old edit");
    }

    /// A drifted path is not touched, so nothing of it lands in the trash.
    #[test]
    fn a_skipped_path_leaves_nothing_in_the_trash() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        put(&root.join("a.md"), "edited by hand");
        let mut snap = Snapshot::new();
        snap.insert(
            PathBuf::from("a.md"),
            Some(FileState {
                bytes: b"old".to_vec(),
                executable: false,
            }),
        );

        let skipped = apply_to_root(
            &staging,
            &root,
            &[(Status::Deleted, PathBuf::from("a.md"))],
            &snap,
            &td.path().join("trash"),
        )
        .unwrap();

        assert_eq!(skipped, vec![PathBuf::from("a.md")]);
        assert!(!td.path().join("trash").exists());
    }

    /// R1: the temp file is created with `O_EXCL`, so a symlink pre-planted at
    /// the predictable temp path is unlinked rather than written through.
    #[test]
    fn a_pre_planted_temp_symlink_is_not_followed() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        put(&staging.join("settings.json"), "incoming");
        let victim = td.path().join("victim");
        put(&victim, "user data");
        let tmp = root.join(format!(".settings.json.{}.dotlore-tmp", std::process::id()));
        symlink(&victim, &tmp).unwrap();

        let mut snap = Snapshot::new();
        snap.insert(PathBuf::from("settings.json"), None);
        let skipped = apply_to_root(
            &staging,
            &root,
            &[(Status::Added, PathBuf::from("settings.json"))],
            &snap,
            &td.path().join("trash"),
        )
        .unwrap();

        assert!(skipped.is_empty());
        assert_eq!(fs::read(&victim).unwrap(), b"user data");
        assert_eq!(fs::read(root.join("settings.json")).unwrap(), b"incoming");
        assert!(!fs::symlink_metadata(root.join("settings.json"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(fs::symlink_metadata(&tmp).is_err());
    }

    /// R1: a leftover temp from a crashed run of the same pid must not wedge
    /// every later apply.
    #[test]
    fn a_stale_temp_file_is_replaced() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        put(&staging.join("a.md"), "incoming");
        let tmp = root.join(format!(".a.md.{}.dotlore-tmp", std::process::id()));
        put(&tmp, "crashed run leftover");
        // Read-only on purpose: this is what discriminates against the
        // pre-R1 `fs::write` implementation, which would have overwritten a
        // writable leftover and passed regardless.
        chmod(&tmp, 0o444);

        let mut snap = Snapshot::new();
        snap.insert(PathBuf::from("a.md"), None);
        let skipped = apply_to_root(
            &staging,
            &root,
            &[(Status::Added, PathBuf::from("a.md"))],
            &snap,
            &td.path().join("trash"),
        )
        .unwrap();

        assert!(skipped.is_empty());
        assert_eq!(fs::read(root.join("a.md")).unwrap(), b"incoming");
        assert!(fs::symlink_metadata(&tmp).is_err());
    }

    #[test]
    fn traversal_paths_are_rejected() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        assert!(apply_to_root(
            &staging,
            &root,
            &[(Status::Added, PathBuf::from("../escape.md"))],
            &Snapshot::new(),
            &td.path().join("trash"),
        )
        .is_err());
    }

    const SENTINEL: &[u8] = b"OUTSIDE-ROOT-SENTINEL";

    fn staging_holds(staging: &Path, needle: &[u8]) -> bool {
        fn go(dir: &Path, needle: &[u8]) -> bool {
            for e in fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                let md = fs::symlink_metadata(&p).unwrap();
                if md.is_dir() {
                    if go(&p, needle) {
                        return true;
                    }
                } else if md.is_file() && fs::read(&p).unwrap() == needle {
                    return true;
                }
            }
            false
        }
        go(staging, needle)
    }

    fn replace_with_symlink(path: &Path, target: &Path) {
        if fs::symlink_metadata(path)
            .map(|m| m.is_dir())
            .unwrap_or(false)
        {
            fs::remove_dir_all(path).unwrap();
        } else {
            let _ = fs::remove_file(path);
        }
        symlink(target, path).unwrap();
    }

    #[test]
    fn the_walk_visits_only_directory_entries_in_the_include_list() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        put(&root.join("docs/a.md"), "docs");
        put(&root.join("docs/b.md"), "docs-b");
        put(&root.join("secrets/x.md"), "secret");
        put(&root.join("CLAUDE.md"), "hi");

        to_staging(&root, &staging, &["docs/", "CLAUDE.md"], "");

        assert_eq!(
            files(&staging),
            vec![
                "CLAUDE.md".to_string(),
                "docs/a.md".to_string(),
                "docs/b.md".to_string(),
            ]
        );
        assert!(!staging.join("secrets").exists());
    }

    #[test]
    fn a_tracked_directory_replaced_by_a_symlink_is_not_read_or_pruned() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        put(&root.join("docs/a.md"), "keep-me");
        to_staging(&root, &staging, &["docs/"], "");
        assert_eq!(fs::read(staging.join("docs/a.md")).unwrap(), b"keep-me");

        let outside = td.path().join("outside");
        put(
            &outside.join("a.md"),
            std::str::from_utf8(SENTINEL).unwrap(),
        );
        replace_with_symlink(&root.join("docs"), &outside);

        let rep = to_staging(&root, &staging, &["docs/"], "");
        assert!(rep.skipped_symlinks.iter().any(|p| p == Path::new("docs")));
        assert_eq!(fs::read(staging.join("docs/a.md")).unwrap(), b"keep-me");
        assert!(!staging_holds(&staging, SENTINEL));

        // Replacement at the inspect/open boundary: a real dir becomes a
        // symlink after the component is classified, before it is opened.
        let td2 = TempDir::new().unwrap();
        let (root2, staging2) = dirs(&td2);
        put(&root2.join("docs/a.md"), "keep-me");
        put(&staging2.join("docs/a.md"), "keep-me");
        let outside2 = td2.path().join("outside");
        put(
            &outside2.join("a.md"),
            std::str::from_utf8(SENTINEL).unwrap(),
        );
        let docs = root2.join("docs");
        let outside2_clone = outside2.clone();
        let rep2 = with_replace_hook(
            move |p| {
                if p == docs.as_path() {
                    replace_with_symlink(&docs, &outside2_clone);
                }
            },
            || to_staging(&root2, &staging2, &["docs/"], ""),
        );
        assert!(rep2.skipped_symlinks.iter().any(|p| p == Path::new("docs")));
        assert_eq!(fs::read(staging2.join("docs/a.md")).unwrap(), b"keep-me");
        assert!(!staging_holds(&staging2, SENTINEL));
    }

    /// Listing must not follow a symlink planted after `inspect` returned
    /// `Dir`. Path-based `read_dir` would list the outside directory (empty
    /// here) and `delete_stale` would publish a deletion of every staged
    /// child.
    #[test]
    fn a_directory_replaced_after_inspect_is_not_listed_or_pruned() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        put(&root.join("docs/a.md"), "live");
        put(&staging.join("docs/a.md"), "keep-me");
        let outside = td.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        let docs = root.join("docs");
        let outside_clone = outside.clone();
        let docs_clone = docs.clone();
        let rep = with_listing_hook(
            move |p| {
                if p == docs_clone.as_path() {
                    replace_with_symlink(&docs_clone, &outside_clone);
                }
            },
            || to_staging(&root, &staging, &["docs/"], ""),
        );
        assert!(
            rep.skipped_symlinks.iter().any(|p| p == Path::new("docs")),
            "listing-time replacement must be opaque, got {rep:?}"
        );
        assert_eq!(fs::read(staging.join("docs/a.md")).unwrap(), b"keep-me");
        assert!(!staging_holds(&staging, SENTINEL));
    }

    #[test]
    fn an_explicit_file_under_a_symlink_ancestor_is_not_read_or_published() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        put(
            &staging.join(".github/copilot-instructions.md"),
            "old-staged",
        );

        let outside = td.path().join("outside");
        put(
            &outside.join("copilot-instructions.md"),
            std::str::from_utf8(SENTINEL).unwrap(),
        );
        symlink(&outside, root.join(".github")).unwrap();

        let rep = to_staging(&root, &staging, &[".github/copilot-instructions.md"], "");
        assert!(rep
            .skipped_symlinks
            .iter()
            .any(|p| p == Path::new(".github")));
        assert_eq!(
            fs::read(staging.join(".github/copilot-instructions.md")).unwrap(),
            b"old-staged"
        );
        assert!(!staging_holds(&staging, SENTINEL));

        let td2 = TempDir::new().unwrap();
        let (root2, staging2) = dirs(&td2);
        put(&root2.join(".github/copilot-instructions.md"), "safe");
        put(
            &staging2.join(".github/copilot-instructions.md"),
            "old-staged",
        );
        let outside2 = td2.path().join("outside");
        put(
            &outside2.join("copilot-instructions.md"),
            std::str::from_utf8(SENTINEL).unwrap(),
        );
        let github = root2.join(".github");
        let outside2_clone = outside2.clone();
        let rep2 = with_replace_hook(
            move |p| {
                if p == github.as_path() {
                    replace_with_symlink(&github, &outside2_clone);
                }
            },
            || to_staging(&root2, &staging2, &[".github/copilot-instructions.md"], ""),
        );
        assert!(rep2
            .skipped_symlinks
            .iter()
            .any(|p| p == Path::new(".github")));
        assert_eq!(
            fs::read(staging2.join(".github/copilot-instructions.md")).unwrap(),
            b"old-staged"
        );
        assert!(!staging_holds(&staging2, SENTINEL));
    }

    #[test]
    fn an_ignored_explicit_file_is_pruned() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        put(&root.join("secret.md"), "live");
        put(&staging.join("secret.md"), "staged");

        to_staging(&root, &staging, &["secret.md"], "secret.md\n");
        assert!(!staging.join("secret.md").exists());
    }

    #[test]
    fn a_missing_tracked_directory_publishes_deletions_without_failing() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        put(&staging.join("docs/a.md"), "gone");
        put(&root.join("keep.md"), "keep");

        to_staging(&root, &staging, &["docs/", "keep.md"], "");
        assert!(!staging.join("docs/a.md").exists());
        assert_eq!(fs::read(staging.join("keep.md")).unwrap(), b"keep");
    }

    #[test]
    fn a_tracked_entry_kind_change_preserves_staging() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        put(&staging.join("docs/a.md"), "keep");
        put(&root.join("docs"), "now a file");

        to_staging(&root, &staging, &["docs/"], "");
        assert_eq!(fs::read(staging.join("docs/a.md")).unwrap(), b"keep");
    }

    #[test]
    fn deleting_a_nested_explicit_file_prunes_only_that_file() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        put(&root.join(".github/copilot-instructions.md"), "live");
        put(&root.join(".github/other.md"), "sibling-live");
        put(&staging.join(".github/copilot-instructions.md"), "staged");
        put(&staging.join(".github/other.md"), "sibling-staged");

        to_staging(&root, &staging, &[".github/copilot-instructions.md"], "");
        assert_eq!(
            fs::read(staging.join(".github/copilot-instructions.md")).unwrap(),
            b"live"
        );
        assert_eq!(
            fs::read(staging.join(".github/other.md")).unwrap(),
            b"sibling-staged"
        );

        fs::remove_file(root.join(".github/copilot-instructions.md")).unwrap();
        to_staging(&root, &staging, &[".github/copilot-instructions.md"], "");
        assert!(!staging.join(".github/copilot-instructions.md").exists());
        assert_eq!(
            fs::read(staging.join(".github/other.md")).unwrap(),
            b"sibling-staged"
        );
    }

    #[test]
    fn deleting_a_file_under_a_nested_directory_entry_is_synced() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        put(&root.join(".kiro/steering/a.md"), "a");
        put(&root.join(".kiro/steering/b.md"), "b");
        put(&root.join(".kiro/hooks/x.md"), "hook");
        put(&staging.join(".kiro/steering/a.md"), "a");
        put(&staging.join(".kiro/steering/b.md"), "b");
        put(&staging.join(".kiro/hooks/x.md"), "hook-staged");

        to_staging(&root, &staging, &[".kiro/steering/"], "");
        assert_eq!(
            files(&staging)
                .into_iter()
                .filter(|p| p.starts_with(".kiro/steering/"))
                .collect::<Vec<_>>(),
            vec![
                ".kiro/steering/a.md".to_string(),
                ".kiro/steering/b.md".to_string()
            ]
        );
        assert_eq!(
            fs::read(staging.join(".kiro/hooks/x.md")).unwrap(),
            b"hook-staged"
        );

        fs::remove_file(root.join(".kiro/steering/a.md")).unwrap();
        to_staging(&root, &staging, &[".kiro/steering/"], "");
        assert!(!staging.join(".kiro/steering/a.md").exists());
        assert_eq!(fs::read(staging.join(".kiro/steering/b.md")).unwrap(), b"b");
        assert_eq!(
            fs::read(staging.join(".kiro/hooks/x.md")).unwrap(),
            b"hook-staged"
        );
    }

    #[test]
    fn untracking_an_entry_leaves_its_blobs_in_staging() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        put(&root.join("a.md"), "a");
        put(&root.join("b.md"), "b");

        to_staging(&root, &staging, &["a.md", "b.md"], "");
        assert_eq!(fs::read(staging.join("b.md")).unwrap(), b"b");

        to_staging(&root, &staging, &["a.md"], "");
        assert_eq!(
            fs::read(staging.join("b.md")).unwrap(),
            b"b",
            "untracking must not prune the leftover blob from staging"
        );
        assert_eq!(fs::read(staging.join("a.md")).unwrap(), b"a");
    }

    #[test]
    fn an_ignored_file_inside_a_tracked_directory_is_still_pruned() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        put(&root.join("docs/keep.md"), "keep");
        put(&root.join("docs/secret.log"), "leak");
        put(&staging.join("docs/keep.md"), "keep");
        put(&staging.join("docs/secret.log"), "leak");

        to_staging(&root, &staging, &["docs/"], "*.log\n");
        assert_eq!(files(&staging), vec!["docs/keep.md".to_string()]);
    }

    #[test]
    fn a_file_over_the_limit_is_not_mirrored_and_is_reported() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        put(&root.join("docs/small.md"), "ok");
        put(&root.join("docs/big.md"), "too-big!!");
        let rep = to_staging_with(&root, &staging, &["docs/"], "", tiny_limits(8));
        assert_eq!(files(&staging), vec!["docs/small.md".to_string()]);
        assert_eq!(
            rep.skipped_too_large,
            vec![(PathBuf::from("docs/big.md"), b"too-big!!".len() as u64)]
        );
        assert!(!staging.join("docs/big.md").exists());
    }

    #[test]
    fn a_file_exactly_at_the_limit_is_still_mirrored() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        put(&root.join("docs/exact.md"), "12345678");
        let rep = to_staging_with(&root, &staging, &["docs/"], "", tiny_limits(8));
        assert_eq!(files(&staging), vec!["docs/exact.md".to_string()]);
        assert!(rep.skipped_too_large.is_empty());
        assert_eq!(
            fs::read(staging.join("docs/exact.md")).unwrap(),
            b"12345678"
        );
    }

    #[test]
    fn a_tracked_file_that_grows_past_the_limit_is_not_deleted_from_staging() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        put(&root.join("docs/grow.md"), "tiny");
        to_staging_with(&root, &staging, &["docs/"], "", tiny_limits(8));
        assert_eq!(fs::read(staging.join("docs/grow.md")).unwrap(), b"tiny");

        put(&root.join("docs/grow.md"), "now-far-too-large");
        let rep = to_staging_with(&root, &staging, &["docs/"], "", tiny_limits(8));
        assert_eq!(
            fs::read(staging.join("docs/grow.md")).unwrap(),
            b"tiny",
            "staging must keep the last-good copy when the live file grows past the limit"
        );
        assert_eq!(
            rep.skipped_too_large,
            vec![(
                PathBuf::from("docs/grow.md"),
                b"now-far-too-large".len() as u64
            )]
        );
    }

    #[test]
    fn an_explicit_file_that_grows_over_the_limit_is_opaque() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        put(&root.join("notes.md"), "ok");
        to_staging_with(&root, &staging, &["notes.md"], "", tiny_limits(8));
        assert_eq!(fs::read(staging.join("notes.md")).unwrap(), b"ok");

        put(&root.join("notes.md"), "now-too-big");
        let rep = to_staging_with(&root, &staging, &["notes.md"], "", tiny_limits(8));
        assert_eq!(fs::read(staging.join("notes.md")).unwrap(), b"ok");
        assert_eq!(rep.skipped_too_large, vec![(PathBuf::from("notes.md"), 11)]);
        assert!(staging.join("notes.md").is_file());
    }

    #[test]
    fn an_explicit_file_obeys_ignore_and_size_checks() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        put(&root.join("ok.md"), "keep");
        put(&root.join("secret.log"), "leak");
        put(&root.join("big.md"), "too-big!!");
        put(&staging.join("secret.log"), "stale");
        put(&staging.join("big.md"), "stale-big");

        let rep = to_staging_with(
            &root,
            &staging,
            &["ok.md", "secret.log", "big.md"],
            "*.log\n",
            tiny_limits(8),
        );
        assert_eq!(
            files(&staging),
            vec!["big.md".to_string(), "ok.md".to_string()]
        );
        assert_eq!(fs::read(staging.join("ok.md")).unwrap(), b"keep");
        assert_eq!(
            fs::read(staging.join("big.md")).unwrap(),
            b"stale-big",
            "an over-limit explicit file is opaque, not pruned"
        );
        assert!(
            !staging.join("secret.log").exists(),
            "an ignored explicit file is still pruned"
        );
        assert_eq!(
            rep.skipped_too_large,
            vec![(PathBuf::from("big.md"), b"too-big!!".len() as u64)]
        );
    }

    // --- restore_to_root ----------------------------------------------------

    /// A root, a 0600 trash copy at `<td>/trash/<rel>` holding `body`, and the
    /// displaced pass folder a replaced live file is kept in.
    fn restore_dirs(td: &TempDir, rel: &str, body: &str) -> (PathBuf, PathBuf, PathBuf) {
        let (root, _) = dirs(td);
        let src = td.path().join("trash").join(rel);
        put(&src, body);
        chmod(&src, 0o600);
        (root, src, td.path().join("displaced"))
    }

    #[test]
    fn restore_writes_a_missing_file_with_parents() {
        let td = TempDir::new().unwrap();
        let (root, src, displaced) = restore_dirs(&td, "docs/deep/a.md", "kept");

        let out = restore_to_root(
            &root,
            Path::new("docs/deep/a.md"),
            &src,
            &displaced,
            OnExisting::Ask,
        )
        .unwrap();

        assert_eq!(out, Restored::Written(PathBuf::from("docs/deep/a.md")));
        assert_eq!(fs::read(root.join("docs/deep/a.md")).unwrap(), b"kept");
        assert!(!displaced.exists());
    }

    #[test]
    fn restore_asks_when_a_different_live_file_exists() {
        let td = TempDir::new().unwrap();
        let (root, src, displaced) = restore_dirs(&td, "a.md", "old");
        put(&root.join("a.md"), "live");

        let out =
            restore_to_root(&root, Path::new("a.md"), &src, &displaced, OnExisting::Ask).unwrap();

        assert_eq!(out, Restored::Exists);
        assert_eq!(fs::read(root.join("a.md")).unwrap(), b"live");
        assert_eq!(files(&root), vec!["a.md".to_string()]);
        assert!(!displaced.exists());
    }

    #[test]
    fn restore_replace_trashes_the_live_file_first_then_overwrites() {
        let td = TempDir::new().unwrap();
        let (root, src, displaced) = restore_dirs(&td, "docs/a.md", "old");
        put(&root.join("docs/a.md"), "live");

        let out = restore_to_root(
            &root,
            Path::new("docs/a.md"),
            &src,
            &displaced,
            OnExisting::Replace,
        )
        .unwrap();

        assert_eq!(out, Restored::Written(PathBuf::from("docs/a.md")));
        assert_eq!(fs::read(root.join("docs/a.md")).unwrap(), b"old");
        assert_eq!(fs::read(displaced.join("docs/a.md")).unwrap(), b"live");
        assert_eq!(mode_of(&displaced.join("docs/a.md")), 0o600);
    }

    #[test]
    fn restore_replace_aborts_before_the_live_file_when_the_trash_copy_fails() {
        let td = TempDir::new().unwrap();
        let (root, src, displaced) = restore_dirs(&td, "docs/a.md", "old");
        put(&root.join("docs/a.md"), "live");
        // A regular file where the displaced pass needs a directory.
        put(&displaced.join("docs"), "in the way");

        let res = restore_to_root(
            &root,
            Path::new("docs/a.md"),
            &src,
            &displaced,
            OnExisting::Replace,
        );

        assert!(res.is_err());
        assert_eq!(fs::read(root.join("docs/a.md")).unwrap(), b"live");
    }

    /// A Keep both copy takes the live file's permissions, so the old bytes of
    /// a 0600 `settings.json` never land in a world-readable sibling.
    #[test]
    fn restore_keep_both_keeps_the_live_files_private_mode() {
        let td = TempDir::new().unwrap();
        let (root, src, displaced) = restore_dirs(&td, "settings.json", "old");
        put(&root.join("settings.json"), "live");
        chmod(&root.join("settings.json"), 0o600);
        let out = restore_to_root(
            &root,
            Path::new("settings.json"),
            &src,
            &displaced,
            OnExisting::KeepBoth,
        )
        .unwrap();
        assert_eq!(
            out,
            Restored::Written(PathBuf::from("settings.restored.json"))
        );
        assert_eq!(mode_of(&root.join("settings.restored.json")), 0o600);

        // Only the exec bit follows the trash copy, set the way a sync
        // overwrite sets it (`write_atomic`).
        let src = td.path().join("trash/run.sh");
        put(&src, "old");
        chmod(&src, 0o700);
        put(&root.join("run.sh"), "live");
        chmod(&root.join("run.sh"), 0o640);
        restore_to_root(
            &root,
            Path::new("run.sh"),
            &src,
            &displaced,
            OnExisting::KeepBoth,
        )
        .unwrap();
        assert_eq!(mode_of(&root.join("run.restored.sh")), 0o751);
    }

    #[test]
    fn restore_keep_both_writes_a_restored_sibling_numbered_if_taken() {
        let td = TempDir::new().unwrap();
        let (root, src, displaced) = restore_dirs(&td, "docs/a.md", "old");
        put(&root.join("docs/a.md"), "live");
        let rel = Path::new("docs/a.md");

        let first = restore_to_root(&root, rel, &src, &displaced, OnExisting::KeepBoth).unwrap();
        let second = restore_to_root(&root, rel, &src, &displaced, OnExisting::KeepBoth).unwrap();

        assert_eq!(
            first,
            Restored::Written(PathBuf::from("docs/a.restored.md"))
        );
        assert_eq!(
            second,
            Restored::Written(PathBuf::from("docs/a.restored.1.md"))
        );
        assert_eq!(fs::read(root.join("docs/a.md")).unwrap(), b"live");
        assert_eq!(fs::read(root.join("docs/a.restored.md")).unwrap(), b"old");
        assert_eq!(fs::read(root.join("docs/a.restored.1.md")).unwrap(), b"old");
        assert_eq!(mode_of(&root.join("docs/a.restored.md")), 0o644);
        assert!(!displaced.exists());

        // No extension, and a dotfile: the suffix goes on the whole name.
        for (name, sibling) in [
            ("Makefile", "Makefile.restored"),
            (".envrc", ".envrc.restored"),
        ] {
            let src = td.path().join("trash").join(name);
            put(&src, "old");
            put(&root.join(name), "live");
            let out = restore_to_root(
                &root,
                Path::new(name),
                &src,
                &displaced,
                OnExisting::KeepBoth,
            )
            .unwrap();
            assert_eq!(out, Restored::Written(PathBuf::from(sibling)));
            assert_eq!(fs::read(root.join(name)).unwrap(), b"live");
        }
    }

    #[test]
    fn restore_keep_both_never_follows_a_planted_sibling_symlink() {
        let td = TempDir::new().unwrap();
        let (root, src, displaced) = restore_dirs(&td, "a.md", "old");
        put(&root.join("a.md"), "live");
        let outside = td.path().join("outside.md");
        put(&outside, "outside");
        symlink(&outside, root.join("a.restored.md")).unwrap();

        let out = restore_to_root(
            &root,
            Path::new("a.md"),
            &src,
            &displaced,
            OnExisting::KeepBoth,
        )
        .unwrap();

        assert_eq!(out, Restored::Written(PathBuf::from("a.restored.1.md")));
        assert_eq!(fs::read(&outside).unwrap(), b"outside");
    }

    #[test]
    fn restore_same_bytes_is_a_no_op() {
        let td = TempDir::new().unwrap();
        let (root, src, displaced) = restore_dirs(&td, "a.md", "same");
        put(&root.join("a.md"), "same");

        for on_existing in [OnExisting::Ask, OnExisting::Replace, OnExisting::KeepBoth] {
            let out =
                restore_to_root(&root, Path::new("a.md"), &src, &displaced, on_existing).unwrap();
            assert_eq!(out, Restored::Same);
        }
        assert_eq!(files(&root), vec!["a.md".to_string()]);
        assert!(!displaced.exists());
    }

    #[test]
    fn restore_never_follows_a_symlinked_root_or_ancestor() {
        let td = TempDir::new().unwrap();
        let (root, src, displaced) = restore_dirs(&td, "docs/a.md", "old");
        let outside = td.path().join("outside");
        put(&outside.join("keep.md"), "user data");
        symlink(&outside, root.join("docs")).unwrap();
        let link = td.path().join("link");
        symlink(&outside, &link).unwrap();

        for on_existing in [OnExisting::Ask, OnExisting::Replace, OnExisting::KeepBoth] {
            let rel = Path::new("docs/a.md");
            let out = restore_to_root(&root, rel, &src, &displaced, on_existing).unwrap();
            assert_eq!(out, Restored::Skipped);
            let out =
                restore_to_root(&link, Path::new("a.md"), &src, &displaced, on_existing).unwrap();
            assert_eq!(out, Restored::Skipped);
        }
        assert_eq!(files(&outside), vec!["keep.md".to_string()]);
        assert!(!displaced.exists());
    }

    #[test]
    fn restore_skips_protected_names() {
        let td = TempDir::new().unwrap();
        let (root, _, displaced) = restore_dirs(&td, "a.md", "old");

        for rel in [
            ".dotloreproject",
            ".dotloreignore",
            "docs/a.conflict-dev1.md",
            ".git/config",
            "a.md.dotlore-tmp",
        ] {
            let src = td.path().join("trash").join(rel);
            put(&src, "old");
            let out = restore_to_root(&root, Path::new(rel), &src, &displaced, OnExisting::Replace)
                .unwrap();
            assert_eq!(out, Restored::Skipped, "{rel}");
        }
        assert!(files(&root).is_empty());
    }

    #[test]
    fn restore_rejects_a_non_regular_source() {
        let td = TempDir::new().unwrap();
        let (root, src, displaced) = restore_dirs(&td, "a.md", "old");
        for rel in ["../a.md", "/a.md", ""] {
            let res = restore_to_root(&root, Path::new(rel), &src, &displaced, OnExisting::Ask);
            assert!(res.is_err(), "{rel}");
        }

        let link = td.path().join("trash/link.md");
        symlink(&src, &link).unwrap();
        let dir = td.path().join("trash/dir.md");
        fs::create_dir_all(&dir).unwrap();
        for bad in [&link, &dir] {
            let err = restore_to_root(&root, Path::new("a.md"), bad, &displaced, OnExisting::Ask)
                .unwrap_err();
            assert!(err.to_string().contains("not a regular file"), "{err}");
        }
        assert!(files(&root).is_empty());
    }

    #[test]
    fn restore_gives_the_written_file_a_normal_mode() {
        let td = TempDir::new().unwrap();
        let (root, src, displaced) = restore_dirs(&td, "a.md", "text");
        let run = td.path().join("trash/run.sh");
        put(&run, "#!/bin/sh");
        chmod(&run, 0o700);

        restore_to_root(&root, Path::new("a.md"), &src, &displaced, OnExisting::Ask).unwrap();
        restore_to_root(
            &root,
            Path::new("run.sh"),
            &run,
            &displaced,
            OnExisting::Ask,
        )
        .unwrap();

        assert_eq!(mode_of(&root.join("a.md")), 0o644);
        assert_eq!(mode_of(&root.join("run.sh")), 0o755);
    }

    // --- restore_state_to_root ----------------------------------------------

    fn state(body: &str, executable: bool) -> FileState {
        FileState {
            bytes: body.as_bytes().to_vec(),
            executable,
        }
    }

    #[test]
    fn restore_state_writes_a_missing_file_with_parents_and_its_exec_bit() {
        let td = TempDir::new().unwrap();
        let (root, _) = dirs(&td);
        let displaced = td.path().join("displaced");

        for (rel, exec, mode) in [
            ("docs/deep/a.md", false, 0o644),
            ("bin/run.sh", true, 0o755),
        ] {
            let out = restore_state_to_root(
                &root,
                Path::new(rel),
                &state("kept", exec),
                &displaced,
                OnExisting::Ask,
            )
            .unwrap();
            assert_eq!(out, Restored::Written(PathBuf::from(rel)));
            assert_eq!(fs::read(root.join(rel)).unwrap(), b"kept");
            assert_eq!(mode_of(&root.join(rel)), mode, "{rel}");
        }
        assert!(!displaced.exists());
    }

    #[test]
    fn restore_state_same_bytes_is_a_no_op() {
        let td = TempDir::new().unwrap();
        let (root, _) = dirs(&td);
        let displaced = td.path().join("displaced");
        put(&root.join("a.md"), "same");

        for on_existing in [OnExisting::Ask, OnExisting::Replace, OnExisting::KeepBoth] {
            let out = restore_state_to_root(
                &root,
                Path::new("a.md"),
                &state("same", false),
                &displaced,
                on_existing,
            )
            .unwrap();
            assert_eq!(out, Restored::Same);
        }
        assert_eq!(files(&root), vec!["a.md".to_string()]);
        assert!(!displaced.exists());
    }

    #[test]
    fn restore_state_replace_trashes_the_live_file_first_then_overwrites() {
        let td = TempDir::new().unwrap();
        let (root, _) = dirs(&td);
        let displaced = td.path().join("displaced");
        put(&root.join("docs/a.md"), "live");

        let out = restore_state_to_root(
            &root,
            Path::new("docs/a.md"),
            &state("old", false),
            &displaced,
            OnExisting::Replace,
        )
        .unwrap();

        assert_eq!(out, Restored::Written(PathBuf::from("docs/a.md")));
        assert_eq!(fs::read(root.join("docs/a.md")).unwrap(), b"old");
        assert_eq!(fs::read(displaced.join("docs/a.md")).unwrap(), b"live");
        assert_eq!(mode_of(&displaced.join("docs/a.md")), 0o600);
    }

    #[test]
    fn restore_state_keep_both_writes_a_restored_sibling() {
        let td = TempDir::new().unwrap();
        let (root, _) = dirs(&td);
        let displaced = td.path().join("displaced");
        put(&root.join("docs/a.md"), "live");

        let out = restore_state_to_root(
            &root,
            Path::new("docs/a.md"),
            &state("old", false),
            &displaced,
            OnExisting::KeepBoth,
        )
        .unwrap();

        assert_eq!(out, Restored::Written(PathBuf::from("docs/a.restored.md")));
        assert_eq!(fs::read(root.join("docs/a.md")).unwrap(), b"live");
        assert_eq!(fs::read(root.join("docs/a.restored.md")).unwrap(), b"old");
        assert!(!displaced.exists());
    }

    #[test]
    fn restore_state_refuses_unsafe_rels_protected_names_and_symlinked_ancestors() {
        let td = TempDir::new().unwrap();
        let (root, _) = dirs(&td);
        let displaced = td.path().join("displaced");
        let old = state("old", false);
        let outside = td.path().join("outside");
        put(&outside.join("keep.md"), "user data");
        symlink(&outside, root.join("docs")).unwrap();

        for rel in ["../a.md", "/a.md", ""] {
            let res =
                restore_state_to_root(&root, Path::new(rel), &old, &displaced, OnExisting::Ask);
            assert!(res.is_err(), "{rel}");
        }
        for rel in [".dotloreproject", "x/a.conflict-dev1.md", "docs/a.md"] {
            let out =
                restore_state_to_root(&root, Path::new(rel), &old, &displaced, OnExisting::Replace)
                    .unwrap();
            assert_eq!(out, Restored::Skipped, "{rel}");
        }
        assert_eq!(files(&outside), vec!["keep.md".to_string()]);
        assert!(!root.join(".dotloreproject").exists());
        assert!(!displaced.exists());
    }
}
