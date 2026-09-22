# `/cf-ship` for Dotlore — usage

How to release Dotlore. This file is for **you**; `SKILL.md` next to it is the
contract the model follows.

> `.gitignore` ignores `.coding-friend/*` but re-includes
> `!.coding-friend/skills/`, so this guide, `SKILL.md` and the scripts **are**
> version-controlled and do reach a fresh clone. Only `.coding-friend/config.json`
> stays local. The `!` line is load-bearing: a `*` in a gitignore pattern does
> not cross `/`, so `.coding-friend/*` matches the `skills/` directory itself —
> re-including that directory is what lets git descend into it and see the files
> inside.

## What one release actually does

`/cf-ship` reads the commits since the last published tag, picks a version,
writes the changelog, bumps two version files plus the website badge, commits,
tags, pushes, then waits for CI and verifies the published artifacts. CI signs,
notarizes and publishes five assets: a universal `.dmg`, a version-less
`Dotlore-universal.dmg` alias, the updater's `Dotlore.app.tar.gz` and its `.sig`,
and the `latest.json` the in-app updater reads. A run takes **25-35 minutes**,
almost all of it the universal Rust build and Apple's notarization round-trip.

## Say it in one line

**Every release is stable.** There is no prerelease flag and no channel to pick.

| You want                                      | You say          | Version goes                         |
| --------------------------------------------- | ---------------- | ------------------------------------ |
| A normal release                              | `/cf-ship`       | `0.1.0` → `0.1.1`                    |
| Let the model pick the level from the commits | `/cf-ship`       | ↑ same, it decides patch/minor/major |
| Force the level                               | `/cf-ship minor` | `0.1.0` → `0.2.0`                    |

`bump-info.sh` computes the candidates itself and prints them under **Next
version** — the guide tells the model to use that answer verbatim rather than
doing the arithmetic by hand, which is how a release ends up skipping a number.

## One channel, and why `isDraft` matters

Every release is published as a normal release: `isPrerelease: false`. The app's
updater reads `/releases/latest/download/latest.json`, and GitHub's "latest"
excludes prereleases — so a prerelease would reach nobody, and a draft would
strand every client on the previous version. Both flags are checked in Step B8,
along with a non-empty signature for each platform key in `latest.json`.

## The version lives in one file

`src-tauri/Cargo.toml`, `[package] version`. `src-tauri/tauri.conf.json` has no
`version` key on purpose (the bundler resolves it from the manifest), and the
root `package.json` has no `version` field at all. `bump.sh` writes that
manifest, refreshes `src-tauri/Cargo.lock`, rewrites the website badge between
the `<!-- dotlore:version -->` markers in `website/index.html` — href and label
together, since the element carries the version twice — then reads all three
back and refuses to exit clean if they disagree.

It also validates every target **before** the first write: the new version must
be `X.Y.Z` — three numeric fields, no leading zeros, no prerelease suffix — and
strictly greater than the one in the manifest (a version is public and only
fixable forward), the manifest must carry a `[package]` version line, and the
website must carry exactly one marker pair, in order, wrapping the badge. Any
of those failing exits without touching a file. Leading zeros are refused
because nothing downstream normalises them: `01.2.3` compares as `1.2.3`, so it
would be written as a bump and then be rejected by Cargo's own parser, by the
tag shape, and by `bump-info.sh` on the next run.

`release.yml` checks the tag against `src-tauri/Cargo.toml` and fails the build
when they differ. That check is the only reason the badge and the lockfile need
verifying locally: CI cannot see them.

## What the report looks like

```
=== Bump Info — dotlore (single package) ===

Latest published tag:  v0.1.0
Tag source:            origin
File version:          0.1.0  (src-tauri/Cargo.toml)
State:                 bump
Commit range:          v0.1.0..HEAD
Requested level:       (none — decide from the commits below)

--- Next version (computed — do NOT do this arithmetic yourself) ---
  patch    0.1.1
  minor    0.2.0
  major    1.0.0
```

`State` is the thing to read first:

| State                      | Meaning                                                                                                |
| -------------------------- | ------------------------------------------------------------------------------------------------------ |
| `bump`                     | Normal. Pick a version and go.                                                                         |
| `already-bumped`           | The version files are ahead of the tag — the bump already happened. Changelog only; do not bump again. |
| `first-release`            | No tag exists at all. Ship the version already in the files.                                           |
| `BROKEN-tag-ahead-of-file` | Something was tagged without bumping. Stop and untangle it by hand.                                    |

`Commit range` names what the counts below were taken over: `v0.1.0..HEAD` on a
normal run, `(entire history — first release)` when no tag exists at all, and —
under the `BUMP_INFO_TAG` test hook — either the real `v0.1.0..HEAD` range with
`(TEST MODE — tag supplied by BUMP_INFO_TAG)` after it, when the hook's tag does
exist in this clone, or the whole history labelled `(TEST MODE — synthetic tag …
is not in this clone; using the entire history)` when it does not. Any of those
is a real range; a range the script could not resolve would show up as all-zero
counts and `HAS APP CHANGES: no`, which is why it refuses to print one.

## Three things that will bite you

**1. "Nothing to release" is a real answer.**
If the only commits since the last tag touched `website/`, `docs/`, `.github/`
or `.coding-friend/`, or are scoped `(website)`, the report says
`HAS APP CHANGES: no`. That means stop — not "ship a patch anyway". Website
changes deploy through `.github/workflows/pages.yml`, which fires on any push to
`main` that touches `website/**`, and they need no version at all; this guide and
its scripts are release tooling, so they need none either.

**2. There is one changelog, not two.**
`CHANGELOG.md` at the root, developer-facing; `release.yml` extracts the section
matching the tag and that becomes the GitHub Release body. Its `###` headings are
feature-grouped — `### Sync`, `### Projects`, `### Desktop app` — not
`Added`/`Fixed`/`Improved`. There is **no** website changelog file: the only
per-release website edit is the badge `bump.sh` already handles. Entries are net
changes against the previous release, never a commit dump, and each one carries
its commit link, which `bump-info.sh` prints ready-made as a `link=` field
before the quoted subject — the link comes from the printed hash, never from
trailing text on a data line, since a subject can contain anything.

**3. A pushed tag is not a release.**
CI runs for ~30 minutes and can fail at the notarization step long after the tag
looks fine. Step B8 is what turns "green" into "verified": `isDraft` and
`isPrerelease` both `false`, five assets, a non-empty signature under both
`darwin-aarch64` and `darwin-x86_64`, and — on the mounted `.dmg` — `spctl`,
`xcrun stapler validate` and `lipo -archs` all clean.

## When it goes wrong

**The run fails.** No release is published, so nothing has to be deleted — but
the tag is not the retry button. Which lever applies depends on whether the
tagged commit is still correct:

- **Transient failure** (Apple notarization outage, expired credential, runner
  hiccup) with the commit still correct → `gh run rerun <run-id>`. Nothing moves.
- **The fix changes a file the release depends on** — above all
  `.github/workflows/release.yml` — → the tag no longer points at a correct
  commit, and a rerun would replay the broken workflow, because a run reads the
  workflow definition from its own ref. The tag has to move, which the guide
  forbids doing unilaterally: **ask the user.**

A plain `git push origin v<version>` of an unchanged, already-pushed tag is a
no-op — `Everything up-to-date`, no new run. It looks like a retry and is not
one.

**The run is green but the app will not update.** Check `latest.json`: every
platform key needs a non-empty `signature`. An empty one is the silent failure
mode — the release looks perfect and every client refuses the update. Step B8
checks this, but check it yourself if you are suspicious:

```bash
gh release download v<version> -p latest.json -O - | python3 -m json.tool
```

**Gatekeeper warns users.** Notarization did not take. On the downloaded `.dmg`:

```bash
spctl -a -vv -t install "/Volumes/Dotlore/Dotlore.app"   # want: accepted / Notarized Developer ID
```

The signing identity and every secret the workflow needs are documented in
`.github/release-setup.md`.

## What it does not do

- **Rotate the updater signing key.** It _cannot_ be rotated — the public half
  is compiled into every shipped build, so regenerating it strands every existing
  install. Never re-run `tauri signer generate`. See `.github/release-setup.md`.
- **Release the website.** That is `.github/workflows/pages.yml`, triggered by
  any push to `main` touching `website/**` (or a manual dispatch). It needs no
  version: the download button points at the version-less
  `Dotlore-universal.dmg` alias.
- **Release for Windows or Linux.** macOS only today.
- **Handle entitlements or a provisioning profile.** Dotlore declares none, so
  there is no embedded profile to check and nothing that expires.

## Testing the scripts without releasing anything

Two env hooks let you exercise every state without touching the real config or
creating a tag. Never set either during a real release — the output labels itself
`TEST MODE` when they are on, and `SKILL.md` turns that label into a stop
condition.

```bash
B=.coding-friend/skills/cf-ship-custom/scripts/bump-info.sh

# Pretend nothing was ever released — the state this repo is in today.
BUMP_INFO_TAG= bash $B

# The three published-tag states, each read against the file version 0.1.0.
BUMP_INFO_TAG=v0.1.0 BUMP_INFO_VERSION=0.1.0 bash $B   # bump
BUMP_INFO_TAG=v0.2.0 bash $B                           # BROKEN-tag-ahead-of-file
BUMP_INFO_TAG=v0.0.1 bash $B                           # already-bumped
```

Every value above is a plain `vX.Y.Z`, because that is the only shape the tag
list accepts: a suffixed value such as `v0.0.1-test` is refused outright, hook or
not, and the message names it. (Alongside a real `vX.Y.Z` tag it is instead
dropped from the comparison and named in the dropped-tag warning.)

Each of those runs labels itself `TEST MODE`, but not on the same line, so do not
go looking for one particular marker:

| Hook you set        | Where the label is printed                                                                                                                                                                                                 |
| ------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `BUMP_INFO_VERSION` | the `NOTE: BUMP_INFO_VERSION override in effect (TEST MODE — not a real state).` line, at the top of the report                                                                                                            |
| `BUMP_INFO_TAG`     | the `Tag source:` line — `BUMP_INFO_TAG override (TEST MODE — not a real release state)` — in every non-empty-hook run                                                                                                     |
| `BUMP_INFO_TAG`     | *also* the `Commit range:` line — `(TEST MODE — synthetic tag <tag> is not in this clone; using the entire history)` if the hook tag is not in this clone, else `<tag>..HEAD  (TEST MODE — tag supplied by BUMP_INFO_TAG)` |
| `BUMP_INFO_TAG=`    | the `Tag source:` line only — an empty hook value is `first-release`, so the `Commit range:` line is the ordinary `(entire history — first release)`, with no label                                                        |

Of the four commands above, exactly one prints a `NOTE:` line —
`BUMP_INFO_TAG=v0.1.0 BUMP_INFO_VERSION=0.1.0` — and that line is the only thing
`BUMP_INFO_VERSION` adds. The other two tag-valued commands (`v0.2.0`, `v0.0.1`)
print no `NOTE:` line, but they each still carry the label **twice**: the
`Tag source:` line always has it, and so does the `Commit range:` line, in
whichever of the two forms below applies. Only `BUMP_INFO_TAG=` prints a single
marker: with no tag to diff against, its range line is the ordinary
`(entire history — first release)`. Do not read `Tag source:` as the only marker
unless the hook value is empty.

The hook tag is synthetic *before v0.1.0 exists*: with no such tag in the clone
there is no revision to diff against, so the commit range cannot be taken over it.
The script falls back to the whole history and says so on the `Commit range:`
line:

```
Commit range:          (TEST MODE — synthetic tag v0.1.0 is not in this clone; using the entire history)
```

Once `v0.1.0` **is** a tag in the clone — after it ships and a fetch brings it in
— the same command takes the real-tag branch, and the range label changes form
(the `Tag source:` line is labelled either way):

```
Commit range:          v0.1.0..HEAD  (TEST MODE — tag supplied by BUMP_INFO_TAG)
```

Either way `HAS APP CHANGES` and the commit lists describe real commits rather
than counting to zero. The *state* is still decided by the hook tag — that is
what the hook is for.

## Files

| Path                            | Role                                                                                               |
| ------------------------------- | -------------------------------------------------------------------------------------------------- |
| `SKILL.md`                      | The contract the model follows. Loaded by `load-custom-guide.sh`.                                  |
| `scripts/bump-info.sh`          | Reads commits and tags, names the state, computes the next version. Writes nothing.                |
| `scripts/bump.sh`               | Writes the version into `Cargo.toml`, `Cargo.lock` and the website badge, and verifies they agree. |
| `.github/workflows/release.yml` | Signs, notarizes, builds universal, publishes the five assets.                                     |
| `.github/release-setup.md`      | Every secret the workflow needs, and where it comes from.                                          |
| `website/index.html`            | The version badge `bump.sh` rewrites between the `<!-- dotlore:version -->` markers.               |
