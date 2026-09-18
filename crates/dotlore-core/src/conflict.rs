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
//! - the winner's blob becomes the live file, so no git conflict marker ever
//!   reaches a tracked path;
//! - the loser's bytes are committed next to it as
//!   `<stem>.conflict-<loser id8>-<blob7>.<ext>`, which carries both the
//!   losing device and the losing blob so a repeated or three-way conflict
//!   adds a sibling instead of overwriting one.
//!
//! Everything here runs against the transaction worktree ([`Transaction`]);
//! `main` and its staging worktree are finalized later, by the transaction.

use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};

use crate::git::Git;
use crate::repo::Transaction;

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
) -> Result<Vec<PathBuf>> {
    let ours_win = winner_is_ours(git, their_ref)?;
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
    use crate::cloud::Kind;
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

    // --- UU: text ---------------------------------------------------------

    #[test]
    fn uu_newer_ours_wins_and_the_loser_becomes_a_sibling() {
        let p = diverged(2_000_000, 1_000_000);
        let ga = p.ga();
        let r = p.fetch(&ga, &p.b, ID_B);
        assert!(winner_is_ours(&ga, &r).unwrap());
        merge_conflicted(&ga, &r);

        let got = resolve_index(&ga, &r, ID_A, ID_B).unwrap();
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

    #[test]
    fn uu_newer_theirs_wins_and_our_bytes_become_the_sibling() {
        let p = diverged(1_000_000, 2_000_000);
        let ga = p.ga();
        let r = p.fetch(&ga, &p.b, ID_B);
        assert!(!winner_is_ours(&ga, &r).unwrap());
        merge_conflicted(&ga, &r);

        resolve_index(&ga, &r, ID_A, ID_B).unwrap();

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
        resolve_index(&ga, &rb, ID_A, ID_B).unwrap();
        ga.ok(&["commit", "--no-edit", "-m", "merge b"]).unwrap();

        merge_conflicted(&gb, &ra);
        resolve_index(&gb, &ra, ID_B, ID_A).unwrap();
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
        resolve_index(&ga, &r, ID_A, ID_B).unwrap();
        ga.ok(&["commit", "--no-edit", "-m", "merge 1"]).unwrap();

        const A2: &[u8] = b"A-again\ntwo\nthree\n";
        const B2: &[u8] = b"B-again\ntwo\nthree\n";
        write(&p.a, "CLAUDE.md", A2);
        commit_at(&p.a, "a2", 4_000_000);
        write(&p.b, "CLAUDE.md", B2);
        commit_at(&p.b, "b2", 3_000_000);

        let r = p.fetch(&ga, &p.b, ID_B);
        merge_conflicted(&ga, &r);
        resolve_index(&ga, &r, ID_A, ID_B).unwrap();
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

        let got = resolve_index(&ga, &r, ID_A, ID_B).unwrap();
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
        resolve_index(&ga, &r, ID_A, ID_B).unwrap();

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
            resolve_index(&ga, &r, ID_A, ID_B).unwrap(),
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
            resolve_index(&ga, &r, ID_A, ID_B).unwrap(),
            Vec::<PathBuf>::new()
        );
        ga.ok(&["commit", "--no-edit", "-m", "merge"]).unwrap();

        assert_eq!(ga.ok(&["status", "--porcelain"]).unwrap(), "");
        assert_eq!(committed(&ga), vec!["keep.md", "take.md"]);
        assert_eq!(read(&p.a, "keep.md"), b"mine\n");
        assert_eq!(read(&p.a, "take.md"), b"theirs\n");
    }

    // --- unknown states ---------------------------------------------------

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
        resolve_index(&ga, &r, ID_A, ID_B).unwrap();
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
        let repo = Repo::init(
            home.path(),
            "proj-claude",
            root.path(),
            "Mac A Pro",
            ID_A,
            Kind::Dir,
        )
        .unwrap();

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
        let repo = Repo::init(
            home.path(),
            "proj-claude",
            root.path(),
            "Mac A Pro",
            ID_A,
            Kind::Dir,
        )
        .unwrap();
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

    impl Conflict {
        fn sibling_s(&self) -> String {
            self.sibling.to_string_lossy().into_owned()
        }
    }
}
