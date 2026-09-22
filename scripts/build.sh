#!/usr/bin/env bash
#
# Release-build Dotlore.app via `cargo tauri`. A universal build still produces
# one desktop executable under Contents/MacOS.
#
# This is a build script, not app code: the "no shell, ever" invariant is about
# what the engine runs at runtime. Everything the app itself executes goes
# through std::process::Command with an argv array.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

want=(aarch64-apple-darwin x86_64-apple-darwin)

# --- targets ----------------------------------------------------------------
# A missing target degrades to a single-arch build with a warning. Failing the
# whole release because one Mac only ever runs one architecture is not useful.
# No rustup at all — a Homebrew toolchain, say — degrades the same way instead
# of dying on a bare "command not found": rustc knows its own host triple, and
# cargo can build for that one without rustup's help.
if command -v rustup >/dev/null 2>&1; then
	installed="$(rustup target list --installed)"
else
	echo "warning: rustup not found — only this Mac's own target is available" >&2
	installed="$(rustc -vV | sed -n 's/^host: //p')"
fi
have=()
n=0
for t in "${want[@]}"; do
	if grep -qx "$t" <<<"$installed"; then
		have+=("$t")
		n=$((n + 1))
	else
		echo "warning: target $t is not installed — 'rustup target add $t' to include it" >&2
	fi
done
if [ "$n" -eq 0 ]; then
	echo "error: no macOS target installed" >&2
	exit 1
fi
if [ "$n" -lt "${#want[@]}" ]; then
	echo "warning: the bundle will NOT be universal (${have[*]} only)" >&2
fi

# --- tauri -------------------------------------------------------------------
# Invoke the Tauri CLI from src-tauri. It resolves tauri.conf.json and
# beforeBuildCommand relative to that directory, where `pnpm ui:build` walks up
# to the root package.
#
# Prefer the @tauri-apps/cli binary scripts/tauri.sh already uses, so a fresh
# clone builds after `pnpm install` alone and CI spends no time on
# `cargo install tauri-cli`. A cargo-tauri already on PATH still works.
tauri="$root/node_modules/.bin/tauri"
if [ ! -x "$tauri" ]; then
	if command -v cargo-tauri >/dev/null 2>&1; then
		tauri="$(command -v cargo-tauri)"
	else
		echo "error: no Tauri CLI found — run pnpm install at the repo root" >&2
		exit 1
	fi
fi

tauri_args=(build)
# universal-apple-darwin hard-fails if a slice is missing. One target: omit
# --target entirely so the CLI builds the host triple.
if [ "$n" -eq "${#want[@]}" ]; then
	tauri_args+=(--target universal-apple-darwin)
fi

# tauri.conf.json pins bundle.macOS.signingIdentity to "-" so a local build
# produces an ad-hoc-signed, runnable bundle without a Developer ID. A real
# release overrides it here rather than relying on whether the
# APPLE_SIGNING_IDENTITY environment variable outranks the config value: that
# precedence is undocumented, and getting it wrong ships an ad-hoc bundle that
# looks fine in CI and is blocked by Gatekeeper on every user's Mac. An explicit
# --config leaves nothing to resolve. python3 builds the JSON so an identity
# containing a quote cannot produce a malformed override.
if [ -n "${APPLE_SIGNING_IDENTITY:-}" ]; then
	tauri_args+=(--config "$(python3 -c '
import json, os
print(json.dumps({"bundle": {"macOS": {"signingIdentity": os.environ["APPLE_SIGNING_IDENTITY"]}}}))
')")
fi

# Pin the target dir so an inherited CARGO_TARGET_DIR cannot move the
# bundle off src-tauri/target/.
export CARGO_TARGET_DIR="$root/src-tauri/target"

(
	cd "$root/src-tauri"
	"$tauri" "${tauri_args[@]}"
)

# A universal build lands under src-tauri/target/universal-apple-darwin/...;
# the documented path is src-tauri/target/release/bundle/. Mirror the WHOLE
# bundle directory, not just Dotlore.app: with createUpdaterArtifacts the
# bundler also emits macos/Dotlore.app.tar.gz and its .sig, and the dmg target
# emits dmg/*.dmg. Mirroring only the .app stranded all three in the universal
# directory, where the release pipeline does not look for them.
bundle_release="$root/src-tauri/target/release/bundle"
app_release="$bundle_release/macos/Dotlore.app"
if [ "$n" -eq "${#want[@]}" ]; then
	bundle_universal="$root/src-tauri/target/universal-apple-darwin/release/bundle"
	if [ ! -d "$bundle_universal/macos/Dotlore.app" ]; then
		echo "error: expected $bundle_universal/macos/Dotlore.app after a universal tauri build" >&2
		exit 1
	fi
	# Replaced wholesale so a single-arch build's leftovers cannot survive next
	# to universal ones and be picked up by a later glob.
	rm -rf "$bundle_release"
	mkdir -p "$(dirname "$bundle_release")"
	ditto "$bundle_universal" "$bundle_release"
fi

if [ ! -d "$app_release" ]; then
	echo "error: bundle not created at $app_release" >&2
	exit 1
fi

# Info.plist names CFBundleIconName, so on macOS 26 the Dock draws the icon out
# of Contents/Resources/Assets.car and falls back to icon.icns — inside the
# system's grey backdrop — when it is missing. The file gets there through
# bundle.resources in tauri.conf.json, a copy that fails silently if the key is
# ever renamed or the source deleted. Asserted here because the result is a
# green release whose icon only looks wrong once it is on a user's Dock.
if [ ! -s "$app_release/Contents/Resources/Assets.car" ]; then
	echo "error: no Contents/Resources/Assets.car in the bundle — check bundle.resources in src-tauri/tauri.conf.json and that src-tauri/icons/Assets.car exists (scripts/icon.sh rebuilds it)" >&2
	exit 1
fi

# The other half. src-tauri/Info.plist is merged by filename convention, with no
# entry in tauri.conf.json to keep it honest, so a CLI that stops honouring that
# convention drops CFBundleIconName and the Dock falls back to icon.icns —
# plated — with the Assets.car sitting right there, unread and unasserted.
if [ "$(plutil -extract CFBundleIconName raw -o - "$app_release/Contents/Info.plist" 2>/dev/null)" != "Icon" ]; then
	echo "error: CFBundleIconName is not \"Icon\" in the built Info.plist — is src-tauri/Info.plist still merged by the Tauri CLI?" >&2
	exit 1
fi

echo "built $app_release"
lipo -info "$app_release/Contents/MacOS/dotlore" || true

# Named, not globbed, so a missing updater artifact is visible here rather than
# at the point the release pipeline tries to upload it.
for extra in "$bundle_release"/dmg/*.dmg "$app_release.tar.gz" "$app_release.tar.gz.sig"; do
	if [ -e "$extra" ]; then
		echo "built $extra"
	else
		echo "note: not produced: $extra" >&2
	fi
done
