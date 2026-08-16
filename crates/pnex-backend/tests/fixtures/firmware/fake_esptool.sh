#!/bin/sh
# esptool de fixture (tests Phase 6) — merge-bin : concatène dans le
# fichier passé après -o tous les autres arguments qui sont des fichiers.
out=""
prev=""
for a in "$@"; do
  if [ "$prev" = "-o" ]; then
    out="$a"
  fi
  prev="$a"
done

: > "$out"
for a in "$@"; do
  if [ -f "$a" ] && [ "$a" != "$out" ]; then
    cat "$a" >> "$out"
  fi
done
