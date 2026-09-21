# Contributing

## Build

From the repo root:

```sh
pnpm install
pnpm tauri dev
pnpm test
pnpm format:check
pnpm build
```

The scripts in `package.json` wrap the engine gates. Those gates still apply: `cargo build -p dotlore-core` and `cargo build -p dotlore-app` must be warning-free. `pnpm tauri dev` and `pnpm build` prepare the CLI sidecar (`crates/dotlore-app/binaries/dotlore-<host-triple>`) and `ui/dist` so you do not have to.

## UI playground (`mockapp/`)

`crates/dotlore-app/ui/mockapp/` is a browser-only preview of the real app UI. It mounts the same `crates/dotlore-app/ui/src/App.tsx` with a mocked Tauri IPC layer and selectable fake-data scenarios — useful for iterating on CSS without compiling Rust.

```sh
pnpm mockapp:dev   # http://localhost:38422
```

Pick a scenario from the right sidebar (or `?scenario=<id>`). **Never change `ui/src` components to make the browser happy** — fix `ui/mockapp/mocks/` instead. Details: [`crates/dotlore-app/ui/mockapp/README.md`](crates/dotlore-app/ui/mockapp/README.md).

## Landing page (`website/`)

`website/` is the public landing page: static HTML + CSS, no build, no tests. Open `website/index.html` in a browser. Do not couple it to `crates/dotlore-app/ui`. Details: [`website/README.md`](website/README.md).

## Invariants

These are not style rules. The engine's correctness rests on them, and each one exists because breaking it loses or leaks someone's data.

**No shell, ever.** `std::process::Command` with argv arrays. The only external programs this project may run are `git`, `brctl`, `hostname`, and `launchctl`.

**Hermetic git.** Every invocation goes through `Git::command`, which clears the child environment and then sets `GIT_CONFIG_GLOBAL=/dev/null`, `GIT_CONFIG_NOSYSTEM=1`, explicit author/committer, `GIT_EDITOR=true`, `GIT_TERMINAL_PROMPT=0`, plus `PATH`. A user's global config — signing, `core.hooksPath`, `autocrlf` — and their `~/.config/git/ignore` and `attributes` must never reach these calls.

**No environment reads.** `config::default_home()` is the only one that affects where state lives, and only binaries call it. Everything else takes `home: &Path` explicitly, which is what lets the test harness run several devices in one process. (`git::Git::command` reads `PATH` to forward it into the child — see below.)

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

In-file `#[cfg(test)]` modules using `tempfile::TempDir`. A test earns its place by failing when its fix is reverted — check that, rather than assuming it. A test that passes either way is worse than no test, because it claims coverage that is not there.

## Commits

One line, conventional: `<type>(<scope>): <summary>`. No body, no AI co-author trailers.
