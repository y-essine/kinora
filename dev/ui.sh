#!/bin/bash
# After editing .blp files: recompile the UI (about a second) and restart the
# development build. No Rust rebuild: dev builds load the UI from _dev/.
# CSS needs nothing at all: dev builds reload data/resources/style.css live.
set -e
ROOT=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
cd "$ROOT/_dev"
eval "$(sed -n 's/^ COMMAND = \(\/usr\/bin\/blueprint-compiler batch-compile.*\)/\1/p' build.ninja)"
ninja -C . data/resources/resources.gresource >/dev/null
exec "$ROOT/dev/run.sh" "$@"
