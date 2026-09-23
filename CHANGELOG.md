## v0.2.0 (2026-09-23)

### Conflicts

- A dedicated Conflicts view, reachable from the sidebar and the footer status, lists every unresolved conflict across projects. [#85f007b9](https://github.com/dinhanhthi/dotlore/commit/85f007b9) [#01d9d0a3](https://github.com/dinhanhthi/dotlore/commit/01d9d0a3)
- A collapsed sidebar section header flags conflicts inside it, so they are never hidden by collapsing the list. [#235de343](https://github.com/dinhanhthi/dotlore/commit/235de343)
- The two sides of a conflict are labeled "on this Mac" and "from cloud", with a vertical divider between them and the conflict status colour tinting the icons. [#e91d89c7](https://github.com/dinhanhthi/dotlore/commit/e91d89c7) [#42f1fb81](https://github.com/dinhanhthi/dotlore/commit/42f1fb81) [#022997a1](https://github.com/dinhanhthi/dotlore/commit/022997a1) [#88130dc4](https://github.com/dinhanhthi/dotlore/commit/88130dc4)

### Settings

- A "Wipe cloud data" button in Settings, behind a confirm dialog, clears the cloud folder and re-seeds every root. [#bdaa1e0d](https://github.com/dinhanhthi/dotlore/commit/bdaa1e0d) [#df0f9e55](https://github.com/dinhanhthi/dotlore/commit/df0f9e55) [#bd05db67](https://github.com/dinhanhthi/dotlore/commit/bd05db67) [#fde915e5](https://github.com/dinhanhthi/dotlore/commit/fde915e5) [#2316b327](https://github.com/dinhanhthi/dotlore/commit/2316b327)
- The cloud folder is picked through a provider and account dropdown instead of a raw path. [#a2f4f3e7](https://github.com/dinhanhthi/dotlore/commit/a2f4f3e7)
- Seed list controls sit on one compact row with the hint below. [#9f4ba5ae](https://github.com/dinhanhthi/dotlore/commit/9f4ba5ae)
- Default seed patterns for agents were trimmed down. [#3c07839a](https://github.com/dinhanhthi/dotlore/commit/3c07839a) [#224f5d0a](https://github.com/dinhanhthi/dotlore/commit/224f5d0a)

### Desktop app

- Wipe, provider switch, agent import, and other slow writes now run as footer tasks instead of freezing the window. [#3891cc98](https://github.com/dinhanhthi/dotlore/commit/3891cc98) [#2316b327](https://github.com/dinhanhthi/dotlore/commit/2316b327)
- Dropdowns close as soon as an item is picked. [#913a7e50](https://github.com/dinhanhthi/dotlore/commit/913a7e50)
- Spinning icons stay centered while rotating. [#e1a22252](https://github.com/dinhanhthi/dotlore/commit/e1a22252)
- Dark-mode editor background and text colours are softer. [#15427d35](https://github.com/dinhanhthi/dotlore/commit/15427d35)
- The Dock icon is drawn from an Icon Composer document. [#089657d8](https://github.com/dinhanhthi/dotlore/commit/089657d8) [#55362da1](https://github.com/dinhanhthi/dotlore/commit/55362da1)

### Platform

- A dev run gets its own state directory. [#1132822c](https://github.com/dinhanhthi/dotlore/commit/1132822c)

## v0.1.0 (2026-09-22)

First release. Dotlore tracks the AI-agent config your projects git-ignore and syncs it between your own Macs through a cloud folder you already sync.

### Sync

- Sync through a folder path — iCloud Drive or Google Drive desktop — never a cloud API.
- Each Mac publishes its own immutable git bundle under `<cloud>/dotlore/<slug>/devices/<device-id>/`; nothing in the cloud folder is rewritten or deleted, and two devices never write the same cloud file.
- Incoming bundles are fetched and merged by the system `git`, so non-overlapping edits merge on their own.
- A background daemon watches tracked roots and polls every 30 s; a burst of saves collapses into one cycle, and session logs and caches are filtered out before they wake it.
- Overlapping edits resolve deterministically: the newer commit wins on every device, and the loser's bytes stay beside it as `<stem>.conflict-<device>-<blob>.<ext>`. No conflict marker is ever written into a live file, and all devices converge on the same tree.
- Binary conflicts are resolved by choosing one side instead of merging bytes.

### Projects

- A project is a folder with a committed include-list (`.dotloreproject`); `CLAUDE.md`, `.claude/`, `.agents/`, and `docs/` are entries inside it, not roots of their own.
- Files stay where they are — never moved, never symlinked, never added to your project's git history.
- Every project gets a private staging git repo under `~/Library/Application Support/dotlore/`.
- Entries are tracked and untracked from the file tree, including nested paths.
- Seed lists ship per agent (Claude Code, Codex, Cursor, Gemini, OpenCode, Continue, Junie, Kiro, Roo, Cline, Windsurf) and per project; both are editable in Settings.
- A never-list (`.dotloreignore`) keeps paths out of a project's sync.
- Size limits of 50 MB per file and 200 MB per seed folder, both configurable.
- An agent home such as `~/.claude` syncs as a root of its own, and agent homes already installed on the Mac can be imported into the sidebar in one step.
- A project folder that moved or went missing is flagged in the sidebar, and sync resumes on its own once the folder is back at its path.

### Desktop app

- One three-column window: sidebar, file tree, and viewer/conflict resolver.
- A menu-bar status item shows sync state and offers Open Dotlore, Sync Now, and Quit Dotlore.
- File preview with syntax highlighting for JSON, Markdown, and YAML, a word-wrap toggle, and Reveal in Finder.
- Side-by-side conflict resolver with Keep LIVE / Keep OTHER and previous/next conflict navigation.
- Settings for the cloud folder, seed lists, pattern catalogs, never-list, and size limits, plus Start at login.
- First run picks the cloud folder inside iCloud Drive or Google Drive; Dotlore creates its own `dotlore/` subfolder there.
- Start at login installs a macOS LaunchAgent, and a second copy on the same home exits quietly instead of opening a duplicate window.
- Check for Updates… in the app menu, plus a silent check at launch: a signed update is offered in a dialog, downloaded and installed, and the app restarts into it.
- Failures surface as toasts, and sync refuses to run when the system `git` is missing, pointing at `xcode-select --install`.

### Platform

- macOS (desktop window plus menu-bar item) is the current priority; Windows and Linux are coming soon.
- Rust engine in `src-tauri/` driving the system `git` binary; React 19, TypeScript, Tailwind CSS v4, and CodeMirror 6 in `src/`.

### Out of scope

- Encryption at rest, direct cloud APIs, and syncing session logs or caches. The threat model is "not in the project's git", not "hide from the cloud provider".
