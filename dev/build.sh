#!/bin/bash
# Build the development version (app ID ...Kinora.Devel) with the test hooks.
# Blueprints are compiled by hand: ninja skips them (their output is a folder).
set -e
ROOT=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
cd "$ROOT"
[ -d _dev ] || meson setup _dev -Dprofile=development >/dev/null
cd _dev
eval "$(sed -n 's/^ COMMAND = \(\/usr\/bin\/blueprint-compiler batch-compile.*\)/\1/p' build.ninja)"
ninja -C . data/resources/resources.gresource >/dev/null
mkdir -p schemas && cp ../data/io.github.y_essine.Kinora.gschema.xml schemas/ && glib-compile-schemas schemas
# Faster when available. mold links faster. sccache shares compiled
# dependencies between checkouts, which saves minutes in a fresh worktree but
# costs ~1 s on every small rebuild, so it is used only in linked worktrees.
command -v mold >/dev/null && export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }-C link-arg=-fuse-ld=mold"
if command -v sccache >/dev/null && [ "$(git rev-parse --git-dir)" != "$(git rev-parse --git-common-dir)" ]; then
  export RUSTC_WRAPPER=sccache
fi
cd .. && cargo build --features dev-hooks 2>&1 | grep -E '^(error|warning)|^\s+-->|Finished'
