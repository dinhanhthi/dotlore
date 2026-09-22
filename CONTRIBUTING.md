# Contributing

## Build

From the repo root:

```sh
pnpm install
pnpm tauri dev
pnpm test
pnpm smoke:two-devices
pnpm format:check
pnpm build
```

The scripts in `package.json` wrap the engine gates. Those gates still apply: `cargo build --manifest-path src-tauri/Cargo.toml` must be warning-free (the desktop app is that package). `pnpm tauri dev` and `pnpm build` build the UI from `src/` into `dist` so you do not have to.

## Layout

The React/TypeScript frontend lives at `src/`. The Tauri shell and the engine live at `src-tauri/` (package and binary `dotlore`). `src-tauri/src/main.rs` is the only environment reader. `src-tauri/src/lib.rs` exports the engine modules and `pub fn run(home, home_dir)`.

## UI playground (`mockapp/`)

`mockapp/` is a browser-only preview of the real app UI. It mounts the same `src/App.tsx` with a mocked Tauri IPC layer and selectable fake-data scenarios — useful for iterating on CSS without compiling Rust.

```sh
pnpm mockapp:dev   # http://localhost:38422
```

Pick a scenario from the right sidebar (or `?scenario=<id>`). **Never change `src/` components to make the browser happy** — fix `mockapp/mocks/` instead. Details: [`mockapp/README.md`](mockapp/README.md).

## Landing page (`website/`)

`website/` is the public landing page: static HTML + CSS, no build, no tests. Open `website/index.html` in a browser. A push to `main` that touches `website/` deploys to GitHub Pages (`dotlore.dinhanhthi.com`). Do not couple it to `src/`. Details: [`website/README.md`](website/README.md).

## Invariants

These are not style rules. The engine's correctness rests on them, and each one exists because breaking it loses or leaks someone's data.

**No shell, ever.** `std::process::Command` with argv arrays. The only external programs this project may run are `git`, `brctl`, `hostname`, and `launchctl`.

*Scope of that rule: the sync engine.* It governs every code path that reads, writes or syncs a user's files — which is why `scripts/build.sh` draws the same line for build scripts rather than claiming to be covered by it. One dependency sits outside it, named here so the invariant stays honest: `tauri-plugin-updater`, reached only from `src-tauri/src/updater.rs` when the user installs an update. On macOS it runs `touch` on the replaced bundle, re-execs the app through `AppHandle::restart`, and — only when the app directory is not writable by the current user — runs an AppleScript `do shell script "rm -rf … && mv -f …" with administrator privileges` to move the new bundle into place. That is a shell, with a prompt for admin rights. It is confined to the update path, it never touches a synced root, and nothing in the engine may call into it.

**Hermetic git.** Every invocation goes through `Git::command`, which clears the child environment and then sets `GIT_CONFIG_GLOBAL=/dev/null`, `GIT_CONFIG_NOSYSTEM=1`, explicit author/committer, `GIT_EDITOR=true`, `GIT_TERMINAL_PROMPT=0`, plus `PATH`. A user's global config — signing, `core.hooksPath`, `autocrlf` — and their `~/.config/git/ignore` and `attributes` must never reach these calls.

**No environment reads.** `config::default_home()` is the only one that affects where state lives, and only `src-tauri/src/main.rs` calls it. `lib.rs` exports the engine modules and `pub fn run(home, home_dir)`. Everything else takes `home: &Path` explicitly, which is what lets the test harness run several devices in one process. (`git::Git::command` reads `PATH` to forward it into the child — see below.)

**The git child environment is an allowlist, not a denylist.** `Git::command` calls `env_clear()` and sets only the hermetic variables plus `PATH`. Never replace this with `env_remove` calls: four consecutive review rounds each found another `GIT_*` variable that had to be added, and two of them (`GIT_CONFIG_PARAMETERS`, `GIT_TEMPLATE_DIR`) were arbitrary code execution via git hooks, reachable despite `GIT_CONFIG_GLOBAL=/dev/null` and `GIT_CONFIG_NOSYSTEM=1`. Clearing is the only form that is closed against variables nobody has thought of yet. Adding something back to the allowlist is a deliberate decision, not a convenience.

**The cloud is immutable.** Bundles are published under a temp name and installed with a no-replace operation. Nothing in the cloud folder is ever rewritten or deleted.

**Live files are sacred.** Tracked files stay in place. Conflict markers never reach them, and neither do `.conflict-*` siblings, `.dotloreignore`, or `.dotloreproject` — those live in staging only and never in a live root. `mirror::apply_to_root` fails closed: a missing snapshot expectation is an error, and a path that drifted from its expectation is skipped and reported, never overwritten.

**Symlinks are never followed** in either direction, and a symlinked path is never replaced.

## Trust boundaries

Two inputs are attacker-controlled and must be treated as such:

1. **The cloud folder.** `manifest.json`, `device.json`, bundle filenames, and device directory names were all written by another device. Anything used to build a filesystem path goes through the single-plain-component guard.
2. **The change set applied to a live root.** It originates in another device's git bundle. Reject traversal (`..`, absolute, empty) and out-of-list paths, and check the *source* file in staging too — git stores symlinks as mode-120000 blobs, so a checkout can put one there.

The files this tool syncs routinely hold API keys. Widening a file's permissions, leaving a temp copy behind, or logging content are security bugs, not cosmetics.

## Dependencies

Locked to `anyhow`, `serde`, `serde_json`, `ignore`, `notify`, with `tempfile` for tests. Stdlib before a new crate — `/dev/urandom` over a uuid crate, `$HOME` over a dirs crate, `std::fs::File::lock` over a lock crate. Adding a dependency needs a reason that a few lines of stdlib cannot cover.

## Tests

```sh
pnpm test
pnpm smoke:two-devices
cargo test --manifest-path src-tauri/Cargo.toml
cargo fmt --manifest-path src-tauri/Cargo.toml --check
```

In-file `#[cfg(test)]` modules using `tempfile::TempDir`. A test earns its place by failing when its fix is reverted — check that, rather than assuming it. A test that passes either way is worse than no test, because it claims coverage that is not there. The two-device smoke path is `src-tauri/tests/two_devices.rs`; `pnpm smoke:two-devices` runs one case of it and does not launch the GUI.

## Commits

One line, conventional: `<type>(<scope>): <summary>`. No body, no AI co-author trailers.
