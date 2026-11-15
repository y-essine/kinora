#!/bin/sh
# Kinora: make Clapper 0.10.0 usable from a Windows DLL. Run in Clapper's
# source folder before `meson setup`.
#
# On Linux every function of libclapper is visible; a Windows DLL exports only
# what is marked CLAPPER_API / CLAPPER_GTK_API, and 0.10.0 misses a few the Rust
# bindings link to. Its MPRIS feature is built only on Linux, but the bindings
# link to it everywhere; GDBus is in GIO on Windows too, so build it there (it
# finds no session bus and stays idle).
#
# Every edit is checked: when a new Clapper fixes (or moves) one, this fails
# and says which, instead of patching nothing.
set -e

fail() { echo "patch-clapper: $1" >&2; exit 1; }

# 1. ClapperHarvest's type (g-ir-scanner links against it).
f=src/lib/clapper/clapper-harvest.h
sed -i 's/^G_DECLARE_FINAL_TYPE (ClapperHarvest/CLAPPER_API\n&/' "$f"
grep -B1 '^G_DECLARE_FINAL_TYPE (ClapperHarvest' "$f" | grep -q '^CLAPPER_API$' || fail "$f"

# 2. clapper_get_*_version and clapper_gtk_get_*_version.
for spec in clapper:CLAPPER_API clapper-gtk:CLAPPER_GTK_API; do
  lib=${spec%%:*}
  api=${spec#*:}
  f=src/lib/$lib/$lib-version.h.in
  sed -i "s|^#include <glib.h>|&\n#include <$lib/$lib-visibility.h>|" "$f"
  sed -i -E "s/^(guint|const gchar \*) +(clapper_(gtk_)?get_)/$api\n\1 \2/" "$f"
  [ "$(grep -c "^$api\$" "$f")" -eq 4 ] || fail "$f"
done

# 3. clapper_gtk_get_resource (the compiled-in GResource).
f=src/lib/clapper-gtk/meson.build
sed -i "s/^  c_name: 'clapper_gtk',$/&\n  export: true,/" "$f"
grep -q "^  export: true,$" "$f" || fail "$f"

# 4. The MPRIS feature on Windows, with plain GIO instead of gio-unix.
f=src/lib/clapper/features/mpris/meson.build
sed -i "s/^os_supported = \['linux'\]/os_supported = ['linux', 'windows']/" "$f"
sed -i "s/dependency('gio-unix-2.0'/dependency('gio-2.0'/" "$f"
grep -q "'windows'\]" "$f" && grep -q "dependency('gio-2.0'" "$f" || fail "$f"

echo "patch-clapper: done"
