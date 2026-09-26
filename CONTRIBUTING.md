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

`cargo build --manifest-path src-tauri/Cargo.toml` must be warning-free. `pnpm tauri dev` and `pnpm build` build the UI from `src/` into `dist` for you.

### A dev run has its own state directory

`pnpm dev` and `pnpm tauri dev` set `DOTLORE_HOME` to `~/Downloads/dotlore-dev`, so a dev run can sit beside the installed app (both lock `<home>/app.lock`; the second one exits silently). An explicit `DOTLORE_HOME` overrides it. The path lives only in `scripts/dev-home.sh` — keep it that way, or `pnpm reset:dev` could wipe the installed app's state.

Still shared with the installed app:

- **Start at login** — toggle it only in the installed app, or macOS launches your debug binary at login.
- **Updates** — in a dev run, choose "Later" on any update prompt, or the released app overwrites your dev bundle.
- **Cloud folder** — a dev run is a separate device. Point it at a different cloud folder, or it leaves a permanent `devices/<id>/` entry.

### Cargo build cache

`src-tauri/Cargo.toml` sets a `dev` profile that keeps `target/` small on macOS:

```toml
[profile.dev]
opt-level = 1
split-debuginfo = "off"               # no per-codegen-unit .o files (the bulk of a 10 GB target)

[profile.dev.package."*"]
debug = "line-tables-only"            # dependencies keep enough for backtraces
incremental = false
```

To share one cache across Rust projects, create `~/.cargo/config.toml` with `[build] target-dir = ".cargo/shared-target"` plus the same profile (see [README](README.md#cargo-build-cache)). Rules:

- Keep that profile identical to `src-tauri/Cargo.toml`; it overrides every project.
- Delete old per-project `target/` directories afterwards — Cargo no longer sees them.
- Build one project at a time. `cargo clean` wipes the whole shared cache.
- `scripts/build.sh` pins `CARGO_TARGET_DIR` to `src-tauri/target` so release bundles stay in the repo. Leave it.

Clean up stale artifacts with `pnpm sweep:cargo` (needs `cargo install cargo-sweep`):

```sh
pnpm sweep:cargo                 # unused for 14 days
pnpm sweep:cargo -- --dry-run
pnpm sweep:cargo -- --days 7
pnpm sweep:cargo -- "$HOME/git"  # every Cargo project under that tree
```

Never `find -mtime … -delete` inside `target/`.

## Layout

- `src/` — React/TypeScript frontend.
- `src-tauri/` — Tauri shell and engine (package and binary `dotlore`). `main.rs` is the only environment reader; `lib.rs` exports `pub fn run(home, home_dir)`.
- `mockapp/` — browser-only UI preview with mocked IPC: `pnpm mockapp:dev` (http://localhost:38422). Fix `mockapp/mocks/`, never `src/`, to make it work. See [`mockapp/README.md`](mockapp/README.md).
- `website/` — static landing page, deployed to GitHub Pages on push to `main`. Do not couple it to `src/`. See [`website/README.md`](website/README.md).

`mockapp/` and `website/` never ship and never trigger a version bump.

## Invariants

Breaking any of these loses or leaks user data.

- **No shell, ever.** `std::process::Command` with argv arrays. Only `git`, `brctl`, `hostname`, `launchctl` may run. This governs the sync engine. _Exception:_ `tauri-plugin-updater`, reached only from `src-tauri/src/updater.rs`, runs `touch`, re-execs the app, and may use an admin AppleScript to move the new bundle. Nothing in the engine may call it.
- **Hermetic git.** Every call goes through `Git::command`, which runs `env_clear()` and sets only the hermetic variables (`GIT_CONFIG_GLOBAL=/dev/null`, `GIT_CONFIG_NOSYSTEM=1`, author/committer, `GIT_EDITOR=true`, `GIT_TERMINAL_PROMPT=0`) plus `PATH`. Never switch to `env_remove`: `GIT_CONFIG_PARAMETERS` and `GIT_TEMPLATE_DIR` both reach code execution, and a denylist cannot keep up.
- **No environment reads.** Only `main.rs` calls `config::default_home()`. Everything else takes `home: &Path`.
- **The cloud is immutable.** Publish under a temp name, install with no-replace, never rewrite or delete. _Exception:_ `Engine::wipe_cloud_data`, reached only from the Settings "Wipe cloud data" confirm dialog. No sync path may call it.
- **Live files are sacred.** Conflict markers, `.conflict-*` siblings, `.dotloreignore` and `.dotloreproject` stay in staging. `mirror::apply_to_root` fails closed: drifted paths are skipped and reported, never overwritten.
- **Symlinks are never followed** and never replaced.

## Trust boundaries

Treat as attacker-controlled:

1. **The cloud folder** — `manifest.json`, `device.json`, bundle and device names. Every path component goes through the single-plain-component guard.
2. **Incoming change sets** — reject `..`, absolute, empty and out-of-list paths, and check the source file in staging for symlinks too.

Synced files hold API keys: widening permissions, leaving temp copies, or logging content are security bugs.

## Dependencies

Locked to `anyhow`, `serde`, `serde_json`, `ignore`, `notify` (`tempfile` for tests). Prefer stdlib; a new crate needs a reason.

## Tests

```sh
pnpm test
pnpm smoke:two-devices
cargo test --manifest-path src-tauri/Cargo.toml
cargo fmt --manifest-path src-tauri/Cargo.toml --check
```

In-file `#[cfg(test)]` modules with `tempfile::TempDir`. A test must fail when its fix is reverted — check it. The two-device smoke test is `src-tauri/tests/two_devices.rs`.

## Commits

One line, conventional: `<type>(<scope>): <summary>`. No body, no AI co-author trailers.
