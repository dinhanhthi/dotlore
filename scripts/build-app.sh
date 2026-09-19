#!/usr/bin/env bash
#
# Build target/Dotlore.app — the menu-bar app, with the `dotlore` CLI shipped
# beside it in the same bundle.
#
# This is a build script, not app code: the "no shell, ever" invariant is about
# what the engine runs at runtime. Everything the app itself executes goes
# through std::process::Command with an argv array.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

app="target/Dotlore.app"
macos="$app/Contents/MacOS"
resources="$app/Contents/Resources"
# The bundle is user-facing, so it is "Dotlore"; the executables inside it are
# code names, so they stay lowercase. See CLAUDE.md.
bins=(dotlore-app dotlore)
want=(aarch64-apple-darwin x86_64-apple-darwin)

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

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

# --- build -------------------------------------------------------------------
flags=()
for t in "${have[@]}"; do flags+=(--target "$t"); done
cargo build --release -p dotlore-app -p dotlore-cli "${flags[@]}"

rm -rf "$app"
mkdir -p "$macos" "$resources"
for bin in "${bins[@]}"; do
	slices=()
	for t in "${have[@]}"; do slices+=("target/$t/release/$bin"); done
	lipo -create "${slices[@]}" -output "$macos/$bin"
done

# --- Info.plist ---------------------------------------------------------------
# `cargo pkgid` prints `<source>#<version>`, or `<source>#<name>@<version>` when
# the directory name differs from the package name. Strip both separators.
version="$(cargo pkgid -p dotlore-app)"
version="${version##*#}"
version="${version##*@}"
sed "s/@VERSION@/$version/g" crates/dotlore-app/Info.plist >"$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist" >/dev/null

# --- icon ----------------------------------------------------------------------
# Sanctioned to fail: an iconless bundle still runs, and a menu-bar app shows
# its icon almost nowhere.
icon() {
	local svg="assets/icon.svg" png="$tmp/icon.svg.png" iconset="$tmp/dotlore.iconset" s
	if [ ! -f "$svg" ]; then
		echo "warning: $svg is missing — building without an icon" >&2
		return 0
	fi
	if ! qlmanage -t -s 1024 -o "$tmp" "$svg" >/dev/null 2>&1 || [ ! -f "$png" ]; then
		echo "warning: qlmanage could not render $svg — building without an icon" >&2
		return 0
	fi
	mkdir -p "$iconset"
	for s in 16 32 128 256 512; do
		sips -z "$s" "$s" "$png" --out "$iconset/icon_${s}x${s}.png" >/dev/null
		sips -z $((s * 2)) $((s * 2)) "$png" --out "$iconset/icon_${s}x${s}@2x.png" >/dev/null
	done
	iconutil -c icns "$iconset" -o "$resources/dotlore.icns"
}
icon

# --- sign -----------------------------------------------------------------------
# Ad-hoc: enough for Gatekeeper to let a locally built app run, and enough for
# the hardened-runtime-free launchd agent. `--deep` warns that it is deprecated;
# the exit status is what matters.
codesign --force --deep -s - "$app"
codesign --verify --strict "$app"

echo "built $app ($version)"
lipo -info "$macos/dotlore-app"
lipo -info "$macos/dotlore"
