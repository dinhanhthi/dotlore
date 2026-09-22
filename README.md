<div align="center">
  <img src="assets/logo_256.png" width="80" alt="Dotlore logo" />
  <h1>Dotlore</h1>
  <p>Sync your AI stuff and keep it away from your main codebase.</p>
  <p>
    <a href="https://dotlore.dinhanhthi.com">Website</a> ·
    <a href="https://github.com/dinhanhthi/dotlore">GitHub</a> ·
    <a href="https://github.com/dinhanhthi/dotlore/releases">Releases</a> ·
    <a href="CONTRIBUTING.md">Contributing</a>
  </p>
</div>

> [!NOTE]
> **macOS** is the current priority. Windows and Linux are coming soon.

Dotlore tracks the AI-agent config your projects git-ignore and syncs it between your own Macs. Point it at a folder — `.claude/`, `.agents/`, your project's `docs/`, your global `~/.claude` — and pick the entries to sync. `CLAUDE.md` is an entry inside a project, not a root of its own. Files stay where they are; nothing is moved, symlinked, or added to your project's git history.

## ✨ Features

- **Files stay put** — projects stay where they are: never moved, never symlinked, never added to git history.
- **Project folders** — a project is always a folder with a synced include-list. `.claude/`, `CLAUDE.md`, `.agents/`, `docs/`, and `~/.claude` are entries inside it, not roots of their own.
- **Cloud folder, not a cloud API** — sync through iCloud Drive or Google Drive desktop, a folder path you already have.
- **Immutable bundles** — each Mac publishes its own git bundle; nothing in the cloud folder is rewritten or deleted.
- **Conflicts without markers** — the newer commit wins on every device; the loser's bytes stay beside it as a sibling file.
- **Desktop + menu bar** — one three-column window (sidebar, file tree, viewer/resolver) and a compact status item.

## 🔄 How it works

Transport is a cloud folder you already sync (iCloud Drive or Google Drive desktop) — a folder path, never a cloud API.

Each project gets a private staging git repo under `~/Library/Application Support/dotlore/`. Dotlore mirrors the include-list into staging, commits, and publishes an **immutable** git bundle to its own device directory in the cloud:

```
<cloud>/dotlore/<slug>/devices/<device-id>/000001.bundle
```

Two devices never write the same cloud file, and nothing there is ever rewritten or deleted. Incoming bundles are fetched and merged by the system `git` — so non-overlapping edits just merge.

Overlapping edits resolve deterministically: the newer commit wins on **every** device, and the loser's bytes are kept beside it as `<stem>.conflict-<device>-<blob>.<ext>`. No version is ever lost, no conflict markers are written into a live file, and all devices converge on the same tree.

## 💻 Platforms

**macOS** (desktop + menu bar) is the current priority. Windows and Linux are coming soon. Encryption at rest, direct cloud APIs, and syncing session logs or caches are out of scope — the threat model is "not in the project's git", not "hide from the cloud provider".

## 🛠️ Tech stack

| Layer    | Technology                                                                 |
| -------- | -------------------------------------------------------------------------- |
| Engine   | Rust in `src-tauri/` (package and binary `dotlore`)                        |
| Git      | System `git` binary (no git library)                                       |
| App      | Tauri 2; React 19 + TypeScript + Vite in `src/`                            |
| UI       | Tailwind CSS v4, shadcn/ui                                                 |
| Editor   | CodeMirror 6 (`@codemirror/merge` for conflicts)                           |
| Login    | macOS `launchctl`                                                          |

## 🚀 Development

**Prerequisites:** [Rust](https://rustup.rs/) 1.89+ (uses `std::fs::File::lock`), [Node.js](https://nodejs.org/) 20+, [pnpm](https://pnpm.io/), the system `git`, and the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for macOS. If `git` is missing, Dotlore refuses to sync and points you at `xcode-select --install`.

`pnpm tauri dev` keeps its state in `~/Downloads/dotlore-dev` instead of `~/Library/Application Support/dotlore/`, so a dev run can sit beside a copy installed in `/Applications`. It prints the path it used, and an explicit `DOTLORE_HOME` overrides it. The directory is created on first use. Two copies sharing one home fight over a lock file, and the loser exits without a window.

```bash
pnpm install

pnpm tauri dev                    # desktop app (opens the window if provider is unset)
pnpm ui:dev                       # frontend only, from src/
pnpm mockapp:dev                  # browser UI preview (mocked backend) — http://localhost:38422

pnpm test
pnpm smoke:two-devices
pnpm format:check
```

### 📦 Build

```bash
pnpm build                        # → src-tauri/target/release/bundle/{macos/Dotlore.app, dmg/*.dmg}
# pnpm tauri build                # same thing
open src-tauri/target/release/bundle/macos/Dotlore.app
```

> [!IMPORTANT]
> `pnpm build` needs the updater signing key. `tauri.conf.json` sets
> `bundle.createUpdaterArtifacts: true` and carries a `plugins.updater.pubkey`,
> so the bundler signs `Dotlore.app.tar.gz` and fails if the private key is not
> exported:
>
> ```bash
> export TAURI_SIGNING_PRIVATE_KEY="$(cat ~/.tauri/dotlore.key)"
> export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="…"
> ```
>
> `pnpm tauri dev` is unaffected — it does not bundle.

To pass a sandbox home into the bundled binary (Finder's `open` does not forward env):

```bash
DOTLORE_HOME="$HOME/Downloads/dotlore" src-tauri/target/release/bundle/macos/Dotlore.app/Contents/MacOS/dotlore
```

## 🌐 Website

The landing page is at [dotlore.dinhanhthi.com](https://dotlore.dinhanhthi.com). Source is static HTML in [`website/`](website/). A push to `main` deploys it. Open `website/index.html` in a browser to preview. No build step.

## 🤝 Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) — it covers the invariants the engine depends on.

## 📄 License

Dotlore is licensed under [MIT](LICENSE).
