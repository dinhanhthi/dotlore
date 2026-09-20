#!/usr/bin/env bash
#
# Put the `dotlore` CLI where tauri-build's copy_binaries looks:
# crates/dotlore-app/binaries/dotlore-<host-triple>.
# Debug by default; pass --release for a release slice (scripts/build.sh
# already builds every installed Apple target itself).
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
host="$(rustc -vV | sed -n 's/^host: //p')"
if [ -z "$host" ]; then
	echo "error: could not read rustc host triple" >&2
	exit 1
fi

bindir="$root/crates/dotlore-app/binaries"
mkdir -p "$bindir"

if [ "${1:-}" = "--release" ]; then
	cargo build --release -p dotlore-cli
	cp "$root/target/release/dotlore" "$bindir/dotlore-$host"
else
	cargo build -p dotlore-cli
	cp "$root/target/debug/dotlore" "$bindir/dotlore-$host"
fi
