#!/bin/sh
# Kinora: run the cargo build meson hands over, then copy the program it built
# to meson's output. A script because ninja runs commands without a shell on
# Windows, where `cargo build ... && cp ...` would give cargo the `&&`.
#
#     cargo-build.sh BUILT OUTPUT COMMAND...
set -e
built="$1"
output="$2"
shift 2
"$@"
cp "$built" "$output"
