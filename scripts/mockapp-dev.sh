#!/usr/bin/env bash
# Start the mockapp Vite preview. Frees port 38422 first (strictPort).
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
port=38422

pids="$(lsof -ti ":${port}" 2>/dev/null || true)"
if [ -n "$pids" ]; then
	echo "Stopping mockapp on port ${port}..."
	# shellcheck disable=SC2086
	kill $pids 2>/dev/null || true
	sleep 1
	pids="$(lsof -ti ":${port}" 2>/dev/null || true)"
	if [ -n "$pids" ]; then
		# shellcheck disable=SC2086
		kill -9 $pids 2>/dev/null || true
	fi
fi

cd "$root/crates/dotlore-app/ui"
exec pnpm exec vite --config mockapp/vite.config.ts
