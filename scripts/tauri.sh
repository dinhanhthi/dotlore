#!/usr/bin/env bash
#
# Run the Tauri CLI from src-tauri so beforeDevCommand / frontendDist
# resolve against the repo root. `pnpm tauri build` is the same as `pnpm build`.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tauri="$root/node_modules/.bin/tauri"

if [ ! -x "$tauri" ]; then
	echo "error: run pnpm install at the repo root first" >&2
	exit 1
fi

if [ "${1:-}" = "build" ]; then
	shift
	exec bash "$root/scripts/build.sh" "$@"
fi

# Keep a dev run off the installed app's state. Both copies lock <home>/app.lock
# and the loser calls exit(0) with no window and only a stderr line, which from
# Finder reads as "the app just did not open". Set below the `build` branch on
# purpose: a release build has no business carrying a dev home.
#
# scripts/dev-home.sh holds the path, shared with scripts/reset-dev.sh. An
# explicit DOTLORE_HOME still wins.
. "$root/scripts/dev-home.sh"
echo "dotlore: DOTLORE_HOME=$DOTLORE_HOME" >&2

# Dev follows Cargo's own target dir: src-tauri/target when nothing else is
# set, or the shared directory from ~/.cargo/config.toml [build] target-dir.
# An already-exported CARGO_TARGET_DIR still wins. Release builds do not —
# scripts/build.sh pins src-tauri/target so the bundle path stays put.

# macOS Dock names a bare Mach-O after its filename. `pnpm tauri build`
# already exec'd scripts/build.sh above, so this runner is dev-only.
if [ "$(uname -s)" = "Darwin" ]; then
	host="$(rustc -vV | sed -n 's/^host: //p')"
	if [ -z "$host" ]; then
		echo "error: could not read rustc host triple" >&2
		exit 1
	fi
	runner_var="CARGO_TARGET_$(printf '%s' "$host" | tr '[:lower:]-' '[:upper:]_')_RUNNER"
	export "$runner_var=$root/scripts/dev-dock-bundle.sh"
fi

cd "$root/src-tauri"
exec "$tauri" "$@"
