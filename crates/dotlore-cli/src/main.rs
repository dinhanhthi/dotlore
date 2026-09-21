//! The `dotlore` command line front end.
//!
//! This binary is one of the two places that may read the environment:
//! `DOTLORE_HOME` (through [`config::default_home`]) and `$HOME`, both once at
//! startup. Everything below takes them as arguments.
//!
//! The subcommand is parsed *before* an [`Engine`] is built, because `help`
//! and `provider` have to work on a state directory that has never been
//! configured — an `Engine` cannot exist until a provider folder does.

use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, bail, Context, Result};
use dotlore_core::cloud::SlugInfo;
use dotlore_core::config::{self, Config};
use dotlore_core::daemon;
use dotlore_core::engine::{
    self, Engine, EntryKind, EntryView, FileSync, InspectedEntry, ResolutionSnapshot,
    ResolveOutcome, RootStatus, SiblingView, TrackOutcome, TrackedFile,
};
use dotlore_core::git;
use dotlore_core::project;

const HELP: &str = "\
Dotlore — sync your AI config between your own Macs through a cloud folder.

Usage: dotlore <command> [arguments]

  provider <path>               use <path> as the cloud folder
  add <path> [--slug <slug>]    start tracking a project folder
  link <slug> <path>            adopt a slug the cloud already has onto a folder
  slugs                         list the slugs in the cloud folder
  sync                          one sync cycle for every tracked root
  status [--json]               tracked roots and their status, after a sync
  conflicts [<slug>]            unresolved conflicts
  show <slug> <path>            the live bytes, then each conflicting sibling
  resolve <slug> <path> --live | --other [--sibling <path>] | --file <f>
                                keep one version of <path> and discard its
                                other conflicting versions
  recover <slug>                rebuild a damaged staging repo
  track <slug> <rel> [--confirm-folder-bytes <n>]
                                add <rel> to the project's include list
  untrack <slug> <rel>          remove an explicit include-list entry
  entries <slug>                include-list entries and tracked files
  ignore <slug>                 print the root's .dotloreignore path
  project <slug>                print the root's .dotloreproject path
  daemon                        watch and sync until killed
  help                          this text

<path> in conflicts, show and resolve is relative to the tracked root, the way
`conflicts` prints it (for example CLAUDE.md). DOTLORE_HOME overrides the
state directory.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    let home = config::default_home();
    let home_dir = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    match dispatch(&home, &home_dir, &argv) {
        Ok(code) => code,
        Err(e) => {
            report(&mut io::stderr(), &e);
            ExitCode::from(1)
        }
    }
}

/// The only place an [`anyhow::Error`] is ever printed.
///
/// An error chain is not a fixed string: it carries whatever text the layer
/// that failed put in it, and several of those layers name a path taken from
/// a peer device's merged tree (`repo.rs`'s `live_path` bail is one, git's own
/// stderr is another). `run_cycle` turns cycle failures into
/// `RootStatus::Error`, which `show_status` sanitizes, but a `?` outside that
/// catch — `resolve_conflict` resuming a journalled transaction, for instance
/// — arrives here raw. Sanitizing at the two funnel points rather than at
/// whichever call site is known to be reachable today is what keeps the next
/// such path from being a new finding.
///
/// `one_line` drops newlines as well, so a multi-line git stderr collapses
/// into one run-on line. Deliberate: same rule as every other label, and one
/// diagnostic per line is what the exit-1 contract promises.
fn report(out: &mut impl Write, e: &anyhow::Error) {
    let _ = writeln!(out, "dotlore: {}", one_line(&format!("{e:#}")));
}

fn dispatch(home: &Path, home_dir: &Path, argv: &[&str]) -> Result<ExitCode> {
    match argv {
        [] | ["help"] | ["--help"] | ["-h"] => print!("{HELP}"),

        // Neither an Engine nor git: this is what a fresh install runs first.
        ["provider", path] => {
            print_rows(&engine::configure_provider(
                home,
                home_dir,
                Path::new(path),
            )?);
        }

        ["add", path] => println!(
            "{}",
            open_git(home, home_dir)?.add_root(Path::new(path), None)?
        ),
        ["add", path, "--slug", slug] => println!(
            "{}",
            open_git(home, home_dir)?.add_root(Path::new(path), Some(slug))?
        ),
        ["link", slug, path] => {
            let status = open_git(home, home_dir)?.link_root(slug, Path::new(path))?;
            match status {
                RootStatus::Pending => println!("{LINK_PENDING}"),
                st => println!("{slug}\t{}", show_status(&st)),
            }
        }
        ["slugs"] => {
            for info in open(home, home_dir)?.cloud.list_slugs() {
                println!("{}", slug_line(&info));
            }
        }
        ["sync"] => print_rows(&open_git(home, home_dir)?.sync_all()?),
        ["status"] => status(&mut open_git(home, home_dir)?, false)?,
        ["status", "--json"] => status(&mut open_git(home, home_dir)?, true)?,
        ["conflicts"] => conflicts(&mut open_git(home, home_dir)?, None)?,
        ["conflicts", slug] => conflicts(&mut open_git(home, home_dir)?, Some(slug))?,
        ["show", slug, path] => show(&mut open_git(home, home_dir)?, slug, Path::new(path))?,
        ["resolve", slug, path, opts @ ..] => {
            resolve(&mut open_git(home, home_dir)?, slug, Path::new(path), opts)?;
        }
        ["recover", slug] => {
            let st = open_git(home, home_dir)?.recover_root(slug)?;
            println!("{slug}\t{}", show_status(&st));
        }
        ["track", slug, rel, opts @ ..] => {
            return track(
                &mut open_git(home, home_dir)?,
                slug,
                Path::new(rel),
                parse_track(opts)?,
            );
        }
        ["untrack", slug, rel] => {
            untrack(&mut open_git(home, home_dir)?, slug, Path::new(rel))?;
        }
        ["entries", slug] => entries(&mut open_git(home, home_dir)?, slug)?,
        ["ignore", slug] => print_staging_file(home, home_dir, slug, project::IGNORE_FILE)?,
        ["project", slug] => print_staging_file(home, home_dir, slug, project::PROJECT_FILE)?,
        ["daemon"] => run_daemon(open_git(home, home_dir)?),

        _ => {
            eprint!("{HELP}");
            return Ok(ExitCode::from(2));
        }
    }
    Ok(ExitCode::SUCCESS)
}

// --- engine construction ---------------------------------------------------

/// Fresh config under the home lock, then an Engine.
///
/// The lock is released before the Engine is used: every entry point takes it
/// again, and `std::fs::File::lock` is not reentrant.
fn open(home: &Path, home_dir: &Path) -> Result<Engine> {
    let cfg = {
        let _g = config::lock(home)?;
        Config::load(home)?
    };
    Engine::new(home, home_dir, cfg).context("run `dotlore provider <folder>` first")
}

/// Same, for the commands that drive the system git.
fn open_git(home: &Path, home_dir: &Path) -> Result<Engine> {
    if git::which_git().is_none() {
        eprintln!("git not found. Install Xcode Command Line Tools: xcode-select --install");
        std::process::exit(3);
    }
    open(home, home_dir)
}

// --- commands --------------------------------------------------------------

fn status(e: &mut Engine, json: bool) -> Result<()> {
    let rows = e.sync_all()?;
    if !json {
        for (slug, st) in &rows {
            println!(
                "{slug}\t{}\t{}",
                show_status(st),
                root_path(e, slug).display()
            );
        }
        return Ok(());
    }
    let out: Vec<serde_json::Value> = rows
        .iter()
        .map(|(slug, st)| {
            serde_json::json!({
                "slug": slug,
                "path": root_path(e, slug),
                "status": show_status(st),
            })
        })
        .collect();
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

fn conflicts(e: &mut Engine, slug: Option<&str>) -> Result<()> {
    let slugs: Vec<String> = match slug {
        Some(s) => vec![s.to_string()],
        None => e.cfg.roots.iter().map(|r| r.slug.clone()).collect(),
    };
    for slug in slugs {
        for c in e.conflicts(&slug)? {
            let mine = if c.loser_is_me { " — this Mac" } else { "" };
            println!(
                "{slug}\t{}\tfrom {} ({}){mine}",
                one_line(&c.live.display().to_string()),
                one_line(&c.loser_name),
                c.loser_id8
            );
        }
    }
    Ok(())
}

fn show(e: &mut Engine, slug: &str, live: &Path) -> Result<()> {
    let snap = e.open_resolution(slug, live)?;
    let tty = io::stdout().is_terminal();
    let mut out = io::stdout().lock();
    write_content(&mut out, &snap.live_bytes, tty)?;
    let mut prev = snap.live_bytes.as_slice();
    for s in &snap.siblings {
        if !prev.is_empty() && !prev.ends_with(b"\n") {
            out.write_all(b"\n")?;
        }
        writeln!(out, "----- OTHER (from {}) -----", one_line(&s.loser_name))?;
        write_content(&mut out, &s.bytes, tty)?;
        prev = s.bytes.as_slice();
    }
    out.flush()?;
    Ok(())
}

/// Which version of the live path survives the resolution.
#[derive(PartialEq, Eq, Debug)]
enum Choice<'a> {
    Live,
    /// `--sibling <path>`, needed only when one live path has several.
    Other(Option<&'a str>),
    File(&'a str),
}

const EXACTLY_ONE: &str = "resolve needs exactly one of --live, --other, --file <f>";

/// `--live` / `--other` / `--file <f>`, plus `--sibling <path>` to disambiguate.
///
/// Every combination that is not exactly one side is an error rather than a
/// last-flag-wins silent choice: `resolve` discards the versions it did not
/// keep, so guessing which one the user meant is the one thing it must not do.
fn parse_resolve<'a>(opts: &[&'a str]) -> Result<Choice<'a>> {
    let mut chosen: Option<Choice<'a>> = None;
    let mut sibling: Option<&'a str> = None;
    let mut it = opts.iter();
    while let Some(opt) = it.next() {
        let mut value = || {
            it.next()
                .copied()
                .ok_or_else(|| anyhow!("{opt} needs a value"))
        };
        let next = match *opt {
            "--live" => Choice::Live,
            "--other" => Choice::Other(None),
            "--file" => Choice::File(value()?),
            "--sibling" => {
                let s = value()?;
                if sibling.is_some() {
                    bail!("--sibling given twice");
                }
                sibling = Some(s);
                continue;
            }
            other => bail!("unknown option {other}"),
        };
        if chosen.is_some() {
            bail!("{EXACTLY_ONE}");
        }
        chosen = Some(next);
    }
    match (chosen, sibling) {
        (Some(Choice::Other(_)), s) => Ok(Choice::Other(s)),
        (Some(_), Some(_)) => bail!("--sibling only applies with --other"),
        (Some(c), None) => Ok(c),
        (None, _) => bail!("{EXACTLY_ONE}"),
    }
}

/// The snapshot is taken here and handed straight back to `resolve_conflict`,
/// so a version that moved in between is reported rather than overwritten.
fn resolve(e: &mut Engine, slug: &str, live: &Path, opts: &[&str]) -> Result<()> {
    // Before `open_resolution`: a malformed command line should not read the
    // repo at all.
    let choice = parse_resolve(opts)?;

    let snap = e.open_resolution(slug, live)?;
    let content = match choice {
        Choice::Live => snap.live_bytes.clone(),
        Choice::Other(sibling) => pick(&snap, sibling)?.bytes.clone(),
        Choice::File(f) => std::fs::read(f).with_context(|| format!("reading {f}"))?,
    };
    // Resolving a live path clears every sibling of that path; the chosen
    // content is the one survivor.
    let selected: Vec<PathBuf> = snap.siblings.iter().map(|s| s.path.clone()).collect();

    match e.resolve_conflict(slug, &snap, &selected, &content)? {
        ResolveOutcome::Applied(st) => println!("{slug}\t{}", show_status(&st)),
        ResolveOutcome::Stale(_) => bail!(
            "{} changed since it was read; nothing was written, run resolve again",
            live.display()
        ),
        // Reached two ways: an older apply this root never finished (the
        // resolution was not even recorded), or this one wrote some paths and
        // not others. Neither is guaranteed to clear by itself — `status`
        // carries the blocked-path line, which says which case it is.
        ResolveOutcome::Pending => bail!(
            "the resolution for {} is not finished: this root has an apply that could not \
             complete. Run `dotlore status` — an Error line for {slug} names the blocked path \
             and how to clear it; a Pending line means the next sync finishes it",
            live.display()
        ),
    }
    Ok(())
}

/// `--confirm-folder-bytes <n>` after `track <slug> <rel>`.
///
/// The count is the measurement the user is confirming. Engine remeasures on
/// every call, so a folder that grew since the last report needs a new flag.
fn parse_track(opts: &[&str]) -> Result<Option<u64>> {
    let mut confirmed: Option<u64> = None;
    let mut it = opts.iter();
    while let Some(opt) = it.next() {
        match *opt {
            "--confirm-folder-bytes" => {
                let v = it
                    .next()
                    .copied()
                    .ok_or_else(|| anyhow!("--confirm-folder-bytes needs a value"))?;
                if confirmed.is_some() {
                    bail!("--confirm-folder-bytes given twice");
                }
                let n: u64 = v
                    .parse()
                    .map_err(|_| anyhow!("--confirm-folder-bytes needs a byte count"))?;
                confirmed = Some(n);
            }
            other => bail!("unknown option {other}"),
        }
    }
    Ok(confirmed)
}

fn track(e: &mut Engine, slug: &str, rel: &Path, confirmed: Option<u64>) -> Result<ExitCode> {
    match e.track_entry(slug, rel, confirmed)? {
        TrackOutcome::Done(st) => {
            println!("{slug}\t{}", show_status(&st));
            Ok(ExitCode::SUCCESS)
        }
        TrackOutcome::NeedsConfirmation(info) => {
            println!("{}", confirm_line(rel, &info));
            Ok(ExitCode::from(1))
        }
    }
}

fn confirm_line(rel: &Path, info: &InspectedEntry) -> String {
    format!(
        "{} is {} bytes (limit {} bytes); pass --confirm-folder-bytes {} to track",
        rel.display(),
        info.bytes,
        info.folder_limit,
        info.bytes
    )
}

fn untrack(e: &mut Engine, slug: &str, rel: &Path) -> Result<()> {
    let st = e.untrack_entry(slug, rel)?;
    println!("{slug}\t{}", show_status(&st));
    let covering = remaining_coverage(&e.list_entries(slug)?, rel);
    if !covering.is_empty() {
        println!(
            "covered by\t{}",
            covering
                .iter()
                .map(|k| one_line(k))
                .collect::<Vec<_>>()
                .join("\t")
        );
    }
    Ok(())
}

/// Remaining explicit entries that still cover `rel` after an untrack.
///
/// After a successful untrack the key is gone from [`Engine::list_entries`]
/// and a parent no longer covers it (the tombstone punches a hole). Only an
/// exact remaining key — a sibling include that still lists this path in
/// `covering` — is reported.
fn remaining_coverage(entries: &[EntryView], rel: &Path) -> Vec<String> {
    let want = rel_key(rel);
    entries
        .iter()
        .find(|e| same_entry_key(&e.key, &want))
        .map(|e| e.covering.clone())
        .unwrap_or_default()
}

fn rel_key(rel: &Path) -> String {
    rel.to_string_lossy().trim_end_matches('/').to_string()
}

fn same_entry_key(entry: &str, rel: &str) -> bool {
    entry.trim_end_matches('/') == rel
}

fn entries(e: &mut Engine, slug: &str) -> Result<()> {
    write_entries(
        &mut io::stdout(),
        &e.list_entries(slug)?,
        &e.tracked_files(slug)?,
    )?;
    Ok(())
}

fn write_entries(
    out: &mut impl Write,
    entries: &[EntryView],
    files: &[TrackedFile],
) -> io::Result<()> {
    for e in entries {
        write!(
            out,
            "entry\t{}\t{}",
            one_line(&e.key),
            entry_kind_label(e.kind)
        )?;
        if !e.covering.is_empty() {
            write!(
                out,
                "\t{}",
                e.covering
                    .iter()
                    .map(|k| one_line(k))
                    .collect::<Vec<_>>()
                    .join(",")
            )?;
        }
        writeln!(out)?;
    }
    for f in files {
        writeln!(
            out,
            "file\t{}\t{}\t{}",
            one_line(&f.rel),
            f.bytes,
            file_sync_label(f.state)
        )?;
    }
    Ok(())
}

fn entry_kind_label(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::File => "file",
        EntryKind::Directory => "directory",
    }
}

fn file_sync_label(state: FileSync) -> &'static str {
    match state {
        FileSync::Synced => "Synced",
        FileSync::TooLarge => "TooLarge",
        FileSync::Pending => "Pending",
    }
}

fn print_staging_file(home: &Path, home_dir: &Path, slug: &str, name: &str) -> Result<()> {
    let e = open(home, home_dir)?;
    if !e.cfg.roots.iter().any(|r| r.slug == *slug) {
        bail!("no tracked root with slug {slug}");
    }
    println!("{}", staging_file(home, slug, name).display());
    Ok(())
}

fn staging_file(home: &Path, slug: &str, name: &str) -> PathBuf {
    home.join("repos").join(slug).join(name)
}

fn pick<'a>(snap: &'a ResolutionSnapshot, sibling: Option<&str>) -> Result<&'a SiblingView> {
    match (sibling, snap.siblings.as_slice()) {
        (None, []) => bail!("{} has no conflicting sibling", snap.live.display()),
        (None, [one]) => Ok(one),
        (None, several) => bail!(
            "--other is ambiguous, add --sibling <path>: {}",
            several
                .iter()
                .map(|s| one_line(&s.path.display().to_string()))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        (Some(p), _) => snap
            .siblings
            .iter()
            .find(|s| s.path == Path::new(p))
            .ok_or_else(|| anyhow!("{p} is not a sibling of {}", snap.live.display())),
    }
}

fn run_daemon(engine: Engine) {
    let (tx, rx) = mpsc::channel();
    daemon::run(
        Arc::new(Mutex::new(engine)),
        tx,
        rx,
        |status| match status {
            Ok(rows) if rows.is_empty() => println!("cycle: no tracked roots"),
            Ok(rows) => {
                let line: Vec<String> = rows
                    .iter()
                    .map(|(slug, st)| format!("{slug}={}", show_status(st)))
                    .collect();
                println!("cycle: {}", line.join(" "));
            }
            Err(e) => report(&mut io::stderr(), &e),
        },
    );
}

// --- formatting ------------------------------------------------------------

const LINK_PENDING: &str = "\
Pending — cloud has the folder but no data yet; \
the daemon will finish linking automatically";

/// Slug plus its cloud display name, one tab-separated row.
///
/// The name is a peer-written string: `one_line` drops control characters so
/// a `\r` or ANSI sequence cannot hide the slug sitting in the next field.
fn slug_line(info: &SlugInfo) -> String {
    format!("{}\t{}", info.slug, one_line(&info.display_name))
}

/// A name or path that came out of a synced bundle, printed as one field of
/// one line.
///
/// Control characters are dropped rather than escaped: these are labels in
/// tab-separated rows, and a `\r` or an ANSI sequence in one could repaint or
/// hide another row.
fn one_line(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control() && !is_unicode_line_break(*c))
        .collect()
}

/// Line/paragraph separators (Zl/Zp). `char::is_control` is category Cc
/// only, so a U+2028 in a peer-written label still splits a printed row.
fn is_unicode_line_break(c: char) -> bool {
    matches!(c, '\u{2028}' | '\u{2029}')
}

/// Bundle content, which is attacker-controlled all the way down.
///
/// On a terminal every control character except `\n` and `\t` is written as
/// `<U+XXXX>`, so an OSC 52 clipboard write or a title-spoofing sequence in a
/// synced `settings.json` is read, not executed; invalid UTF-8 becomes U+FFFD
/// for the same reason. Redirected or piped output is byte-exact, so
/// `dotlore show … > f` still produces the true file.
fn write_content(out: &mut impl Write, bytes: &[u8], tty: bool) -> io::Result<()> {
    if !tty {
        return out.write_all(bytes);
    }
    for c in String::from_utf8_lossy(bytes).chars() {
        match c {
            '\n' | '\t' => write!(out, "{c}")?,
            c if c.is_control() => write!(out, "<U+{:04X}>", c as u32)?,
            c => write!(out, "{c}")?,
        }
    }
    Ok(())
}

fn print_rows(rows: &[(String, RootStatus)]) {
    for (slug, st) in rows {
        println!("{slug}\t{}", show_status(st));
    }
}

fn show_status(st: &RootStatus) -> String {
    match st {
        RootStatus::Synced => "Synced".to_string(),
        RootStatus::Conflicts(n) => format!("Conflicts({n})"),
        RootStatus::Pending => "Pending".to_string(),
        RootStatus::RootMissing => "RootMissing".to_string(),
        RootStatus::GitMissing => "GitMissing".to_string(),
        // The message embeds paths from the merged worktree, which a peer
        // device's bundle populates: `stalled()` reports a blocked path
        // verbatim. Stripped here rather than in each printer, because this
        // is the one place all seven of them go through.
        RootStatus::Error(e) => format!("Error: {}", one_line(e)),
    }
}

fn root_path(e: &Engine, slug: &str) -> PathBuf {
    e.cfg
        .roots
        .iter()
        .find(|r| r.slug == slug)
        .map(|r| r.path.clone())
        .unwrap_or_default()
}

// --- tests -------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sibling(path: &str) -> SiblingView {
        SiblingView {
            path: PathBuf::from(path),
            blob: "0".repeat(40),
            bytes: format!("from {path}\n").into_bytes(),
            loser_id8: "deadbeef".into(),
            loser_name: "Mac B".into(),
        }
    }

    fn snapshot(paths: &[&str]) -> ResolutionSnapshot {
        ResolutionSnapshot {
            slug: "smoke".into(),
            live: PathBuf::from("CLAUDE.md"),
            head: "1".repeat(40),
            live_blob: "2".repeat(40),
            live_executable: false,
            live_bytes: b"live\n".to_vec(),
            root: None,
            siblings: paths.iter().copied().map(sibling).collect(),
        }
    }

    fn err(r: Result<impl std::fmt::Debug>) -> String {
        format!("{:#}", r.unwrap_err())
    }

    #[test]
    fn resolve_accepts_exactly_one_side() {
        assert_eq!(parse_resolve(&["--live"]).unwrap(), Choice::Live);
        assert_eq!(parse_resolve(&["--other"]).unwrap(), Choice::Other(None));
        assert_eq!(
            parse_resolve(&["--other", "--sibling", "a.conflict-1234abcd.md"]).unwrap(),
            Choice::Other(Some("a.conflict-1234abcd.md"))
        );
        assert_eq!(parse_resolve(&["--file", "f"]).unwrap(), Choice::File("f"));
    }

    /// Last-flag-wins here would silently discard the version the user meant
    /// to keep.
    #[test]
    fn resolve_refuses_two_sides_or_none() {
        assert!(err(parse_resolve(&["--live", "--other"])).contains("exactly one"));
        assert!(err(parse_resolve(&["--other", "--live"])).contains("exactly one"));
        assert!(err(parse_resolve(&["--live", "--file", "f"])).contains("exactly one"));
        assert!(err(parse_resolve(&[])).contains("exactly one"));
        assert!(err(parse_resolve(&["--sibling", "x"])).contains("exactly one"));
    }

    #[test]
    fn resolve_refuses_a_sibling_that_cannot_apply_and_a_flag_without_a_value() {
        assert!(err(parse_resolve(&["--live", "--sibling", "x"])).contains("only applies"));
        assert!(err(parse_resolve(&["--file", "f", "--sibling", "x"])).contains("only applies"));
        assert!(err(parse_resolve(&["--file"])).contains("needs a value"));
        assert!(err(parse_resolve(&["--sibling"])).contains("needs a value"));
        assert!(err(parse_resolve(&["--nope"])).contains("unknown option"));
        // Same rule as two sides: last-flag-wins would discard A silently.
        assert!(err(parse_resolve(&[
            "--other",
            "--sibling",
            "a",
            "--sibling",
            "b"
        ]))
        .contains("--sibling given twice"));
    }

    #[test]
    fn other_is_ambiguous_with_several_siblings_and_exact_with_one() {
        let one = snapshot(&["CLAUDE.conflict-1234abcd.md"]);
        assert_eq!(
            pick(&one, None).unwrap().bytes,
            b"from CLAUDE.conflict-1234abcd.md\n"
        );

        let two = snapshot(&["CLAUDE.conflict-1234abcd.md", "CLAUDE.conflict-5678ef90.md"]);
        let msg = err(pick(&two, None));
        assert!(msg.contains("--other is ambiguous"), "{msg}");
        assert!(msg.contains("CLAUDE.conflict-1234abcd.md"), "{msg}");
        assert!(msg.contains("CLAUDE.conflict-5678ef90.md"), "{msg}");

        assert_eq!(
            pick(&two, Some("CLAUDE.conflict-5678ef90.md"))
                .unwrap()
                .bytes,
            b"from CLAUDE.conflict-5678ef90.md\n"
        );
        assert!(err(pick(&two, Some("nope.md"))).contains("is not a sibling"));
        assert!(err(pick(&snapshot(&[]), None)).contains("no conflicting sibling"));
    }

    /// A synced `settings.json` is attacker-controlled, and `show` is exactly
    /// the command a user runs on it.
    #[test]
    fn show_escapes_terminal_control_sequences_only_on_a_tty() {
        let evil = b"\x1b]52;c;ZXZpbA==\x07ok\ttab\nline\r\x00\n";

        let mut tty = Vec::new();
        write_content(&mut tty, evil, true).unwrap();
        assert_eq!(
            String::from_utf8(tty).unwrap(),
            "<U+001B>]52;c;ZXZpbA==<U+0007>ok\ttab\nline<U+000D><U+0000>\n"
        );

        let mut piped = Vec::new();
        write_content(&mut piped, evil, false).unwrap();
        assert_eq!(piped, evil, "redirected output must stay byte-exact");
    }

    /// Every `?` in `dispatch` ends at `report`, and the error chain can carry
    /// a path out of a peer's merged tree: `repo.rs`'s `live_path` bail
    /// escapes `run_cycle`'s catch through `resolve_conflict`'s
    /// `pending_tx`/`resume`.
    #[test]
    fn an_error_chain_from_a_peers_tree_cannot_repaint_the_terminal() {
        let e = anyhow!("file root: unexpected staging entry \u{1b}]52;c;ZXZpbA==\u{7}x\r")
            .context("resolving smoke");
        let mut out = Vec::new();
        report(&mut out, &e);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "dotlore: resolving smoke: file root: unexpected staging entry ]52;c;ZXZpbA==x\n"
        );
    }

    #[test]
    fn a_label_from_the_cloud_cannot_repaint_a_row() {
        assert_eq!(one_line("\x1b[31mMac\r\nfake\trow"), "[31mMacfakerow");
        // U+2028 / U+2029 are Zl/Zp, not Cc — `char::is_control` misses them.
        assert_eq!(
            one_line("Notes\u{2028}evil-slug\tLooks Safe"),
            "Notesevil-slugLooks Safe"
        );
        assert_eq!(one_line("Notes\u{2029}next"), "Notesnext");
    }

    /// Drop 1 is folder-only: `add` no longer offers a file path.
    #[test]
    fn help_describes_add_as_tracking_a_project_folder() {
        assert!(HELP.contains("start tracking a project folder"), "{HELP}");
        assert!(!HELP.contains("folder or file"), "{HELP}");
    }

    /// `list_slugs` now yields a peer-written display name; `dotlore slugs`
    /// prints it on the same line as the slug.
    #[test]
    fn slugs_print_the_display_name_as_one_line() {
        let info = SlugInfo {
            slug: "notes".into(),
            display_name: "\x1b[31mMy\r\nNotes".into(),
            is_agent: false,
        };
        assert_eq!(slug_line(&info), "notes\t[31mMyNotes");
    }

    #[test]
    fn link_pending_says_folder() {
        assert!(LINK_PENDING.contains("folder"), "{LINK_PENDING}");
        assert!(!LINK_PENDING.contains("folder or file"), "{LINK_PENDING}");
    }

    #[test]
    fn help_mentions_track_untrack_entries_and_project() {
        for needle in [
            "track <slug> <rel>",
            "--confirm-folder-bytes",
            "untrack <slug> <rel>",
            "entries <slug>",
            "project <slug>",
            ".dotloreignore",
            ".dotloreproject",
        ] {
            assert!(HELP.contains(needle), "HELP missing {needle:?}:\n{HELP}");
        }
    }

    #[test]
    fn parse_track_reads_the_optional_confirm_flag() {
        assert_eq!(parse_track(&[]).unwrap(), None);
        assert_eq!(
            parse_track(&["--confirm-folder-bytes", "1200000"]).unwrap(),
            Some(1_200_000)
        );
    }

    #[test]
    fn parse_track_refuses_a_flag_without_a_value_or_a_bad_count() {
        assert!(err(parse_track(&["--confirm-folder-bytes"])).contains("needs a value"));
        assert!(err(parse_track(&["--confirm-folder-bytes", "nope"])).contains("byte count"));
        assert!(err(parse_track(&["--nope"])).contains("unknown option"));
        assert!(err(parse_track(&[
            "--confirm-folder-bytes",
            "1",
            "--confirm-folder-bytes",
            "2"
        ]))
        .contains("given twice"));
    }

    #[test]
    fn staging_file_paths_sit_under_the_slug_repo() {
        let home = Path::new("/tmp/dotlore-home");
        assert_eq!(
            staging_file(home, "notes", project::IGNORE_FILE),
            Path::new("/tmp/dotlore-home/repos/notes/.dotloreignore")
        );
        assert_eq!(
            staging_file(home, "notes", project::PROJECT_FILE),
            Path::new("/tmp/dotlore-home/repos/notes/.dotloreproject")
        );
    }

    #[test]
    fn entries_print_explicit_keys_and_file_sizes() {
        let entries = vec![
            EntryView {
                key: "CLAUDE.md".into(),
                kind: EntryKind::File,
                covering: vec![],
            },
            EntryView {
                key: "docs/".into(),
                kind: EntryKind::Directory,
                covering: vec![],
            },
            EntryView {
                key: "docs/readme.md".into(),
                kind: EntryKind::File,
                covering: vec!["docs/".into()],
            },
        ];
        let files = vec![
            TrackedFile {
                rel: "CLAUDE.md".into(),
                bytes: 4,
                state: FileSync::Synced,
            },
            TrackedFile {
                rel: "docs/huge.bin".into(),
                bytes: 9,
                state: FileSync::TooLarge,
            },
        ];
        let mut out = Vec::new();
        write_entries(&mut out, &entries, &files).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("entry\tCLAUDE.md\tfile\n"), "{text}");
        assert!(text.contains("entry\tdocs/\tdirectory\n"), "{text}");
        assert!(
            text.contains("entry\tdocs/readme.md\tfile\tdocs/\n"),
            "{text}"
        );
        assert!(text.contains("file\tCLAUDE.md\t4\tSynced\n"), "{text}");
        assert!(
            text.contains("file\tdocs/huge.bin\t9\tTooLarge\n"),
            "{text}"
        );
    }

    #[test]
    fn untrack_reports_remaining_overlapping_coverage() {
        let entries = vec![EntryView {
            key: "docs/".into(),
            kind: EntryKind::Directory,
            covering: vec![],
        }];
        assert!(remaining_coverage(&entries, Path::new("docs/readme.md")).is_empty());
        assert!(remaining_coverage(&entries, Path::new("notes.md")).is_empty());
    }

    #[test]
    fn confirm_line_names_measured_bytes_and_the_limit() {
        let info = InspectedEntry {
            kind: EntryKind::Directory,
            bytes: 1_200_000,
            folder_limit: 1_048_576,
            confirmation_required: true,
            skipped_too_large: vec![],
        };
        let line = confirm_line(Path::new("docs"), &info);
        assert!(line.contains("1200000"), "{line}");
        assert!(line.contains("1048576"), "{line}");
        assert!(line.contains("--confirm-folder-bytes"), "{line}");
    }

    /// `track` without `--confirm-folder-bytes` must not write the include-list
    /// when the folder is over the seed limit. Uses a real Engine so the CLI
    /// path and Engine remeasure stay in lockstep.
    #[test]
    fn track_without_confirm_does_not_mutate() {
        if git::which_git().is_none() {
            return;
        }
        let scratch = Scratch::new();
        let home = scratch.0.join("home");
        let root = scratch.0.join("root");
        let provider = scratch.0.join("provider");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("CLAUDE.md"), b"one\n").unwrap();
        std::fs::create_dir_all(root.join("notes")).unwrap();
        std::fs::write(root.join("notes/a.md"), b"aaaa\n").unwrap();

        let cfg = Config {
            device_id: "a".repeat(32),
            device_name: "Mac A Pro".into(),
            provider_dir: Some(provider),
            max_seed_folder_mb: Some(0),
            ..Default::default()
        };
        cfg.save(&home).unwrap();
        let mut engine = Engine::new(&home, &home, cfg).unwrap();
        engine.add_root(&root, Some("demo")).unwrap();

        let before = engine.list_entries("demo").unwrap();
        assert!(!before.iter().any(|e| e.key == "notes/"), "{before:?}");

        let confirmed = parse_track(&[]).unwrap();
        match engine
            .track_entry("demo", Path::new("notes"), confirmed)
            .unwrap()
        {
            TrackOutcome::NeedsConfirmation(info) => {
                let line = confirm_line(Path::new("notes"), &info);
                assert!(line.contains(&info.bytes.to_string()), "{line}");
                assert!(line.contains(&info.folder_limit.to_string()), "{line}");
            }
            TrackOutcome::Done(st) => panic!("expected confirmation, got {st:?}"),
        }
        assert_eq!(engine.list_entries("demo").unwrap(), before);
    }

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!(
                "dotlore-cli-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// `RootStatus::Error` carries the paths `stalled()` could not write, and
    /// a peer's bundle decides what those are. `dotlore daemon` prints the
    /// status every cycle with no user action at all.
    #[test]
    fn a_blocked_path_from_the_cloud_cannot_repaint_a_status_line() {
        let st = RootStatus::Error(
            "cannot write \u{1b}]52;c;ZXZpbA==\u{7}innocuous.txt\r in the live root".into(),
        );
        assert_eq!(
            show_status(&st),
            "Error: cannot write ]52;c;ZXZpbA==innocuous.txt in the live root"
        );
    }
}
