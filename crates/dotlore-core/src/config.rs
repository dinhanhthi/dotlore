//! On-disk configuration and tracked-root definitions.
//!
//! The state directory holds `config.json` (device identity, provider folder,
//! tracked roots), a `tmp/` scratch dir and the `lock` file that keeps the
//! app's daemon and a concurrently running CLI off the same staging repos.
//! Every entry point takes `home` explicitly; [`default_home`] is the only
//! place that decides where state lives from the environment, and only
//! binaries call it. (`git::Git::command` reads `PATH`, to forward it into an
//! otherwise cleared child environment.)

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::cloud::Kind;

/// One tracked root: a real path on this device plus its cloud identity.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct Root {
    pub slug: String,
    pub path: PathBuf,
    pub kind: Kind,
    /// Set while the first sync is still populating the root.
    #[serde(default)]
    pub initializing: bool,
}

/// Contents of `<home>/config.json`.
#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub struct Config {
    pub device_id: String,
    pub device_name: String,
    pub provider_dir: Option<PathBuf>,
    pub roots: Vec<Root>,
}

impl Config {
    /// Read `<home>/config.json`, creating `home`, `home/tmp` and — when the
    /// file is missing — a fresh device identity that is saved before return.
    pub fn load(home: &Path) -> Result<Config> {
        fs::create_dir_all(home.join("tmp"))?;
        let path = home.join("config.json");
        match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .with_context(|| format!("parsing {}", path.display())),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let cfg = Config {
                    device_id: random_id()?,
                    device_name: hostname(),
                    ..Default::default()
                };
                cfg.save(home)?;
                Ok(cfg)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Write `<home>/config.json` atomically (temp file + rename).
    pub fn save(&self, home: &Path) -> Result<()> {
        fs::create_dir_all(home)?;
        // Pid alone collides between two threads sharing one home.
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let tmp = home.join(format!("config.json.{}.{nanos}.tmp", std::process::id()));
        let res = (|| -> Result<()> {
            fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
            fs::rename(&tmp, home.join("config.json"))?;
            Ok(())
        })();
        if res.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        res
    }

    /// Short device id, for log lines and conflict file names.
    pub fn id8(&self) -> &str {
        self.device_id.get(..8).unwrap_or(&self.device_id)
    }
}

/// `$DOTLORE_HOME`, else `~/Library/Application Support/dotlore`.
///
/// The only environment read that affects state location; call it from
/// binaries only. (`git::Git::command` also reads `PATH`, to forward it into
/// an otherwise cleared child environment.)
pub fn default_home() -> PathBuf {
    match std::env::var_os("DOTLORE_HOME") {
        Some(h) if !h.is_empty() => PathBuf::from(h),
        _ => PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
            .join("Library/Application Support/dotlore"),
    }
}

/// Lowercase `[a-z0-9-]`, with runs of `-` collapsed and the ends trimmed.
pub fn sanitize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

/// `<parent>-<name without leading dots>`, sanitized; a root directly in
/// `home_dir` gets the literal parent `home` (`~/.claude` → `home-claude`).
pub fn default_slug(path: &Path, home_dir: &Path) -> String {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .trim_start_matches('.');
    let parent = if path.parent() == Some(home_dir) {
        "home"
    } else {
        path.parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("")
    };
    let slug = sanitize(&format!("{parent}-{name}"));
    if slug.is_empty() {
        "root".to_string()
    } else {
        slug
    }
}

/// Exclusive cross-process lock on the state directory; released on drop.
pub struct HomeLock(File);

impl Drop for HomeLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

/// Take the `<home>/lock` file lock, blocking until it is free.
pub fn lock(home: &Path) -> Result<HomeLock> {
    fs::create_dir_all(home)?;
    let f = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(home.join("lock"))?;
    f.lock()?;
    Ok(HomeLock(f))
}

/// 16 bytes of `/dev/urandom` as 32 lowercase hex chars.
fn random_id() -> Result<String> {
    let mut buf = [0u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut buf)?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

/// Trimmed output of `hostname`, `"mac"` on any failure or empty result.
fn hostname() -> String {
    let name = Command::new("hostname")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    if name.is_empty() {
        "mac".to_string()
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;
    use tempfile::TempDir;

    #[test]
    fn load_creates_identity_and_roundtrips() {
        let td = TempDir::new().unwrap();
        let home = td.path();

        let mut cfg = Config::load(home).unwrap();
        assert_eq!(cfg.device_id.len(), 32);
        assert!(cfg.device_id.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(!cfg.device_name.is_empty());
        assert_eq!(cfg.id8(), &cfg.device_id[..8]);
        assert!(home.join("tmp").is_dir());
        assert!(home.join("config.json").is_file());

        cfg.provider_dir = Some(PathBuf::from("/cloud"));
        cfg.roots.push(Root {
            slug: "myproj-claude".into(),
            path: PathBuf::from("/a/myproj/.claude"),
            kind: Kind::Dir,
            initializing: false,
        });
        cfg.save(home).unwrap();

        let again = Config::load(home).unwrap();
        assert_eq!(again.device_id, cfg.device_id);
        assert_eq!(again.provider_dir, cfg.provider_dir);
        assert_eq!(again.roots, cfg.roots);

        let other = TempDir::new().unwrap();
        assert_ne!(Config::load(other.path()).unwrap().device_id, cfg.device_id);
    }

    #[test]
    fn default_slug_uses_parent_and_undotted_name() {
        let home = Path::new("/Users/x");
        assert_eq!(
            default_slug(Path::new("/a/myproj/.claude"), home),
            "myproj-claude"
        );
        assert_eq!(
            default_slug(Path::new("/a/myproj/CLAUDE.md"), home),
            "myproj-claude-md"
        );
        assert_eq!(
            default_slug(Path::new("/Users/x/.claude"), home),
            "home-claude"
        );
        // A slug must never be empty: it becomes a cloud path component.
        assert_eq!(default_slug(Path::new("/"), home), "root");
    }

    /// `initializing` was added after the first configs were written; an older
    /// `config.json` without the key must still load.
    #[test]
    fn a_root_without_initializing_still_loads() {
        let td = TempDir::new().unwrap();
        let home = td.path();
        Config::load(home).unwrap();
        fs::write(
            home.join("config.json"),
            br#"{"device_id":"ab","device_name":"m","provider_dir":null,
                 "roots":[{"slug":"s","path":"/a/.claude","kind":"dir"}]}"#,
        )
        .unwrap();

        let cfg = Config::load(home).unwrap();
        assert_eq!(cfg.roots.len(), 1);
        assert!(!cfg.roots[0].initializing);
    }

    #[test]
    fn sanitize_collapses_and_trims() {
        assert_eq!(sanitize("Thi's MacBook.local"), "thi-s-macbook-local");
    }

    #[test]
    fn second_lock_waits_for_the_first() {
        let td = TempDir::new().unwrap();
        let home = td.path().to_path_buf();

        let held = lock(&home).unwrap();
        let (tx, rx) = mpsc::channel();
        let h = thread::spawn(move || {
            let guard = lock(&home).unwrap();
            tx.send(()).unwrap();
            drop(guard);
        });

        assert!(
            rx.recv_timeout(Duration::from_millis(300)).is_err(),
            "second lock must block while the first is held"
        );
        drop(held);
        rx.recv_timeout(Duration::from_secs(2))
            .expect("second lock must be granted after the first is dropped");
        h.join().unwrap();
    }
}
