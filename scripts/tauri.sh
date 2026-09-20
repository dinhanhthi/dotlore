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

# cargo build of the app crate needs the sidecar on disk.
bash "$root/scripts/sidecar.sh"

cd "$root/crates/dotlore-app"
exec "$tauri" "$@"
