#!/bin/bash
# Kinora: does every program, DLL and Python module in the Windows bundle find
# the DLLs it imports, with nothing but the bundle and Windows itself?
#
#     check-bundle.sh STAGE              report what is missing, fail if any
#     check-bundle.sh STAGE --copy DIR   first copy what is missing from DIR
#                                        into STAGE/bin, until nothing is
#
# It reads the import tables (objdump), as Windows' loader does: ldd misses
# some. Windows looks for imports next to the program (bin\, where the bundle's
# DLLs go), next to the importing file and in System32; the API sets
# (api-ms-win-*, ext-ms-*) are Windows' own. A DLL found elsewhere in the
# bundle is accepted with a note: GStreamer plugins that Clapper's importers
# link to are loaded before them, and Windows reuses a loaded DLL by name.
set -e
stage="$1"
copy_from=""
[ "$2" = "--copy" ] && copy_from="$3"
bin="$stage/bin"
system="$(cygpath -u "${SYSTEMROOT:-C:\\Windows}")/System32"
lower() { tr 'A-Z' 'a-z'; }

# "file<TAB>dll" for every import not found in bin, System32 or beside the
# file. One awk over everything: a process per import is minutes on Windows.
missing() {
  local files root
  files=$(mktemp)
  # D:/... throughout: objdump, a Windows program, prints its paths that way.
  root=$(cygpath -m "$stage")
  find "$root" \( -iname '*.dll' -o -iname '*.pyd' -o -iname '*.exe' \) -type f > "$files"
  {
    # Every name a DLL can be found as: bin, System32, and "dir/name" for
    # each binary of the bundle (found beside its importer).
    { ls "$bin"; ls "$system"; } 2>/dev/null | sed 's/^/K\t/'
    sed 's/^/B\t/' "$files"
    tr '\n' '\0' < "$files" | xargs -0 -n 100 objdump -p 2>/dev/null |
      awk '/file format pei/ { file = $1; sub(/:$/, "", file) }
           /DLL Name:/      { print "I\t" file "\t" $3 }'
  } | awk -F'\t' -v stage="$root/" '
      $1 == "K" { known[tolower($2)] = 1; next }
      $1 == "B" { beside[tolower($2)] = 1; next }
      $1 == "I" {
        dll = tolower($3)
        if (dll ~ /^(api-ms-win-|ext-ms-)/ || dll in known) next
        dir = $2; sub(/\/[^\/]*$/, "", dir)
        if ((tolower(dir "/" $3)) in beside) next
        file = $2
        if (index(file, stage) == 1) file = substr(file, length(stage) + 1)
        print file "\t" $3
      }' | sort -u
  rm -f "$files"
}

if [ -n "$copy_from" ]; then
  for round in 1 2 3 4 5 6 7 8 9 10; do
    copied=0
    for dll in $(missing | cut -f2 | sort -u); do
      if [ -e "$copy_from/$dll" ]; then
        cp "$copy_from/$dll" "$bin/"
        copied=$((copied + 1))
      fi
    done
    echo "check-bundle: round $round copied $copied DLLs"
    [ "$copied" -eq 0 ] && break
  done
fi

problems=0
while IFS=$'\t' read -r file dll; do
  [ -z "$dll" ] && continue
  elsewhere=$(find "$stage" -iname "$dll" -type f | head -1)
  if [ -n "$elsewhere" ]; then
    echo "note: $file imports $dll, loaded from ${elsewhere#"$stage"/} first"
  else
    echo "MISSING: $file needs $dll"
    problems=$((problems + 1))
  fi
done <<<"$(missing)"

if [ "$problems" -gt 0 ]; then
  echo "check-bundle: $problems imports not found"
  exit 1
fi
echo "check-bundle: every import found"
