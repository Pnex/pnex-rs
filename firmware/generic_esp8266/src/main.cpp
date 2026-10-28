//
// Generic ESP8266 firmware (Brick 0) — Tier 1 preset, 100 % driven by
// the server. All the mechanics live in the PneX lib (firmware/lib/pnex,
// ex generic main + ex pnex-transport): pin map pushed in the
// ProvisionAck (the board overlay stays the authority — this sketch declares
// NOTHING), SetMode/Write/Subscribe commands with Ack, timed reads
// as StateReport, safe-states on every loss + backoff 1 s → 60 s.
//
// Device config: -D b64 defines via ${sysenv.*} (server build per
// device, contract docs/architecture/firmware-build.md §2.1).
// PNEX_BOARD_NAME distinguishes the announce ("nodemcu").
//
// Wire contract: crates/pnex-core/src/proto.rs; ChaCha20 framing (D8);
// PING 5 s / PONG 15 s.
//

#include <Pnex.h>

PnexDevice pnex;

void setup() {
    pnex.begin();
}

void loop() {
    pnex.loop();
}
