//
// PneX custom firmware — starter sketch (custom firmware IDE, D87/D88).
//
// Everything PneX needs (WiFi, encrypted link, OTA, debug screen) lives in
// the PneX library: this file only holds YOUR logic. Secrets (WiFi, token,
// key) are never written here — the server injects them at build time.
//
// Rules of thumb:
// - call pnex.loop() on EVERY loop() iteration and never block for long
//   (no long delay()): past the ping timeout the link drops and outputs
//   fall back to their safe state;
// - schedule periodic work with millis(), as below;
// - declare metrics / commands / pins BEFORE pnex.begin().
//
// Example with an I2C sensor (add "Adafruit BME280" from the library
// catalog first):
//
//   #include <Wire.h>
//   #include <Adafruit_BME280.h>
//   Adafruit_BME280 bme;
//   // setup():  Wire.begin(); bme.begin(0x76); pnex.addMetric("temp", "°C");
//   // loop():   pnex.publish("temp", bme.readTemperature());
//

#include <Pnex.h>

PnexDevice pnex;

// Command handler: called when a flow (or the UI) sends "blink" to this
// device. Keep it short — it runs inside the network callback.
static volatile bool blink_requested = false;

bool onBlink(JsonVariantConst args) {
    (void)args;  // free JSON sent with the command, e.g. {"times": 3}
    blink_requested = true;
    return true;  // true = Ack ok, false = Ack "command_failed"
}

void setup() {
    Serial.begin(115200);

    // Metrics: each id becomes the telemetry series {org}/{device}/{id}.
    pnex.addMetric("uptime_s", "s");
    pnex.addMetric("wifi_rssi", "dBm");

    // Commands callable from PneX.
    pnex.onCommand("blink", onBlink);

    // Pins driven by the server can still be declared as usual:
    // pnex.addOutput(5, "relay1");

    // WiFi + first connection to the server + announce.
    pnex.begin();
}

unsigned long last_publish_ms = 0;

void loop() {
    pnex.loop();  // mandatory on every iteration

    if (millis() - last_publish_ms >= 10000) {
        last_publish_ms = millis();
        pnex.publish("uptime_s", millis() / 1000UL);
        pnex.publish("wifi_rssi", (long)WiFi.RSSI());
    }

    if (blink_requested) {
        blink_requested = false;
        Serial.println("[APP] blink requested");
    }
}
