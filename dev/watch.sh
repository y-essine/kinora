#!/bin/bash
# Rebuild and restart the development build whenever you save (needs watchexec).
# Rust/Cargo.toml changes: full build. Only .blp changes: UI recompile (~1 s).
# CSS needs nothing: dev builds reload it live.
ROOT=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
cd "$ROOT"
exec watchexec --restart --emit-events-to=file --exts rs,blp,toml \
  --watch src --watch data/resources/ui --watch Cargo.toml \
  -- "$ROOT/dev/watch-step.sh" "$@"
