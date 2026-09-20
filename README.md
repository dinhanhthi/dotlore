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

## Tech stack

- Rust workspace: `dotlore-core` (engine), `dotlore-cli` (the `dotlore` binary), `dotlore-app` (desktop)
- System `git` binary — no git library
- Tauri v2 + React + TypeScript + Tailwind + shadcn/ui
- CodeMirror 6 (`@codemirror/merge`) for view and resolve
- macOS `launchctl` for the login item

## Requirements

macOS, Rust 1.89+ (uses `std::fs::File::lock`), and the system `git`. If `git` is missing, Dotlore refuses to sync and points you at `xcode-select --install`. The desktop UI also needs Node 20+ and pnpm.

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
pnpm --dir crates/dotlore-app/ui install
DOTLORE_HOME="$DOTLORE_HOME" cargo tauri dev
```

`cargo build -p dotlore-core` must be warning-free.

`cargo build -p dotlore-app` needs `ui/dist` first (`pnpm --dir crates/dotlore-app/ui build`) and a sidecar at `crates/dotlore-app/binaries/dotlore-<host-triple>` — `externalBin` is copied at cargo-build time.

## Build

```sh
bash scripts/build.sh                          # → target/release/bundle/macos/Dotlore.app
open target/release/bundle/macos/Dotlore.app
```

To pass a sandbox home into the bundled binary (Finder's `open` does not forward env):

```sh
DOTLORE_HOME="$HOME/Downloads/dotlore" target/release/bundle/macos/Dotlore.app/Contents/MacOS/dotlore-app
```

## Not in scope

Encryption at rest (the threat model is "not in the project's git", not "hide from the cloud provider"), direct cloud APIs, Windows or Linux, and syncing session logs or caches.

## Contributing

See [CONTRIBUTING.md](./CONTRIBUTING.md) — it covers the invariants the engine depends on.
