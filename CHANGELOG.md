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
- Seed lists ship per agent (Claude Code, Codex, Cursor, Gemini, opencode, Continue, Junie, Kiro, Roo, Cline, Windsurf) and per project; both are editable in Settings.
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
- Failures surface as toasts, and sync refuses to run when the system `git` is missing, pointing at `xcode-select --install`.

### Platform

- macOS only: desktop window plus menu-bar item. Windows and Linux are not in scope.
- Rust engine in `src-tauri/` driving the system `git` binary; React 19, TypeScript, Tailwind CSS v4, and CodeMirror 6 in `src/`.

### Out of scope

- Encryption at rest, direct cloud APIs, and syncing session logs or caches. The threat model is "not in the project's git", not "hide from the cloud provider".
