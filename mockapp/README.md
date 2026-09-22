# Dotlore web preview

Browser preview of the Tauri app UI. It mounts the same `../src/App.tsx` with a mocked IPC layer and fake data — for iterating on CSS and layout without compiling Rust.

## Quick start

From the repo root:

```
pnpm mockapp:dev   # http://localhost:38422
```

`pnpm mockapp:build` typechecks and builds the harness.

## The golden rule

**Never edit `../src` to accommodate the browser.** If a component breaks here, fix `mocks/` instead. What you see at localhost:38422 is the same UI that ships in the app.

## Scenarios

Pick a scenario from the right sidebar (or `?scenario=<id>`).

| ID                    | Screen                                              |
| --------------------- | --------------------------------------------------- |
| `setup`               | Provider setup (no cloud folder yet)                |
| `empty`               | Provider set, no tracked roots                      |
| `populated`           | Default — `dotlore` + `CLAUDE.md`                   |
| `conflicts`           | Conflict resolver on `notes` / `CLAUDE.md`          |
| `all-projects`        | All projects grid                                   |
| `git-missing`         | Populated data plus the git-missing banner          |
| `unlinked-project`    | Cloud project in the sidebar, not yet bound         |
| `include-list-editor` | Settings dialog open on the Patterns tab            |
| `oversized-entry`     | `dump.bin` selected — red TooLarge weight           |

## Adding fixtures

Edit `fixtures/` and wire new IPC results in `mocks/invokeRouter.ts`. Add a row to `scenarios/index.ts` if you need another starting screen.
