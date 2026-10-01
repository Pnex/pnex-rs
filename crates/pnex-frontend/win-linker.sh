#!/bin/sh
# Linker wrapper for the Windows desktop cross-build (x86_64-pc-windows-gnu,
# task build:frontend:windows).
#
# - dx injects MSVC-only flags for every Windows bundle
#   (`/SUBSYSTEM:WINDOWS /ENTRY:mainCRTStartup`); a GNU linker reads them as
#   input files. They are dropped here — the GUI subsystem comes from the
#   `windows_subsystem` attribute in main.rs instead.
# - dx reads the manganis `__ASSETS__` symbols from `<exe>.pdb` when one sits
#   next to the uplifted exe; cargo only uplifts PDBs for MSVC targets. If the
#   linker produced one (lld-based cc), copy it next to the uplifted exe.
#
# Real compiler driver: $PNEX_WIN_CC (default: mingw-w64 gcc).
cc="${PNEX_WIN_CC:-x86_64-w64-mingw32-gcc}"
out=""
prev=""
for arg in "$@"; do
  shift
  case "$arg" in
    /SUBSYSTEM:* | /ENTRY:*) ;;
    *) set -- "$@" "$arg" ;;
  esac
  [ "$prev" = "-o" ] && out="$arg"
  prev="$arg"
done

# A stale PDB (from a previous link with another linker) would be read by dx
# with wrong offsets → garbage asset data / panic: remove both copies first,
# then only propagate a PDB this link produced.
pdb=""
uplifted=""
case "$out" in
  */deps/*.exe)
    pdb="${out%.exe}.pdb"
    stem=$(basename "$out" .exe)
    uplifted="$(dirname "$(dirname "$out")")/${stem%-*}.pdb"
    rm -f "$pdb" "$uplifted"
    ;;
esac

$cc "$@" || exit $?

[ -n "$pdb" ] && [ -f "$pdb" ] && cp "$pdb" "$uplifted"
exit 0
