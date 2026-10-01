//
// Device custom PNeX (Tier 2) — sketch type généré par l'UI PNeX.
//
// Déclarez vos pins ICI (outputs, inputs, analogique) : elles partent
// dans l'`announce`, le serveur les valide (chip-caps : flash, strapping,
// ADC, input-only), les persiste, puis les pilote (SetMode / Write /
// Subscribe). Les labels deviennent les séries de télémétrie
// `{org}/{device}/{label}` dans OpenObserve et l'affichage de l'UI
// (page Pins).
//
// Aucune limite de nombre de capteurs (garde-fou compile-time
// PNEX_MAX_PINS=32 par défaut). Les secrets ne sont PAS ici : ils vivent
// dans le platformio.ini (defines b64).
//
// Sans serveur joignable, les sorties restent pilotables localement
// (déjà appliquées au boot) — et toute perte de lien remet les sorties
// en safe-state (défini dans addOutput).
//

#include <Pnex.h>

PnexDevice pnex;

void setup() {
    Serial.begin(115200);

    // ── Mes sorties (actuateurs) — addOutput(gpio, "label", safe_high) ──
    pnex.addOutput(5, "relay1");        // repos = LOW
    pnex.addOutput(4, "pump", true);    // repos = HIGH

    // ── Mes entrées numériques — addInput(gpio, "label", pullup) ──
    pnex.addInput(12, "door", true);    // contact de porte, pull-up interne
    pnex.addInput(13, "button");

    // ── Mes entrées analogiques — addAnalogInput(gpio, "label") ──
    // ESP8266 : le gpio est ignoré (canal unique A0).
    pnex.addAnalogInput(0, "photo");

    // WiFi (40 essais) + 1re connexion WS + announce. Ensuite : boucle.
    pnex.begin();
}

void loop() {
    pnex.loop();
}
