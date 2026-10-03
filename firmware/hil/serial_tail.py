#!/usr/bin/env python3
"""Prints a board's serial output for a while, without resetting it.

    uv run python hil/serial_tail.py --port /dev/ttyACM0 --seconds 20

DTR/RTS are released RTS first (the other order resets the native USB-JTAG
port and auto-reset bridges), so a running firmware keeps running. The
ESP32-CAM-MB carrier resets on open whatever the order: never tail it in
the middle of a test.
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
    s.open()
    # Linux raises DTR+RTS on open; release RTS FIRST. DTR low while RTS is
    # still high is the reset sequence of the native USB-JTAG port (C3/S3)
    # and pulls EN low on auto-reset bridges (pyserial's preset order).
    s.rts = False
    s.dtr = False
    deadline = time.monotonic() + args.seconds
    try:
        while time.monotonic() < deadline:
            line = s.readline()
            if line:
                try:
                    sys.stdout.write(line.decode("utf-8", "replace"))
                    sys.stdout.flush()
                except BrokenPipeError:
                    return 0
    finally:
        s.close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
