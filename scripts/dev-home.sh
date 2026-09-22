#!/usr/bin/env bash
#
# The state directory a local dev run uses, and the ONE place that path is
# written down. Sourced, never executed.
#
# scripts/tauri.sh exports it so `pnpm dev` stays off the installed app's
# state, and scripts/reset-dev.sh defaults to the same value so `pnpm reset:dev`
# clears what `pnpm dev` created rather than the state of the copy in
# /Applications. Those two must never drift: if this path lived in both files,
# changing one would point `reset:dev` at the real home and delete a user's
# actual projects list.
#
# An explicit DOTLORE_HOME always wins, so a second dev instance or a one-off
# experiment can still choose its own.
#
# Developer script: the "no shell, ever" invariant is about what the engine
# runs, not about the toolchain. Nothing here reaches the app except through
# the environment variable src-tauri/src/main.rs already reads.

export DOTLORE_HOME="${DOTLORE_HOME:-$HOME/Downloads/dotlore-dev}"
