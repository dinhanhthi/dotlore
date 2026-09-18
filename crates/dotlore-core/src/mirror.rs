//! Copying a tracked root into and out of its staging worktree.
//!
//! This is the only place that moves bytes between a live tracked root and its
//! staging git worktree. Two rules shape everything here: nothing git-private
//! (`.git`, `.dotloreignore`, `*.conflict-*`) ever reaches the root, and a root
//! file is only overwritten when its current bytes and mode are exactly what
//! the caller expected them to be.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{ErrorKind, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use ignore::gitignore::{Gitignore, GitignoreBuilder};

use crate::cloud::Kind;

/// The logical staging entry a single-file root always maps to, whatever the
/// local file is called.
const CONTENT: &str = "content";

/// Ignore rules for `~/.claude`. The `/*` + `!/x` idiom is deliberate: with a
/// bare `*` the children of a re-included directory match again.
pub const DEFAULT_HOME_CLAUDE_IGNORE: &str = "\
/*
!/settings*.json
!/CLAUDE.md
!/agents/
!/skills/
!/commands/
!/rules/
!/hooks/
!/scripts/
!/plugins/
/plugins/*
!/plugins/installed_plugins.json
!/plugins/known_marketplaces.json
!/plugins/blocklist.json
*.jsonl
";

/// Project roots (`<project>/.claude`, `CLAUDE.md`) are mirrored whole.
pub const DEFAULT_PROJECT_IGNORE: &str = "";

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
}

/// Copy a tracked root into its staging worktree, filtered by `ignore_text`
/// (gitignore syntax; the caller reads `<staging>/.dotloreignore` or passes a
/// default). Byte- and mode-identical files are left untouched, so a quiet
/// root reports `changed == false`.
pub fn root_to_staging(
    root: &Path,
    staging: &Path,
    kind: Kind,
    ignore_text: &str,
) -> Result<MirrorReport> {
    let md = fs::symlink_metadata(root)
        .with_context(|| format!("tracked root {} is unreadable", root.display()))?;
    if md.file_type().is_symlink() {
        bail!("tracked root {} is a symlink", root.display());
    }

    // rel path in staging -> source file in root
    let mut want: HashMap<PathBuf, PathBuf> = HashMap::new();
    // Paths the walk could not look inside (symlinks, undecodable names). We
    // know nothing about what the root holds there, so staging keeps whatever
    // it has at or under them: deleting it would publish a deletion of data
    // another device legitimately owns.
    let mut opaque: HashSet<PathBuf> = HashSet::new();
    let mut report = MirrorReport::default();

    match kind {
        Kind::File => {
            if !md.is_file() {
                bail!("tracked root {} is not a file", root.display());
            }
            want.insert(PathBuf::from(CONTENT), root.to_path_buf());
        }
        Kind::Dir => {
            if !md.is_dir() {
                bail!("tracked root {} is not a directory", root.display());
            }
            let ignore = build_ignore(root, ignore_text)?;
            walk(
                root,
                Path::new(""),
                &ignore,
                &mut want,
                &mut opaque,
                &mut report,
            )?;
        }
    }

    fs::create_dir_all(staging)?;
    let keep: HashSet<&Path> = want.keys().map(PathBuf::as_path).collect();
    report.changed = delete_stale(staging, Path::new(""), &keep, &opaque)?;
    for (rel, src) in &want {
        // `delete_stale` ran above with `keep` built from `want`, which holds
        // `rel`, so a skipped entry keeps whatever staging already has.
        match copy_if_changed(src, &staging.join(rel))? {
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
pub fn apply_to_root(
    staging: &Path,
    root: &Path,
    kind: Kind,
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
        let target = match kind {
            Kind::File => {
                if rel != Path::new(CONTENT) {
                    bail!("file root: unexpected staging entry {}", rel.display());
                }
                root.to_path_buf()
            }
            Kind::Dir => root.join(rel),
        };

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
    ignore: &Gitignore,
    want: &mut HashMap<PathBuf, PathBuf>,
    opaque: &mut HashSet<PathBuf>,
    report: &mut MirrorReport,
) -> Result<()> {
    let dir = root.join(rel);
    for entry in fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        let name = match entry.file_name().into_string() {
            Ok(n) => n,
            Err(_) => {
                opaque.insert(rel.join(entry.file_name()));
                continue;
            }
        };
        if protected_name(&name) || name == ".DS_Store" {
            continue;
        }
        let child = rel.join(&name);
        let path = entry.path();
        let md = fs::symlink_metadata(&path)?;
        if md.file_type().is_symlink() {
            opaque.insert(child.clone());
            report.skipped_symlinks.push(child);
            continue;
        }
        // Relative paths only: the matcher panics on an absolute path it
        // cannot strip its own root from (/var vs /private/var on macOS).
        if ignore
            .matched_path_or_any_parents(&child, md.is_dir())
            .is_ignore()
        {
            continue;
        }
        if md.is_dir() {
            walk(root, &child, ignore, want, opaque, report)?;
        } else if md.is_file() {
            want.insert(child, path);
        }
    }
    Ok(())
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
) -> Result<bool> {
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
        if opaque.contains(&child) {
            continue;
        }
        let path = entry.path();
        if fs::symlink_metadata(&path)?.is_dir() {
            changed |= delete_stale(staging, &child, keep, opaque)?;
            if fs::read_dir(&path)?.next().is_none() {
                fs::remove_dir(&path)?;
            }
        } else if !keep.contains(child.as_path()) {
            fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
            changed = true;
        }
    }
    Ok(changed)
}

/// `None` means the source was not a regular file when we got to it and was
/// left alone. `walk` filters symlinks out of `want`, but it stats the whole
/// tree before this loop runs: an entry swapped for a symlink inside that
/// window would otherwise be read through and its target's bytes published.
fn copy_if_changed(src: &Path, dst: &Path) -> Result<Option<bool>> {
    let md = fs::symlink_metadata(src)?;
    if !md.file_type().is_file() {
        return Ok(None);
    }
    let executable = is_exec(md.permissions().mode());
    let bytes = fs::read(src).with_context(|| format!("reading {}", src.display()))?;

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
    let exec = is_exec(md.permissions().mode());
    // Only the executable bit is ours to set. An existing target keeps its
    // own permissions: settings.json holds API keys and is routinely 0600,
    // and a fixed 0644 would silently make it world-readable.
    let mode = match fs::symlink_metadata(dst) {
        Ok(dmd) if dmd.file_type().is_file() => {
            let cur = dmd.permissions().mode() & 0o777;
            if exec {
                cur | 0o111
            } else {
                cur & !0o111
            }
        }
        _ => mode_for(exec),
    };

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
        f.write_all(&bytes)?;
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
    name == ".git"
        || name == ".dotloreignore"
        || name.contains(".conflict-")
        || name.ends_with(".dotlore-tmp")
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

        let rep = root_to_staging(&root, &staging, Kind::Dir, DEFAULT_HOME_CLAUDE_IGNORE).unwrap();

        assert!(rep.changed);
        assert_eq!(
            files(&staging),
            vec![
                "agents/x.md".to_string(),
                "plugins/installed_plugins.json".to_string(),
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

        root_to_staging(&root, &staging, Kind::Dir, DEFAULT_PROJECT_IGNORE).unwrap();

        assert_eq!(files(&staging), vec!["sub/file.txt".to_string()]);
    }

    #[test]
    fn second_unchanged_run_reports_no_change() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        put(&root.join("a.md"), "one");
        put(&root.join("deep/b.md"), "two");

        assert!(
            root_to_staging(&root, &staging, Kind::Dir, DEFAULT_PROJECT_IGNORE)
                .unwrap()
                .changed
        );
        assert!(
            !root_to_staging(&root, &staging, Kind::Dir, DEFAULT_PROJECT_IGNORE)
                .unwrap()
                .changed
        );

        fs::remove_file(root.join("deep/b.md")).unwrap();
        assert!(
            root_to_staging(&root, &staging, Kind::Dir, DEFAULT_PROJECT_IGNORE)
                .unwrap()
                .changed
        );
        assert_eq!(files(&staging), vec!["a.md".to_string()]);
    }

    #[test]
    fn file_root_uses_the_content_entry_whatever_the_basename() {
        let td = TempDir::new().unwrap();
        let staging = td.path().join("staging");
        let src = td.path().join("CLAUDE.md");
        put(&src, "guidance");

        root_to_staging(&src, &staging, Kind::File, DEFAULT_PROJECT_IGNORE).unwrap();
        assert_eq!(fs::read(staging.join(CONTENT)).unwrap(), b"guidance");

        // Same logical entry, different local name, not yet present.
        let dst = td.path().join("other/instructions.md");
        let mut snap = Snapshot::new();
        snap.insert(PathBuf::from(CONTENT), None);
        let skipped = apply_to_root(
            &staging,
            &dst,
            Kind::File,
            &[(Status::Added, PathBuf::from(CONTENT))],
            &snap,
        )
        .unwrap();

        assert!(skipped.is_empty());
        assert_eq!(fs::read(&dst).unwrap(), b"guidance");
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
            Kind::Dir,
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
            Kind::Dir,
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

    #[test]
    fn executable_bit_survives_both_directions() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        put(&root.join("hooks/run.sh"), "#!/bin/sh\n");
        chmod(&root.join("hooks/run.sh"), 0o755);
        put(&root.join("hooks/notes.md"), "plain");

        root_to_staging(&root, &staging, Kind::Dir, DEFAULT_PROJECT_IGNORE).unwrap();
        assert_eq!(mode_of(&staging.join("hooks/run.sh")), 0o755);
        assert_eq!(mode_of(&staging.join("hooks/notes.md")), 0o644);

        let other = td.path().join("other");
        let mut snap = Snapshot::new();
        snap.insert(PathBuf::from("hooks/run.sh"), None);
        let skipped = apply_to_root(
            &staging,
            &other,
            Kind::Dir,
            &[(Status::Added, PathBuf::from("hooks/run.sh"))],
            &snap,
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

        let rep = root_to_staging(&root, &staging, Kind::Dir, DEFAULT_PROJECT_IGNORE).unwrap();
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
            Kind::Dir,
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
        put(&staging.join("a.conflict-dev2.md"), "loser");
        put(&root.join("keep.md"), "keep");

        let skipped = apply_to_root(
            &staging,
            &root,
            Kind::Dir,
            &[
                (Status::Added, PathBuf::from(".dotloreignore")),
                (Status::Added, PathBuf::from("a.conflict-dev2.md")),
            ],
            &Snapshot::new(),
        )
        .unwrap();

        assert!(skipped.is_empty());
        assert_eq!(files(&root), vec!["keep.md".to_string()]);

        // ...and the mirror leaves them in staging instead of deleting them.
        let rep = root_to_staging(&root, &staging, Kind::Dir, DEFAULT_PROJECT_IGNORE).unwrap();
        assert!(rep.changed);
        assert_eq!(
            files(&staging),
            vec![
                ".dotloreignore".to_string(),
                "a.conflict-dev2.md".to_string(),
                "keep.md".to_string(),
            ]
        );
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

        let rep = root_to_staging(&root, &staging, Kind::Dir, DEFAULT_PROJECT_IGNORE).unwrap();

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
            Kind::Dir,
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
            Kind::Dir,
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
            Kind::Dir,
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
            Kind::Dir,
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
            Kind::Dir,
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
            Kind::Dir,
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
            Kind::Dir,
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
    #[test]
    fn a_root_entry_swapped_for_a_symlink_is_not_copied_into_staging() {
        let td = TempDir::new().unwrap();
        let (root, staging) = dirs(&td);
        let secret = td.path().join("id_rsa");
        put(&secret, "PRIVATE KEY");
        symlink(&secret, root.join("a.md")).unwrap();
        put(&staging.join("a.md"), "device B data");

        assert_eq!(
            copy_if_changed(&root.join("a.md"), &staging.join("a.md")).unwrap(),
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
            Kind::Dir,
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
            Kind::Dir,
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
            Kind::Dir,
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
            Kind::Dir,
            &[(Status::Added, PathBuf::from("../escape.md"))],
            &Snapshot::new(),
        )
        .is_err());
    }
}
