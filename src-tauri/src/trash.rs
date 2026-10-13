//! The local trash under `<home>/trash`: copies of the live files a sync
//! deleted or overwrote.
//!
//! Layout: `<home>/trash/<slug>/<day>/<pass>/<rel>`, where `day` is
//! `YYYY-MM-DD` (or `undated` when local time was unavailable) and `pass` is
//! `HHMMSS` (`<unix-secs>` under `undated`), with a `-N` suffix for the
//! sibling folders `mirror::keep_in_trash` uses for duplicates. Every id that
//! reaches this module from the webview is untrusted, so each one is checked
//! against that exact shape and no path built here ever follows a symlink.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Serialize;

use crate::config::Root;

/// `[a-z0-9]+(-[a-z0-9]+)*`, the same rule `repo` and `engine` hold slugs
/// to — also the shape that makes a slug safe as a single path component.
fn valid_slug(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && !s.ends_with('-')
        && !s.contains("--")
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// `YYYY-MM-DD` exactly, or `undated`: the two day names `repo::trash_dir`
/// writes.
fn valid_day(s: &str) -> bool {
    if s == "undated" {
        return true;
    }
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && digits(&s[..4])
        && digits(&s[5..7])
        && digits(&s[8..])
}

/// `HHMMSS` under a dated day, `<unix-secs>` under `undated`, either with an
/// optional `-N` (N >= 1, no leading zero) for a sibling duplicate pass.
fn valid_pass(day: &str, pass: &str) -> bool {
    let (time, n) = match pass.split_once('-') {
        Some((time, n)) => (time, Some(n)),
        None => (pass, None),
    };
    let time_ok = if day == "undated" {
        digits(time)
    } else {
        time.len() == 6 && digits(time)
    };
    let n_ok = n.is_none_or(|n| digits(n) && !n.starts_with('0'));
    time_ok && n_ok
}

/// Stat `path` without following it, refusing a symlink. A missing path is
/// an error too: every caller is about to read or remove what is there.
fn no_symlink(path: &Path) -> Result<fs::Metadata> {
    let md = fs::symlink_metadata(path).with_context(|| format!("stat {}", path.display()))?;
    if md.file_type().is_symlink() {
        bail!("refusing symlink {}", path.display());
    }
    Ok(md)
}

fn require_dir(path: &Path) -> Result<()> {
    if !no_symlink(path)?.is_dir() {
        bail!("not a folder: {}", path.display());
    }
    Ok(())
}

/// `<home>/trash/<slug>/<day>/<pass>` for untrusted ids from the webview.
///
/// Each level is stated top-down with `symlink_metadata`, because that only
/// refuses to follow the last component: a symlinked `<home>/trash` is caught
/// only by statting it before anything below it.
pub fn pass_dir(home: &Path, slug: &str, day: &str, pass: &str) -> Result<PathBuf> {
    if !valid_slug(slug) {
        bail!("invalid slug {slug:?}");
    }
    if !valid_day(day) {
        bail!("invalid trash day {day:?}");
    }
    if !valid_pass(day, pass) {
        bail!("invalid trash pass {pass:?}");
    }
    let mut dir = home.join("trash");
    require_dir(&dir)?;
    for part in [slug, day, pass] {
        dir.push(part);
        require_dir(&dir)?;
    }
    Ok(dir)
}

/// A file or folder at `rel` inside a pass, for an untrusted `rel` from the
/// webview: plain names only, and no symlink anywhere on the way down.
pub fn item_path(home: &Path, slug: &str, day: &str, pass: &str, rel: &str) -> Result<PathBuf> {
    let rel_path = Path::new(rel);
    if rel.is_empty()
        || !rel_path
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
    {
        bail!("unsafe trash path {rel:?}");
    }
    let mut path = pass_dir(home, slug, day, pass)?;
    for part in rel_path.components() {
        path.push(part);
        no_symlink(&path)?;
    }
    Ok(path)
}

/// How a trash copy compares with the live file at the same `rel`.
#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum TrashStatus {
    NotInProject,
    Differs,
    Same,
    Unknown,
}

#[derive(Serialize, Clone, PartialEq, Eq, Debug)]
pub struct TrashFile {
    pub rel: String,
    pub bytes: u64,
    /// `None` when the project is no longer tracked.
    pub status: Option<TrashStatus>,
}

#[derive(Serialize, Clone, PartialEq, Eq, Debug)]
pub struct TrashPass {
    pub day: String,
    pub pass: String,
    /// The absolute pass folder, for Reveal in Finder.
    pub path: String,
    pub files: Vec<TrashFile>,
}

#[derive(Serialize, Clone, PartialEq, Eq, Debug)]
pub struct TrashProject {
    pub slug: String,
    /// The live root path; `None` for a removed project.
    pub root: Option<String>,
    pub passes: Vec<TrashPass>,
}

/// Everything in `<home>/trash`, grouped by project and pass, with each
/// file compared against the live root it came from.
///
/// Projects are sorted by slug, passes newest first (`undated` last), files
/// by `rel`. Nothing is followed: symlinks, non-UTF-8 names, names that fail
/// the slug/day/pass checks and anything neither a plain file nor a folder
/// are skipped, and so are passes and projects left with no files. Equal-length
/// files are byte-compared when at most `cap` bytes; larger ones are `Unknown`.
pub fn list(home: &Path, roots: &[Root], cap: u64) -> Result<Vec<TrashProject>> {
    let trash = home.join("trash");
    match fs::symlink_metadata(&trash) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e).with_context(|| format!("stat {}", trash.display())),
        Ok(md) if md.file_type().is_symlink() => bail!("refusing symlink {}", trash.display()),
        Ok(md) if !md.is_dir() => bail!("not a folder: {}", trash.display()),
        Ok(_) => {}
    }
    let mut projects = Vec::new();
    for (slug, slug_dir) in dirs(&trash)? {
        if !valid_slug(&slug) {
            continue;
        }
        let root = roots.iter().find(|r| r.slug == slug);
        let mut passes = Vec::new();
        for (day, day_dir) in dirs(&slug_dir)? {
            if !valid_day(&day) {
                continue;
            }
            for (pass, pass_dir) in dirs(&day_dir)? {
                if !valid_pass(&day, &pass) {
                    continue;
                }
                let mut found = Vec::new();
                walk(&pass_dir, "", &mut found)?;
                if found.is_empty() {
                    continue;
                }
                found.sort_by(|a, b| a.0.cmp(&b.0));
                let files = found
                    .into_iter()
                    .map(|(rel, bytes, path)| TrashFile {
                        status: root.map(|r| status(&r.path, &rel, &path, bytes, cap)),
                        rel,
                        bytes,
                    })
                    .collect();
                passes.push(TrashPass {
                    path: pass_dir.to_string_lossy().into_owned(),
                    day: day.clone(),
                    pass,
                    files,
                });
            }
        }
        if passes.is_empty() {
            continue;
        }
        passes.sort_by(|a, b| pass_key(b).cmp(&pass_key(a)));
        projects.push(TrashProject {
            slug,
            root: root.map(|r| r.path.to_string_lossy().into_owned()),
            passes,
        });
    }
    projects.sort_by(|a, b| a.slug.cmp(&b.slug));
    Ok(projects)
}

/// The plain (non-symlink) subfolders of `dir` with UTF-8 names.
fn dirs(dir: &Path) -> Result<Vec<(String, PathBuf)>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let entry = entry.with_context(|| format!("read {}", dir.display()))?;
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let path = entry.path();
        if fs::symlink_metadata(&path).is_ok_and(|md| md.is_dir()) {
            out.push((name, path));
        }
    }
    Ok(out)
}

/// Collect `(rel, bytes, path)` for every plain file under `dir`, recursing
/// into plain folders only; `rel` uses `/` separators.
fn walk(dir: &Path, prefix: &str, out: &mut Vec<(String, u64, PathBuf)>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let entry = entry.with_context(|| format!("read {}", dir.display()))?;
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let rel = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        let path = entry.path();
        let Ok(md) = fs::symlink_metadata(&path) else {
            continue;
        };
        if md.is_file() {
            out.push((rel, md.len(), path));
        } else if md.is_dir() {
            walk(&path, &rel, out)?;
        }
    }
    Ok(())
}

/// The plain files of a pass, or of the item at `rel` inside it, as sorted
/// `/`-separated rels relative to the pass. Nothing is followed: a symlinked
/// `rel` lists nothing, and so does anything below a symlink. `rel` must
/// already have passed [`item_path`]; this does not take the home lock.
pub(crate) fn files_under(pass_dir: &Path, rel: Option<&str>) -> Result<Vec<String>> {
    let mut found = Vec::new();
    match rel {
        None => walk(pass_dir, "", &mut found)?,
        Some(rel) => {
            let rel = rel.trim_end_matches('/');
            let path = pass_dir.join(rel);
            let md =
                fs::symlink_metadata(&path).with_context(|| format!("stat {}", path.display()))?;
            if md.is_file() {
                return Ok(vec![rel.to_string()]);
            }
            if md.is_dir() {
                walk(&path, rel, &mut found)?;
            }
        }
    }
    let mut rels: Vec<String> = found.into_iter().map(|(rel, _, _)| rel).collect();
    rels.sort();
    Ok(rels)
}

/// Sort key for a pass, ascending from oldest: `undated` before every dated
/// day, then the day, then the time and `-N` suffix compared numerically.
/// Both are validated digit strings (fixed-width HHMMSS, or unix secs and an
/// N without leading zeros), so `(len, digits)` is numeric order.
fn pass_key(p: &TrashPass) -> (bool, &str, (usize, &str), (usize, &str)) {
    let (time, n) = p.pass.split_once('-').unwrap_or((&p.pass, ""));
    (p.day != "undated", &p.day, (time.len(), time), (n.len(), n))
}

/// Compare a trash copy with `<root>/<rel>` without following any symlink.
/// Every failure is `Unknown`: one unreadable file never fails the listing.
fn status(root: &Path, rel: &str, trash: &Path, bytes: u64, cap: u64) -> TrashStatus {
    let parts: Vec<&str> = rel.split('/').collect();
    let mut live = root.to_path_buf();
    for (i, part) in parts.iter().enumerate() {
        live.push(part);
        let md = match fs::symlink_metadata(&live) {
            Ok(md) => md,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return TrashStatus::NotInProject,
            Err(_) => return TrashStatus::Unknown,
        };
        let last = i + 1 == parts.len();
        if md.file_type().is_symlink() || (last && !md.is_file()) || (!last && !md.is_dir()) {
            return TrashStatus::Unknown;
        }
        if last && md.len() != bytes {
            return TrashStatus::Differs;
        }
    }
    if bytes > cap {
        return TrashStatus::Unknown;
    }
    match (fs::read(&live), fs::read(trash)) {
        (Ok(a), Ok(b)) if a == b => TrashStatus::Same,
        (Ok(_), Ok(_)) => TrashStatus::Differs,
        _ => TrashStatus::Unknown,
    }
}

/// Delete one item from the trash at the user's request: the whole pass
/// when `rel` is `None`, else the file or folder at `rel` inside it. Folders
/// left empty are pruned up to, but never including, `<home>/trash`.
///
/// Holds the home lock throughout so it never races a sync cycle writing
/// trash copies; callers must not already hold it.
pub fn delete(home: &Path, slug: &str, day: &str, pass: &str, rel: Option<&str>) -> Result<()> {
    let _lock = crate::config::lock(home)?;
    let target = match rel {
        None => pass_dir(home, slug, day, pass)?,
        Some(rel) => item_path(home, slug, day, pass, rel)?,
    };
    remove(&target)?;
    let trash = home.join("trash");
    let mut cur = target.parent();
    while let Some(dir) = cur {
        if dir == trash || fs::remove_dir(dir).is_err() {
            break;
        }
        cur = dir.parent();
    }
    Ok(())
}

/// Remove everything under `<home>/trash`, keeping the folder itself. A
/// missing trash is already empty; a symlinked or non-folder one is refused.
///
/// Holds the home lock throughout, like [`delete`].
pub fn empty(home: &Path) -> Result<()> {
    let _lock = crate::config::lock(home)?;
    let trash = home.join("trash");
    match fs::symlink_metadata(&trash) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).with_context(|| format!("stat {}", trash.display())),
        Ok(md) if md.file_type().is_symlink() => bail!("refusing symlink {}", trash.display()),
        Ok(md) if !md.is_dir() => bail!("not a folder: {}", trash.display()),
        Ok(_) => {}
    }
    for entry in fs::read_dir(&trash).with_context(|| format!("read {}", trash.display()))? {
        let entry = entry.with_context(|| format!("read {}", trash.display()))?;
        remove(&entry.path())?;
    }
    Ok(())
}

/// Remove `path` without following it: a folder recursively (std's
/// `remove_dir_all` never follows symlinks), anything else — a symlink
/// included — as a single entry.
fn remove(path: &Path) -> Result<()> {
    let md = fs::symlink_metadata(path).with_context(|| format!("stat {}", path.display()))?;
    if md.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
    .with_context(|| format!("remove {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;
    use tempfile::TempDir;

    #[test]
    fn day_and_pass_formats_are_strict() {
        assert!(valid_day("2026-10-13"));
        assert!(valid_day("undated"));
        for day in ["2026-1-3", "20261013", "2026-10-1x", "..", ""] {
            assert!(!valid_day(day), "{day:?}");
        }

        for pass in ["142501", "142501-2", "142501-10"] {
            assert!(valid_pass("2026-10-13", pass), "{pass:?}");
        }
        for pass in ["1728828301", "1728828301-1", "7"] {
            assert!(valid_pass("undated", pass), "{pass:?}");
        }
        for pass in [
            "14250",
            "1425011",
            "142501-",
            "142501-x",
            "142501-0",
            "142501-01",
            "..",
            "",
        ] {
            assert!(!valid_pass("2026-10-13", pass), "{pass:?}");
        }
        for pass in ["abc", "", "-1", "1728828301-", "1728828301-0"] {
            assert!(!valid_pass("undated", pass), "{pass:?}");
        }
    }

    #[test]
    fn item_paths_reject_escapes_and_symlinks() {
        let td = TempDir::new().unwrap();
        let home = td.path().join("home");
        let pass = home.join("trash/s/2026-10-13/142501");
        fs::create_dir_all(pass.join("docs")).unwrap();
        fs::write(pass.join("docs/a.md"), b"a").unwrap();

        // A valid file and a valid folder are accepted.
        assert_eq!(
            item_path(&home, "s", "2026-10-13", "142501", "docs/a.md").unwrap(),
            pass.join("docs/a.md")
        );
        assert_eq!(
            item_path(&home, "s", "2026-10-13", "142501", "docs").unwrap(),
            pass.join("docs")
        );
        assert_eq!(pass_dir(&home, "s", "2026-10-13", "142501").unwrap(), pass);

        // Escapes and malformed ids.
        for rel in ["../x", "/abs", "a/../b", "./docs", ""] {
            assert!(
                item_path(&home, "s", "2026-10-13", "142501", rel).is_err(),
                "{rel:?}"
            );
        }
        for slug in ["a/b", "..", "", "S"] {
            assert!(
                pass_dir(&home, slug, "2026-10-13", "142501").is_err(),
                "{slug:?}"
            );
        }
        assert!(pass_dir(&home, "s", "..", "142501").is_err());
        assert!(pass_dir(&home, "s", "2026-10-13", "..").is_err());

        // Missing pass or item.
        assert!(pass_dir(&home, "s", "2026-10-13", "142502").is_err());
        assert!(item_path(&home, "s", "2026-10-13", "142501", "docs/b.md").is_err());

        // A symlinked file, and a symlinked folder on the way to one.
        let outside = td.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret"), b"x").unwrap();
        symlink(outside.join("secret"), pass.join("docs/link.md")).unwrap();
        symlink(&outside, pass.join("dirlink")).unwrap();
        assert!(item_path(&home, "s", "2026-10-13", "142501", "docs/link.md").is_err());
        assert!(item_path(&home, "s", "2026-10-13", "142501", "dirlink/secret").is_err());

        // A symlinked pass dir.
        symlink(&outside, home.join("trash/s/2026-10-13/142501-1")).unwrap();
        assert!(pass_dir(&home, "s", "2026-10-13", "142501-1").is_err());

        // A symlinked `<home>/trash`.
        let home2 = td.path().join("home2");
        fs::create_dir_all(&home2).unwrap();
        symlink(home.join("trash"), home2.join("trash")).unwrap();
        assert!(pass_dir(&home2, "s", "2026-10-13", "142501").is_err());
        assert!(item_path(&home2, "s", "2026-10-13", "142501", "docs/a.md").is_err());
    }

    fn root(slug: &str, path: &Path) -> Root {
        Root {
            slug: slug.into(),
            path: path.to_path_buf(),
            initializing: false,
        }
    }

    fn put(path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn rels(p: &TrashPass) -> Vec<&str> {
        p.files.iter().map(|f| f.rel.as_str()).collect()
    }

    fn statuses(projects: &[TrashProject]) -> Vec<(String, Option<TrashStatus>)> {
        projects
            .iter()
            .flat_map(|p| &p.passes)
            .flat_map(|p| &p.files)
            .map(|f| (f.rel.clone(), f.status))
            .collect()
    }

    #[test]
    fn listing_groups_projects_passes_and_files_newest_first() {
        let td = TempDir::new().unwrap();
        let home = td.path().join("home");
        let trash = home.join("trash");
        let live = td.path().join("live");
        fs::create_dir_all(&live).unwrap();

        // Missing trash lists nothing.
        assert_eq!(list(&home, &[], 100).unwrap(), vec![]);

        for pass in [
            "b/2026-10-12/090000",
            "a/undated/999",
            "a/undated/1000",
            "a/2026-10-12/235959",
            "a/2026-10-13/142501",
            "a/2026-10-13/142501-2",
            "a/2026-10-13/142501-10",
            "a/2026-10-13/142501-1",
            "a/2026-10-13/090000",
        ] {
            put(&trash.join(pass).join("x.md"), b"x");
        }
        let pass = trash.join("a/2026-10-13/142501");
        put(&pass.join("z.md"), b"zz");
        put(&pass.join("docs/b.md"), b"bbb");
        put(&pass.join("docs/a.md"), b"aaaa");

        let got = list(&home, &[root("a", &live)], 100).unwrap();
        let slugs: Vec<&str> = got.iter().map(|p| p.slug.as_str()).collect();
        assert_eq!(slugs, ["a", "b"]);
        assert_eq!(got[0].root.as_deref(), Some(live.to_str().unwrap()));
        assert_eq!(got[1].root, None);

        let passes: Vec<(&str, &str)> = got[0]
            .passes
            .iter()
            .map(|p| (p.day.as_str(), p.pass.as_str()))
            .collect();
        assert_eq!(
            passes,
            [
                ("2026-10-13", "142501-10"),
                ("2026-10-13", "142501-2"),
                ("2026-10-13", "142501-1"),
                ("2026-10-13", "142501"),
                ("2026-10-13", "090000"),
                ("2026-10-12", "235959"),
                ("undated", "1000"),
                ("undated", "999"),
            ]
        );

        let p = &got[0].passes[3];
        assert_eq!(p.path, pass.to_str().unwrap());
        assert_eq!(rels(p), ["docs/a.md", "docs/b.md", "x.md", "z.md"]);
        let bytes: Vec<u64> = p.files.iter().map(|f| f.bytes).collect();
        assert_eq!(bytes, [4, 3, 1, 2]);
    }

    #[test]
    fn listing_ignores_unknown_folders_and_never_follows_symlinks() {
        let td = TempDir::new().unwrap();
        let home = td.path().join("home");
        let trash = home.join("trash");
        let outside = td.path().join("outside");
        put(&outside.join("secret.md"), b"s");

        let pass = trash.join("s/2026-10-13/142501");
        put(&pass.join("keep.md"), b"k");
        symlink(&outside, pass.join("dirlink")).unwrap();
        symlink(outside.join("secret.md"), pass.join("link.md")).unwrap();
        put(&trash.join("s/notes.txt"), b"n");
        put(&trash.join("s/2026-1-3/142501/bad-day.md"), b"b");
        put(&trash.join("s/2026-10-13/14250/bad-pass.md"), b"b");
        put(&trash.join("Bad/2026-10-13/142501/bad-slug.md"), b"b");
        fs::create_dir_all(trash.join("s/2026-10-13/142502")).unwrap();
        symlink(&outside, trash.join("s/2026-10-13/142503")).unwrap();
        symlink(&outside, trash.join("s/2026-10-14")).unwrap();
        symlink(&outside, trash.join("t")).unwrap();
        let links_only = trash.join("u/2026-10-13/142501");
        fs::create_dir_all(&links_only).unwrap();
        symlink(outside.join("secret.md"), links_only.join("l.md")).unwrap();

        let got = list(&home, &[], 100).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].slug, "s");
        assert_eq!(got[0].passes.len(), 1);
        assert_eq!(got[0].passes[0].pass, "142501");
        assert_eq!(rels(&got[0].passes[0]), ["keep.md"]);

        // A symlinked `<home>/trash` is refused outright.
        let home2 = td.path().join("home2");
        fs::create_dir_all(&home2).unwrap();
        symlink(&trash, home2.join("trash")).unwrap();
        assert!(list(&home2, &[], 100).is_err());
    }

    #[test]
    fn status_compares_against_the_live_file() {
        let td = TempDir::new().unwrap();
        let home = td.path().join("home");
        let pass = home.join("trash/s/2026-10-13/142501");
        let live = td.path().join("live");
        let outside = td.path().join("outside");
        put(&outside.join("x.md"), b"same");

        for rel in [
            "missing.md",
            "nodir/missing.md",
            "same.md",
            "differs.md",
            "longer.md",
            "dir.md",
            "link.md",
            "linkdir/x.md",
        ] {
            put(&pass.join(rel), b"same");
        }
        put(&live.join("same.md"), b"same");
        put(&live.join("differs.md"), b"diff");
        put(&live.join("longer.md"), b"same, longer");
        fs::create_dir_all(live.join("dir.md")).unwrap();
        symlink(outside.join("x.md"), live.join("link.md")).unwrap();
        symlink(&outside, live.join("linkdir")).unwrap();

        let got = list(&home, &[root("s", &live)], 100).unwrap();
        use TrashStatus::*;
        let want = [
            ("differs.md", Differs),
            ("dir.md", Unknown),
            ("link.md", Unknown),
            ("linkdir/x.md", Unknown),
            ("longer.md", Differs),
            ("missing.md", NotInProject),
            ("nodir/missing.md", NotInProject),
            ("same.md", Same),
        ];
        let want: Vec<(String, Option<TrashStatus>)> = want
            .iter()
            .map(|(r, s)| (r.to_string(), Some(*s)))
            .collect();
        assert_eq!(statuses(&got), want);
    }

    #[test]
    fn status_is_unknown_above_the_cap() {
        let td = TempDir::new().unwrap();
        let home = td.path().join("home");
        let pass = home.join("trash/s/2026-10-13/142501");
        let live = td.path().join("live");
        put(&pass.join("big.md"), b"12345");
        put(&live.join("big.md"), b"12345");
        put(&pass.join("bigger.md"), b"12345");
        put(&live.join("bigger.md"), b"123456");
        put(&pass.join("small.md"), b"12");
        put(&live.join("small.md"), b"12");

        let got = list(&home, &[root("s", &live)], 3).unwrap();
        assert_eq!(
            statuses(&got),
            [
                ("big.md".to_string(), Some(TrashStatus::Unknown)),
                ("bigger.md".to_string(), Some(TrashStatus::Differs)),
                ("small.md".to_string(), Some(TrashStatus::Same)),
            ]
        );
    }

    #[test]
    fn removed_projects_list_without_status() {
        let td = TempDir::new().unwrap();
        let home = td.path().join("home");
        let pass = home.join("trash/gone/2026-10-13/142501");
        let live = td.path().join("live");
        put(&pass.join("a.md"), b"a");
        put(&pass.join("docs/b.md"), b"b");
        put(&live.join("a.md"), b"a");

        // A root under another slug does not adopt the trash of `gone`.
        let got = list(&home, &[root("other", &live)], 100).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].slug, "gone");
        assert_eq!(got[0].root, None);
        assert_eq!(
            statuses(&got),
            [("a.md".to_string(), None), ("docs/b.md".to_string(), None)]
        );
    }

    #[test]
    fn files_under_lists_plain_files_of_a_pass_folder_or_file() {
        let td = TempDir::new().unwrap();
        let pass = td.path().join("pass");
        let outside = td.path().join("outside");
        put(&outside.join("secret.md"), b"s");
        put(&pass.join("z.md"), b"z");
        put(&pass.join("docs/b.md"), b"b");
        put(&pass.join("docs/deep/a.md"), b"a");
        symlink(&outside, pass.join("docs/dirlink")).unwrap();
        symlink(outside.join("secret.md"), pass.join("link.md")).unwrap();

        assert_eq!(
            files_under(&pass, None).unwrap(),
            ["docs/b.md", "docs/deep/a.md", "z.md"]
        );
        for rel in ["docs", "docs/"] {
            assert_eq!(
                files_under(&pass, Some(rel)).unwrap(),
                ["docs/b.md", "docs/deep/a.md"],
                "{rel:?}"
            );
        }
        assert_eq!(files_under(&pass, Some("z.md")).unwrap(), ["z.md"]);
        assert!(files_under(&pass, Some("link.md")).unwrap().is_empty());
        assert!(files_under(&pass, Some("docs/dirlink")).unwrap().is_empty());
    }

    #[test]
    fn delete_removes_a_file_a_folder_or_a_pass_and_prunes_empty_parents() {
        let td = TempDir::new().unwrap();
        let home = td.path().join("home");
        let trash = home.join("trash");
        let day = trash.join("s/2026-10-13");
        let pass = day.join("142501");
        put(&pass.join("docs/deep/a.md"), b"a");
        put(&pass.join("docs/b.md"), b"b");
        put(&pass.join("notes/c.md"), b"c");
        put(&day.join("142502/d.md"), b"d");

        // One file: its now-empty folder goes, the sibling's folder stays.
        delete(&home, "s", "2026-10-13", "142501", Some("docs/deep/a.md")).unwrap();
        assert!(!pass.join("docs/deep").exists());
        assert!(pass.join("docs/b.md").is_file());

        // A folder.
        delete(&home, "s", "2026-10-13", "142501", Some("notes")).unwrap();
        assert!(!pass.join("notes").exists());
        assert!(pass.join("docs/b.md").is_file());

        // A whole pass; the day stays while another pass is in it.
        delete(&home, "s", "2026-10-13", "142501", None).unwrap();
        assert!(!pass.exists());
        assert!(day.join("142502/d.md").is_file());

        // The last pass of the last day prunes the slug, never the trash.
        delete(&home, "s", "2026-10-13", "142502", None).unwrap();
        assert!(!trash.join("s").exists());
        assert!(trash.is_dir());
    }

    #[test]
    fn delete_refuses_paths_outside_the_trash() {
        let td = TempDir::new().unwrap();
        let home = td.path().join("home");
        let trash = home.join("trash");
        let pass = trash.join("s/2026-10-13/142501");
        put(&pass.join("a.md"), b"a");
        let outside = td.path().join("outside");
        put(&outside.join("secret.md"), b"s");
        put(&trash.join("s/2026-10-13/x"), b"x");

        assert!(delete(&home, "s", "2026-10-13", "142501", Some("../x")).is_err());
        assert!(trash.join("s/2026-10-13/x").is_file());
        assert!(delete(&home, "..", "2026-10-13", "142501", None).is_err());
        symlink(&outside, trash.join("s/2026-10-13/142501-1")).unwrap();
        assert!(delete(&home, "s", "2026-10-13", "142501-1", None).is_err());
        assert!(delete(&home, "s", "2026-10-13", "142501-1", Some("secret.md")).is_err());
        assert!(outside.join("secret.md").is_file());
        assert!(pass.join("a.md").is_file());
    }

    #[test]
    fn empty_removes_everything_under_trash_and_nothing_else() {
        let td = TempDir::new().unwrap();
        let home = td.path().join("home");
        let trash = home.join("trash");

        // A missing trash is already empty.
        empty(&home).unwrap();
        assert!(!trash.exists());

        let outside = td.path().join("outside");
        put(&outside.join("secret.md"), b"s");
        put(&home.join("repos/x"), b"x");
        put(&trash.join("s/2026-10-13/142501/a.md"), b"a");
        put(&trash.join("stray.txt"), b"t");
        symlink(outside.join("secret.md"), trash.join("link.md")).unwrap();
        symlink(&outside, trash.join("s/2026-10-13/142501/dirlink")).unwrap();

        empty(&home).unwrap();
        assert!(trash.is_dir());
        assert_eq!(fs::read_dir(&trash).unwrap().count(), 0);
        assert!(home.join("repos/x").is_file());
        assert!(outside.join("secret.md").is_file());
    }

    #[test]
    fn empty_refuses_a_symlinked_trash() {
        let td = TempDir::new().unwrap();
        let home = td.path().join("home");
        let outside = td.path().join("outside");
        put(&outside.join("s/2026-10-13/142501/a.md"), b"a");
        fs::create_dir_all(&home).unwrap();
        symlink(&outside, home.join("trash")).unwrap();

        assert!(empty(&home).is_err());
        assert!(outside.join("s/2026-10-13/142501/a.md").is_file());
        assert!(fs::symlink_metadata(home.join("trash"))
            .unwrap()
            .file_type()
            .is_symlink());
    }
}
