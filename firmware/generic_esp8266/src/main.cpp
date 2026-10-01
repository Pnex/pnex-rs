//
// Firmware générique ESP8266 (Brick 0) — preset Tier 1, 100 % piloté par
// le serveur. Toute la mécanique vit dans la lib PneX (firmware/lib/pnex,
// ex-main générique + ex-pnex-transport) : pin map poussée dans le
// ProvisionAck (l'overlay board reste l'autorité — ce sketch ne déclare
// RIEN), commandes SetMode/Write/Subscribe avec Ack, lectures cadencées
// en StateReport, safe-states sur chaque perte + backoff 1 s → 60 s.
//
// Config device : -D defines b64 via ${sysenv.*} (build serveur par
// device, contrat docs/architecture/firmware-build.md §2.1).
// PNEX_BOARD_NAME distingue l'announce (« nodemcu »).
//
// Contrat fil : crates/pnex-core/src/proto.rs ; framing ChaCha20 (D8) ;
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
