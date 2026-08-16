#!/bin/sh
# Toolchain pio de fixture (tests Phase 6) — remplace PlatformIO.
#
# Comportement piloté par WIFI_SSID (env du child, jamais argv) :
#   "fail"  → exit 1 (échec de compilation, sortie standard renseignée)
#   "sleep" → dort 60 s (chemin du timeout — test.yaml : timeout 2 s)
#   autre   → fabrique .pio/build/stub/{firmware,bootloader,partitions}.bin
#             dont le contenu matérialise les env reçues (le test prouve
#             la propagation : WiFi clair, HOST/TOKEN/DEVICE_ID base64).
mkdir -p .pio/build/stub

case "$WIFI_SSID" in
  fail)
    echo "fixture: erreur de compilation simulée"
    exit 1
    ;;
  sleep)
    sleep 60
    exit 0
    ;;
esac

echo "fixture ssid=$WIFI_SSID host=$HOST token=$TOKEN devid=$DEVICE_ID" > .pio/build/stub/firmware.bin
echo "boot" > .pio/build/stub/bootloader.bin
echo "part" > .pio/build/stub/partitions.bin
