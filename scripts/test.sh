#!/usr/bin/env bash
#
# Frontend + engine + app tests. Used by `pnpm test`.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

cargo test --manifest-path src-tauri/Cargo.toml
pnpm exec vitest run
bash scripts/reset-dev.test.sh
