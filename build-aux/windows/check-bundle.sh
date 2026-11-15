#!/bin/bash
# Kinora: does every program, DLL and Python module in the Windows bundle find
# the DLLs it imports, with nothing but the bundle and Windows itself? Prints
# each one that would not, and fails if any.
#
#     check-bundle.sh STAGE
#
# Windows looks for a DLL's imports next to the program (bin\, where every DLL
# of the bundle goes), next to the importing DLL and in System32; the API sets
# (api-ms-win-*, ext-ms-*) are Windows' own.
set -e
stage="$1"
bin="$stage/bin"
system="$(cygpath -u "${SYSTEMROOT:-C:\\Windows}")/System32"

# Lower-case names of everything a DLL can be found as.
known=$( { ls "$bin"; ls "$system"; } 2>/dev/null | tr 'A-Z' 'a-z' | sort -u)

find "$stage" \( -iname '*.dll' -o -iname '*.pyd' -o -iname '*.exe' \) -type f -print0 |
  xargs -0 -n 50 objdump -p 2>/dev/null |
  awk '/file format pei/ { file = $1; sub(/:$/, "", file) }
       /DLL Name:/      { print file "\t" $3 }' |
  while IFS=$'\t' read -r file dll; do
    name=$(echo "$dll" | tr 'A-Z' 'a-z')
    case "$name" in api-ms-win-*|ext-ms-*) continue ;; esac
    if ! grep -qxF "$name" <<<"$known" && [ ! -e "$(dirname "$file")/$dll" ]; then
      echo "${file#"$stage"/} needs $dll"
    fi
  done | sort -u > /tmp/missing-dlls.txt

if [ -s /tmp/missing-dlls.txt ]; then
  echo "check-bundle: $(wc -l < /tmp/missing-dlls.txt) imports not found in the bundle:"
  cat /tmp/missing-dlls.txt
  exit 1
fi
echo "check-bundle: every import found"
