#!/usr/bin/env bash
#
# Drop Cargo build artifacts that have not been used for N days (default 14).
# cargo-sweep follows the target directory Cargo is actually using, including
# a shared target-dir from ~/.cargo/config.toml. One run from this repo is
# enough once that shared directory is in place; pass a tree to sweep every
# project under it while targets are still per-project.
#
#   pnpm sweep:cargo
#   pnpm sweep:cargo -- --dry-run
#   pnpm sweep:cargo -- --days 7
#   pnpm sweep:cargo -- "$HOME/git"
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if ! cargo sweep --help >/dev/null 2>&1; then
	echo "error: cargo-sweep is not installed. Run: cargo install cargo-sweep" >&2
	exit 1
fi

days=14
dry=0
paths=()
while [ "$#" -gt 0 ]; do
	case "$1" in
	--dry-run)
		dry=1
		shift
		;;
	--days)
		if [ "$#" -lt 2 ]; then
			echo "error: --days needs a positive integer" >&2
			exit 2
		fi
		days="$2"
		shift 2
		;;
	--days=*)
		days="${1#--days=}"
		shift
		;;
	--)
		shift
		;;
	-*)
		echo "error: unknown option: $1" >&2
		exit 2
		;;
	*)
		paths+=("$1")
		shift
		;;
	esac
done

case "$days" in
'' | *[!0-9]*)
	echo "error: --days expects a positive integer" >&2
	exit 2
	;;
esac
if [ "$days" -lt 1 ]; then
	echo "error: --days expects a positive integer" >&2
	exit 2
fi

sweep() {
	if [ "$dry" -eq 1 ]; then
		cargo sweep --dry-run "$@"
	else
		cargo sweep "$@"
	fi
}

if [ "${#paths[@]}" -eq 0 ]; then
	sweep --time "$days" "$root/src-tauri"
else
	for path in "${paths[@]}"; do
		sweep --recursive --time "$days" "$path"
	done
fi
