#!/bin/sh
# Fixture pio toolchain (Phase 6 tests) — replaces PlatformIO.
#
# Behaviour driven by WIFI_SSID (child env in base64, never argv):
#   b64("fail")  → exit 1 (compile failure, stdout filled in)
#   b64("sleep") → sleeps 60 s (timeout path — test.yaml: timeout 2 s)
#   other        → builds .pio/build/stub/{firmware,bootloader,partitions}.bin
#                 whose content materialises the received env (the test proves
#                 the propagation: the 5 device vars in base64, WS_SSL
#                 true/false; the WiFi password too, decrypted from the
#                 vault by the worker — secrets.md S6).
mkdir -p .pio/build/stub

case "$WIFI_SSID" in
  ZmFpbA==)  # base64("fail")
    echo "fixture: simulated compile error"
    # A tool echoing its flags: the worker must mask the token (O4).
    echo "flags -DTOKEN=$TOKEN"
    exit 1
    ;;
  c2xlZXA=)  # base64("sleep")
    sleep 60
    exit 0
    ;;
esac

echo "fixture ssid=$WIFI_SSID pass=$WIFI_PASSWORD host=$HOST token=$TOKEN devid=$DEVICE_ID" > .pio/build/stub/firmware.bin
echo "boot" > .pio/build/stub/bootloader.bin
echo "part" > .pio/build/stub/partitions.bin
