#!/usr/bin/env bash
#
# Release-build Dotlore.app via `cargo tauri`, with the `dotlore` CLI shipped
# as a sidecar in Contents/MacOS.
#
# This is a build script, not app code: the "no shell, ever" invariant is about
# what the engine runs at runtime. Everything the app itself executes goes
# through std::process::Command with an argv array.
#
# tauri-build copies bundle.externalBin at `cargo build` time (see copy_binaries
# in tauri-build), not only at bundle time. A host-triple sidecar must exist
# under crates/dotlore-app/binaries/ before `cargo build -p dotlore-app` will
# succeed; this script writes those files before it invokes cargo-tauri.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

want=(aarch64-apple-darwin x86_64-apple-darwin)
bindir="$root/crates/dotlore-app/binaries"
cli_bin=dotlore

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

# --- sidecar ----------------------------------------------------------------
mkdir -p "$bindir"
slices=()
for t in "${have[@]}"; do
	cargo build --release -p dotlore-cli --target "$t"
	# Per-slice names: tauri-build's copy_binaries looks up
	# binaries/dotlore-<TARGET> while compiling each architecture.
	cp "target/$t/release/$cli_bin" "$bindir/$cli_bin-$t"
	slices+=("target/$t/release/$cli_bin")
done
if [ "$n" -eq "${#want[@]}" ]; then
	# Bundler target universal-apple-darwin looks for this exact name.
	lipo -create "${slices[@]}" -output "$bindir/$cli_bin-universal-apple-darwin"
fi

# --- tauri -------------------------------------------------------------------
# Invoke the cargo-tauri binary, not `cargo tauri`. Cargo subcommands start
# from the workspace root, which makes `pnpm --dir ui` miss crates/dotlore-app/ui.
# cargo-tauri itself resolves tauri.conf.json and beforeBuildCommand relative
# to the app crate.
if ! command -v cargo-tauri >/dev/null 2>&1; then
	echo "error: cargo-tauri is not on PATH" >&2
	exit 1
fi

tauri_args=(build)
# universal-apple-darwin hard-fails if a slice is missing. One target: omit
# --target entirely so cargo-tauri builds the host triple.
if [ "$n" -eq "${#want[@]}" ]; then
	tauri_args+=(--target universal-apple-darwin)
fi

(
	cd "$root/crates/dotlore-app"
	cargo-tauri "${tauri_args[@]}"
)

# A universal build lands under target/universal-apple-darwin/...; the
# documented path is target/release/bundle/macos/Dotlore.app. Mirror it so
# both locations are usable.
app_release="$root/target/release/bundle/macos/Dotlore.app"
if [ "$n" -eq "${#want[@]}" ]; then
	app_universal="$root/target/universal-apple-darwin/release/bundle/macos/Dotlore.app"
	if [ ! -d "$app_universal" ]; then
		echo "error: expected $app_universal after a universal tauri build" >&2
		exit 1
	fi
	mkdir -p "$(dirname "$app_release")"
	rm -rf "$app_release"
	ditto "$app_universal" "$app_release"
fi

if [ ! -d "$app_release" ]; then
	echo "error: bundle not created at $app_release" >&2
	exit 1
fi

echo "built $app_release"
lipo -info "$app_release/Contents/MacOS/dotlore-app" || true
lipo -info "$app_release/Contents/MacOS/dotlore" || true
