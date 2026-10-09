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
# - lowest CPU / IO priority and a high OOM score (the build is the
#   kernel's first victim, before anything interactive);
# - refuses to start under PNEX_GUARD_MIN_DISK_GB free on the target disk
#   (a full disk crashed the linker and starved Postgres the same day);
# - one heavy build at a time across every session/worktree (machine-wide
#   lock, waits its turn); PNEX_GUARD_NO_LOCK=1 for long-running processes
#   (dev servers, `dx serve`) that must not hold it;
# - CARGO_BUILD_JOBS defaults to 6.
#
# Tunables (env): PNEX_GUARD_MEM_MAX (default 24G), PNEX_GUARD_MIN_DISK_GB
# (25), PNEX_GUARD_NO_LOCK, CARGO_BUILD_JOBS.
# Without a systemd user session (CI, containers) it runs the command with
# the priority settings only.
set -euo pipefail

if [ "$#" -eq 0 ]; then
  echo "usage: scripts/guarded.sh <command...>" >&2
  exit 64
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
mem_max="${PNEX_GUARD_MEM_MAX:-24G}"
min_disk_gb="${PNEX_GUARD_MIN_DISK_GB:-25}"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-6}"

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

run() {
  if command -v systemd-run >/dev/null && systemctl --user show-environment >/dev/null 2>&1; then
    exec systemd-run --user --scope --quiet --collect \
      --unit="pnex-build-$$" \
      -p MemoryMax="$mem_max" -p MemorySwapMax=0 \
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
