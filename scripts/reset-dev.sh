#!/usr/bin/env bash
#
# Wipe disposable Dotlore state so a later schema drop can start clean.
#
# Developer script: the "no shell, ever" invariant is about the engine, not
# this. Never guesses a cloud folder — provider_dir comes from config.json
# or the cloud path is skipped.
set -euo pipefail

HOME_DIR="${DOTLORE_HOME:-$HOME/Library/Application Support/dotlore}"
WEBKIT_DIR="$HOME/Library/WebKit/dotlore"
PLIST="$HOME/Library/LaunchAgents/dev.dinhanhthi.dotlore.plist"
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
consider "$WEBKIT_DIR"

if [ "$YES" -eq 1 ]; then
	launchctl bootout "gui/$(id -u)/dev.dinhanhthi.dotlore" >/dev/null 2>&1 || true
fi
consider "$PLIST"
