//
// Firmware générique ESP32 (DevKit WROOM, preset Tier 1) — 100 % piloté
// par le serveur. Même lib PneX que le C3 et l'8266 (aucune divergence
// protocolaire) : pin map poussée dans le ProvisionAck (l'overlay board
// reste l'autorité — ce sketch ne déclare RIEN), commandes
// SetMode/Write/Subscribe avec Ack, lectures cadencées en StateReport,
// safe-states + backoff.
//
// Spécifique ESP32 classique (absorbé dans la lib, rappel ici) :
// - analogRead sur le GPIO du pin (12-bit) — chip-caps limitent analog_in
//   aux GPIO32–39 (ADC1 : ADC2 est cassé avec le WiFi actif) ;
// - GPIO34–39 input-only (pas de driver de sortie, pas de pull-up interne) ;
// - le monitor passe par le pont UART GPIO1/3 (réservés console) ;
// - pas de watchdog utilisateur armé — safe-states + backoff couvrent les
//   pertes de lien (§3/§8).
//
// Config device : -D defines b64 via ${sysenv.*} (build serveur par
// device, contrat docs/architecture/firmware-build.md §2.1).
// PNEX_BOARD_NAME distingue l'announce (id fil de la variante).
//

#include <Pnex.h>

PnexDevice pnex;

void setup() {
    pnex.begin();
}

void loop() {
    pnex.loop();
}
