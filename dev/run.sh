#!/bin/bash
# Run the development build fully apart from an installed Kinora: its own
# data, cache and settings under .dev/home, the UI and CSS from the source tree.
# Extra arguments are environment settings, e.g. KH_OPEN=0 (see the test hooks
# in src/gui/window.rs and src/gui/video_page.rs) or RUST_LOG=kinora=debug.
ROOT=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
H=$ROOT/.dev/home
mkdir -p "$H"
pkill -f "^$ROOT/target/debug/kinora" 2>/dev/null; sleep 0.5
exec env \
  KINORA_DEV_RESOURCES="$ROOT/_dev/data/resources/resources.gresource" \
  KINORA_DEV_CSS="$ROOT/data/resources/style.css" \
  XDG_DATA_HOME="$H/data" XDG_CACHE_HOME="$H/cache" XDG_CONFIG_HOME="$H/config" \
  GSETTINGS_BACKEND=keyfile GSETTINGS_SCHEMA_DIR="$ROOT/_dev/schemas" \
  KINORA_BROWSER_CONFIG="${XDG_CONFIG_HOME:-$HOME/.config}" \
  PATH="$ROOT/extras:$PATH" RUST_LOG="${RUST_LOG:-kinora=info}" "$@" \
  "$ROOT/target/debug/kinora"
