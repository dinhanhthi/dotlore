//! Two-device (and three-device) integration harness.
//!
//! Every test drives real `Engine` instances over real temp directories and a
//! real `git`. Each device owns its own provider TempDir — its private view of
//! the shared cloud folder — and [`sync_cloud`] copies files between views, so
//! a test controls exactly when another device's bundles become visible.
//!
//! The load-bearing assertion in almost every test is not a return value: it
//! is that two roots end up byte-identical, that no version of the user's
//! bytes was dropped, and that [`dance`] reaches a fixpoint. A `dance` that
//! hits its round cap means the devices are merging each other forever, which
//! is the failure mode the whole design exists to prevent.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use dotlore_core::cloud::Cloud;
use dotlore_core::config::Config;
use dotlore_core::engine::{configure_provider, Engine, FileSync, ResolveOutcome, RootStatus};
use dotlore_core::git::Git;
use dotlore_core::project::{self, PROJECT_FILE};
use dotlore_core::repo::{provider_key, remote_ref, FetchMode, Repo, Transaction};
use tempfile::TempDir;

const SLUG: &str = "proj-claude";

/// Arbitrary filenames the harness writes. Production defaults would leave
/// most of these untracked; each test that adds a file after `add_root`
/// must also `track_entry` it.
const TEST_PATTERNS: &[&str] = &[
    "CLAUDE.md",
    "agents/",
    "icon.png",
    "docs/",
    "from-a.md",
    "from-b.md",
    ".gitattributes",
    "crlf.md",
    "extra.md",
    "fresh.md",
    "sub/",
    "hooks/",
    ".gitignore",
    "unrelated.md",
    "notes.md",
    "settings.json",
    "settings.md",
];

// --- harness ---------------------------------------------------------------

struct Device {
    home: TempDir,
    root: TempDir,
    provider: TempDir,
    engine: Engine,
}

impl Device {
    fn new(letter: char) -> Device {
        let home = TempDir::new().unwrap();
        let root = TempDir::new().unwrap();
        let provider = TempDir::new().unwrap();
        let cfg = Config {
            device_id: letter.to_ascii_lowercase().to_string().repeat(32),
            // Deliberately not derived from the id, and not slug-shaped: any
            // code that confuses the display name with the identity shows up.
            device_name: format!("Mac {letter} Pro"),
            provider_dir: Some(provider.path().to_path_buf()),
            roots: Vec::new(),
            default_patterns: Some(TEST_PATTERNS.iter().map(|s| (*s).to_string()).collect()),
            ..Default::default()
        };
        // Must hit disk before the Engine exists: every public method reloads
        // config under the home lock, and `Config::load` on a missing file
        // mints a *random* identity and saves it over this one.
        cfg.save(home.path()).unwrap();
        let cfg = Config::load(home.path()).unwrap();
        let engine = Engine::new(home.path(), home.path(), cfg).unwrap();
        Device {
            home,
            root,
            provider,
            engine,
        }
    }

    fn id(&self) -> String {
        self.engine.cfg.device_id.clone()
    }

    fn id8(&self) -> String {
        self.engine.cfg.device_id[..8].to_string()
    }

    /// This device's private view of the shared cloud folder.
    fn cloud_dir(&self) -> PathBuf {
        self.provider.path().join("dotlore")
    }

    fn staging(&self, slug: &str) -> PathBuf {
        self.home.path().join("repos").join(slug)
    }

    fn git(&self, slug: &str) -> Git {
        Git::new(
            self.staging(slug),
            &self.engine.cfg.device_name,
            &self.engine.cfg.device_id,
        )
    }

    /// `refs/remotes/<provider-key>/<device>/main` as this device records it.
    ///
    /// The key comes from the canonicalized provider path (`/private/var/...`
    /// on macOS, not `/var/...`); building it from the raw config path yields
    /// a ref name that silently never matches.
    fn remote_head(&self, slug: &str, device: &str) -> Option<String> {
        let r = remote_ref(&provider_key(&self.engine.cloud), device);
        self.git(slug).rev(&r)
    }
}

/// Copy every file one device's cloud view has and another's does not.
///
/// Never overwrites: the cloud is immutable, and a test that renames a bundle
/// aside must be able to rely on it staying aside until it says otherwise.
fn sync_cloud(devs: &[&Device]) {
    for (i, from) in devs.iter().enumerate() {
        let files = cloud_files(&from.cloud_dir());
        for (j, to) in devs.iter().enumerate() {
            if i == j {
                continue;
            }
            for rel in &files {
                let dst = to.cloud_dir().join(rel);
                if dst.exists() {
                    continue;
                }
                fs::create_dir_all(dst.parent().unwrap()).unwrap();
                fs::copy(from.cloud_dir().join(rel), &dst).unwrap();
            }
        }
    }
}

/// Relative paths of every non-dot regular file under a cloud view.
///
/// Dot names are skipped on purpose: `.NNNNNN.bundle.icloud` eviction stubs
/// are per-view state a test installs, never something to propagate.
fn cloud_files(base: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect_cloud(base, Path::new(""), &mut out);
    out
}

fn collect_cloud(dir: &Path, prefix: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let rel = prefix.join(&name);
        if e.path().is_dir() {
            collect_cloud(&e.path(), &rel, out);
        } else {
            out.push(rel);
        }
    }
}

fn write(dev: &Device, rel: &str, bytes: &[u8]) {
    let p = dev.root.path().join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(&p, bytes).unwrap();
}

fn read(dev: &Device, rel: &str) -> Vec<u8> {
    fs::read(dev.root.path().join(rel))
        .unwrap_or_else(|e| panic!("reading {rel} in {}: {e}", dev.root.path().display()))
}

fn text(dev: &Device, rel: &str) -> String {
    String::from_utf8(read(dev, rel)).unwrap()
}

#[derive(PartialEq, Eq, Debug)]
enum Entry {
    File(Vec<u8>),
    Link(PathBuf),
}

fn collect(dir: &Path) -> BTreeMap<PathBuf, Entry> {
    let mut out = BTreeMap::new();
    walk(dir, Path::new(""), &mut out);
    out
}

fn walk(dir: &Path, prefix: &Path, out: &mut BTreeMap<PathBuf, Entry>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name();
        if name == ".DS_Store" {
            continue;
        }
        let rel = prefix.join(&name);
        let p = e.path();
        let md = fs::symlink_metadata(&p).unwrap();
        if md.file_type().is_symlink() {
            out.insert(rel, Entry::Link(fs::read_link(&p).unwrap()));
        } else if md.is_dir() {
            walk(&p, &rel, out);
        } else {
            out.insert(rel, Entry::File(fs::read(&p).unwrap()));
        }
    }
}

/// Recursive byte comparison of two live roots, ignoring `.DS_Store`.
fn assert_dir_eq(a: &Path, b: &Path) {
    let (ma, mb) = (collect(a), collect(b));
    let ka: Vec<_> = ma.keys().collect();
    let kb: Vec<_> = mb.keys().collect();
    assert_eq!(
        ka,
        kb,
        "different file sets:\n  {} -> {ka:?}\n  {} -> {kb:?}",
        a.display(),
        b.display()
    );
    for (rel, va) in &ma {
        let vb = &mb[rel];
        assert!(
            va == vb,
            "{} differs between {} and {}: {} vs {}",
            rel.display(),
            a.display(),
            b.display(),
            describe(va),
            describe(vb)
        );
    }
}

fn describe(e: &Entry) -> String {
    match e {
        Entry::Link(t) => format!("symlink -> {}", t.display()),
        Entry::File(b) => match std::str::from_utf8(b) {
            Ok(s) if s.len() < 200 => format!("{s:?}"),
            _ => format!("{} bytes", b.len()),
        },
    }
}

/// A git conflict marker or a `.conflict-` sibling in a live root is data
/// corruption, not cosmetics.
fn no_markers(root: &Path) {
    for (rel, e) in collect(root) {
        assert!(
            !rel.to_string_lossy().contains(".conflict-"),
            "conflict sibling leaked into the live root: {}",
            rel.display()
        );
        if let Entry::File(bytes) = e {
            assert!(
                !bytes.windows(7).any(|w| w == b"<<<<<<<"),
                "conflict marker in live file {}",
                rel.display()
            );
        }
    }
}

fn bundle_count(dev: &Device) -> usize {
    cloud_files(&dev.cloud_dir())
        .iter()
        .filter(|p| p.extension().is_some_and(|e| e == "bundle"))
        .count()
}

fn head_opt(dev: &Device, slug: &str) -> Option<String> {
    dev.git(slug).rev("refs/heads/main")
}

fn head(dev: &Device, slug: &str) -> String {
    head_opt(dev, slug).unwrap_or_else(|| panic!("{} has no main for {slug}", dev.id8()))
}

type Snap = (usize, Vec<(String, Option<String>)>);

fn snap(dev: &Device) -> Snap {
    let heads = dev
        .engine
        .cfg
        .roots
        .iter()
        .map(|r| (r.slug.clone(), head_opt(dev, &r.slug)))
        .collect();
    (bundle_count(dev), heads)
}

/// Sync every device, propagate the cloud, repeat until nothing moves.
///
/// Reaching the fixpoint *is* the convergence assertion. An `Error` status is
/// itself a fixpoint, so it panics rather than quietly "converging".
fn dance(devs: &mut [&mut Device]) {
    let mut prev: Option<Vec<Snap>> = None;
    for round in 0..8 {
        for d in devs.iter_mut() {
            for (slug, st) in d.engine.sync_all().unwrap() {
                if let RootStatus::Error(e) = st {
                    panic!("round {round}: {slug} errored: {e}");
                }
            }
        }
        let views: Vec<&Device> = devs.iter().map(|d| &**d).collect();
        sync_cloud(&views);
        let now: Vec<Snap> = views.iter().map(|d| snap(d)).collect();
        if prev.as_ref() == Some(&now) {
            return;
        }
        prev = Some(now);
    }
    panic!("dance did not converge in 8 rounds; last state: {prev:?}");
}

fn statuses(dev: &mut Device) -> Vec<(String, RootStatus)> {
    dev.engine.sync_all().unwrap()
}

fn assert_synced(dev: &mut Device) {
    assert_eq!(
        statuses(dev),
        vec![(SLUG.to_string(), RootStatus::Synced)],
        "device {} is not Synced",
        dev.id8()
    );
}

// --- seed content ----------------------------------------------------------

fn numbered(n: usize) -> String {
    (1..=n).map(|i| format!("line {i}\n")).collect()
}

fn with_line(base: &str, n: usize, replacement: &str) -> String {
    let mut lines: Vec<String> = base.lines().map(String::from).collect();
    lines[n - 1] = replacement.to_string();
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

fn edit_line(dev: &Device, rel: &str, n: usize, replacement: &str) {
    let updated = with_line(&text(dev, rel), n, replacement);
    write(dev, rel, updated.as_bytes());
}

fn append(dev: &Device, rel: &str, line: &str) {
    let mut s = text(dev, rel);
    s.push_str(line);
    write(dev, rel, s.as_bytes());
}

/// NUL-containing bytes, so git and `mirror::is_binary` both call it binary.
fn binary(tag: u8) -> Vec<u8> {
    let mut v = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    v.extend_from_slice(&[0, 0, 0, tag, 0, tag, 0, 255, tag, 0]);
    v
}

fn seed(dev: &Device) {
    write(dev, "CLAUDE.md", numbered(30).as_bytes());
    write(dev, "agents/x.md", b"agent x\n");
    write(dev, "icon.png", &binary(1));
}

/// A: seeded root added as `proj-claude`; B: linked onto an empty root.
fn standard_start() -> (Device, Device) {
    let mut a = Device::new('a');
    let mut b = Device::new('b');
    seed(&a);
    assert_eq!(a.engine.add_root(a.root.path(), Some(SLUG)).unwrap(), SLUG);
    sync_cloud(&[&a, &b]);
    assert_eq!(
        b.engine.link_root(SLUG, b.root.path()).unwrap(),
        RootStatus::Synced
    );
    assert_dir_eq(a.root.path(), b.root.path());
    (a, b)
}

/// The converged state of `non_overlapping_edits_merge_silently`.
fn non_overlapping() -> (Device, Device) {
    let (mut a, mut b) = standard_start();
    edit_line(&a, "CLAUDE.md", 1, "line 1 from A");
    edit_line(&b, "CLAUDE.md", 20, "line 20 from B");
    dance(&mut [&mut a, &mut b]);
    (a, b)
}

struct Overlap {
    a: Device,
    b: Device,
    a_version: Vec<u8>,
    b_version: Vec<u8>,
}

/// Both devices change line 5 of `CLAUDE.md`; B's commit is the newer one.
fn overlapping() -> Overlap {
    let (mut a, mut b) = standard_start();
    edit_line(&a, "CLAUDE.md", 5, "line 5 from A");
    edit_line(&b, "CLAUDE.md", 5, "line 5 from B");
    let a_version = read(&a, "CLAUDE.md");
    let b_version = read(&b, "CLAUDE.md");
    // The winner is decided by commit timestamp (1s resolution) and the commit
    // happens inside `sync_all`, not at write time.
    a.engine.sync_all().unwrap();
    std::thread::sleep(Duration::from_secs(2));
    b.engine.sync_all().unwrap();
    dance(&mut [&mut a, &mut b]);
    Overlap {
        a,
        b,
        a_version,
        b_version,
    }
}

// --- tests -----------------------------------------------------------------

/// One tracked edit on A reaches B and both roots report Synced.
#[test]
fn two_devices_smoke_syncs_one_file() {
    let (mut a, mut b) = standard_start();
    edit_line(&a, "CLAUDE.md", 1, "line 1 from A");
    dance(&mut [&mut a, &mut b]);

    assert_eq!(read(&a, "CLAUDE.md"), read(&b, "CLAUDE.md"));
    assert_dir_eq(a.root.path(), b.root.path());
    assert_synced(&mut a);
    assert_synced(&mut b);
}

#[test]
fn non_overlapping_edits_merge_silently() {
    let (mut a, mut b) = non_overlapping();

    assert_dir_eq(a.root.path(), b.root.path());
    let merged = text(&a, "CLAUDE.md");
    assert!(merged.contains("line 1 from A"), "{merged}");
    assert!(merged.contains("line 20 from B"), "{merged}");
    no_markers(a.root.path());
    no_markers(b.root.path());
    assert_synced(&mut a);
    assert_synced(&mut b);

    let before = (
        bundle_count(&a),
        bundle_count(&b),
        head(&a, SLUG),
        head(&b, SLUG),
    );
    dance(&mut [&mut a, &mut b]);
    let after = (
        bundle_count(&a),
        bundle_count(&b),
        head(&a, SLUG),
        head(&b, SLUG),
    );
    assert_eq!(before, after, "converged state is not quiescent");
}

#[test]
fn edits_after_adoption_propagate_both_ways() {
    // One of the two devices has adopted the other's merge commit here; a
    // publish from the adopting side has to reach the other across the forced
    // refspec, or this hangs at the `dance` cap.
    let (mut a, mut b) = non_overlapping();

    append(&a, "CLAUDE.md", "from A after adoption\n");
    dance(&mut [&mut a, &mut b]);
    assert!(text(&b, "CLAUDE.md").contains("from A after adoption"));

    append(&b, "CLAUDE.md", "from B after adoption\n");
    dance(&mut [&mut a, &mut b]);
    assert!(text(&a, "CLAUDE.md").contains("from B after adoption"));

    assert_eq!(head(&a, SLUG), head(&b, SLUG));
    assert_dir_eq(a.root.path(), b.root.path());

    let before = (bundle_count(&a), bundle_count(&b), head(&a, SLUG));
    dance(&mut [&mut a, &mut b]);
    assert_eq!(before, (bundle_count(&a), bundle_count(&b), head(&a, SLUG)));
}

#[test]
fn overlapping_edits_winner_live_loser_sibling_no_data_loss() {
    let Overlap {
        mut a,
        mut b,
        a_version,
        b_version,
    } = overlapping();

    // B committed later, so B's bytes are live on both devices.
    assert_eq!(read(&a, "CLAUDE.md"), b_version);
    assert_eq!(read(&b, "CLAUDE.md"), b_version);
    assert_eq!(
        statuses(&mut a),
        vec![(SLUG.to_string(), RootStatus::Conflicts(1))]
    );
    assert_eq!(
        statuses(&mut b),
        vec![(SLUG.to_string(), RootStatus::Conflicts(1))]
    );

    for (dev, is_me) in [(&mut a, true), (&mut b, false)] {
        let cs = dev.engine.conflicts(SLUG).unwrap();
        assert_eq!(cs.len(), 1, "{cs:?}");
        assert_eq!(cs[0].live, Path::new("CLAUDE.md"));
        assert_eq!(cs[0].loser_id8, "aaaaaaaa");
        assert_eq!(cs[0].loser_name, "Mac a Pro");
        assert_eq!(cs[0].loser_is_me, is_me);
        let (live, sibling) = dev
            .engine
            .conflict_sides(SLUG, &cs[0].live, &cs[0].sibling)
            .unwrap();
        assert_eq!(live, b_version);
        assert_eq!(sibling, a_version, "the losing version was lost");
    }

    assert_eq!(head(&a, SLUG), head(&b, SLUG));
    no_markers(a.root.path());
    no_markers(b.root.path());
    assert_dir_eq(a.root.path(), b.root.path());
}

#[test]
fn converged_state_is_quiescent() {
    let Overlap { mut a, mut b, .. } = overlapping();

    let before = (
        bundle_count(&a),
        bundle_count(&b),
        head(&a, SLUG),
        head(&b, SLUG),
    );
    dance(&mut [&mut a, &mut b]);
    let after = (
        bundle_count(&a),
        bundle_count(&b),
        head(&a, SLUG),
        head(&b, SLUG),
    );
    assert_eq!(before, after);
}

#[test]
fn repeated_conflict_keeps_both_siblings() {
    let Overlap {
        mut a,
        mut b,
        a_version,
        ..
    } = overlapping();

    edit_line(&a, "CLAUDE.md", 5, "line 5 from A again");
    edit_line(&b, "CLAUDE.md", 5, "line 5 from B again");
    let second_a = read(&a, "CLAUDE.md");
    let second_b = read(&b, "CLAUDE.md");
    // No pinned ordering here: the two commits can land in the same second, so
    // this is the one conflict in the file whose winner may be decided by the
    // commit-hash tie-break.
    dance(&mut [&mut a, &mut b]);

    for dev in [&mut a, &mut b] {
        let cs = dev.engine.conflicts(SLUG).unwrap();
        assert_eq!(cs.len(), 2, "expected two siblings, got {cs:?}");
        assert!(cs.iter().all(|c| c.live == Path::new("CLAUDE.md")));
        assert_ne!(cs[0].sibling, cs[1].sibling, "sibling names collided");
        let mut preserved = Vec::new();
        for c in &cs {
            let (_, sibling) = dev
                .engine
                .conflict_sides(SLUG, &c.live, &c.sibling)
                .unwrap();
            preserved.push(sibling);
        }
        assert!(
            preserved.contains(&a_version),
            "the first conflict's losing bytes were overwritten"
        );
        assert!(
            preserved.contains(&second_a) || preserved.contains(&second_b),
            "the second conflict's losing bytes are missing"
        );
    }
    no_markers(a.root.path());
    no_markers(b.root.path());
}

#[test]
fn delete_vs_modify_keeps_modification() {
    let (mut a, mut b) = standard_start();

    fs::remove_file(a.root.path().join("agents/x.md")).unwrap();
    write(&b, "agents/x.md", b"agent x, edited by B\n");
    dance(&mut [&mut a, &mut b]);

    assert_eq!(read(&a, "agents/x.md"), b"agent x, edited by B\n");
    assert_eq!(read(&b, "agents/x.md"), b"agent x, edited by B\n");
    assert_synced(&mut a);
    assert_synced(&mut b);
    assert_dir_eq(a.root.path(), b.root.path());
}

#[test]
fn binary_conflict_newer_wins_loser_preserved() {
    let (mut a, mut b) = standard_start();

    write(&a, "icon.png", &binary(7));
    write(&b, "icon.png", &binary(9));
    a.engine.sync_all().unwrap();
    std::thread::sleep(Duration::from_secs(2));
    b.engine.sync_all().unwrap();
    dance(&mut [&mut a, &mut b]);

    assert_eq!(read(&a, "icon.png"), binary(9));
    assert_eq!(read(&b, "icon.png"), binary(9));
    for dev in [&mut a, &mut b] {
        let cs = dev.engine.conflicts(SLUG).unwrap();
        assert_eq!(cs.len(), 1, "{cs:?}");
        assert_eq!(cs[0].live, Path::new("icon.png"));
        let (live, sibling) = dev
            .engine
            .conflict_sides(SLUG, &cs[0].live, &cs[0].sibling)
            .unwrap();
        assert_eq!(live, binary(9));
        assert_eq!(sibling, binary(7));
    }
    no_markers(a.root.path());
    assert_dir_eq(a.root.path(), b.root.path());
}

#[test]
fn third_device_links_byte_for_byte() {
    let (mut a, mut b) = non_overlapping();
    let mut c = Device::new('c');

    sync_cloud(&[&a, &b, &c]);
    assert_eq!(
        c.engine.link_root(SLUG, c.root.path()).unwrap(),
        RootStatus::Synced
    );
    assert_dir_eq(c.root.path(), a.root.path());

    dance(&mut [&mut a, &mut b, &mut c]);
    assert_dir_eq(c.root.path(), a.root.path());
    let before = (
        bundle_count(&a),
        bundle_count(&b),
        bundle_count(&c),
        head(&a, SLUG),
        head(&c, SLUG),
    );
    dance(&mut [&mut a, &mut b, &mut c]);
    assert_eq!(
        before,
        (
            bundle_count(&a),
            bundle_count(&b),
            bundle_count(&c),
            head(&a, SLUG),
            head(&c, SLUG),
        )
    );
}

#[test]
fn link_onto_non_empty_root_merges() {
    let (mut a, mut b) = non_overlapping();
    let mut c = Device::new('c');
    write(&c, "extra.md", b"only on C\n");
    let c_version = with_line(&numbered(30), 5, "line 5 written only on C");
    write(&c, "CLAUDE.md", c_version.as_bytes());
    let merged_ab = read(&a, "CLAUDE.md");

    sync_cloud(&[&a, &b, &c]);
    c.engine.link_root(SLUG, c.root.path()).unwrap();
    c.engine
        .track_entry(SLUG, Path::new("extra.md"), None)
        .unwrap();
    dance(&mut [&mut a, &mut b, &mut c]);

    for dev in [&a, &b, &c] {
        assert_eq!(read(dev, "extra.md"), b"only on C\n");
        no_markers(dev.root.path());
    }
    assert_dir_eq(a.root.path(), b.root.path());
    assert_dir_eq(a.root.path(), c.root.path());

    for dev in [&mut a, &mut b, &mut c] {
        let cs = dev.engine.conflicts(SLUG).unwrap();
        assert_eq!(cs.len(), 1, "{cs:?}");
        assert_eq!(cs[0].live, Path::new("CLAUDE.md"));
        let (live, sibling) = dev
            .engine
            .conflict_sides(SLUG, &cs[0].live, &cs[0].sibling)
            .unwrap();
        let mut both = [live, sibling];
        both.sort();
        let mut want = [merged_ab.clone(), c_version.clone().into_bytes()];
        want.sort();
        assert_eq!(both, want, "a version went missing on {}", dev.id8());
    }
}

#[test]
fn link_onto_empty_root_creates_no_local_commit() {
    let (a, b) = non_overlapping();
    let mut c = Device::new('c');

    sync_cloud(&[&a, &b, &c]);
    assert_eq!(
        c.engine.link_root(SLUG, c.root.path()).unwrap(),
        RootStatus::Synced
    );

    let log = c.git(SLUG).ok(&["log", "--format=%s"]).unwrap();
    // The adopted history legitimately contains A's and B's `local` commits;
    // what must be absent is one minted by C.
    assert!(
        !log.contains(&format!("local {}", c.id8())),
        "C committed its empty root:\n{log}"
    );
    assert_eq!(head(&c, SLUG), head(&a, SLUG));
    assert!(
        c.staging(SLUG).join(".dotloreignore").is_file(),
        ".dotloreignore did not arrive with the history"
    );
    assert_dir_eq(c.root.path(), a.root.path());
}

fn tracked_rels(dev: &mut Device) -> Vec<String> {
    dev.engine
        .tracked_files(SLUG)
        .unwrap()
        .into_iter()
        .map(|f| f.rel)
        .collect()
}

fn listed_keys(dev: &mut Device) -> Vec<String> {
    dev.engine
        .list_entries(SLUG)
        .unwrap()
        .into_iter()
        .map(|e| e.key)
        .collect()
}

/// Blob ids of user files in `HEAD`, excluding staging-private names.
fn content_blobs(dev: &Device) -> BTreeMap<String, String> {
    let out = dev.git(SLUG).ok(&["ls-tree", "-r", "HEAD"]).unwrap();
    let mut map = BTreeMap::new();
    for line in out.lines() {
        let Some((meta, path)) = line.split_once('\t') else {
            continue;
        };
        if path == PROJECT_FILE || path == project::IGNORE_FILE {
            continue;
        }
        let Some(hash) = meta.split_whitespace().nth(2) else {
            continue;
        };
        map.insert(path.to_string(), hash.to_string());
    }
    map
}

fn name_status(dev: &Device, before: &str, after: &str) -> String {
    dev.git(SLUG)
        .ok(&["diff", "--name-status", before, after])
        .unwrap()
}

fn assert_only_project_file_changed(dev: &Device, before: &str, after: &str) {
    let diff = name_status(dev, before, after);
    let changed: Vec<&str> = diff.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(
        changed.len(),
        1,
        "untrack must publish only {PROJECT_FILE}, got:\n{diff}"
    );
    assert!(
        changed[0].ends_with(PROJECT_FILE),
        "untrack changed something besides {PROJECT_FILE}:\n{diff}"
    );
    assert!(
        !diff.lines().any(|l| l.starts_with('D')),
        "untrack committed a content deletion:\n{diff}"
    );
}

fn assert_no_project_in_live(dev: &Device) {
    let live = collect(dev.root.path());
    assert!(
        !live.keys().any(|p| p.as_os_str() == PROJECT_FILE
            || p.components().any(|c| c.as_os_str() == PROJECT_FILE)),
        "{PROJECT_FILE} leaked into the live root: {:?}",
        live.keys().collect::<Vec<_>>()
    );
    assert!(
        !live.keys().any(|p| p.as_os_str() == project::IGNORE_FILE
            || p.components()
                .any(|c| c.as_os_str() == project::IGNORE_FILE)),
        ".dotloreignore leaked into the live root: {:?}",
        live.keys().collect::<Vec<_>>()
    );
}

/// The include-list is a committed staging-private file: it arrives with the
/// history on link, never appears in a live folder, and a later `track_entry`
/// of a directory that seed missed travels to the peer.
#[test]
fn the_include_list_travels_with_the_history() {
    let (mut a, mut b) = standard_start();

    assert!(
        b.staging(SLUG).join(PROJECT_FILE).is_file(),
        "{PROJECT_FILE} did not arrive with the history"
    );
    assert_no_project_in_live(&a);
    assert_no_project_in_live(&b);

    // `docs/` is in TEST_PATTERNS but was not on disk at add/seed time.
    write(&a, "docs/guide.md", b"guide from A\n");
    write(&a, "docs/deep/x.md", b"deep from A\n");
    a.engine.track_entry(SLUG, Path::new("docs"), None).unwrap();

    dance(&mut [&mut a, &mut b]);

    assert_no_project_in_live(&a);
    assert_no_project_in_live(&b);
    no_markers(a.root.path());
    no_markers(b.root.path());

    let files = b.engine.tracked_files(SLUG).unwrap();
    let rels: Vec<&str> = files.iter().map(|f| f.rel.as_str()).collect();
    assert!(
        rels.contains(&"docs/guide.md") && rels.contains(&"docs/deep/x.md"),
        "B is missing the travelled docs/** paths: {rels:?}"
    );
    for f in &files {
        if f.rel.starts_with("docs/") {
            assert_eq!(f.state, FileSync::Synced, "{}", f.rel);
            let bytes = read(&b, &f.rel);
            assert_eq!(f.bytes, bytes.len() as u64, "{}", f.rel);
        }
    }
    assert_eq!(read(&b, "docs/guide.md"), b"guide from A\n");
    assert_eq!(read(&b, "docs/deep/x.md"), b"deep from A\n");
    assert_eq!(read(&a, "docs/guide.md"), read(&b, "docs/guide.md"));
    assert_eq!(read(&a, "docs/deep/x.md"), read(&b, "docs/deep/x.md"));
    assert!(listed_keys(&mut b).iter().any(|k| k == "docs/"));
}

/// A's tombstone wins a race where B commits a cycle that still carries the
/// older include-list. After `dance` both devices agree `Removed`.
#[test]
fn an_untracked_entry_is_not_resurrected_by_a_peer() {
    let (mut a, mut b) = standard_start();
    write(&a, "notes/one.md", b"notes from A\n");
    a.engine
        .track_entry(SLUG, Path::new("notes"), None)
        .unwrap();
    dance(&mut [&mut a, &mut b]);
    assert!(listed_keys(&mut a).iter().any(|k| k == "notes/"));
    assert!(listed_keys(&mut b).iter().any(|k| k == "notes/"));
    assert_eq!(read(&b, "notes/one.md"), b"notes from A\n");

    assert_eq!(
        a.engine.untrack_entry(SLUG, Path::new("notes")).unwrap(),
        RootStatus::Synced
    );
    assert_eq!(
        project::read(&a.staging(SLUG)).unwrap().entries["notes/"].state,
        project::State::Removed
    );

    // B has not seen A's bundle. A local edit forces B to publish a cycle
    // whose tree still lists `notes/` as Tracked.
    append(&b, "CLAUDE.md", "B raced with the old include-list\n");
    assert_synced(&mut b);
    assert_eq!(
        project::read(&b.staging(SLUG)).unwrap().entries["notes/"].state,
        project::State::Tracked,
        "B must still be on the pre-tombstone include-list before dance"
    );

    dance(&mut [&mut a, &mut b]);

    for (label, dev) in [("A", &mut a), ("B", &mut b)] {
        let file = project::read(&dev.staging(SLUG)).unwrap();
        assert_eq!(
            file.entries["notes/"].state,
            project::State::Removed,
            "{label} did not converge on Removed"
        );
        let keys = listed_keys(dev);
        assert!(
            !keys.iter().any(|k| k == "notes/"),
            "{label} still lists notes/ as tracked: {keys:?}"
        );
        let rels = tracked_rels(dev);
        assert!(
            !rels
                .iter()
                .any(|r| r == "notes/one.md" || r.starts_with("notes/")),
            "{label} still reports notes/** as tracked: {rels:?}"
        );
    }
    assert_eq!(read(&a, "notes/one.md"), b"notes from A\n");
    assert_eq!(read(&b, "notes/one.md"), b"notes from A\n");
    no_markers(a.root.path());
    no_markers(b.root.path());
}

/// Untrack commits a tombstone only: live bytes and content blob ids stay put
/// on both homes. Missing live files and parent/child overlap are separate.
#[test]
fn untrack_preserves_bytes_and_publishes_only_manifest_changes() {
    let (mut a, mut b) = standard_start();
    write(&a, "notes.md", b"keep these bytes\n");
    a.engine
        .track_entry(SLUG, Path::new("notes.md"), None)
        .unwrap();
    dance(&mut [&mut a, &mut b]);

    let live_a = read(&a, "notes.md");
    let live_b = read(&b, "notes.md");
    let blobs_a = content_blobs(&a);
    let blobs_b = content_blobs(&b);
    let notes_blob = a.git(SLUG).ok(&["rev-parse", "HEAD:notes.md"]).unwrap();
    let before = head(&a, SLUG);

    assert_eq!(
        a.engine.untrack_entry(SLUG, Path::new("notes.md")).unwrap(),
        RootStatus::Synced
    );
    let after = head(&a, SLUG);
    assert_ne!(before, after, "untrack must commit the tombstone");
    assert_only_project_file_changed(&a, &before, &after);
    assert_eq!(read(&a, "notes.md"), live_a);
    assert_eq!(
        a.git(SLUG).ok(&["rev-parse", "HEAD:notes.md"]).unwrap(),
        notes_blob
    );
    assert_eq!(content_blobs(&a), blobs_a);

    dance(&mut [&mut a, &mut b]);
    assert_eq!(read(&a, "notes.md"), live_a);
    assert_eq!(read(&b, "notes.md"), live_b);
    assert_eq!(content_blobs(&a), blobs_a);
    assert_eq!(content_blobs(&b), blobs_b);
    assert_eq!(
        a.git(SLUG).ok(&["rev-parse", "HEAD:notes.md"]).unwrap(),
        notes_blob
    );
    assert_eq!(
        b.git(SLUG).ok(&["rev-parse", "HEAD:notes.md"]).unwrap(),
        notes_blob
    );
    assert!(!listed_keys(&mut a).iter().any(|k| k == "notes.md"));
    assert!(!listed_keys(&mut b).iter().any(|k| k == "notes.md"));

    // Missing explicit entry: the live file is already gone; untrack is lexical.
    write(&a, "gone.md", b"was here\n");
    a.engine
        .track_entry(SLUG, Path::new("gone.md"), None)
        .unwrap();
    dance(&mut [&mut a, &mut b]);
    let gone_blob = a.git(SLUG).ok(&["rev-parse", "HEAD:gone.md"]).unwrap();
    let gone_on_b = read(&b, "gone.md");
    fs::remove_file(a.root.path().join("gone.md")).unwrap();

    let before = head(&a, SLUG);
    assert_eq!(
        a.engine.untrack_entry(SLUG, Path::new("gone.md")).unwrap(),
        RootStatus::Synced
    );
    let after = head(&a, SLUG);
    assert_only_project_file_changed(&a, &before, &after);
    assert!(!a.root.path().join("gone.md").exists());
    assert_eq!(
        a.git(SLUG).ok(&["rev-parse", "HEAD:gone.md"]).unwrap(),
        gone_blob
    );

    dance(&mut [&mut a, &mut b]);
    assert!(!a.root.path().join("gone.md").exists());
    assert_eq!(read(&b, "gone.md"), gone_on_b);
    assert_eq!(
        a.git(SLUG).ok(&["rev-parse", "HEAD:gone.md"]).unwrap(),
        gone_blob
    );
    assert_eq!(
        b.git(SLUG).ok(&["rev-parse", "HEAD:gone.md"]).unwrap(),
        gone_blob
    );
    assert!(!listed_keys(&mut a).iter().any(|k| k == "gone.md"));
    assert!(!listed_keys(&mut b).iter().any(|k| k == "gone.md"));

    // Parent + child: untracking the child punches a hole; siblings stay.
    write(&a, "notes/readme.md", b"child\n");
    write(&a, "notes/other.md", b"sibling\n");
    a.engine
        .track_entry(SLUG, Path::new("notes"), None)
        .unwrap();
    a.engine
        .track_entry(SLUG, Path::new("notes/readme.md"), None)
        .unwrap();
    dance(&mut [&mut a, &mut b]);
    let readme_live = read(&a, "notes/readme.md");
    let other_live = read(&a, "notes/other.md");
    let notes_blobs = content_blobs(&a);

    let before = head(&a, SLUG);
    a.engine
        .untrack_entry(SLUG, Path::new("notes/readme.md"))
        .unwrap();
    assert_only_project_file_changed(&a, &before, &head(&a, SLUG));
    assert!(listed_keys(&mut a).iter().any(|k| k == "notes/"));
    assert!(!listed_keys(&mut a).iter().any(|k| k == "notes/readme.md"));
    let rels = tracked_rels(&mut a);
    assert!(
        !rels.iter().any(|r| r == "notes/readme.md"),
        "untracking the child must punch a hole: {rels:?}"
    );
    assert!(rels.iter().any(|r| r == "notes/other.md"));

    dance(&mut [&mut a, &mut b]);
    assert_eq!(read(&a, "notes/readme.md"), readme_live);
    assert_eq!(read(&b, "notes/readme.md"), readme_live);
    assert_eq!(read(&a, "notes/other.md"), other_live);
    assert_eq!(read(&b, "notes/other.md"), other_live);
    assert_eq!(content_blobs(&a), notes_blobs);
    assert_eq!(content_blobs(&b), notes_blobs);
    assert!(listed_keys(&mut b).iter().any(|k| k == "notes/"));
    assert!(!listed_keys(&mut b).iter().any(|k| k == "notes/readme.md"));
    assert!(!tracked_rels(&mut b).iter().any(|r| r == "notes/readme.md"));
    assert!(tracked_rels(&mut b).iter().any(|r| r == "notes/other.md"));

    // Untracking the parent leaves the child entry.
    a.engine
        .track_entry(SLUG, Path::new("notes/readme.md"), None)
        .unwrap();
    dance(&mut [&mut a, &mut b]);
    let notes_blobs = content_blobs(&a);
    let before = head(&a, SLUG);
    a.engine.untrack_entry(SLUG, Path::new("notes")).unwrap();
    assert_only_project_file_changed(&a, &before, &head(&a, SLUG));
    assert!(listed_keys(&mut a).iter().any(|k| k == "notes/readme.md"));
    assert!(!listed_keys(&mut a).iter().any(|k| k == "notes/"));
    let rels = tracked_rels(&mut a);
    assert!(rels.iter().any(|r| r == "notes/readme.md"));
    assert!(
        !rels.iter().any(|r| r == "notes/other.md"),
        "untracking the parent must drop sibling coverage: {rels:?}"
    );

    dance(&mut [&mut a, &mut b]);
    assert_eq!(read(&a, "notes/readme.md"), readme_live);
    assert_eq!(read(&b, "notes/readme.md"), readme_live);
    assert_eq!(read(&a, "notes/other.md"), other_live);
    assert_eq!(read(&b, "notes/other.md"), other_live);
    assert_eq!(content_blobs(&a), notes_blobs);
    assert_eq!(content_blobs(&b), notes_blobs);
    assert!(listed_keys(&mut b).iter().any(|k| k == "notes/readme.md"));
    assert!(!listed_keys(&mut b).iter().any(|k| k == "notes/"));
    assert!(tracked_rels(&mut b).iter().any(|r| r == "notes/readme.md"));
    assert!(!tracked_rels(&mut b).iter().any(|r| r == "notes/other.md"));
    no_markers(a.root.path());
    no_markers(b.root.path());
}

/// A link writes the include-list and nothing else. Extra local files stay;
/// out-of-list peer files — including ones still sitting in the peer's tree
/// after an untrack — must not appear.
#[test]
fn a_fresh_link_writes_only_include_list_entries() {
    let mut a = Device::new('a');
    let mut b = Device::new('b');
    seed(&a);
    write(&a, "secret-on-a.md", b"never tracked\n");
    write(&a, "leftover.md", b"tracked then untracked\n");
    assert_eq!(a.engine.add_root(a.root.path(), Some(SLUG)).unwrap(), SLUG);
    a.engine
        .track_entry(SLUG, Path::new("leftover.md"), None)
        .unwrap();
    a.engine
        .untrack_entry(SLUG, Path::new("leftover.md"))
        .unwrap();
    assert!(
        a.git(SLUG).rev("HEAD:leftover.md").is_some(),
        "untracked leftover.md must remain in the tree so a naive link would resurrect it"
    );

    write(&b, "local-only.md", b"B's extra\n");
    write(&b, "keep-me/private.md", b"B's private\n");
    sync_cloud(&[&a, &b]);
    assert_eq!(
        b.engine.link_root(SLUG, b.root.path()).unwrap(),
        RootStatus::Synced
    );

    dance(&mut [&mut a, &mut b]);

    let a_files = collect(a.root.path());
    let b_files = collect(b.root.path());
    let a_keys: Vec<_> = a_files.keys().cloned().collect();
    let b_keys: Vec<_> = b_files.keys().cloned().collect();
    assert_ne!(a_keys, b_keys, "roots must differ outside the include-list");

    for rel in ["CLAUDE.md", "agents/x.md", "icon.png"] {
        assert!(
            b_files.contains_key(Path::new(rel)),
            "B is missing include-list path {rel}: {b_keys:?}"
        );
        assert_eq!(read(&a, rel), read(&b, rel), "{rel} diverged");
    }

    assert_eq!(read(&b, "local-only.md"), b"B's extra\n");
    assert_eq!(read(&b, "keep-me/private.md"), b"B's private\n");
    assert!(a_files.contains_key(Path::new("secret-on-a.md")));
    assert!(a_files.contains_key(Path::new("leftover.md")));

    for rel in ["secret-on-a.md", "leftover.md"] {
        assert!(
            !b_files.contains_key(Path::new(rel)),
            "out-of-list peer file {rel} appeared on B: {b_keys:?}"
        );
    }
    assert!(!a_files.contains_key(Path::new("local-only.md")));
    assert!(!a_files.contains_key(Path::new("keep-me/private.md")));
    assert_no_project_in_live(&a);
    assert_no_project_in_live(&b);
    no_markers(a.root.path());
    no_markers(b.root.path());
}

/// A `.gitattributes` in the tracked root is mirrored into staging like any
/// other user file, but it must not get a vote on how our blobs are stored.
/// With `* text=auto` acting, `git add` normalises CRLF on check-in: A keeps
/// its own CRLF bytes (its next mirror re-normalises to the same blob, so it
/// never sees a diff) while B writes LF into its live root — two roots that
/// can never converge, and a silent line-ending rewrite on B's side.
#[test]
fn a_root_with_gitattributes_round_trips_crlf_bytes_unchanged() {
    let mut a = Device::new('a');
    let mut b = Device::new('b');
    seed(&a);
    write(&a, ".gitattributes", b"* text=auto\n");
    write(&a, "crlf.md", b"a\r\nb\r\n");
    a.engine.add_root(a.root.path(), Some(SLUG)).unwrap();
    sync_cloud(&[&a, &b]);

    assert_eq!(
        b.engine.link_root(SLUG, b.root.path()).unwrap(),
        RootStatus::Synced
    );
    assert_eq!(
        read(&b, "crlf.md"),
        b"a\r\nb\r\n",
        "the linking device rewrote the line endings"
    );
    assert_dir_eq(a.root.path(), b.root.path());

    // And it stays byte-identical once both sides have edited around it.
    append(&b, "crlf.md", "c\r\n");
    edit_line(&a, "CLAUDE.md", 1, "line 1 from A");
    dance(&mut [&mut a, &mut b]);
    assert_eq!(read(&a, "crlf.md"), b"a\r\nb\r\nc\r\n");
    assert_dir_eq(a.root.path(), b.root.path());
}

/// The attributes guard has to be repaired, not just written once.
///
/// `Repo::init` runs at add/link time and in the recovery branch; every
/// ordinary cycle goes through `Repo::open`. So a staging repo built by a
/// build that predates the guard, or one whose `info` directory an external
/// `git gc`, a manual cleanup or a filesystem repair removed, would keep the
/// CRLF-rewrite hazard open forever unless the open path restores the file.
#[test]
fn a_deleted_info_attributes_is_restored_by_an_ordinary_cycle() {
    let mut a = Device::new('a');
    let mut b = Device::new('b');
    seed(&a);
    write(&a, ".gitattributes", b"* text=auto\n");
    write(&a, "crlf.md", b"a\r\nb\r\n");
    a.engine.add_root(a.root.path(), Some(SLUG)).unwrap();
    sync_cloud(&[&a, &b]);
    assert_eq!(
        b.engine.link_root(SLUG, b.root.path()).unwrap(),
        RootStatus::Synced
    );

    let attrs = |d: &Device| d.staging(SLUG).join(".git").join("info").join("attributes");
    for d in [&a, &b] {
        fs::remove_file(attrs(d)).unwrap();
    }

    // One ordinary cycle each — no add/link/recover, so nothing here reaches
    // `Repo::init`.
    assert_synced(&mut a);
    assert_synced(&mut b);
    for d in [&a, &b] {
        assert!(
            attrs(d).is_file(),
            "{}: the attributes file was not restored",
            d.id8()
        );
    }

    // Restored in substance, not just in name. It has to be a file whose
    // *first* check-in happens after the deletion: git leaves a path the index
    // already holds with CRLF alone even under `text=auto`, so an existing
    // file would round-trip either way and prove nothing. A fresh one is
    // exactly the case a pre-guard staging repo is in.
    write(&b, "fresh.md", b"x\r\ny\r\n");
    b.engine
        .track_entry(SLUG, Path::new("fresh.md"), None)
        .unwrap();
    edit_line(&a, "CLAUDE.md", 1, "line 1 from A");
    dance(&mut [&mut a, &mut b]);
    assert_eq!(
        read(&a, "fresh.md"),
        b"x\r\ny\r\n",
        "the line endings of a newly tracked file were rewritten"
    );
    assert_eq!(read(&b, "fresh.md"), b"x\r\ny\r\n");
    assert_eq!(read(&a, "crlf.md"), b"a\r\nb\r\n");
    assert_dir_eq(a.root.path(), b.root.path());
}

/// A live path the user has symlinked elsewhere is never written through, and
/// never silently retried forever either.
///
/// Symlinking a config file into a personal dotfiles repo is exactly this
/// product's audience. The apply is fail-closed, which is right; the danger is
/// that the skip is reported as `Pending` — the same status the UI shows for
/// "the bundles have not arrived yet" — so the whole root stops committing and
/// publishing with nothing to act on. It must name the path, and clearing the
/// path must let the next sync finish.
#[test]
fn a_symlinked_live_path_is_reported_instead_of_freezing_the_root() {
    let (mut a, mut b) = standard_start();

    // A points a not-yet-existing tracked path at a file of their own.
    let elsewhere = a.home.path().join("elsewhere.md");
    fs::write(&elsewhere, b"a's own copy\n").unwrap();
    std::os::unix::fs::symlink(&elsewhere, a.root.path().join("agents/new.md")).unwrap();

    // B creates that very file and publishes it.
    write(&b, "agents/new.md", b"new from B\n");
    b.engine.sync_all().unwrap();
    sync_cloud(&[&a, &b]);

    for round in 0..2 {
        match &statuses(&mut a)[0].1 {
            RootStatus::Error(m) => assert!(
                m.contains("agents/new.md"),
                "round {round}: the status must name the offending path: {m}"
            ),
            other => panic!("round {round}: expected Error, got {other:?}"),
        }
    }
    // Fail closed: neither the symlink nor what it points at was touched.
    assert_eq!(
        fs::read_link(a.root.path().join("agents/new.md")).unwrap(),
        elsewhere
    );
    assert_eq!(fs::read(&elsewhere).unwrap(), b"a's own copy\n");

    // Clearing the path un-freezes the root; the transaction was waiting.
    fs::remove_file(a.root.path().join("agents/new.md")).unwrap();
    dance(&mut [&mut a, &mut b]);
    assert_eq!(read(&a, "agents/new.md"), b"new from B\n");
    assert_dir_eq(a.root.path(), b.root.path());
    no_markers(a.root.path());
}

#[test]
fn link_before_any_bundle_is_pending_then_syncs() {
    let mut a = Device::new('a');
    let mut b = Device::new('b');
    seed(&a);
    a.engine.add_root(a.root.path(), Some(SLUG)).unwrap();

    // Only the manifest reaches B: the provider has not delivered the bundles.
    let rel = Path::new(SLUG).join("manifest.json");
    fs::create_dir_all(b.cloud_dir().join(SLUG)).unwrap();
    fs::copy(a.cloud_dir().join(&rel), b.cloud_dir().join(&rel)).unwrap();

    assert_eq!(
        b.engine.link_root(SLUG, b.root.path()).unwrap(),
        RootStatus::Pending
    );
    assert!(
        collect(b.root.path()).is_empty(),
        "a Pending link touched the root"
    );

    sync_cloud(&[&a, &b]);
    assert_eq!(
        statuses(&mut b),
        vec![(SLUG.to_string(), RootStatus::Synced)]
    );
    assert_dir_eq(b.root.path(), a.root.path());
}

#[test]
fn evicted_stub_is_tolerated() {
    let (mut a, mut b) = standard_start();
    edit_line(&a, "CLAUDE.md", 1, "line 1 from A");
    a.engine.sync_all().unwrap();
    sync_cloud(&[&a, &b]);

    let bundle = b
        .cloud_dir()
        .join(SLUG)
        .join("devices")
        .join(a.id())
        .join("000002.bundle");
    assert!(bundle.is_file(), "expected A seq 2 at {}", bundle.display());
    let stub = bundle.with_file_name(".000002.bundle.icloud");
    fs::rename(&bundle, &stub).unwrap();

    let before = b.remote_head(SLUG, &a.id());
    assert_eq!(
        b.engine.sync_root(SLUG).unwrap(),
        RootStatus::Synced,
        "an evicted bundle must not fail the cycle"
    );
    assert_eq!(
        b.remote_head(SLUG, &a.id()),
        before,
        "the remote ref moved despite an unreadable bundle"
    );
    assert_ne!(read(&b, "CLAUDE.md"), read(&a, "CLAUDE.md"));

    fs::rename(&stub, &bundle).unwrap();
    assert_eq!(b.engine.sync_root(SLUG).unwrap(), RootStatus::Synced);
    assert_eq!(read(&b, "CLAUDE.md"), read(&a, "CLAUDE.md"));
    assert_ne!(b.remote_head(SLUG, &a.id()), before);
}

#[test]
fn fetch_reaches_fixpoint_across_devices() {
    // A's seq 2 needs B's commit, and B's seq 1 needs A's: a single pass over
    // one device's chain cannot deliver either.
    let (mut a, mut b) = standard_start();
    edit_line(&b, "CLAUDE.md", 20, "line 20 from B");
    b.engine.sync_all().unwrap();
    sync_cloud(&[&a, &b]);
    a.engine.sync_all().unwrap();
    edit_line(&a, "CLAUDE.md", 1, "line 1 from A");
    a.engine.sync_all().unwrap();
    dance(&mut [&mut a, &mut b]);

    // Without this the test would pass on a single fetch pass and prove
    // nothing: A's chain has to *need* a commit only B's chain carries.
    let (a_pre, a_heads) = bundle_graph(&a, &a.id());
    let (_, b_heads) = bundle_graph(&a, &b.id());
    assert!(
        a_pre
            .iter()
            .any(|p| !a_heads.contains(p) && b_heads.contains(p)),
        "no cross-device prerequisite: one fetch pass would have sufficed\n\
         A prereqs {a_pre:?}\n A heads {a_heads:?}\n B heads {b_heads:?}"
    );

    let mut z = Device::new('z');
    sync_cloud(&[&a, &b, &z]);
    assert_eq!(
        z.engine.link_root(SLUG, z.root.path()).unwrap(),
        RootStatus::Synced
    );

    assert_eq!(head(&z, SLUG), head(&a, SLUG));
    assert!(
        z.remote_head(SLUG, &a.id()).is_some(),
        "A's remote ref is missing in Z's staging"
    );
    assert!(
        z.remote_head(SLUG, &b.id()).is_some(),
        "B's remote ref is missing in Z's staging"
    );
    assert_dir_eq(z.root.path(), a.root.path());
}

#[test]
fn nested_git_repo_inside_root_is_skipped() {
    let (mut a, mut b) = standard_start();

    write(&a, "sub/file.txt", b"nested content\n");
    a.engine.track_entry(SLUG, Path::new("sub"), None).unwrap();
    Git::new(a.root.path().join("sub"), "t", "t")
        .ok(&["init", "-b", "main"])
        .unwrap();
    assert!(a.root.path().join("sub/.git").is_dir());

    dance(&mut [&mut a, &mut b]);

    assert_eq!(read(&b, "sub/file.txt"), b"nested content\n");
    assert!(
        !b.root.path().join("sub/.git").exists(),
        "a nested git repo was mirrored"
    );
}

#[test]
fn executable_bit_and_symlinks() {
    let (mut a, mut b) = standard_start();

    write(&a, "hooks/run.sh", b"#!/bin/sh\necho hi\n");
    a.engine
        .track_entry(SLUG, Path::new("hooks"), None)
        .unwrap();
    let script = a.root.path().join("hooks/run.sh");
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    fs::create_dir_all(a.root.path().join("skills")).unwrap();
    std::os::unix::fs::symlink("../agents", a.root.path().join("skills/link")).unwrap();

    dance(&mut [&mut a, &mut b]);

    let copied = b.root.path().join("hooks/run.sh");
    let mode = fs::symlink_metadata(&copied).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o755, "executable bit was not carried across");
    assert_eq!(read(&b, "hooks/run.sh"), b"#!/bin/sh\necho hi\n");
    assert!(
        fs::symlink_metadata(b.root.path().join("skills/link")).is_err(),
        "a symlink was synced"
    );
}

#[test]
fn missing_root_does_not_commit_deletion() {
    let (mut a, mut b) = standard_start();
    let before = collect(b.root.path());

    let published = bundle_count(&a);
    fs::remove_dir_all(a.root.path()).unwrap();
    assert_eq!(a.engine.sync_root(SLUG).unwrap(), RootStatus::RootMissing);
    assert_eq!(
        bundle_count(&a),
        published,
        "a missing root was published as a mass deletion"
    );

    sync_cloud(&[&a, &b]);
    b.engine.sync_all().unwrap();
    assert_eq!(collect(b.root.path()), before, "B lost files to a deletion");
}

#[test]
fn own_seq_survives_evicted_own_bundle() {
    let (mut a, _b) = standard_start();
    edit_line(&a, "CLAUDE.md", 1, "line 1 from A");
    a.engine.sync_all().unwrap();

    let dir = a.cloud_dir().join(SLUG).join("devices").join(a.id());
    let two = dir.join("000002.bundle");
    assert!(two.is_file());
    fs::rename(&two, dir.join(".000002.bundle.icloud")).unwrap();

    edit_line(&a, "CLAUDE.md", 2, "line 2 from A");
    a.engine.sync_all().unwrap();

    assert!(
        dir.join("000003.bundle").is_file(),
        "expected 000003.bundle in {:?}",
        fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect::<Vec<_>>()
    );
    assert!(!two.exists(), "an evicted bundle name was reused");
}

#[test]
fn same_slug_added_on_both_devices_merges() {
    let mut a = Device::new('a');
    let mut b = Device::new('b');
    write(&a, "from-a.md", b"only A had this\n");
    write(&b, "from-b.md", b"only B had this\n");

    a.engine.add_root(a.root.path(), Some(SLUG)).unwrap();
    b.engine.add_root(b.root.path(), Some(SLUG)).unwrap();
    dance(&mut [&mut a, &mut b]);

    for dev in [&a, &b] {
        assert_eq!(read(dev, "from-a.md"), b"only A had this\n");
        assert_eq!(read(dev, "from-b.md"), b"only B had this\n");
        no_markers(dev.root.path());
    }
    assert_eq!(head(&a, SLUG), head(&b, SLUG));
    assert_dir_eq(a.root.path(), b.root.path());
}

#[test]
fn resolve_conflict_clears_badge_on_both_devices() {
    // Resolving only once the dance has reached quiescence is the point: a
    // resolution published into a still-converging pair is a different (and
    // easier) test.
    let Overlap { mut a, mut b, .. } = overlapping();

    let snapshot = a
        .engine
        .open_resolution(SLUG, Path::new("CLAUDE.md"))
        .unwrap();
    assert_eq!(snapshot.siblings.len(), 1);
    let selected: Vec<PathBuf> = snapshot.siblings.iter().map(|s| s.path.clone()).collect();
    let merged = with_line(&numbered(30), 5, "line 5 merged by hand");

    match a
        .engine
        .resolve_conflict(SLUG, &snapshot, &selected, merged.as_bytes())
        .unwrap()
    {
        ResolveOutcome::Applied(RootStatus::Synced) => {}
        other => panic!("expected Applied(Synced), got {other:?}"),
    }

    dance(&mut [&mut a, &mut b]);

    for dev in [&mut a, &mut b] {
        assert_eq!(
            dev.engine.conflicts(SLUG).unwrap(),
            Vec::new(),
            "the badge is still set on {}",
            dev.id8()
        );
    }
    assert_eq!(text(&a, "CLAUDE.md"), merged);
    assert_eq!(text(&b, "CLAUDE.md"), merged);
    assert_synced(&mut a);
    assert_synced(&mut b);
    no_markers(a.root.path());
    no_markers(b.root.path());
    assert_dir_eq(a.root.path(), b.root.path());

    let before = (bundle_count(&a), bundle_count(&b), head(&a, SLUG));
    dance(&mut [&mut a, &mut b]);
    assert_eq!(before, (bundle_count(&a), bundle_count(&b), head(&a, SLUG)));
}

// --- regression coverage ---------------------------------------------------
//
// The labels are the ones from `docs/plans/.../README.md` (C1/C2, I1–I10).

fn journal_path(dev: &Device, slug: &str) -> PathBuf {
    dev.staging(slug).join(".git").join("dotlore-apply.json")
}

/// (I5) Every rejection happens before a single byte is written.
#[test]
fn invalid_input_is_rejected_before_any_mutation() {
    let mut a = Device::new('a');
    seed(&a);

    for bad in [
        "", "Bad", "a--b", "-a", "a-", "../x", "a/b", "a b", "a_b", ".",
    ] {
        assert!(
            a.engine.add_root(a.root.path(), Some(bad)).is_err(),
            "slug {bad:?} was accepted"
        );
    }
    assert!(a.engine.cfg.roots.is_empty());
    assert!(!a.home.path().join("repos").exists(), "staging was created");
    assert!(!a.cloud_dir().exists(), "the cloud folder was touched");

    // Containment, root -> state home / provider.
    assert!(a
        .engine
        .add_root(a.home.path(), Some("state-home"))
        .is_err());
    assert!(a.engine.add_root(a.provider.path(), Some("prov")).is_err());
    let inside = a.home.path().join("nested");
    fs::create_dir_all(&inside).unwrap();
    assert!(a.engine.add_root(&inside, Some("nested")).is_err());

    // Containment the other way round: the state home inside the root.
    let outer = TempDir::new().unwrap();
    let inner_home = outer.path().join("state");
    let prov = TempDir::new().unwrap();
    let cfg = Config {
        device_id: "d".repeat(32),
        device_name: "Mac D Pro".to_string(),
        provider_dir: Some(prov.path().to_path_buf()),
        roots: Vec::new(),
        ..Default::default()
    };
    cfg.save(&inner_home).unwrap();
    let mut e = Engine::new(&inner_home, &inner_home, Config::load(&inner_home).unwrap()).unwrap();
    assert!(e.add_root(outer.path(), Some("outer")).is_err());

    // A symlinked root is never adopted.
    let link = a.root.path().join("agents-link");
    std::os::unix::fs::symlink("agents", &link).unwrap();
    assert!(a.engine.add_root(&link, Some("linked")).is_err());
    fs::remove_file(&link).unwrap();

    // A file path is refused as a project.
    let file = a.root.path().join("CLAUDE.md");
    assert!(
        a.engine.add_root(&file, Some("as-a-file")).is_err(),
        "a file path was accepted as a project"
    );
    assert!(a.engine.cfg.roots.is_empty());
    assert!(!a.home.path().join("repos").exists(), "staging was created");
    assert!(!a.cloud_dir().exists(), "the cloud folder was touched");

    let mut b = Device::new('b');
    a.engine.add_root(a.root.path(), Some(SLUG)).unwrap();
    sync_cloud(&[&a, &b]);
    let b_file = b.root.path().join("CLAUDE.md");
    fs::write(&b_file, b"local\n").unwrap();
    assert!(
        b.engine.link_root(SLUG, &b_file).is_err(),
        "a file path was accepted as a link target"
    );
    assert!(b.engine.cfg.roots.is_empty());
    assert!(!b.home.path().join("repos").exists());
    assert_eq!(fs::read(&b_file).unwrap(), b"local\n");
}

/// (I6) A copied `.gitignore`, nested ones included, cannot hide a file the
/// mirror selected.
#[test]
fn gitignore_cannot_exclude_mirror_selected_files() {
    let (mut a, mut b) = standard_start();

    write(&a, ".gitignore", b"docs/\n*.png\nagents/\n");
    a.engine
        .track_entry(SLUG, Path::new(".gitignore"), None)
        .unwrap();
    write(&a, "docs/x.md", b"doc x\n");
    a.engine.track_entry(SLUG, Path::new("docs"), None).unwrap();
    write(&a, "docs/.gitignore", b"*\n");
    write(&a, "docs/deep/y.md", b"deep y\n");
    write(&a, "docs/deep/.gitignore", b"**\n");
    dance(&mut [&mut a, &mut b]);

    assert_eq!(read(&b, "docs/x.md"), b"doc x\n");
    assert_eq!(read(&b, "docs/deep/y.md"), b"deep y\n");
    assert_eq!(read(&b, ".gitignore"), b"docs/\n*.png\nagents/\n");
    assert_eq!(read(&b, "icon.png"), binary(1));
    assert_eq!(read(&b, "agents/x.md"), b"agent x\n");
    assert_dir_eq(a.root.path(), b.root.path());
}

/// (C2) Resolving one live file leaves the siblings of a same-stem file with a
/// different extension, and of the same name in another directory, alone.
#[test]
fn resolution_is_isolated_per_exact_live_path() {
    let paths = ["settings.json", "settings.md", "sub/settings.json"];
    let (mut a, mut b) = standard_start();
    for p in paths {
        write(&a, p, b"base\n");
        a.engine.track_entry(SLUG, Path::new(p), None).unwrap();
    }
    dance(&mut [&mut a, &mut b]);

    for p in paths {
        write(&a, p, format!("from A: {p}\n").as_bytes());
        write(&b, p, format!("from B: {p}\n").as_bytes());
    }
    a.engine.sync_all().unwrap();
    std::thread::sleep(Duration::from_secs(2));
    b.engine.sync_all().unwrap();
    dance(&mut [&mut a, &mut b]);
    assert_eq!(a.engine.conflicts(SLUG).unwrap().len(), 3);

    let snapshot = a
        .engine
        .open_resolution(SLUG, Path::new("settings.json"))
        .unwrap();
    assert_eq!(snapshot.siblings.len(), 1);
    let selected: Vec<PathBuf> = snapshot.siblings.iter().map(|s| s.path.clone()).collect();
    match a
        .engine
        .resolve_conflict(SLUG, &snapshot, &selected, b"merged settings.json\n")
        .unwrap()
    {
        ResolveOutcome::Applied(RootStatus::Conflicts(2)) => {}
        other => panic!("expected Applied(Conflicts(2)), got {other:?}"),
    }

    let left: Vec<PathBuf> = a
        .engine
        .conflicts(SLUG)
        .unwrap()
        .into_iter()
        .map(|c| c.live)
        .collect();
    assert_eq!(
        left,
        vec![
            PathBuf::from("settings.md"),
            PathBuf::from("sub/settings.json")
        ]
    );

    dance(&mut [&mut a, &mut b]);
    for dev in [&mut a, &mut b] {
        assert_eq!(dev.engine.conflicts(SLUG).unwrap().len(), 2);
    }
    assert_eq!(read(&b, "settings.json"), b"merged settings.json\n");
    assert_dir_eq(a.root.path(), b.root.path());
}

/// (I1) A public entry point that took the home lock twice would deadlock
/// forever, not fail; the deadline is the only way to see it.
#[test]
fn public_entry_points_finish_under_a_deadline() {
    let (tx, rx) = std::sync::mpsc::channel();
    let h = std::thread::spawn(move || {
        let (mut a, mut b) = standard_start();
        a.engine.sync_all().unwrap();
        b.engine.sync_all().unwrap();
        a.engine.conflicts(SLUG).unwrap();
        a.engine.remove_root(SLUG).unwrap();
        let _ = tx.send(());
    });
    rx.recv_timeout(Duration::from_secs(120))
        .expect("add_root / link_root / sync_all deadlocked on the home lock");
    h.join().unwrap();
}

/// (I4) Two live Engines over one home: the second must not save from the
/// config it was constructed with.
#[test]
fn a_second_engine_does_not_overwrite_fresh_config() {
    let a = Device::new('a');
    seed(&a);
    let other = TempDir::new().unwrap();
    fs::write(other.path().join("notes.md"), b"other root\n").unwrap();

    let load = || Config::load(a.home.path()).unwrap();
    let mut e1 = Engine::new(a.home.path(), a.home.path(), load()).unwrap();
    let mut e2 = Engine::new(a.home.path(), a.home.path(), load()).unwrap();

    e1.add_root(a.root.path(), Some(SLUG)).unwrap();
    // e2 still holds the empty root list it was built with.
    e2.add_root(other.path(), Some("other-root")).unwrap();

    let mut slugs: Vec<String> = load().roots.iter().map(|r| r.slug.clone()).collect();
    slugs.sort();
    assert_eq!(slugs, vec!["other-root".to_string(), SLUG.to_string()]);

    e1.remove_root("other-root").unwrap();
    let slugs: Vec<String> = load().roots.iter().map(|r| r.slug.clone()).collect();
    assert_eq!(slugs, vec![SLUG.to_string()]);
}

/// (I7) A live edit between Open and Save is `Stale`, and deletes nothing.
#[test]
fn a_local_edit_between_open_and_save_is_stale() {
    let Overlap { mut a, .. } = overlapping();
    let snapshot = a
        .engine
        .open_resolution(SLUG, Path::new("CLAUDE.md"))
        .unwrap();
    let selected: Vec<PathBuf> = snapshot.siblings.iter().map(|s| s.path.clone()).collect();

    append(&a, "CLAUDE.md", "typed while the resolver was open\n");

    match a
        .engine
        .resolve_conflict(SLUG, &snapshot, &selected, b"merged\n")
        .unwrap()
    {
        ResolveOutcome::Stale(fresh) => assert_eq!(fresh.siblings.len(), 1),
        other => panic!("expected Stale, got {other:?}"),
    }
    assert_eq!(a.engine.conflicts(SLUG).unwrap().len(), 1);
    assert!(text(&a, "CLAUDE.md").contains("typed while the resolver was open"));
}

/// (I7) HEAD moving between Open and Save is `Stale`, and deletes nothing.
#[test]
fn a_head_change_between_open_and_save_is_stale() {
    let Overlap { mut a, .. } = overlapping();
    let snapshot = a
        .engine
        .open_resolution(SLUG, Path::new("CLAUDE.md"))
        .unwrap();
    let selected: Vec<PathBuf> = snapshot.siblings.iter().map(|s| s.path.clone()).collect();

    write(&a, "unrelated.md", b"a commit that moves HEAD\n");
    a.engine
        .track_entry(SLUG, Path::new("unrelated.md"), None)
        .unwrap();
    assert_ne!(head(&a, SLUG), snapshot.head);

    match a
        .engine
        .resolve_conflict(SLUG, &snapshot, &selected, b"merged\n")
        .unwrap()
    {
        ResolveOutcome::Stale(fresh) => assert_eq!(fresh.siblings.len(), 1),
        other => panic!("expected Stale, got {other:?}"),
    }
    assert_eq!(a.engine.conflicts(SLUG).unwrap().len(), 1);
}

/// (C1) An edit landing after the merge tree was computed but before it was
/// applied: both changes have to survive.
#[test]
fn an_edit_racing_the_merge_keeps_both_changes() {
    let (mut a, mut b) = standard_start();
    edit_line(&b, "CLAUDE.md", 20, "line 20 from B");
    b.engine.sync_all().unwrap();
    sync_cloud(&[&a, &b]);

    // `Transaction::apply` reconciles internally, so an empty skip list is the
    // success case here: what must never happen is the racing edit being
    // silently replaced by the merged tree.
    hand_driven_merge(&a, |a| {
        edit_line(a, "CLAUDE.md", 1, "line 1 from A");
    });
    let raced = text(&a, "CLAUDE.md");
    assert!(
        raced.contains("line 1 from A"),
        "the racing edit was overwritten:\n{raced}"
    );

    dance(&mut [&mut a, &mut b]);
    for dev in [&a, &b] {
        let t = text(dev, "CLAUDE.md");
        assert!(t.contains("line 1 from A"), "lost the local edit:\n{t}");
        assert!(t.contains("line 20 from B"), "lost the remote edit:\n{t}");
    }
    assert_dir_eq(a.root.path(), b.root.path());
    no_markers(a.root.path());
}

/// (C1) A journal left behind by a crash between merge and apply is replayed,
/// not restarted from a fresh mirror.
#[test]
fn an_unfinalized_journal_is_finished_by_the_next_cycle() {
    let (mut a, mut b) = standard_start();
    edit_line(&b, "CLAUDE.md", 20, "line 20 from B");
    b.engine.sync_all().unwrap();
    sync_cloud(&[&a, &b]);
    edit_line(&a, "CLAUDE.md", 1, "line 1 from A");

    // Compute the merge, then stop dead: the journal is the crash.
    crash_after_merge(&a);
    let journal = journal_path(&a, SLUG);
    assert!(journal.is_file(), "no journal was written");
    let head_after_crash = head(&a, SLUG);

    dance(&mut [&mut a, &mut b]);
    assert!(!journal.exists(), "the journal outlived the recovery");
    assert_ne!(head(&a, SLUG), head_after_crash);
    for dev in [&a, &b] {
        let t = text(dev, "CLAUDE.md");
        assert!(t.contains("line 1 from A"), "{t}");
        assert!(t.contains("line 20 from B"), "{t}");
    }
    assert_dir_eq(a.root.path(), b.root.path());
}

/// (I9) Recovery keeps the backup and the config registration.
#[test]
fn recover_root_keeps_backup_and_registration() {
    let (mut a, mut b) = non_overlapping();
    let before = head(&a, SLUG);
    let files = collect(a.root.path());

    assert_eq!(a.engine.recover_root(SLUG).unwrap(), RootStatus::Synced);
    assert_eq!(head(&a, SLUG), before);
    assert_eq!(collect(a.root.path()), files);
    assert_eq!(a.engine.cfg.roots.len(), 1);
    assert_eq!(a.engine.cfg.roots[0].slug, SLUG);
    let backups = fs::read_dir(a.home.path().join("recovery"))
        .unwrap()
        .count();
    assert_eq!(backups, 1, "the verified backup was not kept");

    dance(&mut [&mut a, &mut b]);
    assert_dir_eq(a.root.path(), b.root.path());
}

/// (I9) With the staging repo gone, recovery must import this device's own
/// bundles — normal fetch skips them, and here they are the only history.
#[test]
fn recover_root_imports_own_bundles() {
    let mut a = Device::new('a');
    seed(&a);
    a.engine.add_root(a.root.path(), Some(SLUG)).unwrap();
    edit_line(&a, "CLAUDE.md", 1, "line 1 from A");
    a.engine.sync_all().unwrap();
    let before = head(&a, SLUG);
    let files = collect(a.root.path());

    fs::remove_dir_all(a.staging(SLUG)).unwrap();
    let status = a.engine.recover_root(SLUG).unwrap();
    assert!(
        matches!(status, RootStatus::Synced | RootStatus::Conflicts(_)),
        "{status:?}"
    );
    assert!(
        a.git(SLUG).is_ancestor(&before, "refs/heads/main"),
        "this device's own published history was not recovered"
    );
    assert_eq!(collect(a.root.path()), files, "recovery changed the root");
    assert_eq!(a.engine.cfg.roots.len(), 1);
}

/// (I9) Unreadable local transaction state fails closed: nothing is rebuilt,
/// the backup is kept and the root is untouched.
#[test]
fn recover_root_fails_closed_on_unreadable_journal() {
    let (mut a, _b) = standard_start();
    let files = collect(a.root.path());
    let head_before = head(&a, SLUG);
    fs::write(journal_path(&a, SLUG), b"{ not json").unwrap();

    let err = a.engine.recover_root(SLUG).unwrap_err();
    assert!(
        format!("{err:#}").contains("dotlore-apply.json"),
        "unexpected error: {err:#}"
    );
    assert_eq!(collect(a.root.path()), files);
    assert_eq!(head(&a, SLUG), head_before);
    assert_eq!(a.engine.cfg.roots.len(), 1);
    assert_eq!(
        fs::read_dir(a.home.path().join("recovery"))
            .unwrap()
            .count(),
        1,
        "the backup was not retained"
    );
}

/// (I3) Switching provider leaves HEAD alone, bootstraps the destination
/// completely, keeps each provider's delivery state separate, and merges an
/// already-populated destination.
///
/// A and C both point their config at the same `dest` directory, so that one
/// is genuinely shared and needs no `sync_cloud`; A's own provider TempDir
/// stays the original destination it switches back to.
#[test]
fn provider_switch_bootstraps_and_switches_back() {
    let (mut a, b) = non_overlapping();
    let head_before = head(&a, SLUG);
    let origin = a.provider.path().canonicalize().unwrap();
    let dest = TempDir::new().unwrap();

    // 1. An empty destination: HEAD must not move, and the bootstrap must be
    //    complete enough to link from.
    let out = configure_provider(a.home.path(), a.home.path(), dest.path()).unwrap();
    assert_eq!(out, vec![(SLUG.to_string(), RootStatus::Synced)]);
    assert_eq!(head(&a, SLUG), head_before, "the switch moved HEAD");

    let mut c = Device::new('c');
    configure_provider(c.home.path(), c.home.path(), dest.path()).unwrap();
    assert_eq!(
        c.engine.link_root(SLUG, c.root.path()).unwrap(),
        RootStatus::Synced
    );
    assert_eq!(head(&c, SLUG), head_before);
    assert_dir_eq(c.root.path(), a.root.path());

    // 2. Back to the original provider: HEAD still still where it was, its own
    //    acknowledgement state intact, and the two namespaces separate.
    let out = configure_provider(a.home.path(), a.home.path(), &origin).unwrap();
    assert_eq!(out, vec![(SLUG.to_string(), RootStatus::Synced)]);
    assert_eq!(head(&a, SLUG), head_before, "switching back moved HEAD");
    a.engine.sync_all().unwrap();
    assert_eq!(a.engine.cfg.provider_dir, Some(origin.clone()));
    assert_eq!(head(&a, SLUG), head_before);

    let dest_key = provider_key(&Cloud {
        base: dest.path().canonicalize().unwrap().join("dotlore"),
    });
    let origin_key = provider_key(&Cloud {
        base: origin.join("dotlore"),
    });
    assert!(
        sent_ref(&a, &origin_key).is_some() && sent_ref(&a, &dest_key).is_some(),
        "each provider keeps its own acknowledgement state"
    );
    assert!(
        a.remote_head(SLUG, &b.id()).is_some(),
        "the original provider's remote refs were discarded"
    );
    assert!(
        a.git(SLUG).rev(&remote_ref(&dest_key, &b.id())).is_none(),
        "B's head leaked into the destination namespace"
    );

    // 3. A nonempty destination, now carrying history A has never seen.
    edit_line(&c, "CLAUDE.md", 30, "line 30 from C");
    c.engine.sync_all().unwrap();
    configure_provider(a.home.path(), a.home.path(), dest.path()).unwrap();
    for _ in 0..6 {
        a.engine.sync_all().unwrap();
        c.engine.sync_all().unwrap();
        if head(&a, SLUG) == head(&c, SLUG) {
            break;
        }
    }
    assert_eq!(head(&a, SLUG), head(&c, SLUG));
    assert!(text(&a, "CLAUDE.md").contains("line 30 from C"));
    assert_dir_eq(a.root.path(), c.root.path());
    no_markers(a.root.path());

    // Now that C has actually published, the same device id is recorded in one
    // namespace only, and the two providers have diverged sent refs.
    assert!(
        a.git(SLUG).rev(&remote_ref(&dest_key, &c.id())).is_some(),
        "C's head was not recorded in the destination namespace"
    );
    assert!(
        a.git(SLUG).rev(&remote_ref(&origin_key, &c.id())).is_none(),
        "the destination's head leaked into the original provider's namespace"
    );
    assert_ne!(
        sent_ref(&a, &origin_key),
        sent_ref(&a, &dest_key),
        "the two providers share one acknowledgement state"
    );

    // 4. The spec's actual claim, which "each id appears in one namespace" is
    //    weaker than: the *same* device id carries a different head at each
    //    destination. C publishes into the original provider too, having moved
    //    on since the head the destination namespace recorded for it.
    let c_at_dest = a.git(SLUG).rev(&remote_ref(&dest_key, &c.id())).unwrap();
    configure_provider(c.home.path(), c.home.path(), &origin).unwrap();
    edit_line(&c, "CLAUDE.md", 2, "line 2 from C");
    c.engine.sync_all().unwrap();
    configure_provider(a.home.path(), a.home.path(), &origin).unwrap();
    for _ in 0..6 {
        a.engine.sync_all().unwrap();
        c.engine.sync_all().unwrap();
        if head(&a, SLUG) == head(&c, SLUG) {
            break;
        }
    }
    assert_eq!(head(&a, SLUG), head(&c, SLUG));
    let c_at_origin = a
        .git(SLUG)
        .rev(&remote_ref(&origin_key, &c.id()))
        .expect("C's head was not recorded in the original provider's namespace");
    assert_eq!(
        a.git(SLUG).rev(&remote_ref(&dest_key, &c.id())),
        Some(c_at_dest.clone()),
        "publishing to one provider moved the other's record of the same device"
    );
    assert_ne!(
        c_at_origin, c_at_dest,
        "one device id must keep a separate head per provider namespace"
    );
}

/// `(prerequisites, heads)` over every bundle one device published, read from
/// the plain-text header git writes before the pack.
fn bundle_graph(view: &Device, device: &str) -> (Vec<String>, Vec<String>) {
    let (mut pre, mut heads) = (Vec::new(), Vec::new());
    for rel in cloud_files(&view.cloud_dir()) {
        if rel.extension().is_none_or(|e| e != "bundle") || !rel.to_string_lossy().contains(device)
        {
            continue;
        }
        let raw = fs::read(view.cloud_dir().join(&rel)).unwrap();
        let end = raw.windows(2).position(|w| w == b"\n\n").unwrap();
        for line in String::from_utf8_lossy(&raw[..end]).lines().skip(1) {
            let (target, rest) = match line.strip_prefix('-') {
                Some(rest) => (&mut pre, rest),
                None => (&mut heads, line),
            };
            target.push(rest.split(' ').next().unwrap_or_default().to_string());
        }
    }
    (pre, heads)
}

fn sent_ref(dev: &Device, key: &str) -> Option<String> {
    dev.git(SLUG).rev(&format!("refs/dotlore/sent/{key}"))
}

// --- raw transaction driving (C1) ------------------------------------------

/// Run one cycle by hand up to `set_target`, invoke `between` (an edit that
/// races the apply), then apply.
fn hand_driven_merge(dev: &Device, between: impl FnOnce(&Device)) {
    let (repo, mut tx) = merge_to_target(dev);
    between(dev);
    tx.apply(&repo, dev.home.path()).unwrap();
}

/// Stop right after the merge target is pinned: the journal left on disk is
/// exactly what a crash at that moment leaves behind.
fn crash_after_merge(dev: &Device) {
    let _ = merge_to_target(dev);
}

fn merge_to_target(dev: &Device) -> (Repo, Transaction) {
    let root = dev.engine.cfg.roots[0].clone();
    let repo = Repo::open(
        dev.home.path(),
        &root.slug,
        &root.path,
        &dev.engine.cfg.device_name,
        &dev.engine.cfg.device_id,
    )
    .unwrap();
    let key = provider_key(&dev.engine.cloud);
    repo.commit_local(false, dev.home.path()).unwrap();
    let devices = repo
        .fetch_bundles(&dev.engine.cloud, FetchMode::Normal)
        .unwrap();
    let mut tx = repo
        .begin_tx(&key, "HEAD", dotlore_core::conflict::resolve_index)
        .unwrap();
    for d in &devices {
        repo.merge_remote(&tx, d).unwrap();
    }
    tx.set_target(&repo).unwrap();
    (repo, tx)
}
