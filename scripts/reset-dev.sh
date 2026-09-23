#!/usr/bin/env bash
#
# Wipe disposable Dotlore state so a later schema drop can start clean.
#
# Developer script: the "no shell, ever" invariant is about the engine, not
# this. Never guesses a cloud folder — provider_dir comes from config.json
# or the cloud path is skipped.
set -euo pipefail

# Same default as `pnpm dev` — see scripts/dev-home.sh. Without this, a
# `pnpm reset:dev` after `pnpm dev` would wipe the state of the installed app
# instead of the dev one.
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/dev-home.sh"
HOME_DIR="$DOTLORE_HOME"
# The dev run and the bundled app do not always key WebKit storage the same
# way; `consider` skips whichever is absent.
WEBKIT_DIR="$HOME/Library/WebKit/dotlore"
WEBKIT_BUNDLE_DIR="$HOME/Library/WebKit/dev.dinhanhthi.dotlore"
PLIST="$HOME/Library/LaunchAgents/dev.dinhanhthi.dotlore.plist"
# macOS keys these by the bundle identifier, not by DOTLORE_HOME, so they
# outlive a state-directory wipe and make the next launch look used.
BUNDLE_ID="dev.dinhanhthi.dotlore"
BUNDLE_PATHS=(
	"$HOME/Library/Application Support/$BUNDLE_ID"
	"$HOME/Library/Caches/$BUNDLE_ID"
	"$HOME/Library/Preferences/$BUNDLE_ID.plist"
	"$HOME/Library/Saved Application State/$BUNDLE_ID.savedState"
)
CFG="$HOME_DIR/config.json"

YES=0
for arg in "$@"; do
	if [ "$arg" = "--yes" ]; then
		YES=1
	fi
done

# Read provider_dir before anything is deleted. Distinguishes a real path
# from null / absent / missing-file so we never invent a cloud folder.
PROVIDER_DIR=""
PROVIDER_SKIP=""
read_provider_dir() {
	if [ ! -f "$CFG" ]; then
		PROVIDER_SKIP="config.json not found"
		return
	fi
	local raw rest
	raw=$(tr '\n' ' ' <"$CFG")
	case "$raw" in
	*"\"provider_dir\""*) ;;
	*)
		PROVIDER_SKIP="provider_dir is absent"
		return
		;;
	esac
	rest="${raw#*\"provider_dir\"}"
	rest="${rest#*:}"
	rest="${rest#"${rest%%[![:space:]]*}"}"
	case "$rest" in
	null | null,* | null\}* | null\ *)
		PROVIDER_SKIP="provider_dir is null"
		return
		;;
	\"*)
		rest="${rest#\"}"
		PROVIDER_DIR="${rest%%\"*}"
		if [ -z "$PROVIDER_DIR" ]; then
			PROVIDER_SKIP="provider_dir is null"
			PROVIDER_DIR=""
		fi
		;;
	*)
		PROVIDER_SKIP="provider_dir is absent"
		;;
	esac
}

say() {
	local verb="$1" path="$2" reason="${3:-}"
	if [ "$YES" -eq 0 ]; then
		if [ "$verb" = "removed" ]; then
			echo "would remove $path"
		elif [ -n "$reason" ]; then
			echo "would skip $path ($reason)"
		else
			echo "would skip $path"
		fi
	else
		if [ "$verb" = "removed" ]; then
			echo "removed $path"
		elif [ -n "$reason" ]; then
			echo "skipped $path ($reason)"
		else
			echo "skipped $path"
		fi
	fi
}

# Delete (or preview) one path. Reports removed vs skipped; never claims
# a removal that did not happen.
consider() {
	local path="$1"
	if [ -e "$path" ] || [ -L "$path" ]; then
		if [ "$YES" -eq 1 ]; then
			rm -rf "$path"
		fi
		say removed "$path"
	else
		say skipped "$path" "not present"
	fi
}

read_provider_dir

if [ -n "$PROVIDER_DIR" ]; then
	consider "$PROVIDER_DIR/dotlore"
else
	say skipped "cloud folder" "$PROVIDER_SKIP"
fi

consider "$HOME_DIR/config.json"
consider "$HOME_DIR/lock"
consider "$HOME_DIR/app.lock"
consider "$HOME_DIR/tmp"
consider "$HOME_DIR/repos"
consider "$HOME_DIR/recovery"
consider "$HOME_DIR/adding"
consider "$WEBKIT_DIR"
consider "$WEBKIT_BUNDLE_DIR"
for path in "${BUNDLE_PATHS[@]}"; do
	consider "$path"
done

if [ "$YES" -eq 1 ]; then
	launchctl bootout "gui/$(id -u)/dev.dinhanhthi.dotlore" >/dev/null 2>&1 || true
fi
consider "$PLIST"
