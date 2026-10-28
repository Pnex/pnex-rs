#!/bin/sh
# Fixture esptool (Phase 6 tests) — merge-bin: concatenates into the file
# given after -o all the other arguments that are files.
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
