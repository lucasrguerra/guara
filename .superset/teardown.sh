#!/usr/bin/env bash
# Runs when a Superset workspace is deleted.
# Setup starts no background service. The only long-running process is the Run command
# (`guara ui`), which this workspace builds into its own target/ directory, so stop any
# copy that is still alive before the worktree is removed.
set -uo pipefail

pkill -f -- "$PWD/target/.*/guara ui" 2>/dev/null || true
exit 0
