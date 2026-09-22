//! Running the system `git` binary with a hermetic environment.

use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use std::process::{Command, Output};

/// A git runner bound to one repository and one device identity.
pub struct Git {
    pub repo: PathBuf,
    author: String,
    email: String,
}

impl Git {
    pub fn new(repo: PathBuf, device_name: &str, device_id: &str) -> Git {
        Git {
            repo,
            author: device_name.to_string(),
            email: format!("{device_id}@dotlore"),
        }
    }

    /// Run `git -C <repo> <args>`. A non-zero exit status is *not* an error;
    /// only a failure to spawn the process is.
    pub fn run(&self, args: &[&str]) -> Result<Output> {
        self.command(args)
            .output()
            .with_context(|| format!("failed to spawn git {args:?}"))
    }

    /// The child git process, with an allowlisted environment.
    ///
    /// `env_clear` rather than a list of `env_remove`s: git has dozens of
    /// environment variables that redirect it (`GIT_DIR`, `GIT_COMMON_DIR`,
    /// `GIT_INDEX_FILE`, …), inject config (`GIT_CONFIG_COUNT`,
    /// `GIT_CONFIG_PARAMETERS`) or execute code (`GIT_TEMPLATE_DIR` copies a
    /// `hooks/` directory into every `git init`), new ones appear across
    /// releases, and none of them is suppressed by `GIT_CONFIG_GLOBAL` or
    /// `GIT_CONFIG_NOSYSTEM`. Clearing makes all of them inert at once.
    /// Dropping `HOME` also stops git reading `~/.config/git/{ignore,
    /// attributes}`, which are read by default and are not config files.
    ///
    /// `PATH` is the only ambient variable forwarded: verified against git
    /// 2.50.1 that `init`, `add`, `commit`, `rev-parse`, `merge-base`,
    /// `update-ref`, `bundle create/verify`, `fetch` and `merge` all succeed
    /// under `env -i` with nothing but `PATH` and the set below.
    fn command(&self, args: &[&str]) -> Command {
        let mut c = Command::new("git");
        c.arg("-C")
            .arg(&self.repo)
            .args(args)
            .env_clear()
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", &self.author)
            .env("GIT_COMMITTER_NAME", &self.author)
            .env("GIT_AUTHOR_EMAIL", &self.email)
            .env("GIT_COMMITTER_EMAIL", &self.email)
            .env("GIT_EDITOR", "true")
            .env("GIT_TERMINAL_PROMPT", "0");
        if let Some(p) = std::env::var_os("PATH") {
            c.env("PATH", p);
        }
        c
    }

    /// Run and require exit 0, returning trimmed stdout.
    pub fn ok(&self, args: &[&str]) -> Result<String> {
        let out = self.run(args)?;
        if !out.status.success() {
            bail!(
                "git {args:?} failed ({}): {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    /// Resolve a revision, or `None` if it does not exist.
    ///
    /// `--end-of-options` keeps a ref name that looks like a flag (refs are
    /// built from cloud-controlled device directory names) from being parsed
    /// as one.
    pub fn rev(&self, r: &str) -> Option<String> {
        self.run(&rev_args(r))
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    }

    pub fn is_ancestor(&self, a: &str, b: &str) -> bool {
        self.run(&ancestor_args(a, b))
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

fn rev_args(r: &str) -> [&str; 5] {
    ["rev-parse", "--verify", "--quiet", "--end-of-options", r]
}

fn ancestor_args<'a>(a: &'a str, b: &'a str) -> [&'a str; 5] {
    ["merge-base", "--is-ancestor", "--end-of-options", a, b]
}

/// `Some(path)` if a usable `git` is on `PATH`, `None` otherwise.
// ponytail: returns the PATH-relative name; every caller only needs the
// presence check.
pub fn which_git() -> Option<PathBuf> {
    Command::new("git")
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|_| PathBuf::from("git"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    fn git_at(dir: &TempDir) -> Git {
        Git::new(
            dir.path().to_path_buf(),
            "test-device",
            "0123456789abcdef0123456789abcdef",
        )
    }

    #[test]
    fn commit_then_rev_and_ancestor() -> Result<()> {
        let dir = TempDir::new()?;
        let g = git_at(&dir);
        g.ok(&["init", "-b", "main"])?;
        std::fs::write(dir.path().join("f.txt"), "hello")?;
        g.ok(&["add", "-A"])?;
        g.ok(&["commit", "-m", "x"])?;

        assert!(g.rev("HEAD").is_some());
        assert!(g.is_ancestor("HEAD", "HEAD"));
        assert!(g.rev("refs/heads/nope").is_none());
        Ok(())
    }

    /// The `--end-of-options` separator sits immediately before the first ref
    /// argument; asserting on the outcome instead would be vacuous, since git
    /// exits non-zero for a flag-shaped rev either way.
    #[test]
    fn refs_are_passed_after_end_of_options() {
        let a = rev_args("-x");
        let i = a.iter().position(|s| *s == "-x").unwrap();
        assert_eq!(a[i - 1], "--end-of-options");
        assert_eq!(i, a.len() - 1);

        let b = ancestor_args("-x", "-y");
        let j = b.iter().position(|s| *s == "-x").unwrap();
        assert_eq!(b[j - 1], "--end-of-options");
        assert_eq!(&b[j..], &["-x", "-y"]);
    }

    /// A ref really can be named `-x`, and resolving it must not parse it as
    /// an option. This one fails outright without `--end-of-options`.
    #[test]
    fn a_ref_named_like_a_flag_still_resolves() -> Result<()> {
        let dir = TempDir::new()?;
        let g = git_at(&dir);
        g.ok(&["init", "-b", "main"])?;
        std::fs::write(dir.path().join("f.txt"), "hello")?;
        g.ok(&["add", "-A"])?;
        g.ok(&["commit", "-m", "x"])?;
        g.ok(&["update-ref", "--end-of-options", "refs/heads/-x", "HEAD"])?;

        assert_eq!(g.rev("-x"), g.rev("HEAD"));
        assert!(g.is_ancestor("-x", "HEAD"));
        Ok(())
    }

    #[test]
    fn ok_reports_argv_on_failure() -> Result<()> {
        let dir = TempDir::new()?;
        let g = git_at(&dir);
        let err = g.ok(&["nonsense-cmd"]).unwrap_err().to_string();
        assert!(err.contains("nonsense-cmd"), "{err}");
        Ok(())
    }

    /// `GIT_TEMPLATE_DIR` copies its `hooks/` into every `git init`, execute
    /// bit intact, and the next commit runs them; `GIT_CONFIG_PARAMETERS`
    /// injects config. `env_clear` must make both inert.
    ///
    /// `std::env::set_var` is process-global, and the harness runs tests in
    /// threads. It is safe here: this is the only test in the crate that
    /// mutates the environment, std serialises `set_var` against `var_os` and
    /// `Command` spawning on its own lock, and after `env_clear` no child can
    /// see these variables anyway, so no sibling test can observe them.
    #[test]
    fn a_hostile_parent_environment_cannot_reach_the_child() -> Result<()> {
        let dir = TempDir::new()?;
        let hooks = dir.path().join("template/hooks");
        std::fs::create_dir_all(&hooks)?;
        let marker = dir.path().join("ran");
        let hook = hooks.join("pre-commit");
        std::fs::write(&hook, format!("#!/bin/sh\ntouch {}\n", marker.display()))?;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755))?;

        std::env::set_var("GIT_TEMPLATE_DIR", dir.path().join("template"));
        std::env::set_var(
            "GIT_CONFIG_PARAMETERS",
            format!("'core.hooksPath={}'", hooks.display()),
        );

        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo)?;
        let g = Git::new(repo.clone(), "test-device", "0123456789abcdef");
        g.ok(&["init", "-b", "main"])?;
        std::fs::write(repo.join("f.txt"), "hello")?;
        g.ok(&["add", "-A"])?;
        g.ok(&["commit", "-m", "x"])?;

        assert!(!marker.exists(), "the template hook ran");
        assert!(
            !repo.join(".git/hooks/pre-commit").exists(),
            "the template hook was copied into the new repo"
        );
        assert!(
            !g.run(&["config", "core.hooksPath"])?.status.success(),
            "GIT_CONFIG_PARAMETERS reached the child"
        );
        Ok(())
    }

    #[test]
    fn which_git_finds_git() {
        assert!(which_git().is_some());
    }
}
