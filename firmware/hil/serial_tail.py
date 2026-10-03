#!/usr/bin/env python3
"""Prints a board's serial output for a while, without resetting it.

    uv run python hil/serial_tail.py --port /dev/ttyACM0 --seconds 20

DTR/RTS are released before the port opens (an asserted line resets
auto-reset bridges or holds IO0 low), so a running firmware keeps running.
Used by the hardware e2e tests to assert the real pin values the firmware
logs (`[IO] ...`, `readback=`).
"""

import argparse
import sys
import time

import serial  # pyserial (esptool dependency)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", required=True)
    ap.add_argument("--seconds", type=float, required=True)
    args = ap.parse_args()

    s = serial.Serial()
    s.port = args.port
    s.baudrate = 115200
    s.timeout = 0.2
    s.dtr = False
    s.rts = False
    s.open()
    deadline = time.monotonic() + args.seconds
    try:
        while time.monotonic() < deadline:
            line = s.readline()
            if line:
                sys.stdout.write(line.decode("utf-8", "replace"))
                sys.stdout.flush()
    finally:
        s.close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
