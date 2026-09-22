# Release setup

Everything `.github/workflows/release.yml` needs in order to publish a signed,
notarized macOS build. Maintainer reference — you only do this once.

> This lives in `.github/` rather than `docs/` on purpose: `docs/` is gitignored
> in this repository, so anything written there never reaches a clone. This file
> is self-sufficient: you can produce all eight secrets from it alone.

## How a release happens

1. `/cf-ship` decides the version bump from the commits, writes the
   `CHANGELOG.md` section, and bumps the version in `src-tauri/Cargo.toml`,
   `src-tauri/Cargo.lock` and the website badge.
2. Pushing a `v*` tag triggers `release.yml`.
3. The workflow runs `scripts/build.sh` — the same script that builds Dotlore on
   your Mac — with Developer ID signing, App Store Connect notarization and
   minisign updater signing enabled, then uploads five assets to a GitHub
   Release: the versioned `.dmg`, a version-less `Dotlore-universal.dmg` alias,
   `Dotlore.app.tar.gz`, its `.sig`, and `latest.json`.
4. Installed apps read `latest.json` and offer the update.

**Dotlore ships stable only.** There is no `-rc` / `-beta` channel, no
prerelease flag and no channel selector. Every release is served to every
install through `/releases/latest/download/latest.json`, which is also why the
release must not stay a draft: GitHub's "latest" serves neither drafts nor
prereleases, and either would strand every install on the previous version.

The version lives in **one** place, `src-tauri/Cargo.toml` `[package] version`.
`src-tauri/tauri.conf.json` deliberately has no `version` key — the bundler
resolves it from the manifest — and the root `package.json` has no `version`
field. Do not add one; the workflow's tag check is the single place the tag and
the manifest are allowed to disagree, and it fails the build when they do.

## Required secrets

Eight, all of them. Set at
<https://github.com/dinhanhthi/dotlore/settings/secrets/actions>.

The workflow asserts every one is present before it builds anything, printing
names and pass/fail only, never values.

### Code signing

| Secret                       | What it is                                                     | How to produce it                                                                                                                |
| ---------------------------- | -------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| `APPLE_CERTIFICATE`          | Base64 of the Developer ID Application certificate as a `.p12` | Keychain Access → My Certificates → right-click _Developer ID Application_ → Export as `.p12`, then `base64 -i cert.p12 \| pbcopy` |
| `APPLE_CERTIFICATE_PASSWORD` | The password set while exporting that `.p12`                   | Chosen at export time                                                                                                            |
| `KEYCHAIN_PASSWORD`          | Any random string                                              | Only ever used for a throwaway keychain inside the CI runner                                                                     |

`APPLE_SIGNING_IDENTITY` is deliberately **not** a secret. The workflow reads it
back out of the certificate it just imported (`security find-identity`, grepping
`Developer ID Application`), so it cannot drift from the certificate actually in
use. `scripts/build.sh` then passes it to the bundler as an explicit
`--config` override rather than relying on whether the environment variable
outranks `bundle.macOS.signingIdentity` in `tauri.conf.json` — that precedence
is undocumented, and getting it wrong ships an ad-hoc-signed bundle that looks
fine in CI and is blocked by Gatekeeper on every user's Mac.

> An **Apple Development** certificate is not a substitute. It cannot be
> distributed, and the workflow refuses it by name rather than shipping it.

### Notarization (App Store Connect API key)

| Secret             | What it is                                | How to produce it                                                                                       |
| ------------------ | ----------------------------------------- | ------------------------------------------------------------------------------------------------------- |
| `APPLE_API_ISSUER` | Issuer ID (UUID)                          | App Store Connect → Users and Access → Integrations → App Store Connect API; shown above the keys table |
| `APPLE_API_KEY`    | Key ID (~10 chars)                        | Same table, _KEY ID_ column                                                                             |
| `APPLE_API_KEY_P8` | Base64 of the `AuthKey_*.p8` **contents** | `base64 -i AuthKey_XXXXXXXXXX.p8 \| pbcopy`                                                             |

The `.p8` downloads **once** and Apple will not re-issue it — keep a copy in a
password manager. `APPLE_API_KEY_PATH`, which Tauri actually reads, is a file
path the workflow creates from `APPLE_API_KEY_P8`; it is not a secret.

Without these, notarization is silently skipped and Gatekeeper blocks the
result on every Mac that did not build it.

### Updater signing (minisign)

| Secret                               | What it is                                   | How to produce it                                    |
| ------------------------------------ | -------------------------------------------- | ---------------------------------------------------- |
| `TAURI_SIGNING_PRIVATE_KEY`          | The minisign private key, password-encrypted | `pnpm exec tauri signer generate -w ~/.tauri/dotlore.key` |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Its password                                 | Chosen at generation time                            |

> 🚨 **This key can never be rotated.** Its public half is compiled into every
> shipped build (`src-tauri/tauri.conf.json` → `plugins.updater.pubkey`) and
> Tauri's updater has no in-band key rotation. Replacing it means every existing
> install silently stops being able to verify updates and has to be re-downloaded
> by hand. Keep the private key and its password in a password manager, and
> **never re-run `signer generate`** for this project.

Because it cannot be rotated, a leak is not recoverable by re-signing. Two
things follow, both already in place:

- The key is exported only for the build step, never for the whole job.
- `scripts/ui-build.sh` scrubs it from the environment before the frontend
  build, so no vite plugin, JS dependency or lifecycle script in a release
  build can read it out of `process.env`. That is the only reason that wrapper
  script exists — do not replace it with a bare `vite build`.

### There is no provisioning profile

Dotlore declares no restricted entitlements, so it needs none — and no
entitlements plist either. Hardened runtime is already on
(`codesign -dvv` reports `flags=0x10002(adhoc,runtime)` on a local build); only
the signing identity changes in CI.

## Before the first release

- `pnpm exec tauri signer generate -w ~/.tauri/dotlore.key`, **once, ever**, and
  put `plugins.updater.pubkey` in `src-tauri/tauri.conf.json` from
  `~/.tauri/dotlore.key.pub` verbatim.
- Run the signed build locally before tagging anything. It takes about ten
  minutes and it is the only thing that de-risks a first tag on a pipeline
  nothing has ever exercised:

  ```bash
  export APPLE_SIGNING_IDENTITY="Developer ID Application: <you> (<TEAMID>)"
  export APPLE_API_ISSUER=… APPLE_API_KEY=… APPLE_API_KEY_PATH=~/private_keys/AuthKey_….p8
  export TAURI_SIGNING_PRIVATE_KEY="$(cat ~/.tauri/dotlore.key)"
  export TAURI_SIGNING_PRIVATE_KEY_PASSWORD='…'
  pnpm build
  ```

  Note that `bundle.createUpdaterArtifacts` is `true`, so **`pnpm build` fails
  without `TAURI_SIGNING_PRIVATE_KEY` exported** — for you on a machine where
  you forgot, and for any contributor. `pnpm tauri dev` is unaffected.

## Verifying a release

`conclusion=success` is not proof the artifacts are usable. A pushed tag is not
a release, and a green run is not a verified artifact.

```bash
gh run list --workflow=release.yml --limit 3

gh release view <tag> --json isDraft,isPrerelease,assets
# expect: isDraft false, isPrerelease false, and FIVE assets.

# latest.json must carry a non-empty signature under BOTH darwin keys. An empty
# one is the silent failure mode of updater signing: the release looks complete
# and every client refuses the update, possibly for months. The workflow asserts
# this before the release leaves draft, so this is a second look, not the gate.
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

A tag push that prints success is not proof a release started — confirm with
`git ls-remote --tags origin` and `gh run list`.

## When a run fails

Nothing is published, so the tag is harmless. **A published tag is never deleted
and never force-moved** — clients may already have fetched its `latest.json`.
Which lever applies depends on whether the tagged commit is still correct:

- **Transient failure** — an Apple notarization outage, an expired credential, a
  runner hiccup — and every file the release depends on is still correct at that
  commit: `gh run rerun <run-id>`. Nothing moves.

  If the run reached the *Publish the release* step, it created a **draft**
  release before uploading, and `gh release create` refuses to run twice for one
  tag. The workflow deletes its own leftover draft on the next run, so the rerun
  still works — but check with `gh release view <tag> --json isDraft` if it does
  not, and `gh release delete <tag> --yes` by hand. That deletes the draft only;
  the tag stays, because `--cleanup-tag` is deliberately not passed.
- **The fix changes a file the release depends on**, `release.yml` itself most of
  all: a rerun replays the *broken* workflow, because a run reads its definition
  from its own ref. The tag would have to move, which is never done unilaterally
  — stop and ask.

`git push origin v<version>` on an already-pushed, unchanged tag prints
`Everything up-to-date` and starts **no** run. That is the trap: it reads like a
retry and is not one.

A release that publishes but is broken is fixed **forward** as the next patch,
never by deleting the previous one. The updater resolves the newest release, so
users move forward on their own.

## Decided: `requireSignedVersion` stays OFF for now

`plugins.updater.requireSignedVersion` is unset, so it defaults to `false`.

**Measured 2026-09-22 against a real signed local build** (Tauri CLI 2.11.4, the
version `pnpm-lock.yaml` pins and therefore the version CI signs with):

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

The release workflow's assert step prints the same trusted comment and states
whether the flag is safe to enable, so every run re-checks this for free. Once it
starts saying yes and the flag is on, delete that block rather than printing it
forever.
