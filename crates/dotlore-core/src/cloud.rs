//! Reading and writing immutable per-device bundles in the provider folder.
//!
//! Layout: `<base>/<slug>/manifest.json` and
//! `<base>/<slug>/devices/<device-id>/<seq:06>.bundle`, where `base` is
//! `<provider_dir>/dotlore`. Nothing here is ever rewritten or deleted.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Result};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

/// What a tracked root is: a directory tree or a single file.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Dir,
    File,
}

/// Per-slug metadata, written once by the device that creates the slug.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct Manifest {
    pub slug: String,
    pub kind: Kind,
}

/// One published bundle file.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Bundle {
    pub device: String,
    pub seq: u64,
    pub path: PathBuf,
}

#[derive(Serialize, Deserialize)]
struct DeviceFile {
    name: String,
}

/// The provider folder, rooted at `<provider_dir>/dotlore`.
pub struct Cloud {
    pub base: PathBuf,
}

/// A single plain path component: joining it onto a base can neither escape
/// the base nor (for an absolute string) replace it.
fn plain_name(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && !s.contains('/')
        && !s.contains('\0')
        && Path::new(s).components().count() == 1
}

fn checked(base: &Path, name: &str) -> Result<PathBuf> {
    if !plain_name(name) {
        bail!("unsafe cloud path component {name:?}");
    }
    Ok(base.join(name))
}

impl Cloud {
    pub fn slug_dir(&self, slug: &str) -> Result<PathBuf> {
        checked(&self.base, slug)
    }

    /// Subdirectories of `base` that hold a `manifest.json`, sorted.
    ///
    /// A directory name was written by another device, and every caller
    /// prints it: `dotlore slugs` today, the Phase 5 list next. A name with a
    /// control character is dropped rather than stripped — stripping would
    /// print a slug that exists nowhere, and such a slug can never be linked
    /// anyway (`Repo::open` accepts `[a-z0-9-]` only). This sits with the other
    /// things `list_slugs` already skips silently: no manifest, non-UTF-8.
    pub fn list_slugs(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Ok(entries) = fs::read_dir(&self.base) {
            for e in entries.flatten() {
                if e.path().join("manifest.json").is_file() {
                    if let Some(name) = e
                        .file_name()
                        .to_str()
                        .filter(|n| !n.chars().any(char::is_control))
                    {
                        out.push(name.to_string());
                    }
                }
            }
        }
        out.sort();
        out
    }

    /// A manifest whose `slug` disagrees with the directory it was read from
    /// is rejected: the directory name is the only trustworthy one.
    pub fn read_manifest(&self, slug: &str) -> Option<Manifest> {
        let path = self.slug_dir(slug).ok()?.join("manifest.json");
        read_json_retry::<Manifest>(&path).filter(|m| m.slug == slug)
    }

    /// No-op when the manifest already exists.
    pub fn write_manifest_once(&self, m: &Manifest) -> Result<()> {
        let path = self.slug_dir(&m.slug)?.join("manifest.json");
        write_once(&path, &serde_json::to_vec(m)?)
    }

    /// Every visible bundle, sorted by `(device, seq)`.
    ///
    /// Only names of exactly six ASCII digits plus `.bundle` count. iCloud
    /// dataless stubs (`.<name>.icloud`) trigger a best-effort download and
    /// are skipped for this pass.
    pub fn list_bundles(&self, slug: &str) -> Vec<Bundle> {
        let mut out = Vec::new();
        let devices = match self.slug_dir(slug) {
            Ok(d) => d.join("devices"),
            Err(_) => return out,
        };
        let dev_dirs = match fs::read_dir(&devices) {
            Ok(d) => d,
            Err(_) => return out,
        };
        for dev in dev_dirs.flatten() {
            let dev_dir = dev.path();
            if !dev_dir.is_dir() {
                continue;
            }
            let device = match dev.file_name().to_str() {
                Some(d) => d.to_string(),
                None => continue,
            };
            let files = match fs::read_dir(&dev_dir) {
                Ok(f) => f,
                Err(_) => continue,
            };
            for f in files.flatten() {
                let name = match f.file_name().into_string() {
                    Ok(n) => n,
                    Err(_) => continue,
                };
                if let Some(inner) = name
                    .strip_prefix('.')
                    .and_then(|n| n.strip_suffix(".icloud"))
                {
                    // `"...icloud"` yields `"."` and `"....icloud"` yields
                    // `".."`; every constructed cloud path goes through the
                    // same guard.
                    if plain_name(inner) {
                        download_stub(&dev_dir.join(inner));
                    }
                    continue;
                }
                let stem = match name.strip_suffix(".bundle") {
                    Some(s) => s,
                    None => continue,
                };
                if stem.len() != 6 || !stem.bytes().all(|b| b.is_ascii_digit()) {
                    continue;
                }
                if let Ok(seq) = stem.parse::<u64>() {
                    out.push(Bundle {
                        device: device.clone(),
                        seq,
                        path: f.path(),
                    });
                }
            }
        }
        out.sort_by(|a, b| a.device.cmp(&b.device).then(a.seq.cmp(&b.seq)));
        out
    }

    /// Highest visible seq for `device`, 0 when none. A floor only: the
    /// device's own counter lives in local state.
    pub fn max_seq(&self, slug: &str, device: &str) -> u64 {
        self.list_bundles(slug)
            .iter()
            .filter(|b| b.device == device)
            .map(|b| b.seq)
            .max()
            .unwrap_or(0)
    }

    /// Install `src` as `<seq:06>.bundle` without ever replacing an existing
    /// final bundle. A byte-identical final name is an idempotent retry.
    pub fn publish_bundle(&self, slug: &str, device: &str, seq: u64, src: &Path) -> Result<()> {
        // Past six digits the name no longer matches what `list_bundles`
        // accepts: the bundle would be published but invisible everywhere.
        if seq >= 1_000_000 {
            bail!("bundle seq {seq} exceeds the six-digit name format");
        }
        let dir = checked(&self.slug_dir(slug)?.join("devices"), device)?;
        fs::create_dir_all(&dir)?;
        let name = format!("{seq:06}.bundle");
        let fin = dir.join(&name);
        let tmp = dir.join(tmp_name(&name));

        let res = (|| -> Result<()> {
            let mut r = File::open(src)?;
            // The bundle carries every tracked file, API keys included, into a
            // folder the provider shares; never leave it at the umask default.
            // `create_new` matches `mirror::write_atomic`: O_EXCL refuses to
            // follow a symlink pre-planted at the temp path. `tmp_name` carries
            // pid+nanos, so an AlreadyExists here is never our own leftover and
            // is left to propagate rather than retried.
            let mut w = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&tmp)?;
            io::copy(&mut r, &mut w)?;
            w.sync_all()?;
            drop(w);
            // hard_link never replaces an existing destination.
            match fs::hard_link(&tmp, &fin) {
                Ok(()) => Ok(()),
                Err(_) if fin.exists() => {
                    if fs::read(&tmp)? == fs::read(&fin)? {
                        Ok(())
                    } else {
                        bail!(
                            "bundle {} already exists with different bytes",
                            fin.display()
                        )
                    }
                }
                Err(e) => Err(e.into()),
            }
        })();

        let _ = fs::remove_file(&tmp);
        res
    }

    /// No-op when `devices/<device_id>/device.json` already exists.
    pub fn write_device_name_once(&self, slug: &str, device_id: &str, name: &str) -> Result<()> {
        let path = checked(&self.slug_dir(slug)?.join("devices"), device_id)?.join("device.json");
        let body = serde_json::to_vec(&DeviceFile {
            name: name.to_string(),
        })?;
        write_once(&path, &body)
    }

    pub fn device_name(&self, slug: &str, device_id: &str) -> Option<String> {
        let path = checked(&self.slug_dir(slug).ok()?.join("devices"), device_id)
            .ok()?
            .join("device.json");
        read_json_retry::<DeviceFile>(&path).map(|d| clean_name(&d.name))
    }

    /// Device id → human name, for every device dir with a readable
    /// `device.json`.
    pub fn device_names(&self, slug: &str) -> HashMap<String, String> {
        let mut out = HashMap::new();
        let devices = match self.slug_dir(slug) {
            Ok(d) => d.join("devices"),
            Err(_) => return out,
        };
        if let Ok(entries) = fs::read_dir(&devices) {
            for e in entries.flatten() {
                let id = match e.file_name().into_string() {
                    Ok(i) => i,
                    Err(_) => continue,
                };
                let bytes = match fs::read(e.path().join("device.json")) {
                    Ok(b) => b,
                    Err(_) => continue,
                };
                if let Ok(d) = serde_json::from_slice::<DeviceFile>(&bytes) {
                    out.insert(id, clean_name(&d.name));
                }
            }
        }
        out
    }
}

/// A device name comes from another device's `device.json`, so it is as
/// untrusted as any other cloud byte, and every renderer prints it as one
/// field of one line. Filtering here rather than in each renderer is why a
/// control character cannot forge or hide a `conflicts` row.
fn clean_name(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control() && !is_bidi_control(*c))
        .take(64)
        .collect()
}

/// The bidi controls, which `char::is_control` misses: that one is category
/// Cc, and these are Cf. A U+202E anywhere in a name or a path reverses the
/// rendered remainder of the line it is printed on, which is enough to make
/// one row read as another — the same forgery the Cc filter exists to stop.
///
/// Deliberately only the bidi ones. U+200C/U+200D (ZWNJ, ZWJ) are Cf too and
/// are left alone: they are ordinary text in Persian and the glue inside an
/// emoji sequence, and dropping them would mangle a legitimate device name.
pub fn is_bidi_control(c: char) -> bool {
    matches!(
        c,
        '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
    )
}

/// Ask iCloud to materialise a dataless file. Best effort, errors ignored.
pub fn download_stub(path: &Path) {
    let _ = Command::new("brctl")
        .arg("download")
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Unique, plain (non-dot) temporary name in the destination directory.
fn tmp_name(final_name: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{final_name}.{}.{}.part", std::process::id(), nanos)
}

fn write_once(path: &Path, bytes: &[u8]) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    let dir = path
        .parent()
        .ok_or_else(|| anyhow!("no parent directory for {}", path.display()))?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow!("bad file name for {}", path.display()))?;
    fs::create_dir_all(dir)?;
    let tmp = dir.join(tmp_name(name));
    let res = (|| -> Result<()> {
        fs::write(&tmp, bytes)?;
        fs::rename(&tmp, path)?;
        Ok(())
    })();
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    res
}

/// Three attempts 200 ms apart: Google Drive streams files in, so a read can
/// fail or return a partial (unparseable) file for a moment.
fn read_json_retry<T: DeserializeOwned>(path: &Path) -> Option<T> {
    for i in 0..3 {
        if let Ok(bytes) = fs::read(path) {
            if let Ok(v) = serde_json::from_slice::<T>(&bytes) {
                return Some(v);
            }
        }
        if i < 2 {
            thread::sleep(Duration::from_millis(200));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn cloud(td: &TempDir) -> Cloud {
        Cloud {
            base: td.path().join("dotlore"),
        }
    }

    fn src_file(td: &TempDir, name: &str, body: &str) -> PathBuf {
        let p = td.path().join(name);
        fs::write(&p, body).unwrap();
        p
    }

    fn dir_names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().into_string().unwrap())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn publish_two_bundles_in_order_without_leftovers() {
        let td = TempDir::new().unwrap();
        let c = cloud(&td);
        let a = src_file(&td, "a.bundle", "one");
        let b = src_file(&td, "b.bundle", "two");

        c.publish_bundle("proj-claude", "dev1", 1, &a).unwrap();
        c.publish_bundle("proj-claude", "dev1", 2, &b).unwrap();

        let bundles = c.list_bundles("proj-claude");
        assert_eq!(
            bundles.iter().map(|x| x.seq).collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert!(bundles.iter().all(|x| x.device == "dev1"));
        assert_eq!(fs::read(&bundles[1].path).unwrap(), b"two");

        let dev_dir = c
            .slug_dir("proj-claude")
            .unwrap()
            .join("devices")
            .join("dev1");
        assert_eq!(dir_names(&dev_dir), vec!["000001.bundle", "000002.bundle"]);
        assert_eq!(c.max_seq("proj-claude", "dev1"), 2);
    }

    #[test]
    fn republishing_identical_bytes_is_an_idempotent_retry() {
        let td = TempDir::new().unwrap();
        let c = cloud(&td);
        let a = src_file(&td, "a.bundle", "one");

        c.publish_bundle("s", "dev1", 1, &a).unwrap();
        c.publish_bundle("s", "dev1", 1, &a).unwrap();

        let dev_dir = c.slug_dir("s").unwrap().join("devices").join("dev1");
        assert_eq!(dir_names(&dev_dir), vec!["000001.bundle"]);
    }

    #[test]
    fn republishing_different_bytes_is_an_error_and_keeps_the_original() {
        let td = TempDir::new().unwrap();
        let c = cloud(&td);
        let a = src_file(&td, "a.bundle", "one");
        let b = src_file(&td, "b.bundle", "two");

        c.publish_bundle("s", "dev1", 1, &a).unwrap();
        assert!(c.publish_bundle("s", "dev1", 1, &b).is_err());

        let fin = c.slug_dir("s").unwrap().join("devices/dev1/000001.bundle");
        assert_eq!(fs::read(&fin).unwrap(), b"one");
    }

    /// The bundle holds every tracked file's bytes and lands in a folder the
    /// provider shares; 0644 there would expose `settings.json` API keys.
    #[test]
    fn a_published_bundle_is_not_world_readable() {
        use std::os::unix::fs::PermissionsExt;
        let td = TempDir::new().unwrap();
        let c = cloud(&td);
        let a = src_file(&td, "a.bundle", "one");
        c.publish_bundle("s", "dev1", 1, &a).unwrap();

        let fin = c.slug_dir("s").unwrap().join("devices/dev1/000001.bundle");
        assert_eq!(
            fs::metadata(&fin).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn a_seq_past_six_digits_is_refused() {
        let td = TempDir::new().unwrap();
        let c = cloud(&td);
        let a = src_file(&td, "a.bundle", "one");

        c.publish_bundle("s", "dev1", 999_999, &a).unwrap();
        assert!(c.publish_bundle("s", "dev1", 1_000_000, &a).is_err());

        let dev_dir = c.slug_dir("s").unwrap().join("devices").join("dev1");
        assert_eq!(dir_names(&dev_dir), vec!["999999.bundle"]);
        assert_eq!(c.max_seq("s", "dev1"), 999_999);
    }

    #[test]
    fn traversal_components_are_rejected() {
        let td = TempDir::new().unwrap();
        let c = cloud(&td);
        let a = src_file(&td, "a.bundle", "one");

        assert!(c.slug_dir("../escape").is_err());
        assert!(c.slug_dir("").is_err());
        assert!(c.slug_dir("/etc").is_err());
        assert!(c.publish_bundle("s", "../dev", 1, &a).is_err());
        assert!(c.write_device_name_once("s", "..", "x").is_err());

        // A manifest that lies about its own slug is not trusted.
        c.write_manifest_once(&Manifest {
            slug: "s".into(),
            kind: Kind::Dir,
        })
        .unwrap();
        fs::write(
            c.slug_dir("s").unwrap().join("manifest.json"),
            br#"{"slug":"other","kind":"dir"}"#,
        )
        .unwrap();
        assert_eq!(c.read_manifest("s"), None);
    }

    #[test]
    fn icloud_stub_is_skipped() {
        let td = TempDir::new().unwrap();
        let c = cloud(&td);
        let a = src_file(&td, "a.bundle", "one");
        c.publish_bundle("s", "dev1", 1, &a).unwrap();

        let dev_dir = c.slug_dir("s").unwrap().join("devices").join("dev1");
        fs::write(dev_dir.join(".000002.bundle.icloud"), "").unwrap();

        let bundles = c.list_bundles("s");
        assert_eq!(bundles.len(), 1);
        assert_eq!(bundles[0].seq, 1);
        assert_eq!(c.max_seq("s", "dev1"), 1);
    }

    #[test]
    fn write_manifest_once_keeps_first_content() {
        let td = TempDir::new().unwrap();
        let c = cloud(&td);
        let first = Manifest {
            slug: "s".into(),
            kind: Kind::Dir,
        };
        c.write_manifest_once(&first).unwrap();
        c.write_manifest_once(&Manifest {
            slug: "s".into(),
            kind: Kind::File,
        })
        .unwrap();

        assert_eq!(c.read_manifest("s"), Some(first));
    }

    #[test]
    fn list_slugs_ignores_dirs_without_manifest() {
        let td = TempDir::new().unwrap();
        let c = cloud(&td);
        c.write_manifest_once(&Manifest {
            slug: "beta".into(),
            kind: Kind::Dir,
        })
        .unwrap();
        fs::create_dir_all(c.slug_dir("alpha").unwrap()).unwrap();

        assert_eq!(c.list_slugs(), vec!["beta".to_string()]);
    }

    /// A slug directory name is another device's bytes, and `dotlore slugs`
    /// prints the list straight to the terminal.
    #[test]
    fn a_slug_name_with_control_characters_is_not_listed() {
        let td = TempDir::new().unwrap();
        let c = cloud(&td);
        for slug in ["good", "\u{1b}]0;evil\u{7}", "two\rlines"] {
            c.write_manifest_once(&Manifest {
                slug: slug.into(),
                kind: Kind::Dir,
            })
            .unwrap();
        }

        // The directories exist — `plain_name` allows an ESC — so this is the
        // listing dropping them, not the write failing.
        assert_eq!(dir_names(&c.base).len(), 3);
        assert_eq!(c.list_slugs(), vec!["good".to_string()]);
    }

    #[test]
    fn device_names_maps_id_to_name_and_is_not_a_bundle() {
        let td = TempDir::new().unwrap();
        let c = cloud(&td);
        let id = "0123456789abcdef0123456789abcdef";
        c.write_device_name_once("s", id, "Thi's MacBook Pro")
            .unwrap();

        assert_eq!(c.device_name("s", id).as_deref(), Some("Thi's MacBook Pro"));
        assert_eq!(
            c.device_names("s").get(id).map(String::as_str),
            Some("Thi's MacBook Pro")
        );
        assert!(c.list_bundles("s").is_empty());
    }

    /// The name is printed by `conflicts`, by `show` and (Phase 5) by the UI.
    /// An escape sequence there could repaint or hide a row, and an unbounded
    /// one could push the real rows off screen.
    #[test]
    fn a_device_name_is_stripped_of_control_characters_and_capped() {
        let td = TempDir::new().unwrap();
        let c = cloud(&td);
        let id = "0123456789abcdef0123456789abcdef";
        let evil = format!("\u{1b}[31mEvil\u{202e}\r\n{}", "x".repeat(100));
        c.write_device_name_once("s", id, &evil).unwrap();

        let got = c.device_name("s", id).unwrap();
        assert!(
            !got.chars().any(char::is_control),
            "control character survived: {got:?}"
        );
        assert!(
            !got.chars().any(is_bidi_control),
            "bidi override survived: {got:?}"
        );
        assert_eq!(got.chars().count(), 64, "name was not capped: {got:?}");
        assert!(got.starts_with("[31mEvil"), "{got:?}");
        assert_eq!(c.device_names("s").get(id), Some(&got));
    }
}
