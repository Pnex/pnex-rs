#!/bin/sh
# Image build only (Dockerfile, target `builder`). Installs, without
# compiling, the libraries of every project the worker builds — each
# project's own lib_deps plus the custom-firmware catalog entries its SoC
# supports (lib_catalog.txt) — and keeps them as the per-project seed
# `<core>/pnex-libdeps/<project>`, copied into each build's `.pio/libdeps`
# by the worker. Together with the baked platforms, builds need no network.
#
# Args: <firmware tree> <project>:<board>... (a real board id: PlatformIO
# resolves it even when only installing).
set -eu
fw=$1
shift
catalog=$(dirname "$0")/lib_catalog.txt
# The generic inis interpolate these `${sysenv.*}`; values do not matter.
export PNEX_BOARD_NAME=prewarm PNEX_FW_VERSION=0 WIFI_SSID= WIFI_PASSWORD= \
    HOST= TOKEN= DEVICE_ID= ENCRYPTION_KEY= WS_SSL=0 PNEX_CA_CERT=
for v in SSD1306 ST7735; do export "PNEX_SCREEN_$v=0"; done
for v in SDA SCL SCK MOSI CS DC RST; do export "PNEX_SCREEN_$v=-1"; done
for pb in "$@"; do
    p=${pb%%:*}
    export PNEX_PIO_BOARD="${pb#*:}"
    core=$PLATFORMIO_CORE_DIR
    if grep -q 'pioarduino/platform-espressif32' "$fw/$p/platformio.ini"; then
        core=$PNEX_PIO_CORE_DIR_PIOARDUINO
    fi
    PLATFORMIO_CORE_DIR=$core pio pkg install -d "$fw/$p"
    grep "^$p " "$catalog" | cut -d' ' -f2- | while IFS= read -r spec; do
        PLATFORMIO_CORE_DIR=$core pio pkg install -d "$fw/$p" --no-save -l "$spec"
    done
    mkdir -p "$core/pnex-libdeps"
    rm -rf "$core/pnex-libdeps/$p"
    mv "$fw/$p/.pio/libdeps" "$core/pnex-libdeps/$p"
done
