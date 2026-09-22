#!/usr/bin/env bash
#
# Build the frontend from src/ into dist/.
#
# This exists instead of a bare `vite build` in package.json for one reason:
# tauri.conf.json's beforeBuildCommand runs it as a child of the Tauri CLI, and
# a release build has TAURI_SIGNING_PRIVATE_KEY in its environment. Children
# inherit it, so every vite plugin, every JS dependency and every lifecycle
# script in a release build could read the private half of the update trust
# anchor out of process.env. That key cannot be rotated once a release ships —
# its public half is compiled into every installed client — so a leak is not
# recoverable by re-signing. Scrub it here, where the frontend has no use for it.
#
# The Apple notarization credentials are scrubbed for the same reason. They are
# revocable, so the stakes are lower, but the frontend has no more use for them
# than for the minisign key. APPLE_API_KEY_PATH points at a readable .p8 on
# disk, so leaving it in the child environment hands over the file too.
#
# This is a build script, not app code: the "no shell, ever" invariant is about
# what the engine runs at runtime.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

exec env \
	-u TAURI_SIGNING_PRIVATE_KEY \
	-u TAURI_SIGNING_PRIVATE_KEY_PASSWORD \
	-u APPLE_API_KEY \
	-u APPLE_API_KEY_PATH \
	-u APPLE_API_ISSUER \
	-u APPLE_CERTIFICATE \
	-u APPLE_CERTIFICATE_PASSWORD \
	"$root/node_modules/.bin/vite" build "$@"
