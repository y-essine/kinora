#!/bin/bash
# One watch.sh step: pick the fastest way to get the change on screen.
ROOT=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
if [ -n "$WATCHEXEC_EVENTS_FILE" ] && ! grep -qE '\.(rs|toml)' "$WATCHEXEC_EVENTS_FILE"; then
  exec "$ROOT/dev/ui.sh" "$@"
fi
"$ROOT/dev/build.sh" && exec "$ROOT/dev/run.sh" "$@"
