#!/usr/bin/env bash
#
# Run the Tauri CLI from crates/dotlore-app so beforeDevCommand / frontendDist
# resolve against ui/. `pnpm tauri build` is the same as `pnpm build`.
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

cd "$root/crates/dotlore-app"
exec "$tauri" "$@"
