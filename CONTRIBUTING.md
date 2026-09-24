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

### A dev run has its own state directory

`pnpm dev` and `pnpm tauri dev` default `DOTLORE_HOME` to `~/Downloads/dotlore-dev`, and print the value they used. You do not export anything; a release build is unaffected, because the default is set below the `build` branch in `scripts/tauri.sh`.

This exists so a dev run can sit beside a copy installed in `/Applications`. Both lock `<home>/app.lock` (`src-tauri/src/lib.rs`), and on a clash the second one writes one line to stderr and calls `exit(0)` — no window, exit status zero. Launched from Finder that stderr goes nowhere, so the only symptom is an app that appears not to open. Separate homes avoid it entirely.

`scripts/dev-home.sh` holds that path and is the only place it is written down; `scripts/tauri.sh` and `scripts/reset-dev.sh` both source it. Keep it that way. If the two ever disagree, `pnpm reset:dev` deletes the state of the *installed* app — someone's real project list — while claiming to clear the dev one. An explicit `DOTLORE_HOME` still overrides both.

Three things are shared regardless of `DOTLORE_HOME`, because they are keyed on `$HOME` or on the bundle identifier, not on the state dir:

- **Start at login.** `login_item.rs` has a single `LABEL` (`dev.dinhanhthi.dotlore`) and writes `~/Library/LaunchAgents/dev.dinhanhthi.dotlore.plist` whose `program` points at whichever copy toggled it last. Toggle it in the installed app only, never in a dev run, or macOS launches your `target/debug` binary at login.
- **The updater's target.** `pnpm dev` runs through `scripts/dev-dock-bundle.sh`, which builds a real bundle beside the debug binary so the Dock shows "Dotlore". That bundle is `src-tauri/target/debug/Dotlore.app`, or `<target-dir>/debug/Dotlore.app` when a shared Cargo target dir is set ([Cargo build cache](#cargo-build-cache)). The updater resolves its install target from `current_exe` by walking up out of `Contents/MacOS`, so a dev run that accepts an update overwrites *that* bundle with the released one. Only reachable while the published version is ahead of `src-tauri/Cargo.toml`; in a dev run, choose "Later" — and do not click the menu-bar "Update to Dotlore …" row or the title-bar "Update" badge, which open the same prompt. The background checks (at launch, then every 6 h) never show an error — they swallow it and log.
- **The cloud folder.** `device_id` is random per home (`config.rs`), so a dev run is a second device. Point it at a different cloud folder unless you want a permanent extra `devices/<id>/` entry — the cloud is immutable and nothing but an explicit "Wipe cloud data" deletes one.

### Cargo build cache

`target/` is Cargo's build cache for this Mac, not a second OS you opted into. `pnpm tauri dev` and `cargo test` write the `dev` profile to `debug/`. `pnpm build` writes `release/`. There is no `aarch64-apple-darwin/` or Windows directory here unless you cross-compile. The hash folders under `debug/incremental/` are earlier compiles of the same crate, left behind when `rustc`, the profile, or a feature set changes. Cargo does not delete them.

On macOS the `dev` profile's default `split-debuginfo` is `"unpacked"`, and only while debug info is enabled. Each codegen unit drops a `.o` into `target/debug/deps`. That is the bulk of a debug target that has grown past 10 GB. `src-tauri/Cargo.toml` sets:

```toml
[profile.dev]
opt-level = 1
split-debuginfo = "off"

[profile.dev.package."*"]
debug = "line-tables-only"
incremental = false
```

`split-debuginfo = "off"` stops those object files. Dependency crates keep line tables, enough for a backtrace to name a file and a line. The `dotlore` crate itself keeps full debug info, and incremental compilation stays on for it. `incremental = false` under `package."*"` covers path dependencies; registry crates already skip incremental compilation. Changing the profile does not shrink a target that already exists.

Share one cache across Rust projects by creating `~/.cargo/config.toml` once per machine. This file is outside the repo. Cargo does not expand `~`. A relative path in `~/.cargo/config.toml` is resolved from `$HOME`, so the value below is `~/.cargo/shared-target` on every account:

```toml
[build]
target-dir = ".cargo/shared-target"

[profile.dev]
opt-level = 1
split-debuginfo = "off"

[profile.dev.package."*"]
debug = "line-tables-only"
incremental = false
```

Copy the profile from `src-tauri/Cargo.toml` and keep the two identical. Profile keys in Cargo's config override the same keys in every `Cargo.toml`. You set this once. You do not retune it when you switch projects. A project that leaves `opt-level` at the default `0` compiles a second copy of every dependency next to Dotlore's `opt-level = 1` copies. The build still succeeds. The disk just holds both.

What lands in the shared directory:

- Dependency artifacts in `debug/deps` (`.rlib`, `.rmeta`, build-script output) when the crate version, features, `rustc`, profile flags, and target triple match. Two Tauri 2 apps reuse `tauri`, `wry`, `tokio`, `serde`, and `objc2` under those conditions.
- Each app's own binary, side by side (`debug/dotlore`, `debug/<other>`). Give them different binary names so one does not replace the other.

A different feature set stores another copy of that one crate. `cargo clean` deletes the whole shared directory, every project included. Cargo locks the directory, so build one project at a time.

`scripts/tauri.sh` does not set `CARGO_TARGET_DIR`, so `pnpm tauri dev` and `pnpm test` follow this config. With no config and no environment variable they still use `src-tauri/target`. `scripts/build.sh` exports `CARGO_TARGET_DIR` to `src-tauri/target` on purpose: an inherited target dir must not move `src-tauri/target/release/bundle`. Leave that pin alone. Release artifacts stay per-repo. An exported `CARGO_TARGET_DIR` still overrides the config for dev commands, because the environment outranks `~/.cargo/config.toml`.

After writing the config on a machine that already has per-project `target/` directories, delete those directories. They are no longer on Cargo's path, and `cargo clean` will not see them. The next `pnpm tauri dev` fills `~/.cargo/shared-target`.

Old artifacts still accumulate inside the directory that is in use. `scripts/sweep-cargo.sh` (`pnpm sweep:cargo`) runs `cargo sweep` and removes anything unused for 14 days. Install it once: `cargo install cargo-sweep`.

```sh
pnpm sweep:cargo                 # this repo's target, wherever Cargo put it
pnpm sweep:cargo -- --dry-run
pnpm sweep:cargo -- --days 7
pnpm sweep:cargo -- "$HOME/git"  # every Cargo project under that tree
```

With the shared directory configured, sweeping this repo once sweeps that directory. Passing a tree is for machines that still have a `target/` inside each project. `cargo sweep` reads Cargo's fingerprint data. Do not `find -mtime +14 -delete` inside `target/`: that removes objects the current build still names.

## Layout

The React/TypeScript frontend lives at `src/`. The Tauri shell and the engine live at `src-tauri/` (package and binary `dotlore`). `src-tauri/src/main.rs` is the only environment reader. `src-tauri/src/lib.rs` exports the engine modules and `pub fn run(home, home_dir)`.

## UI playground (`mockapp/`)

`mockapp/` is a browser-only preview of the real app UI. It mounts the same `src/App.tsx` with a mocked Tauri IPC layer and selectable fake-data scenarios — useful for iterating on CSS without compiling Rust.

```sh
pnpm mockapp:dev   # http://localhost:38422
```

Pick a scenario from the right sidebar (or `?scenario=<id>`). **Never change `src/` components to make the browser happy** — fix `mockapp/mocks/` instead. Details: [`mockapp/README.md`](mockapp/README.md).

**`mockapp/` never drives a version bump.** It holds real TypeScript and its own vite config, so it reads like app code — it is not. `pnpm mockapp:build` runs a separate config, and the app's own `beforeBuildCommand` (`pnpm ui:build`) never touches it, so not one byte of `mockapp/` reaches a shipped bundle. `/cf-ship` therefore leaves it out of `APP_PATHS`, and a `(mockapp)`-scoped commit is excluded even when it touched app paths. A commit that changes `mockapp/` *and* `src/` still counts, through `src/`.

## Landing page (`website/`)

`website/` is the public landing page: static HTML + CSS, no build, no tests. Open `website/index.html` in a browser. A push to `main` that touches `website/` deploys to GitHub Pages (`dotlore.dinhanhthi.com`). Do not couple it to `src/`. Details: [`website/README.md`](website/README.md).

**`website/` never drives a version bump** either, by both filters: the path is outside `APP_PATHS`, and a `(website)`-scoped commit is excluded even when it touched app paths. A range containing only `website/` and `mockapp/` commits reports `HAS APP CHANGES: no`, which `/cf-ship` treats as *nothing to release* — not as "bump a patch". The one edit `/cf-ship` does make here is the version badge between the `<!-- dotlore:version -->` markers, which `bump.sh` rewrites as a whole element, href and label together.

## Invariants

These are not style rules. The engine's correctness rests on them, and each one exists because breaking it loses or leaks someone's data.

**No shell, ever.** `std::process::Command` with argv arrays. The only external programs this project may run are `git`, `brctl`, `hostname`, and `launchctl`.

*Scope of that rule: the sync engine.* It governs every code path that reads, writes or syncs a user's files — which is why `scripts/build.sh` draws the same line for build scripts rather than claiming to be covered by it. One dependency sits outside it, named here so the invariant stays honest: `tauri-plugin-updater`, reached only from `src-tauri/src/updater.rs` when the user installs an update. On macOS it runs `touch` on the replaced bundle, re-execs the app through `AppHandle::restart`, and — only when the app directory is not writable by the current user — runs an AppleScript `do shell script "rm -rf … && mv -f …" with administrator privileges` to move the new bundle into place. That is a shell, with a prompt for admin rights. It is confined to the update path, it never touches a synced root, and nothing in the engine may call into it.

**Hermetic git.** Every invocation goes through `Git::command`, which clears the child environment and then sets `GIT_CONFIG_GLOBAL=/dev/null`, `GIT_CONFIG_NOSYSTEM=1`, explicit author/committer, `GIT_EDITOR=true`, `GIT_TERMINAL_PROMPT=0`, plus `PATH`. A user's global config — signing, `core.hooksPath`, `autocrlf` — and their `~/.config/git/ignore` and `attributes` must never reach these calls.

**No environment reads.** `config::default_home()` is the only one that affects where state lives, and only `src-tauri/src/main.rs` calls it. `lib.rs` exports the engine modules and `pub fn run(home, home_dir)`. Everything else takes `home: &Path` explicitly, which is what lets the test harness run several devices in one process. (`git::Git::command` reads `PATH` to forward it into the child — see below.)

**The git child environment is an allowlist, not a denylist.** `Git::command` calls `env_clear()` and sets only the hermetic variables plus `PATH`. Never replace this with `env_remove` calls: four consecutive review rounds each found another `GIT_*` variable that had to be added, and two of them (`GIT_CONFIG_PARAMETERS`, `GIT_TEMPLATE_DIR`) were arbitrary code execution via git hooks, reachable despite `GIT_CONFIG_GLOBAL=/dev/null` and `GIT_CONFIG_NOSYSTEM=1`. Clearing is the only form that is closed against variables nobody has thought of yet. Adding something back to the allowlist is a deliberate decision, not a convenience.

**The cloud is immutable.** Bundles are published under a temp name and installed with a no-replace operation. Nothing in the cloud folder is ever rewritten or deleted.

*One scoped exception:* the Settings "Wipe cloud data" action. `Engine::wipe_cloud_data`, reached only from the `wipe_cloud_data` command after the user confirms a dialog, deletes the whole `<provider>/dotlore` folder — every device's bundles — then the local `repos/`, `recovery/` and `tmp/`, and re-adds every registered root so each one re-seeds from the current patterns. It refuses a symlinked target and never touches a live root. The sync engine itself never rewrites or deletes anything in the cloud, and no sync path may call into the wipe.

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
