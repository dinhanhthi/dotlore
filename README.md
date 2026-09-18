# dotlore
Sync your AI stuff and keep it away from your main codebase.

## Layout
- `crates/dotlore-core` — sync engine: staging git repos, mirroring, cloud bundles, conflict resolution.
- `crates/dotlore-cli` — command-line front end for the engine.
- `crates/dotlore-app` — GPUI menu-bar app with the inline conflict resolver.
