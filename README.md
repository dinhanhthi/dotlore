# Dotlore

Sync your AI stuff and keep it away from your main codebase.

Dotlore tracks the AI-agent config your projects git-ignore and syncs it between your own Macs. Point it at any folder or single file you keep out of git — `.claude/`, `CLAUDE.md`, `.agents/`, your project's `docs/`, your global `~/.claude`, or whatever else you work with. Files stay where they are; nothing is moved, symlinked, or added to your project's git history.

## How it works

Transport is a cloud folder you already sync (iCloud Drive or Google Drive desktop) — a folder path, never a cloud API.

Each tracked root gets a private staging git repo under `~/Library/Application Support/dotlore/`. Dotlore mirrors the root into staging, commits, and publishes an **immutable** git bundle to its own device directory in the cloud:

```
<cloud>/dotlore/<slug>/devices/<device-id>/000001.bundle
```

Two devices never write the same cloud file, and nothing there is ever rewritten or deleted. Incoming bundles are fetched and merged by the system `git` — so non-overlapping edits just merge.

Overlapping edits resolve deterministically: the newer commit wins on **every** device, and the loser's bytes are kept beside it as `<stem>.conflict-<device>-<blob>.<ext>`. No version is ever lost, no conflict markers are written into a live file, and all devices converge on the same tree.

## Status

MVP, in progress. All three crates exist; the inline conflict resolver is the last piece.

| Crate | State |
|---|---|
| `crates/dotlore-core` | sync engine — git runner, mirroring, cloud bundles, conflict resolution |
| `crates/dotlore-cli` | the `dotlore` binary — subcommands plus `dotlore daemon` |
| `crates/dotlore-app` | GPUI menu-bar app — tray, window, login item (inline conflict resolver in progress) |

## Requirements

macOS, Rust 1.89+ (uses `std::fs::File::lock`), and the system `git`. If `git` is missing, Dotlore refuses to sync and points you at `xcode-select --install`.

## Development

Set `DOTLORE_HOME` so a local run does not write to `~/Library/Application Support/dotlore/`. The directory is created on first use. Do not run `dotlore daemon` and `dotlore-app` against the same home at once — they share a lock file.

```sh
export DOTLORE_HOME="$HOME/Downloads/dotlore"
P=$(mktemp -d)

cargo test  -p dotlore-core
cargo fmt   -p dotlore-core --check

# CLI
cargo run -q -p dotlore-cli -- provider "$P"
cargo run -q -p dotlore-cli -- add /path/to/folder --slug demo
cargo run -q -p dotlore-cli -- sync
cargo run -q -p dotlore-cli -- status
cargo run -q -p dotlore-cli -- daemon          # watch + poll until killed
cargo run -q -p dotlore-cli -- help

# menu-bar app (same DOTLORE_HOME; first launch opens the window if provider is unset)
cargo run -p dotlore-app
```

`cargo build -p dotlore-core` must be warning-free.

## Production

```sh
scripts/build-app.sh                           # → target/Dotlore.app
open target/Dotlore.app
```

The script release-builds `dotlore-app` and `dotlore`, lipos a universal binary when both targets are installed, writes `Info.plist`, ad-hoc codesigns, and ships the CLI beside the app in the bundle. A missing `x86_64`/`aarch64` target degrades to this Mac's arch instead of failing.

To pass a sandbox home into the bundled binary (Finder's `open` does not forward env):

```sh
DOTLORE_HOME="$HOME/Downloads/dotlore" target/Dotlore.app/Contents/MacOS/dotlore-app
```

## Not in scope

Encryption at rest (the threat model is "not in the project's git", not "hide from the cloud provider"), direct cloud APIs, Windows or Linux, and syncing session logs or caches.

## Contributing

See [CONTRIBUTING.md](./CONTRIBUTING.md) — it covers the invariants the engine depends on.
