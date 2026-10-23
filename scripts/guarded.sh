#!/usr/bin/env bash
# guarded.sh <command...> — runs a heavy build (cargo, dx, pio, docker
# buildx) without taking the rest of the desktop down with it.
#
# Why (2026-10-09, 5th occurrence): a Rust build run from a terminal lives
# in the terminal's cgroup. When it exhausts the memory, systemd-oomd kills
# the WHOLE cgroup — every terminal tab and session with it
# (`app-gnome-dev.warp.Warp-*.scope: systemd-oomd killed some process(es)`).
#
# What it does:
# - its own transient systemd scope with a hard memory ceiling: past it the
#   kernel OOM-kills inside the build scope only, never the terminal (no
#   MemoryHigh: throttling stalls a build forever instead of failing it; no
#   swap, so it fails fast instead of thrashing the desktop);
# - 2026-10-16: at the ceiling the scope thrashes on page cache instead of
#   OOMing, the pressure spills onto user@.service (oomd limit 50%) and oomd
#   killed GNOME Shell. The scope is now watched by oomd itself at 20%, so
#   it crosses its own limit first and is the one killed;
# - lowest CPU / IO priority and a high OOM score (the build is the
#   kernel's first victim, before anything interactive);
# - refuses to start under PNEX_GUARD_MIN_DISK_GB free on the target disk
#   (a full disk crashed the linker and starved Postgres the same day);
# - one heavy build at a time across every session/worktree (machine-wide
#   lock, waits its turn); PNEX_GUARD_NO_LOCK=1 for long-running processes
#   (dev servers, `dx serve`) that must not hold it;
# - CARGO_BUILD_JOBS defaults to 6;
# - 2026-10-22: a 19 GB workspace test build (60 test binaries of 550 MB
#   linked 6 at a time) under its 24G ceiling, plus a game, two browsers
#   and Docker, pushed the whole session over oomd's 50% and GNOME was
#   killed before the build. The ceiling is now what the desktop can spare
#   when the build starts: MemAvailable minus PNEX_GUARD_RESERVE_GB, at
#   most 16G, 4 jobs (2 under 12G), refused under 8G; and oomd kills the
#   build at 10% pressure in its own scope, before the desktop feels it.
#   A build that hits the ceiling fails; the session must never.
#
# Tunables (env): PNEX_GUARD_MEM_MAX (fixed ceiling, skips the computed
# one), PNEX_GUARD_RESERVE_GB (16), PNEX_GUARD_MIN_DISK_GB (25),
# PNEX_GUARD_NO_LOCK, CARGO_BUILD_JOBS.
# On CI or without a systemd user session (containers) it runs the command
# with the priority settings only.
set -euo pipefail

if [ "$#" -eq 0 ]; then
  echo "usage: scripts/guarded.sh <command...>" >&2
  exit 64
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
min_disk_gb="${PNEX_GUARD_MIN_DISK_GB:-25}"
reserve_gb="${PNEX_GUARD_RESERVE_GB:-16}"
jobs_set="${CARGO_BUILD_JOBS:-}"

# Free space on the disk holding target/ (where builds write).
free_gb="$(df --output=avail -BG "$repo_root" | tail -1 | tr -dc '0-9')"
# CI runners have little disk by design and no desktop to protect: skip.
if [ -z "${CI:-}" ] && [ -n "$free_gb" ] && [ "$free_gb" -lt "$min_disk_gb" ]; then
  echo "guarded: only ${free_gb}G free on the build disk (< ${min_disk_gb}G) — refusing to build." >&2
  echo "guarded: free space first: \`task clean:incremental\`, then \`task clean:size\` (DRY=1 to preview)." >&2
  exit 75
fi

# Priority wrappers, when available.
prio=()
command -v nice >/dev/null && prio+=(nice -n 10)
command -v ionice >/dev/null && prio+=(ionice -c 2 -n 7)
command -v choom >/dev/null && prio+=(choom -n 800 --)

# Ceiling from the memory free right now (read once the lock is held: the
# previous build has given its memory back by then).
size_build() {
  if [ -n "${PNEX_GUARD_MEM_MAX:-}" ]; then
    mem_max="$PNEX_GUARD_MEM_MAX"
    export CARGO_BUILD_JOBS="${jobs_set:-4}"
    return
  fi
  local avail_gb cap_gb
  avail_gb="$(awk '/^MemAvailable:/ {print int($2 / 1048576)}' /proc/meminfo 2>/dev/null || echo 32)"
  cap_gb=$((avail_gb - reserve_gb))
  [ "$cap_gb" -gt 16 ] && cap_gb=16
  if [ -z "${CI:-}" ] && [ "$cap_gb" -lt 8 ]; then
    echo "guarded: only ${avail_gb}G of memory available (${reserve_gb}G kept for the desktop) — refusing to build." >&2
    echo "guarded: close something heavy (game, VM, browser tabs) or stop a dev server, then retry." >&2
    exit 75
  fi
  mem_max="${cap_gb}G"
  if [ -n "$jobs_set" ]; then
    export CARGO_BUILD_JOBS="$jobs_set"
  elif [ "$cap_gb" -lt 12 ]; then
    export CARGO_BUILD_JOBS=2
  else
    export CARGO_BUILD_JOBS=4
  fi
  echo "guarded: ceiling ${mem_max} (${avail_gb}G available), ${CARGO_BUILD_JOBS} jobs" >&2
}

run() {
  size_build
  # CI: no desktop to protect, and the reserve can leave a negative ceiling.
  if [ -z "${CI:-}" ] && command -v systemd-run >/dev/null && systemctl --user show-environment >/dev/null 2>&1; then
    exec systemd-run --user --scope --quiet --collect \
      --unit="pnex-build-$$" \
      -p MemoryMax="$mem_max" -p MemorySwapMax=0 \
      -p ManagedOOMMemoryPressure=kill -p ManagedOOMMemoryPressureLimit=10% \
      -- "${prio[@]}" "$@"
  fi
  exec "${prio[@]}" "$@"
}

if [ "${PNEX_GUARD_NO_LOCK:-}" = "1" ] || ! command -v flock >/dev/null; then
  run "$@"
fi

lock="${XDG_RUNTIME_DIR:-/tmp}/pnex-heavy-build.lock"
exec 9>"$lock"
if ! flock -n 9; then
  echo "guarded: another heavy build is running (lock $lock) — waiting for it…" >&2
  flock 9
fi
run "$@"
