//
// Firmware générique ESP32-C3 (Seeed XIAO ESP32C3) — preset Tier 1,
// 100 % piloté par le serveur. Même lib PneX que l'ESP8266 (ex-miroir
// generic_esp8266, plus AUCUNE divergence protocolaire) : pin map poussée
// dans le ProvisionAck (l'overlay board reste l'autorité — ce sketch ne
// déclare RIEN), commandes SetMode/Write/Subscribe avec Ack, lectures
// cadencées en StateReport, safe-states + backoff.
//
// Spécifique C3 (absorbé dans la lib, rappel ici) :
// - analogRead sur le GPIO du pin (12-bit, 0-4095 ; l'esp8266 lisait A0,
//   canal ADC unique 10-bit) — chip-caps limitent analog_in aux GPIO0-4
//   (ADC1 : ADC2/GPIO5 est cassé avec le WiFi actif) ;
// - pas de watchdog utilisateur armé (loopTask non surveillé sur
//   arduino-esp32) — les pertes de lien restent couvertes par les
//   safe-states + backoff (§3/§8) ;
// - l'USB-CDC (ttyACM0) est le port de log/monitor (ARDUINO_USB_CDC_ON_BOOT).
//
// Config device : -D defines b64 via ${sysenv.*} (build serveur par
// device, contrat docs/architecture/firmware-build.md §2.1).
// PNEX_BOARD_NAME distingue l'announce (« xiao_esp32c3 »).
//

#include <Pnex.h>

PnexDevice pnex;

void setup() {
    pnex.begin();
}

void loop() {
    pnex.loop();
}
