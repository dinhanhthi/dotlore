//! Start at login, as a per-user LaunchAgent.
//!
//! `ponytail:` a `~/Library/LaunchAgents` plist instead of `SMAppService`,
//! which needs an Objective-C framework binding this workspace has no
//! dependency for. Swap when Login Items UI parity (System Settings listing
//! the job under the app's own name rather than under "dotlore") or the App
//! Store matters.
//!
//! ## The one invariant
//!
//! **The durable half is committed before the volatile one, in both
//! directions.** The plist on disk is what the checkbox actually promises —
//! it is what runs, or does not run, at the next login — and `launchctl` only
//! makes that true for the session already in progress. So: write the plist
//! *then* `bootstrap`, remove the plist *then* `bootout`. Neither `launchctl`
//! may move in front of its file operation. (Their failures are handled
//! differently — `bootstrap`'s is returned to the user, `bootout`'s is
//! swallowed — but that is a separate question from the order.)
//!
//! This is not tidiness. `bootout` SIGTERMs this very process when launchd
//! started it at login — the normal start-at-login case — so a `bootout`
//! placed first never returns, and the removal it was ordered in front of
//! never happens: the plist survives, the app comes back at the next login,
//! and the checkbox reads `true` again. With the removal first, the same
//! SIGTERM lands on a setting that has already taken.
//!
//! `ponytail:` one ceiling left in the LaunchAgent design, inherent to
//! `bootstrap` and not fixable inside this module: enabling from the running
//! app bootstraps a `RunAtLoad` job, so launchd starts a **second**
//! `dotlore` immediately. `main`'s single-instance lock is what makes
//! that copy leave again; the alternative was skipping `bootstrap`, which
//! would mean the agent only takes effect at the next login.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

/// The LaunchAgent label, and the bundle identifier in `Info.plist`.
pub const LABEL: &str = "dev.dinhanhthi.dotlore";

/// Absolute, because a Finder-launched `.app` inherits a minimal `PATH`.
/// `launchctl` is on the project's allowed-program list.
const LAUNCHCTL: &str = "/bin/launchctl";

/// Where the agent lives. `home_dir` is the `$HOME` resolved once at startup
/// — this module never reads the environment itself.
pub fn plist_path(home_dir: &Path) -> PathBuf {
    home_dir
        .join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist"))
}

/// Whether the agent is installed. The plist is the durable half of the
/// state: it is what makes the app start at the *next* login, which is what
/// the checkbox claims.
pub fn is_enabled(home_dir: &Path) -> bool {
    plist_path(home_dir).exists()
}

/// Install or remove the agent for this Mac's login session.
pub fn set(home_dir: &Path, enabled: bool) -> Result<()> {
    let program = std::env::current_exe().context("resolving this program's own path")?;
    set_program(home_dir, enabled, &program)
}

/// The body of [`set`], with the program spelled out so a test can point the
/// agent at something harmless instead of relaunching the app.
fn set_program(home_dir: &Path, enabled: bool, program: &Path) -> Result<()> {
    // No `id` subprocess and no `libc`: the owner of `$HOME` is the user
    // whose GUI domain this agent belongs in.
    let uid = std::fs::metadata(home_dir)
        .with_context(|| format!("reading {}", home_dir.display()))?
        .uid();
    let plist = plist_path(home_dir);

    if enabled {
        let program = program
            .to_str()
            .with_context(|| format!("{} is not valid UTF-8", program.display()))?;
        let dir = plist.parent().context("the LaunchAgents directory")?;
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        std::fs::write(&plist, plist_content(program))
            .with_context(|| format!("writing {}", plist.display()))?;
        // Left in place if this fails: the plist alone already does what the
        // checkbox promises at the next login, and removing it would throw
        // that away to make one error message tidier.
        launchctl(&[
            "bootstrap".as_ref(),
            format!("gui/{uid}").as_ref(),
            plist.as_os_str(),
        ])
    } else {
        // The durable half first — see the module's invariant. `bootout`
        // SIGTERMs this process when launchd started it at login, so anything
        // after it may never run.
        match std::fs::remove_file(&plist) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).with_context(|| format!("removing {}", plist.display())),
        }?;
        // Best effort. A plist that was written but never bootstrapped makes
        // this exit non-zero, and the user's way *out* of start-at-login must
        // not depend on launchd agreeing about the current session — removing
        // the plist is what actually stops the next login.
        let _ = launchctl(&["bootout".as_ref(), format!("gui/{uid}/{LABEL}").as_ref()]);
        Ok(())
    }
}

/// No shell: argv array, absolute program, stderr quoted back on failure.
fn launchctl(args: &[&std::ffi::OsStr]) -> Result<()> {
    let out = Command::new(LAUNCHCTL)
        .args(args)
        .output()
        .with_context(|| format!("running {LAUNCHCTL}"))?;
    if out.status.success() {
        return Ok(());
    }
    let why = String::from_utf8_lossy(&out.stderr);
    let why = why.trim();
    bail!(
        "launchctl {} failed ({}){}",
        args[0].to_string_lossy(),
        out.status,
        if why.is_empty() {
            String::new()
        } else {
            format!(": {why}")
        }
    );
}

/// The agent, as launchd wants it.
///
/// `KeepAlive` is false on purpose: quitting from the menu has to mean quit,
/// not "restart me in two seconds".
fn plist_content(program: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{LABEL}</string>
	<key>ProgramArguments</key>
	<array>
		<string>{}</string>
	</array>
	<key>RunAtLoad</key>
	<true/>
	<key>KeepAlive</key>
	<false/>
</dict>
</plist>
"#,
        xml(program)
    )
}

/// XML text escaping for the one value that is not a literal. A `&` in a
/// folder name is not exotic, and an unescaped one makes the whole plist
/// unparseable.
fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_agent_lands_where_launchd_looks_for_it() {
        assert_eq!(
            plist_path(Path::new("/Users/someone")),
            Path::new("/Users/someone/Library/LaunchAgents/dev.dinhanhthi.dotlore.plist")
        );
    }

    #[test]
    fn an_absent_plist_means_the_agent_is_off() {
        assert!(!is_enabled(Path::new("/nonexistent-dotlore-home")));
    }

    #[test]
    fn the_agent_runs_at_login_and_is_not_resurrected() {
        let p = plist_content("/Applications/Dotlore.app/Contents/MacOS/dotlore");
        assert!(p.contains("<key>Label</key>\n\t<string>dev.dinhanhthi.dotlore</string>"));
        assert!(p.contains("<string>/Applications/Dotlore.app/Contents/MacOS/dotlore</string>"));
        assert!(p.contains("<key>RunAtLoad</key>\n\t<true/>"));
        assert!(p.contains("<key>KeepAlive</key>\n\t<false/>"));
    }

    #[test]
    fn an_ampersand_in_the_path_does_not_break_the_plist() {
        let p = plist_content("/Users/a&b/<x>/dotlore");
        assert!(p.contains("<string>/Users/a&amp;b/&lt;x&gt;/dotlore</string>"));
        assert!(!p.contains("a&b"));
    }
}
