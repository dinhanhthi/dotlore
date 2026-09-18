# Contributing

## Build

```sh
cargo build -p dotlore-core          # must be warning-free
cargo test  -p dotlore-core
cargo fmt   -p dotlore-core --check
```

Zero warnings is a gate, not a preference.

## Invariants

These are not style rules. The engine's correctness rests on them, and each one exists because breaking it loses or leaks someone's data.

**No shell, ever.** `std::process::Command` with argv arrays. The only external programs this project may run are `git`, `brctl`, `hostname`, and `launchctl`.

**Hermetic git.** Every invocation sets `GIT_CONFIG_GLOBAL=/dev/null`, `GIT_CONFIG_NOSYSTEM=1`, explicit author/committer, `GIT_EDITOR=true`, `GIT_TERMINAL_PROMPT=0`, and clears `GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE`, `GIT_OBJECT_DIRECTORY`, `GIT_COMMON_DIR`. A user's global config — signing, `core.hooksPath`, `autocrlf` — must never reach these calls.

**No environment reads.** `config::default_home()` is the single exception in the crate, and only binaries call it. Everything else takes `home: &Path` explicitly, which is what lets the test harness run several devices in one process.

**The cloud is immutable.** Bundles are published under a temp name and installed with a no-replace operation. Nothing in the cloud folder is ever rewritten or deleted.

**Live files are sacred.** Tracked files stay in place. Conflict markers never reach them, and neither do `.conflict-*` siblings or `.dotloreignore` — those live in staging only. `mirror::apply_to_root` fails closed: a missing snapshot expectation is an error, and a path that drifted from its expectation is skipped and reported, never overwritten.

**Symlinks are never followed** in either direction, and a symlinked path is never replaced.

## Trust boundaries

Two inputs are attacker-controlled and must be treated as such:

1. **The cloud folder.** `manifest.json`, `device.json`, bundle filenames, and device directory names were all written by another device. Anything used to build a filesystem path goes through the single-plain-component guard.
2. **The change set applied to a live root.** It originates in another device's git bundle. Reject traversal (`..`, absolute, empty), and check the *source* file in staging too — git stores symlinks as mode-120000 blobs, so a checkout can put one there.

The files this tool syncs routinely hold API keys. Widening a file's permissions, leaving a temp copy behind, or logging content are security bugs, not cosmetics.

## Dependencies

Locked to `anyhow`, `serde`, `serde_json`, `ignore`, `notify`, with `tempfile` for tests. Stdlib before a new crate — `/dev/urandom` over a uuid crate, `$HOME` over a dirs crate, `std::fs::File::lock` over a lock crate. Adding a dependency needs a reason that a few lines of stdlib cannot cover.

## Tests

In-file `#[cfg(test)]` modules using `tempfile::TempDir`. A test earns its place by failing when its fix is reverted — check that, rather than assuming it. A test that passes either way is worse than no test, because it claims coverage that is not there.

## Commits

One line, conventional: `<type>(<scope>): <summary>`. No body, no AI co-author trailers.
