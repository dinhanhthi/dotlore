# dotlore

Sync your AI stuff and keep it away from your main codebase.

dotlore tracks the AI-agent config your projects git-ignore — `.claude/`, `CLAUDE.md`, `.agents/`, `docs/`, and your global `~/.claude` — and syncs it between your own Macs. Files stay where they are; nothing is moved, symlinked, or added to your project's git history.

## How it works

Transport is a cloud folder you already sync (iCloud Drive or Google Drive desktop) — a folder path, never a cloud API.

Each tracked root gets a private staging git repo under `~/Library/Application Support/dotlore/`. dotlore mirrors the root into staging, commits, and publishes an **immutable** git bundle to its own device directory in the cloud:

```
<cloud>/dotlore/<slug>/devices/<device-id>/000001.bundle
```

Two devices never write the same cloud file, and nothing there is ever rewritten or deleted. Incoming bundles are fetched and merged by the system `git` — so non-overlapping edits just merge.

Overlapping edits resolve deterministically: the newer commit wins on **every** device, and the loser's bytes are kept beside it as `<stem>.conflict-<device>-<blob>.<ext>`. No version is ever lost, no conflict markers are written into a live file, and all devices converge on the same tree.

## Status

MVP, in progress. `dotlore-core` is the only crate that exists today.

| Crate | State |
|---|---|
| `crates/dotlore-core` | sync engine — git runner, mirroring, cloud bundles, conflict resolution |
| `crates/dotlore-cli` | planned — command-line front end |
| `crates/dotlore-app` | planned — GPUI menu-bar app with an inline conflict resolver |

## Requirements

macOS, Rust 1.89+ (uses `std::fs::File::lock`), and the system `git`. If `git` is missing, dotlore refuses to sync and points you at `xcode-select --install`.

```sh
cargo build -p dotlore-core
cargo test -p dotlore-core
```

## Not in scope

Encryption at rest (the threat model is "not in the project's git", not "hide from the cloud provider"), direct cloud APIs, Windows or Linux, and syncing session logs or caches.

## Contributing

See [CONTRIBUTING.md](./CONTRIBUTING.md) — it covers the invariants the engine depends on.
