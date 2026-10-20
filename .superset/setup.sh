#!/usr/bin/env bash
# Runs once when a Superset workspace (git worktree) is created. Safe to run again.
set -euo pipefail

# Local files that are not tracked by git but that a new worktree needs.
# Nothing is overwritten if it already exists in the workspace.
if [ -n "${SUPERSET_ROOT_PATH:-}" ] && [ -d "$SUPERSET_ROOT_PATH" ]; then
  for f in "$SUPERSET_ROOT_PATH"/.env "$SUPERSET_ROOT_PATH"/.env.* "$SUPERSET_ROOT_PATH"/CLAUDE.md; do
    [ -f "$f" ] || continue
    dest="./$(basename "$f")"
    [ -e "$dest" ] || cp "$f" "$dest"
  done
fi

# Download the crates now so the first build and the first test run do not wait on the network.
cargo fetch --locked
