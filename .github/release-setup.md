# Release setup

How to publish a signed, notarized macOS build of Dotlore from your Mac with
`scripts/release-local.sh` (`pnpm release`). Maintainer reference.

> This lives in `.github/` rather than `docs/` on purpose: `docs/` is gitignored
> in this repository, so anything written there never reaches a clone. This file
> is self-sufficient: you can set up a Mac for releasing from it alone.

## How a release happens

1. `/cf-ship` decides the version bump from the commits, writes the
   `CHANGELOG.md` section, bumps the version in `src-tauri/Cargo.toml`,
   `src-tauri/Cargo.lock` and the website badge, commits and pushes to `main`.
2. `pnpm release --expect-version X.Y.Z` then, in order:
   - runs the preflight: `gh` is authenticated, `origin` is
     `dinhanhthi/dotlore`, `src-tauri/Cargo.toml` is at `X.Y.Z`, `CHANGELOG.md`
     has a non-empty `## vX.Y.Z` section, you are on a clean `main` equal to
     `origin/main`, the tag `vX.Y.Z` exists neither locally nor on `origin`, and
     every one-time setup item below is in place;
   - runs `scripts/build.sh` — the same script that builds Dotlore day to day —
     with Developer ID signing, App Store Connect notarization and minisign
     updater signing enabled;
   - checks every artifact (see [Verifying a release](#verifying-a-release))
     and generates `latest.json`;
   - pushes the lightweight tag `vX.Y.Z`, creates a **draft** release with the
     changelog section as notes, uploads five assets, then publishes it;
   - dispatches `pages.yml` so the website shows the new version.
3. Installed apps read `latest.json` and offer the update.

The five assets: the versioned `.dmg`, a version-less `Dotlore-universal.dmg`
alias (the website's Download button), `Dotlore.app.tar.gz`, its `.sig`, and
`latest.json`. Nothing is tagged or published unless the build and every check
pass.

A full run takes about 25-35 minutes, most of it the universal build and
Apple's notarization. Run it in its own terminal, or in the background from an
agent (it outlives a 10-minute command timeout).

Other modes, each with `--expect-version`:

- `--dry-run` — the preflight only.
- `--build-only` — build and every artifact check, no tag and no publish. Skips
  the clean-`main` and tag-absent checks, so it works on an already-released
  version. Leaves `latest.json` next to the bundle and prints the artifact
  paths.
- `--publish-only` — resume after the tag push, reusing the built artifacts.

**Dotlore ships stable only.** There is no `-rc` / `-beta` channel, no
prerelease flag and no channel selector. Every release is served to every
install through `/releases/latest/download/latest.json`, which is also why the
release must not stay a draft: GitHub's "latest" serves neither drafts nor
prereleases, and either would strand every install on the previous version.

The version lives in **one** place, `src-tauri/Cargo.toml` `[package] version`.
`src-tauri/tauri.conf.json` deliberately has no `version` key — the bundler
resolves it from the manifest — and the root `package.json` has no `version`
field. Do not add one; the script's `--expect-version` check is the single place
the tag and the manifest are compared, and it refuses to build when they differ.

## One-time setup on a Mac

The script reads every secret from the login keychain and `~/.tauri`, passes it
to `scripts/build.sh` as an environment prefix only, and never prints it. Each
missing item fails the preflight and names it, most with the command that fixes
it.

### Code signing

The **Developer ID Application** identity
`Developer ID Application: Anh-Thi Dinh (86H6CNLN4C)` (certificate and private
key) must be in the login keychain. Check with:

```bash
security find-identity -v -p codesigning
```

> An **Apple Development** certificate is not a substitute. It cannot be
> distributed, and the script checks the bundle's signing authority by name
> rather than shipping it.

`scripts/build.sh` passes the identity to the bundler as an explicit `--config`
override rather than relying on whether the environment variable outranks
`bundle.macOS.signingIdentity` in `tauri.conf.json` — that precedence is
undocumented, and getting it wrong ships an ad-hoc-signed bundle that Gatekeeper
blocks on every user's Mac.

There is no provisioning profile. Dotlore declares no restricted entitlements,
so it needs none — and no entitlements plist either. Hardened runtime is already
on; only the signing identity changes for a release.

### Notarization (App Store Connect API key)

Key ID `Q76HXMGCWL`. Install its `.p8`:

```bash
install -d -m 700 ~/.appstoreconnect/private_keys
install -m 600 <path to AuthKey_Q76HXMGCWL.p8> ~/.appstoreconnect/private_keys/
```

The `.p8` downloads **once** and Apple will not re-issue it — keep a copy in a
password manager.

Store the Issuer ID (App Store Connect → Users and Access → Integrations → App
Store Connect API, shown above the keys table) in the keychain. `-w` last makes
`security` prompt for it, so it never lands on the command line or in shell
history:

```bash
security add-generic-password -a "$USER" -s dotlore-notary-issuer -w
```

Without a working key, notarization does not happen and Gatekeeper blocks the
result on every Mac that did not build it. The script's stapler and `spctl`
checks catch that before anything is tagged.

### Updater signing (minisign)

The minisign private key lives at `~/.tauri/dotlore.key`. Store its password in
the keychain the same way:

```bash
security add-generic-password -a "$USER" -s dotlore-updater-signing -w
```

Both items keep the default access list: the `security` tool that created them
reads them without a prompt, so a release never asks for anything. The cost is
that any process running as you can do the same, and `~/.tauri/dotlore.key` is
on the same disk. For a prompt on every read (your login password, twice per
release, and a background `/cf-ship` run waits for it), create them with
`-T ""` instead of the default.

> 🚨 **This key can never be rotated.** Its public half is compiled into every
> shipped build (`src-tauri/tauri.conf.json` → `plugins.updater.pubkey`) and
> Tauri's updater has no in-band key rotation. Replacing it means every existing
> install silently stops being able to verify updates and has to be re-downloaded
> by hand. Keep the private key and its password backed up in a password
> manager, and **never re-run `tauri signer generate`** for this project. On a
> new Mac, restore `~/.tauri/dotlore.key` from the backup.

Because it cannot be rotated, a leak is not recoverable by re-signing. Two
things follow, both already in place:

- The script passes the key only as an environment prefix on the
  `scripts/build.sh` call, never exported and never on a command line.
- `scripts/ui-build.sh` scrubs it from the environment before the frontend
  build, so no vite plugin, JS dependency or lifecycle script in a release
  build can read it out of `process.env`. That is the only reason that wrapper
  script exists — do not replace it with a bare `vite build`.

`bundle.createUpdaterArtifacts` is `true`, so a plain `pnpm build` fails without
`TAURI_SIGNING_PRIVATE_KEY` in the environment. `pnpm release` supplies it;
`pnpm tauri dev` is unaffected.

### Toolchain and GitHub

```bash
rustup target add aarch64-apple-darwin x86_64-apple-darwin
gh auth login
```

Both Rust targets are required: without one, `scripts/build.sh` would quietly
build a single-arch bundle, so the script fails instead.

### First run

The first run may show keychain prompts (`codesign` using the Developer ID key,
`security` reading an item). Answer **Always Allow** so later runs are
unattended.

Rehearse on the version already released, which tags and publishes nothing:

```bash
pnpm release --expect-version <current version> --build-only
```

It runs the setup checks first, so a missing item fails in seconds, then proves
the signing, notarization and updater signing end to end. `--dry-run` checks
the setup without building, but only for a version not yet tagged: on a released
version it stops at "tag already exists" before the setup checks. Run it after
`/cf-ship` has bumped the version, right before the real release:

```bash
pnpm release --expect-version X.Y.Z --dry-run
```

## Verifying a release

The script asserts all of this before the tag push, and the release state after
publishing:

- `Info.plist` `CFBundleShortVersionString` is `X.Y.Z`.
- `lipo -archs` on `Contents/MacOS/dotlore` has both `x86_64` and `arm64`.
- `codesign` reports `Authority=Developer ID Application: Anh-Thi Dinh (86H6CNLN4C)`.
- `xcrun stapler validate` passes and `spctl` reports `Notarized Developer ID`.
- `Dotlore.app.tar.gz` and its `.sig` exist and are non-empty.
- Exactly one `.dmg`, and its name carries the version.
- `latest.json` has the version, both `darwin-aarch64` and `darwin-x86_64`
  (plus `darwin-universal`), signatures that base64-decode to a minisign
  signature file, and `https` URLs.
- After publishing: `isDraft` and `isPrerelease` are both false, and GitHub's
  latest release is the new tag.

A second look, not the gate:

```bash
gh release view <tag> --json isDraft,isPrerelease,assets
# expect: isDraft false, isPrerelease false, and FIVE assets.

# latest.json must carry a non-empty signature under BOTH darwin keys. An empty
# one is the silent failure mode of updater signing: the release looks complete
# and every client refuses the update, possibly for months.
gh release download <tag> -p latest.json -O - | python3 -m json.tool

# On a Mac that has never built this app — mount the dmg, then:
APP="$(mount | sed -n 's/.* on \(\/Volumes\/Dotlore[^ ]*\) .*/\1/p' | head -1)/Dotlore.app"
spctl -a -vv -t install "$APP"                 # expect: accepted, Notarized Developer ID
xcrun stapler validate "$APP"                  # expect: The validate action worked!
lipo -archs "$APP/Contents/MacOS/dotlore"      # expect: x86_64 arm64
```

The shipped app has exactly **one** binary, `Contents/MacOS/dotlore`. There is
no sidecar and no CLI.

Then check in the installed app: the window opens, the menu-bar item appears,
and **Dotlore → Check for Updates…** reports the right state.

## When a run fails

**A pushed tag is never deleted and never moved**, and **a published release is
never touched** — clients may already have fetched its `latest.json`. Which
lever applies depends on how far the run got:

- **Before the tag push** (preflight, build or an artifact check failed, or
  `main` moved during the build): nothing is tagged or published. Fix the
  cause, commit and push if it needs a code change, and rerun
  `pnpm release --expect-version X.Y.Z`.
- **After the tag push** (the script prints
  `tag vX.Y.Z is pushed; never delete it`): from the tagged commit, resume with
  `pnpm release --expect-version X.Y.Z --publish-only`. It reruns the artifact
  checks on the existing build, deletes a leftover **draft** of that tag (never
  the tag itself), and publishes. If the fix would need a change to the tagged
  commit, the tag would have to move, which is never done — stop and ask, or
  ship the fix as the next patch.
- **The release is published but the website still shows the old version**:
  `gh workflow run pages.yml --ref main`.

A release that publishes but is broken is fixed **forward** as the next patch,
never by deleting the previous one. The updater resolves the newest release, so
users move forward on their own.

## Decided: `requireSignedVersion` stays OFF for now

`plugins.updater.requireSignedVersion` is unset, so it defaults to `false`.

**Measured 2026-09-22 against a real signed local build** (Tauri CLI 2.11.4, the
version `pnpm-lock.yaml` pins and therefore the version releases sign with):

```
$ base64 -d < …/Dotlore.app.tar.gz.sig | grep 'trusted comment'
trusted comment: timestamp:1790099115	file:Dotlore.app.tar.gz
```

No `version:` field. `signed_version()`
(`tauri-plugin-updater-2.12.0/src/updater.rs:1600`) splits the trusted comment on
tabs and looks for exactly that prefix, so it returns `None`, and
`verify_signed_version` (`:1567`) then returns `MissingSignedVersion` **whenever
the flag is set**. Turning it on today would make every install refuse every
update, permanently. So: off.

**What we accept by leaving it off.** The artifact is still always verified
against the compiled-in pubkey, so nobody can serve bytes we did not sign. What
the flag would additionally stop is a *rollback*: someone who can serve a
crafted `latest.json` — the manifest is not signed — pairing an inflated
`version` with an older release's still-valid `url` and `signature`, pushing
clients back onto a known-vulnerable build. That requires control of the GitHub
releases endpoint, i.e. the repository itself. For v0.1.0 there is no older
release to roll back to at all; the exposure begins once v0.1.1 exists.

**Deferring is safe — the plan's "free now, expensive later" framing was wrong.**
The flag is compiled into the *client* and is checked only against the artifact
that client is downloading right now; it never looks at historical signatures.
An app built with the flag on simply requires the *next* artifact to carry the
stamp. So it can be switched on in any later release without stranding anything,
provided the CLI in use by then stamps the version.

**Owner for revisiting it: the first release after v0.1.0.** At that point, or
whenever `@tauri-apps/cli` is next upgraded, re-run the one-liner above. If the
trusted comment gains `version:`, add `"requireSignedVersion": true` to
`plugins.updater` and ship it. If it has not, record that here again rather than
letting the question lapse into an unowned "later".

The release script's `latest.json` check prints the same trusted comment and
states whether the flag is safe to enable, so every run (including
`--build-only`) re-checks this for free. Once it starts saying yes and the flag
is on, delete that block rather than printing it forever.

## The old GitHub Actions secrets

Releases used to run in GitHub Actions. Its repository secrets
(`APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `KEYCHAIN_PASSWORD`,
`APPLE_API_ISSUER`, `APPLE_API_KEY`, `APPLE_API_KEY_P8`,
`TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`) were deleted
on 2026-11-02, after the first local release (v0.9.4), so no unused signing
material sits in the repository. To release from CI again, revert the commit
that added `scripts/release-local.sh` (it also restores the `pages.yml` trigger)
and re-add those secrets from the local copies: the Developer ID `.p12`, the
`.p8`, `~/.tauri/dotlore.key` and the passwords in the keychain.
