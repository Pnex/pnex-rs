#!/usr/bin/env python3
"""PneX pin self-test bench (D122, docs/architecture/pin-selftest.md).

Flashes the bench firmware (firmware/selftest) on a catalog board, runs
every (gpio, mode) step of the board's plan over the serial console and
writes the report the CI gate checks:

    uv run python hil/pnex_hil.py --board esp32-c3 --port /dev/ttyACM0

The plan and the pass/fail rules live in Rust (pnex_core::selftest); this
script only moves bytes: it never decides a verdict on its own.

The port is ALWAYS explicit (never esptool auto-detection: it once flashed
the wrong board).
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import subprocess
import sys
import time
from pathlib import Path

import serial  # pyserial (esptool dependency)

FIRMWARE_DIR = Path(__file__).resolve().parent.parent
REPO_DIR = FIRMWARE_DIR.parent
REPORTS_DIR = FIRMWARE_DIR / "hil" / "reports"
PREFIX = "PNEXT "


def selftest_cli(*args: str) -> subprocess.CompletedProcess:
    """Runs the pnex-core `selftest` example (plan / check)."""
    return subprocess.run(
        ["cargo", "run", "-q", "-p", "pnex-core", "--example", "selftest", "--", *args],
        cwd=REPO_DIR,
        capture_output=True,
        text=True,
    )


def load_plan(board: str) -> dict:
    out = selftest_cli("plan", board)
    if out.returncode != 0:
        sys.exit(out.stderr.strip() or "plan failed")
    return json.loads(out.stdout)


def flash(plan: dict, port: str) -> None:
    env = dict(os.environ, PNEX_PIO_BOARD=plan["pio_board"], PNEX_BOARD_NAME=plan["board"])
    cmd = ["uv", "run", "pio", "run", "-d", "selftest", "-e", plan["pio_env"],
           "-t", "upload", "--upload-port", port]
    print(f"[hil] flashing bench firmware: {' '.join(cmd)}")
    if subprocess.run(cmd, cwd=FIRMWARE_DIR, env=env).returncode != 0:
        sys.exit("[hil] flash failed")


class Console:
    """Line console over pyserial; every received line goes to the log."""

    def __init__(self, port: str, log_path: Path):
        self.log = log_path.open("w", encoding="utf-8")
        s = serial.Serial()
        s.port = port
        s.baudrate = 115200
        s.timeout = 0.2
        # DTR/RTS released BEFORE open: on auto-reset bridges (NodeMCU,
        # ESP32-CAM-MB) an asserted line holds EN low or IO0 low (boot mode).
        s.dtr = False
        s.rts = False
        s.open()
        self.ser = s

    def reset(self) -> None:
        """EN pulse (no effect on native USB-CDC boards)."""
        self.ser.dtr = False
        self.ser.rts = True
        time.sleep(0.1)
        self.ser.rts = False

    def send(self, line: str) -> None:
        self.log.write(f">>> {line}\n")
        self.ser.write((line + "\n").encode())
        self.ser.flush()

    def wait(self, accept, timeout: float) -> dict | None:
        """Next PNEXT object for which `accept(obj)` is true."""
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            raw = self.ser.readline()
            if not raw:
                continue
            line = raw.decode("utf-8", "replace").rstrip()
            self.log.write(line + "\n")
            if not line.startswith(PREFIX):
                continue
            try:
                obj = json.loads(line[len(PREFIX):])
            except json.JSONDecodeError:
                continue
            if accept(obj):
                return obj
        return None

    def close(self) -> None:
        self.ser.close()
        self.log.close()


def handshake(con: Console) -> dict:
    con.reset()
    for _ in range(15):
        con.send("info")
        info = con.wait(lambda o: o.get("op") in ("info", "ready"), 1.0)
        if info:
            return info
    sys.exit("[hil] no answer from the bench firmware (wrong port? not flashed?)")


def run_steps(con: Console, plan: dict) -> list[dict]:
    results = []
    for step in plan["steps"]:
        cmd = f"test {step['gpio']} {step['mode']}" + (" safe_high" if step["safe_high"] else "")
        con.send(cmd)
        reply = con.wait(
            lambda o: o.get("op") == "test" and (o.get("gpio") == step["gpio"] or "err" in o),
            8.0,
        )
        raw = {k: v for k, v in (reply or {"err": "timeout"}).items() if k not in ("op", "gpio", "mode")}
        print(f"[hil] GPIO{step['gpio']:<3} {step['label']:<10} {step['mode']:<18} {json.dumps(raw)}")
        results.append({"gpio": step["gpio"], "mode": step["mode"], "raw": raw})
    return results


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--board", required=True, help="catalog board name (pnex_core::catalog)")
    ap.add_argument("--port", required=True, help="serial port of THAT board, e.g. /dev/ttyUSB0")
    ap.add_argument("--no-flash", action="store_true", help="bench firmware already flashed")
    ap.add_argument("--level", default="L1", choices=["L1", "L2"])
    args = ap.parse_args()

    plan = load_plan(args.board)
    print(f"[hil] {plan['board']} ({plan['soc']}): {len(plan['steps'])} steps, "
          f"{len(plan['skipped'])} pins skipped")
    if not args.no_flash:
        flash(plan, args.port)

    REPORTS_DIR.mkdir(parents=True, exist_ok=True)
    log_path = REPORTS_DIR / f"{plan['board']}.log"
    con = Console(args.port, log_path)
    try:
        info = handshake(con)
        print(f"[hil] bench firmware: {info}")
        if info.get("chip") != plan["soc"]:
            sys.exit(f"[hil] chip {info.get('chip')} != plan soc {plan['soc']} — wrong board on {args.port}?")
        results = run_steps(con, plan)
    finally:
        con.close()

    report = {
        "board": plan["board"],
        "soc": plan["soc"],
        "level": args.level,
        "date": dt.datetime.now().astimezone().isoformat(timespec="seconds"),
        "chip": info["chip"],
        "results": results,
    }
    report_path = REPORTS_DIR / f"{plan['board']}.json"
    report_path.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(f"[hil] report: {report_path.relative_to(REPO_DIR)} (serial log: {log_path.name})")

    check = selftest_cli("check", str(report_path))
    print(check.stdout, end="")
    print(check.stderr, end="", file=sys.stderr)
    return check.returncode


if __name__ == "__main__":
    sys.exit(main())
