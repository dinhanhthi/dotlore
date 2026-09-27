//! Copying a tracked root into and out of its staging worktree.
//!
//! This is the only place that moves bytes between a live tracked root and its
//! staging git worktree. Two rules shape everything here: nothing git-private
//! (`.git`, `.dotloreignore`, `*.conflict-*`) ever reaches the root, and a root
//! file is only overwritten when its current bytes and mode are exactly what
//! the caller expected them to be.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::io::{ErrorKind, Read, Write};
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use ignore::gitignore::{Gitignore, GitignoreBuilder};

use crate::nofollow::{
    is_unsafe_open, listing_replace_hook, open_chain, open_dir_nofollow, open_root_dir, read_dir_fd,
};
use crate::portable;
use crate::project::{EntryList, Limits};

/// What happened to one logical entry between two staging commits.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Added,
    Modified,
    Deleted,
}

/// The content of one file as far as dotlore cares: bytes plus the only mode
/// bit git records.
///
/// `executable` is `None` when this filesystem cannot report the bit. That is
/// not "not executable": the mode the tree already has for the path is kept,
/// and a path with no tree entry is `100644`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FileState {
    pub bytes: Vec<u8>,
    pub executable: Option<bool>,
}

/// Bytes match, and the executable bit matches when both sides know it.
///
/// `None` agrees with either bit, so a filesystem that cannot report the bit
/// is not drift against a tree that stored `100755`.
pub(crate) fn states_match(a: &Option<FileState>, b: &Option<FileState>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.bytes == b.bytes && exec_agrees(a.executable, b.executable),
        _ => false,
    }
}

fn exec_agrees(a: Option<bool>, b: Option<bool>) -> bool {
    match (a, b) {
        (Some(x), Some(y)) => x == y,
        _ => true,
    }
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
    /// Non-portable names and case-collision losers. Not copied. The caller
    /// puts back an index blob HEAD already has, so the skip is not a deletion.
    /// A non-portable file already in staging is left in place.
    pub skipped_nonportable: Vec<(PathBuf, String)>,
}

/// Copy a tracked root into its staging worktree, scoped by `entries` and
/// filtered by `ignore_text` (gitignore syntax; the caller reads
/// `<staging>/.dotloreignore` or passes a default). Byte- and mode-identical
/// files are left untouched, so a quiet root reports `changed == false`.
///
/// `also_tracked` is paths already in the staging index. They join the live
/// walk when deciding case collisions, so a second casing that this
/// filesystem cannot store beside the live file is still seen. One dirent in
/// a fold is one file on every volume: the live bytes are copied onto the
/// byte-wise-first spelling already in the index, and that path is not a
/// collision. Any other spelling in the fold stays one.
pub fn root_to_staging(
    root: &Path,
    staging: &Path,
    entries: &EntryList,
    ignore_text: &str,
    limits: Limits,
    also_tracked: &[PathBuf],
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

    // One dirent in a fold is one file on every volume, not only a
    // case-insensitive one. Copy its live bytes onto the byte-wise-first
    // spelling already in the index. A second dirent (case-sensitive) and
    // any index spelling with no dirent of its own stay collisions.
    let mut copy_src: HashMap<PathBuf, PathBuf> = HashMap::new();
    retarget_case_folds(&mut want, also_tracked, entries, &mut copy_src);

    // Live files plus names already committed. A skipped path is not copied.
    let mut tracked_names: Vec<String> = want
        .iter()
        .filter_map(|rel| rel.to_str().map(str::to_string))
        .collect();
    for rel in also_tracked {
        if entries.contains_rel(rel) {
            if let Some(name) = rel.to_str() {
                tracked_names.push(name.to_string());
            }
        }
    }
    let skips = portable::publish_skips(tracked_names.iter().map(String::as_str));
    for (rel, reason) in &skips {
        want.remove(Path::new(rel.as_str()));
        report
            .skipped_nonportable
            .push((PathBuf::from(rel.as_str()), reason.clone()));
    }
    // Byte-wise first spelling of each fold: the one `publish_skips` keeps.
    let mut winner_of: HashMap<String, String> = HashMap::new();
    for name in &tracked_names {
        winner_of
            .entry(name.to_lowercase())
            .and_modify(|cur| {
                if name.as_str() < cur.as_str() {
                    *cur = name.clone();
                }
            })
            .or_insert_with(|| name.clone());
    }

    fs::create_dir_all(staging)?;
    // Keep a skipped path's inode only when deleting it would delete something
    // we are about to copy back, or when the name cannot exist on Windows.
    // A case fold whose live dirent is gone must be free to drop the winner.
    let keep: HashSet<&Path> = want.iter().map(PathBuf::as_path).collect();
    let mut preserve_case: HashSet<String> = HashSet::new();
    for (rel, _) in &skips {
        let fold = rel.to_lowercase();
        let winner_kept = winner_of
            .get(&fold)
            .is_some_and(|winner| keep.contains(Path::new(winner.as_str())));
        if portable::name_issue(rel).is_some() || winner_kept {
            preserve_case.insert(fold);
            opaque.insert(PathBuf::from(rel.as_str()));
        }
    }
    let fold_volume = crate::platform::case_insensitive_fs(staging);
    report.changed = delete_stale(
        staging,
        Path::new(""),
        &keep,
        &opaque,
        entries,
        &preserve_case,
    )?;
    for rel in &want {
        // `delete_stale` ran above with `keep` built from `want`, which holds
        // `rel`, so a skipped entry keeps whatever staging already has.
        // A case-rename reads the live dirent and writes the index spelling.
        let src_rel = copy_src.get(rel).map(PathBuf::as_path).unwrap_or(rel);
        match copy_if_changed(root, src_rel, &staging.join(rel))? {
            Some(changed) => {
                report.changed |= changed;
                // `fs::write` through the other casing does not rename the
                // dirent. `git add` later uses the directory entry's name.
                if fold_volume {
                    match_file_spelling(staging, rel)?;
                }
            }
            None => report.skipped_symlinks.push(rel.clone()),
        }
    }
    Ok(report)
}

/// Point the one live dirent in a case fold at the spelling already in the index.
///
/// Runs on every volume. One dirent is one file: its bytes are copied onto
/// the byte-wise-first index spelling, even when the dirent matches a losing
/// spelling exactly. Any other index spelling has no dirent and stays a
/// collision. Two dirents are left alone so a case-sensitive volume keeps both.
fn retarget_case_folds(
    want: &mut HashSet<PathBuf>,
    also_tracked: &[PathBuf],
    entries: &EntryList,
    copy_src: &mut HashMap<PathBuf, PathBuf>,
) {
    let mut index_by_fold: HashMap<String, Vec<String>> = HashMap::new();
    for rel in also_tracked {
        if !entries.contains_rel(rel) {
            continue;
        }
        let Some(name) = rel.to_str() else { continue };
        index_by_fold
            .entry(name.to_lowercase())
            .or_default()
            .push(name.to_string());
    }
    let mut live_by_fold: HashMap<String, Vec<PathBuf>> = HashMap::new();
    for rel in want.iter() {
        let Some(name) = rel.to_str() else { continue };
        live_by_fold
            .entry(name.to_lowercase())
            .or_default()
            .push(rel.clone());
    }
    for (fold, live) in live_by_fold {
        if live.len() != 1 {
            continue;
        }
        let rel = &live[0];
        let Some(name) = rel.to_str() else { continue };
        let Some(index_names) = index_by_fold.get(&fold) else {
            continue;
        };
        // Byte-wise first, the spelling `publish_skips` would keep. With one
        // index path that is the path already in HEAD.
        let Some(head) = index_names.iter().min() else {
            continue;
        };
        if head == name {
            continue;
        }
        let head_path = PathBuf::from(head);
        want.remove(rel);
        want.insert(head_path.clone());
        copy_src.insert(head_path, rel.clone());
    }
}

/// Rename the staging dirent so its bytes match `rel`, on a volume that folds case.
///
/// Writing `A.md` when the directory entry is `a.md` updates the bytes and
/// leaves the name `a.md`. The publish step adds whatever name `read_dir` returns.
fn match_file_spelling(staging: &Path, rel: &Path) -> Result<()> {
    let Some(want_name) = rel.file_name().and_then(|n| n.to_str()) else {
        return Ok(());
    };
    let parent = rel.parent().unwrap_or(Path::new(""));
    let dir = staging.join(parent);
    let want_fold = want_name.to_lowercase();
    let mut actual: Option<String> = None;
    for entry in fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if name.to_lowercase() == want_fold {
            actual = Some(name);
            break;
        }
    }
    let Some(actual) = actual else {
        return Ok(());
    };
    if actual != want_name {
        fs::rename(dir.join(&actual), dir.join(want_name)).with_context(|| {
            format!(
                "renaming {} to the index spelling {}",
                parent.join(&actual).display(),
                rel.display()
            )
        })?;
    }
    Ok(())
}

/// Apply staging changes back to the live root.
///
/// `snap` holds the expected current root state per logical path; any entry
/// whose real state differs is left alone and returned, as is any path that is
/// (or sits under) a symlink and any whose staging source is missing or not a
/// regular file. A missing expectation is an error: every write must be
/// justified by one.
pub fn apply_to_root(
    staging: &Path,
    root: &Path,
    changes: &[(Status, PathBuf)],
    snap: &Snapshot,
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

        // Unknown executable bit matches either side. Comparing it with `==`
        // would skip every apply on a filesystem that cannot report the bit.
        if !states_match(&current, expected) {
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
                    Ok(md) if md.file_type().is_file() => write_atomic(&src, &target)?,
                    _ => skipped.push(rel.clone()),
                }
            }
            Status::Deleted => match fs::remove_file(&target) {
                Ok(()) => {}
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(e) => return Err(e).with_context(|| format!("removing {}", target.display())),
            },
        }
    }
    Ok(skipped)
}

/// A NUL byte in the first 8000 bytes, the same heuristic git uses.
pub fn is_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8000).any(|&b| b == 0)
}

// --- root -> staging ------------------------------------------------------

fn build_ignore(root: &Path, ignore_text: &str) -> Result<Gitignore> {
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

/// Live include-list files with sizes, using the same walk as
/// `root_to_staging`. `too_large` is true when the file exceeds
/// `limits.max_file_bytes` and was therefore not copied into staging.
pub(crate) fn list_live(
    root: &Path,
    entries: &EntryList,
    ignore_text: &str,
    limits: Limits,
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
    let mut out = Vec::new();
    for rel in want {
        match inspect(root, &rel)? {
            Node::File { len } => out.push((rel, len, false)),
            _ => {}
        }
    }
    for (rel, len) in report.skipped_too_large {
        if entries.contains_rel(&rel) {
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
    preserve_case: &HashSet<String>,
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
                changed |= delete_stale(staging, &child, keep, opaque, entries, preserve_case)?;
                if fs::read_dir(&path)?.next().is_none() {
                    fs::remove_dir(&path)?;
                }
            }
        } else if name == ".DS_Store" {
            fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
            changed = true;
        } else if entries.contains_rel(&child) && !keep.contains(child.as_path()) {
            // Set only when this fold's winner is still being copied, or when
            // the name cannot exist on Windows. A shared lowercase key must
            // not keep the winner after the live file is gone.
            if child
                .to_str()
                .is_some_and(|name| preserve_case.contains(&name.to_lowercase()))
            {
                continue;
            }
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
    let executable = exec_bit(&md);
    let mut bytes = Vec::new();
    src.read_to_end(&mut bytes)
        .with_context(|| format!("reading {}", root.join(rel).display()))?;

    match fs::symlink_metadata(dst) {
        Ok(dmd) if dmd.is_file() => {
            if exec_agrees(exec_bit(&dmd), executable) && fs::read(dst)? == bytes {
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
    // `None`: leave the worktree mode alone. `git add` would otherwise record
    // `100644` for a file the tree already has at `100755`. The commit path
    // puts the index mode back, and uses `100644` only for a new path.
    #[cfg(unix)]
    if let Some(bit) = executable {
        fs::set_permissions(dst, fs::Permissions::from_mode(mode_for(bit)))?;
    }
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
            executable: exec_bit(&md),
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
    // Only the executable bit is ours to set, and only when this filesystem
    // can report it. Unknown must not be written as "not executable".
    #[cfg(unix)]
    let mode = unix_dest_mode(dst, exec_bit(&md));

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
        opts.write(true).create_new(true);
        #[cfg(unix)]
        if let Some(mode) = mode {
            opts.mode(mode);
        }
        let mut f = match opts.open(&tmp) {
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                fs::remove_file(&tmp)?;
                opts.open(&tmp)?
            }
            other => other?,
        };
        f.write_all(&bytes)?;
        // `mode` was masked by the umask at open; restore it exactly. On the
        // open handle: `File::set_permissions` is `fchmod`, so it cannot be
        // redirected by swapping the predictable temp path for a symlink.
        // Unknown bit: do not call `set_permissions` at all.
        #[cfg(unix)]
        if let Some(mode) = mode {
            f.set_permissions(fs::Permissions::from_mode(mode))?;
        }
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

#[cfg(test)]
thread_local! {
    static EXEC_BIT_UNKNOWN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
fn exec_bit_forced_unknown() -> bool {
    EXEC_BIT_UNKNOWN.with(|c| c.get())
}

#[cfg(not(test))]
fn exec_bit_forced_unknown() -> bool {
    false
}

/// Test seam: [`exec_bit`] returns `None` on this thread, as it does on a
/// filesystem that cannot report the bit.
#[cfg(test)]
pub(crate) fn with_exec_bit_unknown<T>(body: impl FnOnce() -> T) -> T {
    EXEC_BIT_UNKNOWN.with(|c| c.set(true));
    let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));
    EXEC_BIT_UNKNOWN.with(|c| c.set(false));
    match out {
        Ok(value) => value,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

/// Execute bit of `md`, when this platform can see one.
///
/// Unix returns `Some(mode & 0o111 != 0)`. `None` means the filesystem cannot
/// tell — not "not executable". Callers keep the tree's mode for that path.
pub fn exec_bit(md: &fs::Metadata) -> Option<bool> {
    if exec_bit_forced_unknown() {
        return None;
    }
    #[cfg(unix)]
    {
        Some(md.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        let _ = md;
        None
    }
}

/// `false` when [`exec_bit`] is `None` for every file (non-unix, or the test seam).
pub(crate) fn reports_exec_bit() -> bool {
    !exec_bit_forced_unknown() && cfg!(unix)
}

/// Mode to write on the way out. `None` means do not call `set_permissions`.
///
/// A known bit changes only `0o111` on an existing file; the other bits stay,
/// so a `0600` secret does not become `0644`.
#[cfg(unix)]
fn unix_dest_mode(dst: &Path, exec: Option<bool>) -> Option<u32> {
    let exec = exec?;
    Some(match fs::symlink_metadata(dst) {
        Ok(dmd) if dmd.file_type().is_file() => {
            let cur = dmd.permissions().mode() & 0o777;
            if exec {
                cur | 0o111
            } else {
                cur & !0o111
            }
        }
        _ => mode_for(exec),
    })
}

#[cfg(unix)]
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
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};
    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    #[cfg(unix)]
    use crate::nofollow::{with_listing_hook, with_replace_hook};
    use tempfile::TempDir;

    fn put(path: &Path, body: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    #[cfg(unix)]
    fn chmod(path: &Path, mode: u32) {
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    #[cfg(unix)]
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
        root_to_staging(root, staging, &tracked(keys), ignore, limits, &[]).unwrap()
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
            &[],
        )
        .unwrap_err();
        assert!(err.to_string().contains("is not a directory"));
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
                executable: Some(false),
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

    #[cfg(unix)]
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
        )
        .unwrap();

        assert!(skipped.is_empty());
        assert_eq!(mode_of(&other.join("hooks/run.sh")), 0o755);
    }

    #[cfg(unix)]
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
        put(&staging.join(".dotloreproject"), "{}\n");
        put(&staging.join("a.conflict-dev2.md"), "loser");
        put(&root.join("keep.md"), "keep");

        let skipped = apply_to_root(
            &staging,
            &root,
            &[
                (Status::Added, PathBuf::from(".dotloreignore")),
                (Status::Added, PathBuf::from(".dotloreproject")),
                (Status::Added, PathBuf::from("a.conflict-dev2.md")),
            ],
            &Snapshot::new(),
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
    #[cfg(unix)]
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
    #[cfg(unix)]
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
        )
        .unwrap();

        assert_eq!(skipped, vec![PathBuf::from("agents/x.md")]);
        assert_eq!(fs::read(root.join("agents")).unwrap(), b"not a directory");
        assert_eq!(fs::read(root.join("b.md")).unwrap(), b"incoming");
    }

    /// I6: applying to an existing file keeps its permissions; only the
    /// executable bit follows the staging file.
    #[cfg(unix)]
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
                executable: Some(false),
            }),
        );
        let skipped = apply_to_root(
            &staging,
            &root,
            &[(Status::Modified, PathBuf::from("settings.json"))],
            &snap,
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
    #[cfg(unix)]
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
                executable: Some(false),
            }),
        );
        let skipped = apply_to_root(
            &staging,
            &root,
            &[(Status::Modified, PathBuf::from("notes.md"))],
            &snap,
        )
        .unwrap();

        assert!(skipped.is_empty());
        assert_eq!(fs::read(root.join("notes.md")).unwrap(), b"new");
        assert_eq!(mode_of(&root.join("notes.md")), 0o666);
    }

    /// I6, both directions: the executable bit follows staging, every other
    /// permission bit of an existing target is left alone.
    #[cfg(unix)]
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
                executable: Some(exec),
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
    #[cfg(unix)]
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
        )
        .unwrap();

        assert_eq!(skipped, vec![PathBuf::from("gone.md")]);
        assert_eq!(files(&root), vec!["b.md".to_string()]);
    }

    /// F2: `walk` stats the whole tree before the copy loop runs, so a root
    /// entry swapped for a symlink inside that window reaches
    /// `copy_if_changed` as a symlink. The window is not deterministically
    /// drivable through `root_to_staging`, so the guard is exercised directly.
    #[cfg(unix)]
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
                executable: Some(false),
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
        )
        .unwrap();

        assert_eq!(skipped, vec![PathBuf::from("drifted.md")]);
        assert!(!root.join("clean.md").exists());
        assert_eq!(
            fs::read(root.join("drifted.md")).unwrap(),
            b"locally edited"
        );
        assert_eq!(files(&root), vec!["drifted.md".to_string()]);
    }

    /// R1: the temp file is created with `O_EXCL`, so a symlink pre-planted at
    /// the predictable temp path is unlinked rather than written through.
    #[cfg(unix)]
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
    #[cfg(unix)]
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

    #[cfg(unix)]
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

    #[cfg(unix)]
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
    #[cfg(unix)]
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

    #[cfg(unix)]
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

    #[test]
    fn nonportable_names_are_not_copied_and_an_existing_copy_is_kept() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        put(&root.join("ok.md"), "ok");
        put(&root.join("a:b.md"), "new");
        put(&root.join("CON.md"), "con");
        put(&staging.join("a:b.md"), "old");

        let rep = to_staging(&root, &staging, &["ok.md", "a:b.md", "CON.md"], "");
        assert_eq!(fs::read(staging.join("ok.md")).unwrap(), b"ok");
        assert_eq!(fs::read(staging.join("a:b.md")).unwrap(), b"old");
        assert!(!staging.join("CON.md").exists());
        assert_eq!(fs::read(root.join("a:b.md")).unwrap(), b"new");
        assert_eq!(
            rep.skipped_nonportable,
            vec![
                (
                    PathBuf::from("CON.md"),
                    "name cannot exist on Windows: is a reserved Windows device name".to_string()
                ),
                (
                    PathBuf::from("a:b.md"),
                    "name cannot exist on Windows: contains ':'".to_string()
                ),
            ]
        );
    }

    #[test]
    fn a_case_collision_loser_supplied_from_the_index_is_skipped() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        put(&root.join("A.md"), "upper");
        put(&root.join("ok.md"), "ok");

        let rep = root_to_staging(
            &root,
            &staging,
            &tracked(&["A.md", "a.md", "ok.md"]),
            "",
            Limits::default(),
            &[PathBuf::from("A.md"), PathBuf::from("a.md")],
        )
        .unwrap();
        assert_eq!(fs::read(staging.join("A.md")).unwrap(), b"upper");
        assert_eq!(fs::read(staging.join("ok.md")).unwrap(), b"ok");
        assert!(rep
            .skipped_nonportable
            .iter()
            .any(|(path, reason)| path == Path::new("a.md")
                && reason == "differs only by case from A.md"));
        assert!(!rep
            .skipped_nonportable
            .iter()
            .any(|(path, _)| path == Path::new("A.md")));
    }

    /// Both spellings are in the index and the only dirent is the loser.
    /// The live bytes belong on the byte-wise-first spelling; the loser stays
    /// a collision.
    #[test]
    fn the_losing_dirent_is_copied_onto_the_index_winner() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        put(&root.join("a.md"), "new");
        put(&root.join("ok.md"), "ok");
        put(&staging.join("A.md"), "old");

        let rep = root_to_staging(
            &root,
            &staging,
            &tracked(&["A.md", "a.md", "ok.md"]),
            "",
            Limits::default(),
            &[PathBuf::from("A.md"), PathBuf::from("a.md")],
        )
        .unwrap();
        assert_eq!(fs::read(staging.join("A.md")).unwrap(), b"new");
        assert_eq!(fs::read(staging.join("ok.md")).unwrap(), b"ok");
        assert!(rep.skipped_nonportable.iter().any(|(path, reason)| {
            path == Path::new("a.md") && reason == "differs only by case from A.md"
        }));
        assert!(!rep
            .skipped_nonportable
            .iter()
            .any(|(path, _)| path == Path::new("A.md")));
    }

    /// One dirent plus one index path, on either kind of volume. The new bytes
    /// land on the spelling already tracked; the renamed spelling is not a
    /// second path and not a skip.
    #[test]
    fn one_live_dirent_is_copied_onto_the_tracked_spelling() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        put(&root.join("docs/notes.md"), "new");
        put(&staging.join("docs/Notes.md"), "old");

        let rep = root_to_staging(
            &root,
            &staging,
            &tracked(&["docs/"]),
            "",
            Limits::default(),
            &[PathBuf::from("docs/Notes.md")],
        )
        .unwrap();
        assert_eq!(fs::read(staging.join("docs/Notes.md")).unwrap(), b"new");
        assert!(
            rep.skipped_nonportable.is_empty(),
            "{:?}",
            rep.skipped_nonportable
        );
    }

    /// No dirent left in the fold: the winning spelling may leave staging.
    /// `a:b.md` and `CON.md` are not case losers; an existing copy stays.
    #[test]
    fn deleting_the_live_file_drops_the_case_winner_and_keeps_nonportable() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        put(&root.join("ok.md"), "ok");
        put(&staging.join("A.md"), "upper");
        put(&staging.join("a:b.md"), "colon");
        put(&staging.join("CON.md"), "con");
        put(&staging.join("ok.md"), "old-ok");

        let rep = root_to_staging(
            &root,
            &staging,
            &tracked(&["ok.md", "A.md", "a.md", "a:b.md", "CON.md"]),
            "",
            Limits::default(),
            &[
                PathBuf::from("ok.md"),
                PathBuf::from("A.md"),
                PathBuf::from("a.md"),
                PathBuf::from("a:b.md"),
                PathBuf::from("CON.md"),
            ],
        )
        .unwrap();
        assert!(
            !staging.join("A.md").exists(),
            "winner stayed after the live file was deleted"
        );
        assert_eq!(fs::read(staging.join("a:b.md")).unwrap(), b"colon");
        assert_eq!(fs::read(staging.join("CON.md")).unwrap(), b"con");
        assert_eq!(fs::read(staging.join("ok.md")).unwrap(), b"ok");
        assert!(rep
            .skipped_nonportable
            .iter()
            .any(|(path, _)| path == Path::new("a.md")));
        assert!(rep
            .skipped_nonportable
            .iter()
            .any(|(path, _)| path == Path::new("a:b.md")));
        assert!(rep
            .skipped_nonportable
            .iter()
            .any(|(path, _)| path == Path::new("CON.md")));
        assert!(!rep
            .skipped_nonportable
            .iter()
            .any(|(path, _)| path == Path::new("A.md")));
    }

    /// Two real dirents are a case-sensitive collision, not one file to retarget.
    #[test]
    fn two_dirents_in_one_fold_are_not_retargeted() {
        let mut want = HashSet::from([PathBuf::from("A.md"), PathBuf::from("a.md")]);
        let mut copy_src = HashMap::new();
        retarget_case_folds(
            &mut want,
            &[PathBuf::from("A.md"), PathBuf::from("a.md")],
            &tracked(&["A.md", "a.md"]),
            &mut copy_src,
        );
        assert!(want.contains(Path::new("A.md")));
        assert!(want.contains(Path::new("a.md")));
        assert!(copy_src.is_empty());
    }

    /// The dirent matches the losing index path exactly. It still moves onto
    /// the byte-wise-first spelling; the other index path is not a dirent.
    #[test]
    fn a_dirent_matching_the_loser_retargets_onto_the_winner() {
        let mut want = HashSet::from([PathBuf::from("a.md")]);
        let mut copy_src = HashMap::new();
        retarget_case_folds(
            &mut want,
            &[PathBuf::from("A.md"), PathBuf::from("a.md")],
            &tracked(&["A.md", "a.md"]),
            &mut copy_src,
        );
        assert!(want.contains(Path::new("A.md")));
        assert!(!want.contains(Path::new("a.md")));
        assert_eq!(
            copy_src.get(Path::new("A.md")),
            Some(&PathBuf::from("a.md"))
        );
    }
}
