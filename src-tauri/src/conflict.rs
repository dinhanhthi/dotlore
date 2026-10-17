//! Deterministic conflict resolution and sibling files.
//!
//! Two devices merge each other's history without talking to each other, so
//! the resolution has to be a *function of the two commits* — anything that
//! depends on which side is running it makes the devices disagree, and two
//! trees that disagree re-merge each other forever.
//!
//! The policy, computed identically on both sides:
//!
//! - one winner per merge (not per file): the newer commit, tie broken by the
//!   larger commit hash;
//! - except in a root-edit reconcile (`Transaction::reconcile`), a merge only
//!   one device makes: there the already-merged target always wins
//!   ([`Winner::Theirs`]) and the racing root edit becomes the sibling;
//! - the winner's blob becomes the live file, so no git conflict marker ever
//!   reaches a tracked path;
//! - the loser's bytes are committed next to it as
//!   `<stem>.conflict-<loser id8>-<blob7>.<ext>`, which carries both the
//!   losing device and the losing blob so a repeated or three-way conflict
//!   adds a sibling instead of overwriting one.
//!
//! Everything here runs against the transaction worktree ([`Transaction`]);
//! `main` and its staging worktree are finalized later, by the transaction.

use std::collections::{BTreeSet, HashMap};
use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};

use crate::git::Git;
use crate::project::{self, EntryList};
use crate::repo::{self, Transaction, Winner};

/// Separator between a live name and the generated conflict suffix.
const DELIM: &str = ".conflict-";
/// Characters of the losing device id in a sibling name.
const ID8: usize = 8;
/// Characters of the losing blob's git id in a sibling name.
const BLOB7: usize = 7;

/// A sibling holding the bytes that lost one merge.
#[derive(Debug, Clone, PartialEq)]
pub struct Conflict {
    pub live: PathBuf,
    pub sibling: PathBuf,
    pub loser_id8: String,
}

/// Where the loser's bytes go for `live`.
///
/// `CLAUDE.md` → `CLAUDE.conflict-<id8>-<blob7>.md`, `Makefile` →
/// `Makefile.conflict-<id8>-<blob7>`. The directory is preserved, so two roots
/// with the same file name never collide.
pub fn sibling_name(live: &Path, loser_id8: &str, blob7: &str) -> PathBuf {
    let name = live
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (stem, ext) = split_name(&name);
    let mut out = format!("{stem}{DELIM}{loser_id8}-{blob7}");
    if let Some(ext) = ext {
        out.push('.');
        out.push_str(ext);
    }
    match live.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.join(out),
        _ => PathBuf::from(out),
    }
}

/// Does this device's side of the merge win?
///
/// Both devices run this over the same two commits and must reach opposite
/// answers. Committer time decides; on a tie the larger commit hash wins,
/// which is arbitrary but identical everywhere.
pub fn winner_is_ours(git: &Git, their_ref: &str) -> Result<bool> {
    let ours = commit_time(git, "HEAD")?;
    let theirs = commit_time(git, their_ref)?;
    if ours != theirs {
        return Ok(ours > theirs);
    }
    let ours = head_id(git, "HEAD")?;
    let theirs = head_id(git, their_ref)?;
    Ok(ours > theirs)
}

/// Resolve every unmerged index entry left by a failed `git merge`.
///
/// Returns the live paths that gained a sibling. The winner is computed once:
/// a merge that split its files between the two sides would leave the devices
/// with different trees.
pub fn resolve_index(
    git: &Git,
    their_ref: &str,
    my_id: &str,
    their_id: &str,
    winner: Winner,
) -> Result<Vec<PathBuf>> {
    let ours_win = match winner {
        Winner::Newer => winner_is_ours(git, their_ref)?,
        Winner::Theirs => false,
    };
    let loser_id8 = short8(if ours_win { their_id } else { my_id })?;
    let mut out = Vec::new();
    for (code, path) in unmerged(git)? {
        if let Some(live) = resolve_one(git, &code, &path, ours_win, loser_id8)? {
            out.push(live);
        }
    }
    Ok(out)
}

/// Every committed sibling in this worktree, with the live path it belongs to.
///
/// A tracked file whose name merely contains `.conflict-` but does not carry a
/// well-formed suffix is a user's own file, not a conflict.
pub fn list(git: &Git) -> Result<Vec<Conflict>> {
    let mut out = Vec::new();
    for field in split_nul(&raw(git, &["ls-files", "-z"])?) {
        let Ok(s) = std::str::from_utf8(field) else {
            continue;
        };
        let p = Path::new(s);
        if let Some(c) = parse_sibling(p) {
            out.push(c);
        }
    }
    Ok(out)
}

/// Record a user's resolution of `live` in the transaction worktree.
///
/// Only siblings that parse back to exactly this live path — same directory,
/// same stem, same extension — are dropped, so resolving `settings.json`
/// cannot delete a sibling of `settings.md` or of `sub/settings.json`.
/// `main` is untouched; the transaction finalizes it after guarded apply.
pub fn resolve(tx: &Transaction, live: &Path, siblings: &[PathBuf], content: &[u8]) -> Result<()> {
    let live_spec = pathspec(live)?;
    write_regular(&tx.worktree.join(live), content)?;
    for sibling in siblings {
        match parse_sibling(sibling) {
            Some(c) if c.live == live => {}
            _ => continue,
        }
        tx.git
            .ok(&["rm", "-q", "--cached", "--", &pathspec(sibling)?])?;
        let path = tx.worktree.join(sibling);
        if let Err(e) = fs::remove_file(&path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                return Err(e).with_context(|| format!("removing {}", path.display()));
            }
        }
    }
    tx.git.ok(&["add", "--", &live_spec])?;
    tx.git
        .ok(&["commit", "-m", &format!("resolve {}", live.display())])?;
    Ok(())
}

// --- presence: delete vs re-add -------------------------------------------

/// Seconds within which a delete and a re-add count as simultaneous; a tie
/// keeps the file.
pub const PRESENCE_TIE_SECS: i64 = 5;

/// Apply the newer-wins presence rule to an in-progress merge.
///
/// git keeps a path one side deleted only when the other side changed it,
/// so a restore with the base's bytes loses to an older delete, and an add
/// that another device deletes later survives. For every path one side
/// deleted since the merge-bases and exactly one tip still holds, each
/// side's last user event on it since the bases is weighed: the holder's
/// and the deleter's. The rule acts only when the holder's last event is a
/// present one and the deleter's is a delete; otherwise git's result
/// stands. A present event at least as new as the delete (minus
/// [`PRESENCE_TIE_SECS`]) keeps or restores the path, otherwise it is
/// removed, but only when the merged blob and mode are what the deleter
/// saw. A modification always beats a deletion.
///
/// Holder and deleter are named by the tips' contents and each side's
/// events are the [`repo::user_events`] of `<tip> ^<bases>`, so the
/// outcome does not depend on which side is HEAD and both devices reach
/// the same tree. Without a merge-base nothing is done: every old delete
/// would be in range. Returns the paths it changed in the index and
/// worktree.
pub fn settle_presence(
    g: &Git,
    ours: &str,
    theirs: &str,
    entries: &EntryList,
) -> Result<Vec<PathBuf>> {
    let tips = [head_id(g, ours)?, head_id(g, theirs)?];
    let bases = merge_bases(g, &tips[0], &tips[1])?;
    if bases.is_empty() {
        return Ok(Vec::new());
    }
    let nots: Vec<String> = bases.iter().map(|b| format!("^{b}")).collect();

    let mut args = vec!["--diff-filter=D", "--end-of-options", &tips[0], &tips[1]];
    args.extend(nots.iter().map(String::as_str));
    args.push("--");
    let deleted: BTreeSet<PathBuf> = repo::user_events(g, &args)?
        .into_iter()
        .flat_map(|(_, changes)| changes)
        .filter(|(rel, change)| *change == Some(None) && entries.contains_rel(rel))
        .map(|(rel, _)| rel)
        .collect();
    if deleted.is_empty() {
        return Ok(Vec::new());
    }
    let holders = one_sided(g, &tips)?;
    let candidates: Vec<(PathBuf, usize)> = deleted
        .into_iter()
        .filter_map(|rel| holders.get(&rel).map(|&h| (rel, h)))
        .collect();
    if candidates.is_empty() {
        return Ok(Vec::new());
    }

    let last = [
        last_events(g, &tips[0], &nots)?,
        last_events(g, &tips[1], &nots)?,
    ];
    let mut changed = Vec::new();
    for (rel, holder) in candidates {
        let (Some(h), Some(d)) = (last[holder].get(&rel), last[1 - holder].get(&rel)) else {
            continue;
        };
        if !h.present || d.present {
            continue;
        }
        if settle_path(
            g,
            &rel,
            &tips[holder],
            h.time >= d.time - PRESENCE_TIE_SECS,
            &d.commit,
        )? {
            changed.push(rel);
        }
    }
    Ok(changed)
}

/// A side's last user event on one path.
struct LastEvent {
    time: i64,
    present: bool,
    commit: String,
}

/// The last user event on each path in `tip ^<bases>`. The newest time
/// wins; at an equal time a present event beats a delete (as a tie keeps
/// the file), then the larger commit id, so the choice never depends on
/// log order.
fn last_events(g: &Git, tip: &str, nots: &[String]) -> Result<HashMap<PathBuf, LastEvent>> {
    let mut args = vec!["--end-of-options", tip];
    args.extend(nots.iter().map(String::as_str));
    args.push("--");
    let mut map: HashMap<PathBuf, LastEvent> = HashMap::new();
    for (event, changes) in repo::user_events(g, &args)? {
        for (rel, change) in changes {
            let Some(blob) = change else {
                continue;
            };
            let e = LastEvent {
                time: event.time,
                present: blob.is_some(),
                commit: event.commit.clone(),
            };
            let newer = map.get(&rel).is_none_or(|old| {
                (e.time, e.present, &e.commit) > (old.time, old.present, &old.commit)
            });
            if newer {
                map.insert(rel, e);
            }
        }
    }
    Ok(map)
}

/// Make the index agree with the decision for one candidate. `holder` is
/// the tip that still has `rel`; `deleter` is the commit of the deleting
/// side's last event on it.
fn settle_path(
    g: &Git,
    rel: &Path,
    holder: &str,
    want_present: bool,
    deleter: &str,
) -> Result<bool> {
    let spec = pathspec(rel)?;
    let staged = entry_at(g, &["ls-files", "-s", "-z", "--", &spec], rel)?;
    if want_present {
        if staged.is_some() {
            return Ok(false);
        }
        g.ok(&["checkout", "--end-of-options", holder, "--", &spec])?;
        return Ok(true);
    }
    // Only an informed delete removes: the deleter saw these exact bytes
    // and mode. Anything else is a modification, which wins.
    let Some(staged) = staged else {
        return Ok(false);
    };
    let parent = format!("{deleter}^");
    let seen = entry_at(
        g,
        &[
            "ls-tree",
            "-r",
            "-z",
            "--end-of-options",
            &parent,
            "--",
            &spec,
        ],
        rel,
    )?;
    if seen != Some(staged) {
        return Ok(false);
    }
    g.ok(&["rm", "-q", "-f", "--", &spec])?;
    Ok(true)
}

/// `(mode, object)` of `rel` in an `ls-files -s -z` or `ls-tree -z`
/// listing. An unmerged index entry is an error: this runs after
/// [`resolve_index`].
fn entry_at(g: &Git, args: &[&str], rel: &Path) -> Result<Option<(String, String)>> {
    let mut found = None;
    for record in split_nul(&raw(g, args)?) {
        // ls-files: "<mode> <object> <stage>\t<path>";
        // ls-tree: "<mode> <type> <object>\t<path>".
        let Some(tab) = record.iter().position(|b| *b == b'\t') else {
            continue;
        };
        if Path::new(OsStr::from_bytes(&record[tab + 1..])) != rel {
            continue;
        }
        let meta = String::from_utf8_lossy(&record[..tab]);
        let f: Vec<&str> = meta.split(' ').collect();
        let (mode, object) = match (args[0], f.as_slice()) {
            ("ls-files", [mode, object, "0"]) => (mode, object),
            ("ls-files", _) => bail!("{} is unmerged", rel.display()),
            (_, [mode, _, object]) => (mode, object),
            _ => continue,
        };
        found = Some((mode.to_string(), object.to_string()));
    }
    Ok(found)
}

/// Regular files present in exactly one tip, mapped to the index of that
/// tip in `tips`.
fn one_sided(g: &Git, tips: &[String; 2]) -> Result<HashMap<PathBuf, usize>> {
    let out = raw(
        g,
        &[
            "diff-tree",
            "-r",
            "-z",
            "--no-renames",
            "--no-abbrev",
            "--diff-filter=AD",
            "--end-of-options",
            &tips[0],
            &tips[1],
            "--",
        ],
    )?;
    let mut map = HashMap::new();
    let mut fields = out.split(|b| *b == 0);
    while let Some(meta) = fields.next() {
        let Some(path) = fields.next() else {
            break;
        };
        // ":<old mode> <new mode> <old oid> <new oid> <status>"
        let meta = String::from_utf8_lossy(meta.strip_prefix(b":").unwrap_or(meta));
        let f: Vec<&str> = meta.split(' ').collect();
        let holder = match f.as_slice() {
            [old, _, _, _, "D"] if old.starts_with("100") => 0,
            [_, new, _, _, "A"] if new.starts_with("100") => 1,
            _ => continue,
        };
        map.insert(PathBuf::from(OsStr::from_bytes(path)), holder);
    }
    Ok(map)
}

/// Every merge-base of two commits; empty for unrelated histories.
pub(crate) fn merge_bases(g: &Git, a: &str, b: &str) -> Result<Vec<String>> {
    let out = g.run(&["merge-base", "--all", "--end-of-options", a, b])?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    match out.status.code() {
        Some(0) => Ok(stdout.lines().map(str::to_string).collect()),
        Some(1) if stdout.trim().is_empty() => Ok(Vec::new()),
        _ => bail!(
            "git merge-base failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ),
    }
}

// --- one unmerged entry ----------------------------------------------------

/// The whole policy table. Every unmerged state git can produce is named here;
/// anything else is an error, because silently skipping an entry leaves the
/// index unmerged and the cycle stalls with no way to see why.
fn resolve_one(
    git: &Git,
    code: &str,
    path: &Path,
    ours_win: bool,
    loser_id8: &str,
) -> Result<Option<PathBuf>> {
    let spec = pathspec(path)?;
    match code {
        // Include-list: union the two blobs. Never consults ours_win, never
        // writes a sibling. Guarded on the status so AU/UA/UD/DU/DD still
        // reach the existing one-sided arms — stage_blob bails when only one
        // side has a blob.
        "UU" | "AA" if path == Path::new(project::PROJECT_FILE) => {
            let ours = raw(git, &["cat-file", "blob", &stage_blob(git, path, 2)?])?;
            let theirs = raw(git, &["cat-file", "blob", &stage_blob(git, path, 3)?])?;
            let merged = project::merge(&project::parse(&ours)?, &project::parse(&theirs)?);
            write_regular(&git.repo.join(path), &merged.to_bytes()?)?;
            git.ok(&["add", "--", &spec])?;
            Ok(None)
        }
        // Both sides have a blob: winner lives, loser is preserved beside it.
        "UU" | "AA" => {
            let (win, lose) = if ours_win {
                ("--ours", 3)
            } else {
                ("--theirs", 2)
            };
            git.ok(&["checkout", win, "--", &spec])?;
            let blob = stage_blob(git, path, lose)?;
            let bytes = raw(git, &["cat-file", "blob", &blob])?;
            let sibling = sibling_name(path, loser_id8, &blob[..BLOB7]);
            write_regular(&git.repo.join(&sibling), &bytes)?;
            git.ok(&["add", "--", &spec, &pathspec(&sibling)?])?;
            Ok(Some(path.to_path_buf()))
        }
        // Only our side has a blob; the worktree already holds it.
        "AU" | "UD" => {
            git.ok(&["add", "--", &spec])?;
            Ok(None)
        }
        // Only their side has a blob.
        "UA" | "DU" => {
            git.ok(&["checkout", "--theirs", "--", &spec])?;
            git.ok(&["add", "--", &spec])?;
            Ok(None)
        }
        // Neither side kept it.
        "DD" => {
            git.ok(&["rm", "-q", "--cached", "--", &spec])?;
            Ok(None)
        }
        _ => bail!("unhandled merge state {code} for {}", path.display()),
    }
}

// --- git plumbing ----------------------------------------------------------

fn commit_time(git: &Git, r: &str) -> Result<i64> {
    let s = git.ok(&["log", "-1", "--format=%ct", "--end-of-options", r])?;
    s.parse()
        .with_context(|| format!("commit time of {r} is not a number: {s:?}"))
}

fn head_id(git: &Git, r: &str) -> Result<String> {
    git.rev(r).ok_or_else(|| anyhow!("cannot resolve {r}"))
}

/// The unmerged entries, in `git status` order.
fn unmerged(git: &Git) -> Result<Vec<(String, PathBuf)>> {
    parse_status(&raw(
        git,
        &["status", "--porcelain=v1", "-z", "--untracked-files=no"],
    )?)
}

/// `-z` records are `XY <path>` with no quoting; a rename or copy appends the
/// original path as a bare extra record, which must not be read as a status.
fn parse_status(stdout: &[u8]) -> Result<Vec<(String, PathBuf)>> {
    let mut out = Vec::new();
    let mut fields = split_nul(stdout).into_iter();
    while let Some(f) = fields.next() {
        if f.len() < 4 || f[2] != b' ' {
            bail!(
                "unparseable git status record {:?}",
                String::from_utf8_lossy(f)
            );
        }
        let code = std::str::from_utf8(&f[..2])
            .with_context(|| "non-utf8 git status code".to_string())?
            .to_string();
        if code.starts_with('R') || code.starts_with('C') {
            fields.next();
        }
        if !is_unmerged(&code) {
            continue;
        }
        // `Git` takes `&str` argv, so a path we cannot decode could never be
        // handed back to git to resolve it.
        let path = std::str::from_utf8(&f[3..])
            .map_err(|_| anyhow!("non-utf8 unmerged path in git status"))?;
        out.push((code, PathBuf::from(path)));
    }
    Ok(out)
}

/// git's own rule: a `U` on either side, plus both-added and both-deleted.
/// Deliberately not a list of the seven known codes — a state this module has
/// no arm for must reach [`resolve_one`]'s error, not be filtered out here.
fn is_unmerged(code: &str) -> bool {
    let b = code.as_bytes();
    b[0] == b'U' || b[1] == b'U' || code == "AA" || code == "DD"
}

/// The object id of one merge stage of `path`.
///
/// This is exactly what `git hash-object --stdin` over that stage's bytes
/// returns — it is the same blob — and it costs one command instead of two
/// plus a pipe (`Git` forwards no stdin).
fn stage_blob(git: &Git, path: &Path, stage: u8) -> Result<String> {
    let listing = git.ok(&["ls-files", "-u", "--", &pathspec(path)?])?;
    for line in listing.lines() {
        // "<mode> <object> <stage>\t<path>"
        let (meta, _) = line.split_once('\t').unwrap_or((line, ""));
        let mut it = meta.split_whitespace();
        let (Some(_mode), Some(object), Some(s)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        if s == stage.to_string() && object.len() >= BLOB7 {
            return Ok(object.to_string());
        }
    }
    bail!(
        "no stage {stage} for {} in the merged index",
        path.display()
    )
}

/// Run and require exit 0, keeping stdout as raw bytes.
///
/// [`Git::ok`] trims and lossily decodes, which silently corrupts a binary
/// blob and drops trailing newlines — both fatal for bytes we are about to
/// commit as someone's file.
fn raw(git: &Git, args: &[&str]) -> Result<Vec<u8>> {
    let out = git.run(args)?;
    if !out.status.success() {
        bail!(
            "git {args:?} failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(out.stdout)
}

fn split_nul(bytes: &[u8]) -> Vec<&[u8]> {
    bytes.split(|b| *b == 0).filter(|f| !f.is_empty()).collect()
}

// --- names -----------------------------------------------------------------

/// `Path::file_stem` / `Path::extension`, on a name we already have as `str`.
fn split_name(name: &str) -> (&str, Option<&str>) {
    match name.rfind('.') {
        Some(i) if i > 0 && i + 1 < name.len() => (&name[..i], Some(&name[i + 1..])),
        _ => (name, None),
    }
}

/// The [`Conflict`] a sibling name encodes, or `None` if it encodes none.
///
/// Occurrences of the delimiter are tried right to left: the sibling of a
/// sibling (`x.conflict-a-b.conflict-c-d.md`) belongs to `x.conflict-a-b.md`,
/// and only the rightmost reading reconstructs that.
fn parse_sibling(sibling: &Path) -> Option<Conflict> {
    let name = sibling.file_name()?.to_str()?;
    let mut search = name.len();
    while let Some(i) = name[..search].rfind(DELIM) {
        search = i;
        let stem = &name[..i];
        let tail = &name[i + DELIM.len()..];
        let (ids, ext) = match tail.find('.') {
            Some(d) => (&tail[..d], Some(&tail[d + 1..])),
            None => (tail, None),
        };
        // A generated extension comes from `Path::extension`: never empty,
        // never containing a dot of its own.
        if ext.is_some_and(|e| e.is_empty() || e.contains('.')) {
            continue;
        }
        if stem.is_empty() || !valid_ids(ids) {
            continue;
        }
        let mut live = String::from(stem);
        if let Some(ext) = ext {
            live.push('.');
            live.push_str(ext);
        }
        let live = match sibling.parent() {
            Some(p) if !p.as_os_str().is_empty() => p.join(live),
            _ => PathBuf::from(live),
        };
        return Some(Conflict {
            live,
            sibling: sibling.to_path_buf(),
            loser_id8: ids[..ID8].to_string(),
        });
    }
    None
}

/// `<8 device chars>-<7 blob chars>`, the complete generated suffix.
fn valid_ids(ids: &str) -> bool {
    let b = ids.as_bytes();
    b.len() == ID8 + 1 + BLOB7
        && b[ID8] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == ID8 || c.is_ascii_alphanumeric())
}

fn short8(device_id: &str) -> Result<&str> {
    device_id
        .get(..ID8)
        .ok_or_else(|| anyhow!("device id {device_id:?} is shorter than {ID8} characters"))
}

/// A repo-relative path as a literal pathspec.
///
/// `:(literal)` stops a real file called `a[b].md` from being read as a glob.
/// The traversal check matters for [`resolve`], whose sibling list is chosen
/// in the UI and reaches `fs::remove_file`.
fn pathspec(p: &Path) -> Result<String> {
    let s = p
        .to_str()
        .ok_or_else(|| anyhow!("non-utf8 path {}", p.display()))?;
    if s.is_empty() || p.components().any(|c| !matches!(c, Component::Normal(_))) {
        bail!("unsafe path {s:?}");
    }
    Ok(format!(":(literal){s}"))
}

/// Write bytes, refusing anything that is not already a plain file.
///
/// Live files are sacred and symlinks are never followed: a mode-120000 blob
/// checked out from another device must not turn a write into a write
/// somewhere else.
fn write_regular(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Ok(md) = fs::symlink_metadata(path) {
        if !md.file_type().is_file() {
            bail!("refusing to write {}: not a regular file", path.display());
        }
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::Repo;
    use std::process::Command;
    use tempfile::TempDir;

    const ID_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const ID_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const PK: &str = "pk";

    const BASE: &[u8] = b"one\ntwo\nthree\n";
    const A_TEXT: &[u8] = b"A-line\ntwo\nthree\n";
    const B_TEXT: &[u8] = b"B-line\ntwo\nthree\n";

    // --- fixtures ---------------------------------------------------------

    /// Two clones of one base commit, the shape every device pair has.
    struct Pair {
        _dir: TempDir,
        a: PathBuf,
        b: PathBuf,
    }

    impl Pair {
        fn ga(&self) -> Git {
            Git::new(self.a.clone(), "Mac A Pro", ID_A)
        }
        fn gb(&self) -> Git {
            Git::new(self.b.clone(), "Mac B Pro", ID_B)
        }
        /// Fetch one side's `main` into the other's provider-scoped ref.
        fn fetch(&self, into: &Git, from: &Path, their_id: &str) -> String {
            let r = format!("refs/remotes/{PK}/{their_id}/main");
            into.ok(&["fetch", from.to_str().unwrap(), &format!("+main:{r}")])
                .unwrap();
            r
        }
        /// The blob id git would give these bytes, hashed outside any repo.
        fn blob7(&self, bytes: &[u8]) -> String {
            let f = self._dir.path().join("hash-src");
            fs::write(&f, bytes).unwrap();
            let id = self
                .ga()
                .ok(&["hash-object", "--", f.to_str().unwrap()])
                .unwrap();
            id[..BLOB7].to_string()
        }
    }

    fn configure(git: &Git) {
        for (k, v) in [
            ("core.autocrlf", "false"),
            ("merge.conflictstyle", "merge"),
            ("gc.auto", "0"),
            ("commit.gpgsign", "false"),
        ] {
            git.ok(&["config", k, v]).unwrap();
        }
    }

    fn pair(seed: &[(&str, &[u8])]) -> Pair {
        let dir = TempDir::new().unwrap();
        let a = dir.path().join("A");
        let b = dir.path().join("B");
        fs::create_dir_all(&a).unwrap();
        let ga = Git::new(a.clone(), "Mac A Pro", ID_A);
        ga.ok(&["init", "-b", "main"]).unwrap();
        configure(&ga);
        for (rel, body) in seed {
            write(&a, rel, body);
        }
        commit_at(&a, "base", 1_000_000);
        ga.ok(&["clone", "-q", a.to_str().unwrap(), b.to_str().unwrap()])
            .unwrap();
        configure(&Git::new(b.clone(), "Mac B Pro", ID_B));
        Pair { _dir: dir, a, b }
    }

    /// Both devices rewrite line 1 of `CLAUDE.md` at the given commit times.
    fn diverged(a_epoch: i64, b_epoch: i64) -> Pair {
        let p = pair(&[("CLAUDE.md", BASE)]);
        write(&p.a, "CLAUDE.md", A_TEXT);
        commit_at(&p.a, "a1", a_epoch);
        write(&p.b, "CLAUDE.md", B_TEXT);
        commit_at(&p.b, "b1", b_epoch);
        p
    }

    /// `git merge`, asserting it really left an unmerged index.
    fn merge_conflicted(git: &Git, r: &str) {
        let out = git.run(&["merge", "--no-edit", r]).unwrap();
        assert!(
            !out.status.success(),
            "expected a conflicted merge: {}",
            String::from_utf8_lossy(&out.stdout)
        );
    }

    /// A fixture commit at a pinned time.
    ///
    /// `Git`'s child environment is an allowlist and `GIT_COMMITTER_DATE` is
    /// not in it — deliberately, since production commits are dated now. The
    /// allowlist is replicated here rather than widened there, so these
    /// commits are just as hermetic as the engine's.
    fn commit_at(repo: &Path, msg: &str, epoch: i64) {
        fixture_git(repo, epoch, &["add", "-A"]);
        fixture_git(repo, epoch, &["commit", "-q", "-m", msg]);
    }

    fn fixture_git(repo: &Path, epoch: i64, args: &[&str]) {
        let date = format!("@{epoch} +0000");
        let mut c = Command::new("git");
        c.arg("-C")
            .arg(repo)
            .args(args)
            .env_clear()
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "fixture")
            .env("GIT_COMMITTER_NAME", "fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@dotlore")
            .env("GIT_COMMITTER_EMAIL", "fixture@dotlore")
            .env("GIT_EDITOR", "true")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_AUTHOR_DATE", &date)
            .env("GIT_COMMITTER_DATE", &date);
        if let Some(p) = std::env::var_os("PATH") {
            c.env("PATH", p);
        }
        let out = c.output().unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn write(base: &Path, rel: &str, body: &[u8]) {
        let p = base.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }

    fn read(base: &Path, rel: &str) -> Vec<u8> {
        fs::read(base.join(rel)).unwrap()
    }

    /// No file under `dir` (outside `.git`) carries a git conflict marker.
    fn no_markers(dir: &Path) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            if entry.file_name() == ".git" {
                continue;
            }
            let p = entry.path();
            if p.is_dir() {
                no_markers(&p);
            } else {
                let bytes = fs::read(&p).unwrap();
                assert!(
                    !bytes.windows(7).any(|w| w == b"<<<<<<<"),
                    "conflict marker in {}",
                    p.display()
                );
            }
        }
    }

    fn committed(git: &Git) -> Vec<String> {
        git.ok(&["ls-tree", "-r", "--name-only", "HEAD"])
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn pf_bytes(pairs: &[(&str, u64, crate::project::State)]) -> Vec<u8> {
        use crate::project::{EntryRecord, ProjectFile};
        ProjectFile {
            entries: pairs
                .iter()
                .map(|(k, g, s)| ((*k).to_string(), EntryRecord { gen: *g, state: *s }))
                .collect(),
        }
        .to_bytes()
        .unwrap()
    }

    fn no_conflict_siblings(dir: &Path) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            if entry.file_name() == ".git" {
                continue;
            }
            let name = entry.file_name();
            assert!(
                !name.to_string_lossy().contains(".conflict-"),
                "sibling written: {}",
                name.to_string_lossy()
            );
            if entry.path().is_dir() {
                no_conflict_siblings(&entry.path());
            }
        }
    }

    // --- UU: text ---------------------------------------------------------

    #[test]
    fn uu_newer_ours_wins_and_the_loser_becomes_a_sibling() {
        let p = diverged(2_000_000, 1_000_000);
        let ga = p.ga();
        let r = p.fetch(&ga, &p.b, ID_B);
        assert!(winner_is_ours(&ga, &r).unwrap());
        merge_conflicted(&ga, &r);

        let got = resolve_index(&ga, &r, ID_A, ID_B, Winner::Newer).unwrap();
        assert_eq!(got, vec![PathBuf::from("CLAUDE.md")]);

        let sibling = format!("CLAUDE.conflict-bbbbbbbb-{}.md", p.blob7(B_TEXT));
        assert_eq!(read(&p.a, "CLAUDE.md"), A_TEXT);
        assert_eq!(read(&p.a, &sibling), B_TEXT);
        no_markers(&p.a);

        ga.ok(&["commit", "--no-edit", "-m", "merge bbbbbbbb"])
            .unwrap();
        assert_eq!(ga.ok(&["status", "--porcelain"]).unwrap(), "");
        assert_eq!(committed(&ga), vec![sibling, "CLAUDE.md".to_string()]);
    }

    /// `Theirs` ignores commit time: ours is strictly newer and still loses.
    #[test]
    fn uu_theirs_policy_wins_even_when_ours_is_newer() {
        let p = diverged(2_000_000, 1_000_000);
        let ga = p.ga();
        let r = p.fetch(&ga, &p.b, ID_B);
        assert!(winner_is_ours(&ga, &r).unwrap());
        merge_conflicted(&ga, &r);

        resolve_index(&ga, &r, ID_A, ID_B, Winner::Theirs).unwrap();

        let sibling = format!("CLAUDE.conflict-aaaaaaaa-{}.md", p.blob7(A_TEXT));
        assert_eq!(read(&p.a, "CLAUDE.md"), B_TEXT);
        assert_eq!(read(&p.a, &sibling), A_TEXT);
        no_markers(&p.a);
    }

    #[test]
    fn uu_newer_theirs_wins_and_our_bytes_become_the_sibling() {
        let p = diverged(1_000_000, 2_000_000);
        let ga = p.ga();
        let r = p.fetch(&ga, &p.b, ID_B);
        assert!(!winner_is_ours(&ga, &r).unwrap());
        merge_conflicted(&ga, &r);

        resolve_index(&ga, &r, ID_A, ID_B, Winner::Newer).unwrap();

        let sibling = format!("CLAUDE.conflict-aaaaaaaa-{}.md", p.blob7(A_TEXT));
        assert_eq!(read(&p.a, "CLAUDE.md"), B_TEXT);
        assert_eq!(read(&p.a, &sibling), A_TEXT);
        no_markers(&p.a);

        ga.ok(&["commit", "--no-edit", "-m", "merge bbbbbbbb"])
            .unwrap();
        assert_eq!(ga.ok(&["status", "--porcelain"]).unwrap(), "");
    }

    /// The convergence guarantee, and the only test that pins the tie-break
    /// *direction*: both devices flipping together would still converge.
    #[test]
    fn equal_times_let_the_larger_hash_win_on_both_devices() {
        let p = diverged(1_500_000, 1_500_000);
        let (ga, gb) = (p.ga(), p.gb());
        let (ha, hb) = (ga.rev("HEAD").unwrap(), gb.rev("HEAD").unwrap());
        assert_ne!(ha, hb);

        // Both fetches happen before either merge, so each side still sees
        // the other's pre-merge head.
        let rb = p.fetch(&ga, &p.b, ID_B);
        let ra = p.fetch(&gb, &p.a, ID_A);

        assert_eq!(winner_is_ours(&ga, &rb).unwrap(), ha > hb);
        assert_eq!(winner_is_ours(&gb, &ra).unwrap(), hb > ha);

        merge_conflicted(&ga, &rb);
        resolve_index(&ga, &rb, ID_A, ID_B, Winner::Newer).unwrap();
        ga.ok(&["commit", "--no-edit", "-m", "merge b"]).unwrap();

        merge_conflicted(&gb, &ra);
        resolve_index(&gb, &ra, ID_B, ID_A, Winner::Newer).unwrap();
        gb.ok(&["commit", "--no-edit", "-m", "merge a"]).unwrap();

        assert_eq!(
            ga.ok(&["ls-tree", "-r", "HEAD"]).unwrap(),
            gb.ok(&["ls-tree", "-r", "HEAD"]).unwrap(),
            "the two devices did not converge on one tree"
        );
        let winner = if ha > hb { A_TEXT } else { B_TEXT };
        assert_eq!(read(&p.a, "CLAUDE.md"), winner);
        assert_eq!(read(&p.b, "CLAUDE.md"), winner);
        no_markers(&p.a);
        no_markers(&p.b);
    }

    #[test]
    fn a_second_conflict_adds_a_second_sibling_and_keeps_the_first() {
        let p = diverged(2_000_000, 1_000_000);
        let ga = p.ga();
        let r = p.fetch(&ga, &p.b, ID_B);
        merge_conflicted(&ga, &r);
        resolve_index(&ga, &r, ID_A, ID_B, Winner::Newer).unwrap();
        ga.ok(&["commit", "--no-edit", "-m", "merge 1"]).unwrap();

        const A2: &[u8] = b"A-again\ntwo\nthree\n";
        const B2: &[u8] = b"B-again\ntwo\nthree\n";
        write(&p.a, "CLAUDE.md", A2);
        commit_at(&p.a, "a2", 4_000_000);
        write(&p.b, "CLAUDE.md", B2);
        commit_at(&p.b, "b2", 3_000_000);

        let r = p.fetch(&ga, &p.b, ID_B);
        merge_conflicted(&ga, &r);
        resolve_index(&ga, &r, ID_A, ID_B, Winner::Newer).unwrap();
        ga.ok(&["commit", "--no-edit", "-m", "merge 2"]).unwrap();

        let first = format!("CLAUDE.conflict-bbbbbbbb-{}.md", p.blob7(B_TEXT));
        let second = format!("CLAUDE.conflict-bbbbbbbb-{}.md", p.blob7(B2));
        assert_ne!(first, second);
        assert_eq!(read(&p.a, "CLAUDE.md"), A2);
        assert_eq!(read(&p.a, &first), B_TEXT, "the earlier sibling was lost");
        assert_eq!(read(&p.a, &second), B2);

        let mut names: Vec<String> = list(&ga)
            .unwrap()
            .into_iter()
            .map(|c| c.sibling_s())
            .collect();
        names.sort();
        assert_eq!(names, {
            let mut v = vec![first, second];
            v.sort();
            v
        });
    }

    // --- UU: binary -------------------------------------------------------

    /// `AA`: the only arm where `checkout --ours` resolves against an index
    /// with no stage 1. Both devices created the same name with different
    /// bytes, so there is no common version to fall back on — if the winner
    /// were taken from a base that isn't there, this is where it shows.
    #[test]
    fn aa_both_devices_added_the_same_name_with_different_bytes() {
        let p = pair(&[("agents/x.md", b"agent x\n")]);
        write(&p.a, "CLAUDE.md", A_TEXT);
        commit_at(&p.a, "a adds CLAUDE.md", 2_000_000);
        write(&p.b, "CLAUDE.md", B_TEXT);
        commit_at(&p.b, "b adds CLAUDE.md", 1_000_000);

        let ga = p.ga();
        let r = p.fetch(&ga, &p.b, ID_B);
        merge_conflicted(&ga, &r);

        // The shape that makes this fixture worth having: stages 2 and 3 only.
        let stages: Vec<String> = ga
            .ok(&["ls-files", "-u", "--", ":(literal)CLAUDE.md"])
            .unwrap()
            .lines()
            .map(|l| l.split_whitespace().nth(2).unwrap().to_string())
            .collect();
        assert_eq!(stages, vec!["2", "3"], "expected an AA entry, no stage 1");
        assert_eq!(unmerged(&ga).unwrap()[0].0, "AA");

        let got = resolve_index(&ga, &r, ID_A, ID_B, Winner::Newer).unwrap();
        assert_eq!(got, vec![PathBuf::from("CLAUDE.md")]);

        let sibling = format!("CLAUDE.conflict-bbbbbbbb-{}.md", p.blob7(B_TEXT));
        assert_eq!(read(&p.a, "CLAUDE.md"), A_TEXT, "the newer side lives");
        assert_eq!(read(&p.a, &sibling), B_TEXT);
        no_markers(&p.a);

        ga.ok(&["commit", "--no-edit", "-m", "merge bbbbbbbb"])
            .unwrap();
        assert_eq!(ga.ok(&["status", "--porcelain"]).unwrap(), "");
        assert_eq!(
            committed(&ga),
            vec![sibling, "CLAUDE.md".to_string(), "agents/x.md".to_string()]
        );
    }

    #[test]
    fn uu_binary_follows_the_same_rule() {
        let base: &[u8] = &[0x89, b'P', b'N', b'G', 0x00, 0x01];
        let mine: &[u8] = &[0x89, b'P', b'N', b'G', 0x00, 0xAA, 0xBB, 0x00];
        let theirs: &[u8] = &[0x89, b'P', b'N', b'G', 0x00, 0xCC, 0x00, 0xDD];
        assert!(crate::mirror::is_binary(theirs));

        let p = pair(&[("icon.png", base)]);
        write(&p.a, "icon.png", mine);
        commit_at(&p.a, "a1", 2_000_000);
        write(&p.b, "icon.png", theirs);
        commit_at(&p.b, "b1", 1_000_000);

        let ga = p.ga();
        let r = p.fetch(&ga, &p.b, ID_B);
        merge_conflicted(&ga, &r);
        resolve_index(&ga, &r, ID_A, ID_B, Winner::Newer).unwrap();

        let sibling = format!("icon.conflict-bbbbbbbb-{}.png", p.blob7(theirs));
        assert_eq!(read(&p.a, "icon.png"), mine);
        assert_eq!(read(&p.a, &sibling), theirs);
    }

    // --- one-sided states -------------------------------------------------

    /// Rename/rename is the state that produces `AU`, `UA` and `DD` at once.
    #[test]
    fn rename_rename_keeps_both_names_and_drops_the_original() {
        let p = pair(&[("f.md", b"shared\n"), ("k.md", b"k\n")]);
        fs::rename(p.a.join("f.md"), p.a.join("a.md")).unwrap();
        commit_at(&p.a, "a1", 2_000_000);
        fs::rename(p.b.join("f.md"), p.b.join("b.md")).unwrap();
        commit_at(&p.b, "b1", 1_000_000);

        let ga = p.ga();
        let r = p.fetch(&ga, &p.b, ID_B);
        merge_conflicted(&ga, &r);
        // `UA` must take *their* stage, not whatever the merge left on disk.
        write(&p.a, "b.md", b"stale junk\n");

        assert_eq!(
            resolve_index(&ga, &r, ID_A, ID_B, Winner::Newer).unwrap(),
            Vec::<PathBuf>::new()
        );
        ga.ok(&["commit", "--no-edit", "-m", "merge"]).unwrap();

        assert_eq!(ga.ok(&["status", "--porcelain"]).unwrap(), "");
        assert_eq!(committed(&ga), vec!["a.md", "b.md", "k.md"]);
        assert_eq!(read(&p.a, "a.md"), b"shared\n");
        assert_eq!(read(&p.a, "b.md"), b"shared\n");
        assert!(list(&ga).unwrap().is_empty());
    }

    /// `UD` keeps our modification; `DU` restores theirs. Both in one merge.
    #[test]
    fn delete_versus_modify_keeps_the_modification_on_either_side() {
        let p = pair(&[("keep.md", b"base\n"), ("take.md", b"base\n")]);
        write(&p.a, "keep.md", b"mine\n");
        fs::remove_file(p.a.join("take.md")).unwrap();
        commit_at(&p.a, "a1", 2_000_000);
        fs::remove_file(p.b.join("keep.md")).unwrap();
        write(&p.b, "take.md", b"theirs\n");
        commit_at(&p.b, "b1", 1_000_000);

        let ga = p.ga();
        let r = p.fetch(&ga, &p.b, ID_B);
        merge_conflicted(&ga, &r);
        // `DU` must take their stage, not whatever the merge left on disk.
        write(&p.a, "take.md", b"stale junk\n");

        assert_eq!(
            resolve_index(&ga, &r, ID_A, ID_B, Winner::Newer).unwrap(),
            Vec::<PathBuf>::new()
        );
        ga.ok(&["commit", "--no-edit", "-m", "merge"]).unwrap();

        assert_eq!(ga.ok(&["status", "--porcelain"]).unwrap(), "");
        assert_eq!(committed(&ga), vec!["keep.md", "take.md"]);
        assert_eq!(read(&p.a, "keep.md"), b"mine\n");
        assert_eq!(read(&p.a, "take.md"), b"theirs\n");
    }

    // --- unknown states ---------------------------------------------------

    // --- include-list (.dotloreproject) ------------------------------------

    #[test]
    fn two_include_lists_merge_to_their_union_without_a_sibling() {
        use crate::project::{self, State};
        let a_list = pf_bytes(&[("CLAUDE.md", 1, State::Tracked)]);
        let b_list = pf_bytes(&[("docs/", 1, State::Tracked)]);
        let p = pair(&[("notes.md", b"shared\n")]);
        write(&p.a, project::PROJECT_FILE, &a_list);
        commit_at(&p.a, "a adds include-list", 2_000_000);
        write(&p.b, project::PROJECT_FILE, &b_list);
        commit_at(&p.b, "b adds include-list", 1_000_000);

        let ga = p.ga();
        let r = p.fetch(&ga, &p.b, ID_B);
        merge_conflicted(&ga, &r);
        assert_eq!(unmerged(&ga).unwrap()[0].0, "AA");

        let got = resolve_index(&ga, &r, ID_A, ID_B, Winner::Newer).unwrap();
        assert_eq!(got, Vec::<PathBuf>::new());
        no_conflict_siblings(&p.a);
        assert!(list(&ga).unwrap().is_empty());

        let expected = project::merge(
            &project::parse(&a_list).unwrap(),
            &project::parse(&b_list).unwrap(),
        )
        .to_bytes()
        .unwrap();
        assert_eq!(read(&p.a, project::PROJECT_FILE), expected);

        ga.ok(&["commit", "--no-edit", "-m", "merge"]).unwrap();
        assert_eq!(ga.ok(&["status", "--porcelain"]).unwrap(), "");
        assert!(!committed(&ga).iter().any(|n| n.contains(".conflict-")));
    }

    #[test]
    fn a_tombstone_outlives_a_peers_older_include_list() {
        use crate::project::{self, State};
        let base = pf_bytes(&[("docs/", 1, State::Tracked)]);
        let tombstone = pf_bytes(&[("docs/", 2, State::Removed)]);
        let older = pf_bytes(&[
            ("docs/", 1, State::Tracked),
            ("CLAUDE.md", 1, State::Tracked),
        ]);
        let p = pair(&[(project::PROJECT_FILE, base.as_slice())]);
        write(&p.a, project::PROJECT_FILE, &tombstone);
        commit_at(&p.a, "a untracks docs", 2_000_000);
        write(&p.b, project::PROJECT_FILE, &older);
        commit_at(&p.b, "b still tracks docs", 1_000_000);

        let ga = p.ga();
        let r = p.fetch(&ga, &p.b, ID_B);
        merge_conflicted(&ga, &r);
        resolve_index(&ga, &r, ID_A, ID_B, Winner::Newer).unwrap();

        let merged = project::parse(&read(&p.a, project::PROJECT_FILE)).unwrap();
        assert_eq!(merged.entries["docs/"].gen, 2);
        assert_eq!(merged.entries["docs/"].state, State::Removed);
        assert_eq!(merged.entries["CLAUDE.md"].gen, 1);
        assert_eq!(merged.entries["CLAUDE.md"].state, State::Tracked);
        no_conflict_siblings(&p.a);
    }

    #[test]
    fn both_devices_compute_the_same_merged_include_list() {
        use crate::project::{self, State};
        let a_list = pf_bytes(&[("docs/", 1, State::Tracked)]);
        let b_list = pf_bytes(&[("CLAUDE.md", 1, State::Tracked)]);
        let expected = project::merge(
            &project::parse(&a_list).unwrap(),
            &project::parse(&b_list).unwrap(),
        )
        .to_bytes()
        .unwrap();
        assert_eq!(
            expected,
            project::merge(
                &project::parse(&b_list).unwrap(),
                &project::parse(&a_list).unwrap(),
            )
            .to_bytes()
            .unwrap()
        );

        let p = pair(&[("notes.md", b"shared\n")]);
        write(&p.a, project::PROJECT_FILE, &a_list);
        commit_at(&p.a, "a list", 2_000_000);
        write(&p.b, project::PROJECT_FILE, &b_list);
        commit_at(&p.b, "b list", 1_000_000);

        let (ga, gb) = (p.ga(), p.gb());
        let rb = p.fetch(&ga, &p.b, ID_B);
        let ra = p.fetch(&gb, &p.a, ID_A);

        merge_conflicted(&ga, &rb);
        resolve_index(&ga, &rb, ID_A, ID_B, Winner::Newer).unwrap();
        merge_conflicted(&gb, &ra);
        resolve_index(&gb, &ra, ID_B, ID_A, Winner::Newer).unwrap();

        assert_eq!(read(&p.a, project::PROJECT_FILE), expected);
        assert_eq!(read(&p.b, project::PROJECT_FILE), expected);
        assert_eq!(
            read(&p.a, project::PROJECT_FILE),
            read(&p.b, project::PROJECT_FILE)
        );
        no_conflict_siblings(&p.a);
        no_conflict_siblings(&p.b);
    }

    #[test]
    fn a_project_file_conflict_does_not_change_a_user_files_winner() {
        use crate::project::{self, State};
        let base_list = pf_bytes(&[("notes.md", 1, State::Tracked)]);
        let a_list = pf_bytes(&[("docs/", 1, State::Tracked)]);
        let b_list = pf_bytes(&[("agents/", 1, State::Tracked)]);
        let p = pair(&[
            ("CLAUDE.md", BASE),
            (project::PROJECT_FILE, base_list.as_slice()),
        ]);
        write(&p.a, "CLAUDE.md", A_TEXT);
        write(&p.a, project::PROJECT_FILE, &a_list);
        commit_at(&p.a, "a1", 2_000_000);
        write(&p.b, "CLAUDE.md", B_TEXT);
        write(&p.b, project::PROJECT_FILE, &b_list);
        commit_at(&p.b, "b1", 1_000_000);

        let ga = p.ga();
        let r = p.fetch(&ga, &p.b, ID_B);
        assert!(winner_is_ours(&ga, &r).unwrap());
        merge_conflicted(&ga, &r);

        let got = resolve_index(&ga, &r, ID_A, ID_B, Winner::Newer).unwrap();
        assert_eq!(got, vec![PathBuf::from("CLAUDE.md")]);

        let sibling = format!("CLAUDE.conflict-bbbbbbbb-{}.md", p.blob7(B_TEXT));
        assert_eq!(read(&p.a, "CLAUDE.md"), A_TEXT);
        assert_eq!(read(&p.a, &sibling), B_TEXT);

        let expected = project::merge(
            &project::parse(&a_list).unwrap(),
            &project::parse(&b_list).unwrap(),
        )
        .to_bytes()
        .unwrap();
        assert_eq!(read(&p.a, project::PROJECT_FILE), expected);
        assert!(!p.a.read_dir().unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(&format!("{}.conflict-", project::PROJECT_FILE))));
    }

    #[test]
    fn a_one_sided_project_file_conflict_falls_through_to_the_existing_arms() {
        use crate::project::{self, State};
        let body = pf_bytes(&[("docs/", 1, State::Tracked)]);
        let p = pair(&[("f.md", body.as_slice())]);
        fs::rename(p.a.join("f.md"), p.a.join(project::PROJECT_FILE)).unwrap();
        commit_at(&p.a, "a renames to project file", 2_000_000);
        fs::rename(p.b.join("f.md"), p.b.join("other.md")).unwrap();
        commit_at(&p.b, "b renames elsewhere", 1_000_000);

        let ga = p.ga();
        let r = p.fetch(&ga, &p.b, ID_B);
        merge_conflicted(&ga, &r);
        let codes: Vec<(String, PathBuf)> = unmerged(&ga).unwrap();
        assert!(
            codes
                .iter()
                .any(|(c, path)| c == "AU" && path == Path::new(project::PROJECT_FILE)),
            "expected AU on {}: {codes:?}",
            project::PROJECT_FILE
        );

        assert!(resolve_one(
            &ga,
            "AU",
            Path::new(project::PROJECT_FILE),
            true,
            "bbbbbbbb",
        )
        .unwrap()
        .is_none());
        assert_eq!(
            resolve_index(&ga, &r, ID_A, ID_B, Winner::Newer).unwrap(),
            Vec::<PathBuf>::new()
        );
        ga.ok(&["commit", "--no-edit", "-m", "merge"]).unwrap();
        assert_eq!(ga.ok(&["status", "--porcelain"]).unwrap(), "");
        assert!(p.a.join(project::PROJECT_FILE).is_file());
    }

    /// git cannot produce an eighth unmerged code from a real index — the
    /// seven are exhaustive over the stage subsets — so the arm is reached
    /// directly. Swallowing it would leave the index unmerged forever.
    #[test]
    fn an_unhandled_merge_state_is_reported_not_swallowed() {
        let p = pair(&[("CLAUDE.md", BASE)]);
        let err = resolve_one(&p.ga(), "XU", Path::new("CLAUDE.md"), true, "bbbbbbbb")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("unhandled merge state XU for CLAUDE.md"),
            "{err}"
        );
    }

    #[test]
    fn status_parsing_keeps_unknown_unmerged_codes_and_skips_rename_origins() {
        let buf = b"R  new.md\0old.md\0XU weird.md\0?? junk.md\0M  quiet.md\0UU CLAUDE.md\0";
        assert_eq!(
            parse_status(buf).unwrap(),
            vec![
                ("XU".to_string(), PathBuf::from("weird.md")),
                ("UU".to_string(), PathBuf::from("CLAUDE.md")),
            ]
        );
    }

    // --- names ------------------------------------------------------------

    #[test]
    fn sibling_names_round_trip_through_parsing() {
        for live in [
            "CLAUDE.md",
            "Makefile",
            "a.tar.gz",
            ".env",
            "sub/dir/settings.json",
        ] {
            let s = sibling_name(Path::new(live), "bbbbbbbb", "1234567");
            let c = parse_sibling(&s).unwrap_or_else(|| panic!("{live} -> {s:?} did not parse"));
            assert_eq!(c.live, PathBuf::from(live));
            assert_eq!(c.sibling, s);
            assert_eq!(c.loser_id8, "bbbbbbbb");
        }
    }

    #[test]
    fn sibling_names_are_spelled_as_the_spec_says() {
        assert_eq!(
            sibling_name(Path::new("agents/CLAUDE.md"), "bbbbbbbb", "1234567"),
            PathBuf::from("agents/CLAUDE.conflict-bbbbbbbb-1234567.md")
        );
        assert_eq!(
            sibling_name(Path::new("Makefile"), "bbbbbbbb", "1234567"),
            PathBuf::from("Makefile.conflict-bbbbbbbb-1234567")
        );
    }

    /// A user's own file that merely contains the delimiter is not a conflict;
    /// treating it as one would offer to delete it.
    #[test]
    fn a_malformed_name_is_not_a_conflict() {
        for name in [
            "notes.conflict-log.md",
            "x.conflict-bbbbbbbb.md",
            "x.conflict-bbbbbbbb-12345.md",
            "x.conflict-bbbbbbbb-1234567.tar.gz",
            ".conflict-bbbbbbbb-1234567.md",
        ] {
            assert!(
                parse_sibling(Path::new(name)).is_none(),
                "{name} parsed as a conflict"
            );
        }
    }

    /// The sibling of a sibling belongs to the sibling, not to the original.
    #[test]
    fn the_rightmost_suffix_names_the_live_file() {
        let c = parse_sibling(Path::new(
            "x.conflict-aaaaaaaa-1111111.conflict-bbbbbbbb-2222222.md",
        ))
        .unwrap();
        assert_eq!(c.live, PathBuf::from("x.conflict-aaaaaaaa-1111111.md"));
        assert_eq!(c.loser_id8, "bbbbbbbb");
    }

    // --- list -------------------------------------------------------------

    #[test]
    fn list_reports_the_committed_sibling_and_its_live_path() {
        let p = diverged(2_000_000, 1_000_000);
        let ga = p.ga();
        let r = p.fetch(&ga, &p.b, ID_B);
        merge_conflicted(&ga, &r);
        resolve_index(&ga, &r, ID_A, ID_B, Winner::Newer).unwrap();
        ga.ok(&["commit", "--no-edit", "-m", "merge"]).unwrap();

        assert_eq!(
            list(&ga).unwrap(),
            vec![Conflict {
                live: PathBuf::from("CLAUDE.md"),
                sibling: PathBuf::from(format!("CLAUDE.conflict-bbbbbbbb-{}.md", p.blob7(B_TEXT))),
                loser_id8: "bbbbbbbb".to_string(),
            }]
        );
    }

    // --- blob7 ------------------------------------------------------------

    /// The spec names `hash-object --stdin` over the loser's bytes; this uses
    /// the merge stage's object id instead. If those ever differ, every
    /// device stops agreeing on sibling names.
    #[test]
    fn the_sibling_hash_is_git_s_hash_of_the_loser_bytes() {
        let p = diverged(2_000_000, 1_000_000);
        let ga = p.ga();
        let r = p.fetch(&ga, &p.b, ID_B);
        merge_conflicted(&ga, &r);

        let stage = stage_blob(&ga, Path::new("CLAUDE.md"), 3).unwrap();
        let f = p._dir.path().join("loser");
        fs::write(&f, B_TEXT).unwrap();
        assert_eq!(
            stage,
            ga.ok(&["hash-object", "--", f.to_str().unwrap()]).unwrap()
        );
    }

    // --- resolve ----------------------------------------------------------

    /// C2: resolving `settings.json` may not touch the siblings of
    /// `settings.md` or of `sub/settings.json`, and an unselected sibling of
    /// the same live file survives too. `main` stays where it was.
    #[test]
    fn resolve_removes_only_the_selected_siblings_of_that_live_path() {
        let home = TempDir::new().unwrap();
        let root = TempDir::new().unwrap();
        let repo = Repo::init(home.path(), "proj-claude", root.path(), "Mac A Pro", ID_A).unwrap();

        let selected = "settings.conflict-bbbbbbbb-1111111.json";
        let kept = [
            "settings.conflict-bbbbbbbb-2222222.json",
            "settings.conflict-bbbbbbbb-3333333.md",
            "sub/settings.conflict-bbbbbbbb-4444444.json",
        ];
        for rel in [
            "settings.json",
            "settings.md",
            "sub/settings.json",
            selected,
            kept[0],
            kept[1],
            kept[2],
        ] {
            write(&repo.staging, rel, rel.as_bytes());
        }
        repo.git.ok(&["add", "-A"]).unwrap();
        repo.git.ok(&["commit", "-m", "seed"]).unwrap();
        let main_before = repo.git.rev("refs/heads/main").unwrap();
        let files_before = repo.git.ok(&["ls-files"]).unwrap();

        let tx = repo.begin_tx(PK, "HEAD", resolve_index).unwrap();
        // The two wrong-live entries are offered and must be ignored.
        let selection = [
            PathBuf::from(selected),
            PathBuf::from(kept[1]),
            PathBuf::from(kept[2]),
        ];
        resolve(&tx, Path::new("settings.json"), &selection, b"merged").unwrap();

        let tracked = committed(&tx.git);
        assert!(
            !tracked.iter().any(|f| f == selected),
            "the selected sibling survived: {tracked:?}"
        );
        for k in kept {
            assert!(
                tracked.iter().any(|f| f == k),
                "{k} was removed: {tracked:?}"
            );
        }
        assert_eq!(
            fs::read(tx.worktree.join("settings.json")).unwrap(),
            b"merged"
        );
        assert!(!tx.worktree.join(selected).exists());
        assert_eq!(tx.git.ok(&["status", "--porcelain"]).unwrap(), "");

        assert_eq!(repo.git.rev("refs/heads/main").unwrap(), main_before);
        assert_eq!(repo.git.ok(&["ls-files"]).unwrap(), files_before);
        assert_eq!(
            fs::read(repo.staging.join(selected)).unwrap(),
            selected.as_bytes()
        );
    }

    /// Symlinks are never followed. Another device can commit a mode-120000
    /// blob at a live path; writing "through" it would put a user's merged
    /// bytes wherever it points.
    #[test]
    fn resolve_refuses_to_write_through_a_symlink() {
        let home = TempDir::new().unwrap();
        let root = TempDir::new().unwrap();
        let repo = Repo::init(home.path(), "proj-claude", root.path(), "Mac A Pro", ID_A).unwrap();
        write(&repo.staging, "elsewhere.json", b"untouched");
        std::os::unix::fs::symlink("elsewhere.json", repo.staging.join("settings.json")).unwrap();
        repo.git.ok(&["add", "-A"]).unwrap();
        repo.git.ok(&["commit", "-m", "seed"]).unwrap();

        let tx = repo.begin_tx(PK, "HEAD", resolve_index).unwrap();
        let err = resolve(&tx, Path::new("settings.json"), &[], b"merged")
            .unwrap_err()
            .to_string();
        assert!(err.contains("not a regular file"), "{err}");
        assert_eq!(
            fs::read(tx.worktree.join("elsewhere.json")).unwrap(),
            b"untouched"
        );
        assert!(fs::symlink_metadata(tx.worktree.join("settings.json"))
            .unwrap()
            .file_type()
            .is_symlink());
    }

    // --- presence: delete vs re-add ----------------------------------------

    const V: &[u8] = b"v\n";
    const W: &[u8] = b"w\n";
    const T0: i64 = 1_000_000;

    fn tracked(keys: &[&str]) -> EntryList {
        use crate::project::{EntryRecord, ProjectFile, State};
        ProjectFile {
            entries: keys
                .iter()
                .map(|k| {
                    let rec = EntryRecord {
                        gen: 1,
                        state: State::Tracked,
                    };
                    ((*k).to_string(), rec)
                })
                .collect(),
        }
        .tracked()
    }

    /// A user event writing `rel`, at `T0 + at`.
    fn put(repo: &Path, rel: &str, body: &[u8], subject: &str, at: i64) {
        write(repo, rel, body);
        commit_at(repo, subject, T0 + at);
    }

    /// A user event deleting `rels`, at `T0 + at`.
    fn del(repo: &Path, rels: &[&str], subject: &str, at: i64) {
        for rel in rels {
            fs::remove_file(repo.join(rel)).unwrap();
        }
        commit_at(repo, subject, T0 + at);
    }

    /// `git merge --no-commit` as the engine will run it: a conflict is
    /// resolved first, so the index holds the merge result.
    fn merge_in_progress(git: &Git, r: &str, my_id: &str, their_id: &str, extra: &[&str]) {
        let mut args = vec!["merge", "--no-ff", "--no-commit", "--no-edit"];
        args.extend_from_slice(extra);
        args.push(r);
        let out = git.run(&args).unwrap();
        if !out.status.success() {
            resolve_index(git, r, my_id, their_id, Winner::Newer).unwrap();
        }
    }

    fn index_tree(git: &Git) -> String {
        git.ok(&["write-tree"]).unwrap()
    }

    fn in_index(git: &Git, rel: &str) -> bool {
        !git.ok(&["ls-files", "--", &format!(":(literal){rel}")])
            .unwrap()
            .is_empty()
    }

    /// Each device merges the other and settles. Returns what each settle
    /// changed and asserts the two index trees are identical.
    fn settle_both(p: &Pair, entries: &EntryList) -> (Vec<PathBuf>, Vec<PathBuf>) {
        let (ga, gb) = (p.ga(), p.gb());
        let rb = p.fetch(&ga, &p.b, ID_B);
        let ra = p.fetch(&gb, &p.a, ID_A);
        merge_in_progress(&ga, &rb, ID_A, ID_B, &[]);
        merge_in_progress(&gb, &ra, ID_B, ID_A, &[]);
        let got_a = settle_presence(&ga, "HEAD", &rb, entries).unwrap();
        let got_b = settle_presence(&gb, "HEAD", &ra, entries).unwrap();
        assert_eq!(index_tree(&ga), index_tree(&gb), "settled trees differ");
        (got_a, got_b)
    }

    /// `rel` is staged and live on both devices with `body`, or gone from both.
    fn assert_presence(p: &Pair, rel: &str, body: Option<&[u8]>) {
        for (dir, git) in [(&p.a, p.ga()), (&p.b, p.gb())] {
            assert_eq!(in_index(&git, rel), body.is_some(), "{rel} in index");
            match body {
                Some(b) => assert_eq!(read(dir, rel), b, "{rel} bytes"),
                None => assert!(!dir.join(rel).exists(), "{rel} still live"),
            }
        }
    }

    fn paths(rels: &[&str]) -> Vec<PathBuf> {
        rels.iter().map(PathBuf::from).collect()
    }

    /// A deletes F and restores it; B deletes F `b_delete` seconds in. git
    /// alone deletes F (A's F equals the base).
    fn restore_vs_delete(a_restore: i64, b_delete: i64) -> Pair {
        let p = pair(&[("F.md", V)]);
        del(&p.a, &["F.md"], "local aaaaaaaa", 100);
        put(&p.a, "F.md", V, "local aaaaaaaa", a_restore);
        del(&p.b, &["F.md"], "local bbbbbbbb", b_delete);
        p
    }

    #[test]
    fn presence_a_re_add_newer_than_the_delete_is_kept() {
        let p = restore_vs_delete(300, 200);
        let (a, b) = settle_both(&p, &tracked(&["F.md"]));
        assert_eq!((a, b), (paths(&["F.md"]), paths(&["F.md"])));
        assert_presence(&p, "F.md", Some(V));
    }

    #[test]
    fn presence_a_delete_newer_than_the_restore_stays_deleted() {
        let p = restore_vs_delete(200, 300);
        let (a, b) = settle_both(&p, &tracked(&["F.md"]));
        assert_eq!((a, b), (vec![], vec![]));
        assert_presence(&p, "F.md", None);
    }

    /// `tp == td - PRESENCE_TIE_SECS` is a tie and keeps the file; one more
    /// second and the delete wins.
    #[test]
    fn presence_events_within_the_tie_window_keep_the_file() {
        let p = restore_vs_delete(200, 200 + PRESENCE_TIE_SECS);
        let (a, b) = settle_both(&p, &tracked(&["F.md"]));
        assert_eq!((a, b), (paths(&["F.md"]), paths(&["F.md"])));
        assert_presence(&p, "F.md", Some(V));

        let p = restore_vs_delete(200, 201 + PRESENCE_TIE_SECS);
        let (a, b) = settle_both(&p, &tracked(&["F.md"]));
        assert_eq!((a, b), (vec![], vec![]));
        assert_presence(&p, "F.md", None);
    }

    /// The base lacks F. A adds F and deletes it later; B adds F in between.
    /// git keeps B's add, so the settle itself has to remove it.
    fn add_vs_later_delete(b_body: &[u8], b_exec: bool) -> Pair {
        use std::os::unix::fs::PermissionsExt;
        let p = pair(&[("other.md", V)]);
        put(&p.a, "F.md", V, "local aaaaaaaa", 100);
        del(&p.a, &["F.md"], "local aaaaaaaa", 300);
        write(&p.b, "F.md", b_body);
        if b_exec {
            fs::set_permissions(p.b.join("F.md"), fs::Permissions::from_mode(0o755)).unwrap();
        }
        commit_at(&p.b, "local bbbbbbbb", T0 + 200);
        p
    }

    #[test]
    fn presence_a_newer_informed_delete_removes_a_peer_add() {
        let p = add_vs_later_delete(V, false);
        let (a, b) = settle_both(&p, &tracked(&["F.md"]));
        assert_eq!((a, b), (paths(&["F.md"]), paths(&["F.md"])));
        assert_presence(&p, "F.md", None);
    }

    #[test]
    fn presence_never_removes_different_bytes() {
        let p = add_vs_later_delete(W, false);
        let (a, b) = settle_both(&p, &tracked(&["F.md"]));
        assert_eq!((a, b), (vec![], vec![]));
        assert_presence(&p, "F.md", Some(W));
    }

    #[test]
    fn presence_never_removes_a_changed_mode() {
        let p = add_vs_later_delete(V, true);
        let (a, b) = settle_both(&p, &tracked(&["F.md"]));
        assert_eq!((a, b), (vec![], vec![]));
        assert_presence(&p, "F.md", Some(V));
        let mode = p.ga().ok(&["ls-files", "-s", "--", "F.md"]).unwrap();
        assert!(mode.starts_with("100755"), "{mode}");
    }

    /// Modify/delete: resolve_index keeps the modification, and a newer
    /// delete does not take it away.
    #[test]
    fn presence_a_newer_delete_never_beats_a_modification() {
        let p = pair(&[("F.md", V)]);
        put(&p.a, "F.md", W, "local aaaaaaaa", 100);
        del(&p.b, &["F.md"], "local bbbbbbbb", 300);
        let (a, b) = settle_both(&p, &tracked(&["F.md"]));
        assert_eq!((a, b), (vec![], vec![]));
        assert_presence(&p, "F.md", Some(W));
    }

    #[test]
    fn presence_skips_untracked_private_and_sibling_paths() {
        let names = [
            "docs/keep.md",
            "docs/.dotloreproject",
            "docs/x.conflict-aaaaaaaa-1234567.md",
            "other.md",
        ];
        let seed: Vec<(&str, &[u8])> = names.iter().map(|n| (*n, V)).collect();
        let p = pair(&seed);
        del(&p.a, &names, "local aaaaaaaa", 100);
        for n in names {
            write(&p.a, n, V);
        }
        commit_at(&p.a, "local aaaaaaaa", T0 + 300);
        del(&p.b, &names, "local bbbbbbbb", 200);

        let (a, b) = settle_both(&p, &tracked(&["docs/"]));
        assert_eq!((a, b), (paths(&["docs/keep.md"]), paths(&["docs/keep.md"])));
        assert_presence(&p, "docs/keep.md", Some(V));
        for n in &names[1..] {
            assert_presence(&p, n, None);
        }
    }

    /// The result is a function of the two tips, not of which one is HEAD.
    #[test]
    fn presence_swapping_ours_and_theirs_gives_the_same_tree() {
        let p = pair(&[("F.md", V), ("G.md", V), ("H.md", V)]);
        // F: A restores late. G: B deletes late. H: both edit, a conflict.
        del(&p.a, &["F.md", "G.md"], "local aaaaaaaa", 100);
        write(&p.a, "F.md", V);
        write(&p.a, "G.md", V);
        write(&p.a, "H.md", W);
        commit_at(&p.a, "local aaaaaaaa", T0 + 300);
        del(&p.b, &["F.md"], "local bbbbbbbb", 200);
        write(&p.b, "H.md", b"b\n");
        commit_at(&p.b, "local bbbbbbbb", T0 + 250);
        del(&p.b, &["G.md"], "local bbbbbbbb", 400);

        let (a, b) = settle_both(&p, &tracked(&["F.md", "G.md", "H.md"]));
        assert_eq!(a, b);
        assert_presence(&p, "F.md", Some(V));
        assert_presence(&p, "G.md", None);
    }

    /// Case 4. M0 has F; C deletes it Sunday; A deletes it Monday and B
    /// restores it Tuesday on top. When D_A is the kept head R fast-forwards;
    /// when D_C is kept, R merges into D_C. This function never compares
    /// hashes, so that merge in both directions is every order there is.
    #[test]
    fn presence_case_4_keeps_the_restore_in_either_direction() {
        const DAY: i64 = 86_400;
        let p = pair(&[("F.md", V)]);
        del(&p.b, &["F.md"], "local cccccccc", DAY);
        del(&p.a, &["F.md"], "local aaaaaaaa", 2 * DAY);
        put(&p.a, "F.md", V, "local bbbbbbbb", 3 * DAY);

        let (a, b) = settle_both(&p, &tracked(&["F.md"]));
        assert_eq!((a, b), (paths(&["F.md"]), paths(&["F.md"])));
        assert_presence(&p, "F.md", Some(V));
    }

    /// Without a merge-base every old event would be in range; an ancient
    /// delete of a same-named path must not act.
    #[test]
    fn presence_does_nothing_without_a_merge_base() {
        let dir = TempDir::new().unwrap();
        let (a, b) = (dir.path().join("A"), dir.path().join("B"));
        for d in [&a, &b] {
            fs::create_dir_all(d).unwrap();
            fixture_git(d, T0, &["init", "-q", "-b", "main"]);
        }
        put(&a, "G.md", V, "local aaaaaaaa", 50);
        put(&a, "F.md", V, "local aaaaaaaa", 100);
        del(&a, &["F.md"], "local aaaaaaaa", 300);
        put(&b, "F.md", V, "local bbbbbbbb", 200);

        let ga = Git::new(a.clone(), "Mac A Pro", ID_A);
        let r = "refs/remotes/pk/b/main";
        ga.ok(&["fetch", b.to_str().unwrap(), &format!("+main:{r}")])
            .unwrap();
        merge_in_progress(&ga, r, ID_A, ID_B, &["--allow-unrelated-histories"]);
        let got = settle_presence(&ga, "HEAD", r, &tracked(&["F.md", "G.md"])).unwrap();
        assert!(got.is_empty(), "{got:?}");
        assert!(in_index(&ga, "F.md"));
        assert_eq!(read(&a, "F.md"), V);
    }

    #[test]
    fn presence_has_no_candidates_when_a_peer_only_added_files() {
        let p = pair(&[("F.md", V)]);
        put(&p.a, "H.md", V, "local aaaaaaaa", 100);
        put(&p.b, "G.md", V, "local bbbbbbbb", 200);
        let (a, b) = settle_both(&p, &tracked(&["F.md", "G.md", "H.md"]));
        assert_eq!((a, b), (vec![], vec![]));
        for f in ["F.md", "G.md", "H.md"] {
            assert_presence(&p, f, Some(V));
        }
    }

    /// One side deleted, the other never touched the path: there is no
    /// present event to weigh, so git's clean delete stands.
    #[test]
    fn presence_leaves_a_lone_delete_alone() {
        let p = pair(&[("F.md", V)]);
        del(&p.a, &["F.md"], "local aaaaaaaa", 100);
        put(&p.b, "G.md", V, "local bbbbbbbb", 200);
        let (a, b) = settle_both(&p, &tracked(&["F.md", "G.md"]));
        assert_eq!((a, b), (vec![], vec![]));
        assert_presence(&p, "F.md", None);
    }

    /// A edits F and deletes it within the tie window; B never touches F.
    /// Both events are A's, so there is nothing to weigh: the delete stands.
    #[test]
    fn presence_a_same_side_edit_then_delete_is_left_alone() {
        let p = pair(&[("F.md", V)]);
        put(&p.a, "F.md", W, "local aaaaaaaa", 100);
        del(&p.a, &["F.md"], "local aaaaaaaa", 103);
        put(&p.b, "G.md", V, "local bbbbbbbb", 200);
        let (a, b) = settle_both(&p, &tracked(&["F.md", "G.md"]));
        assert_eq!((a, b), (vec![], vec![]));
        assert_presence(&p, "F.md", None);
    }

    /// A deletes F and re-adds it within the tie window; B never touches F.
    /// git's result stands.
    #[test]
    fn presence_a_same_side_delete_then_re_add_is_left_alone() {
        let p = pair(&[("F.md", V)]);
        del(&p.a, &["F.md"], "local aaaaaaaa", 100);
        put(&p.a, "F.md", V, "local aaaaaaaa", 103);
        put(&p.b, "G.md", V, "local bbbbbbbb", 200);
        let (a, b) = settle_both(&p, &tracked(&["F.md", "G.md"]));
        assert_eq!((a, b), (vec![], vec![]));
        assert_presence(&p, "F.md", Some(V));
    }

    impl Conflict {
        fn sibling_s(&self) -> String {
            self.sibling.to_string_lossy().into_owned()
        }
    }
}
