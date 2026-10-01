// OLED dev bench: replays a scripted network timeline into pnex_status and
// ticks pnex_screen, so the 0.96" pages can be tuned without a server.
//
// Timeline (seconds since boot, then reboot):
//   0  WiFi connecting      4  WiFi OK, WS connecting    9  Registered
//   10 MAIN + random TX/RX  22 link lost (3 s)           28 OTA download
//   34 flashing             37 failed (8 s hold → MAIN)  50 reboot
// Serial prints the worst tick() duration every 2 s (SW I2C cost check).

#include <Arduino.h>

#include "pnex_screen.h"
#include "pnex_status.h"

using pnex_status::PinEntry;
using pnex_status::PinMode;

static const PinEntry DEMO_PINS[] = {
    {0, PinMode::DigitalIn, 0, "flash"},  {2, PinMode::DigitalOut, 0, "led"},
    {4, PinMode::DigitalIn, 0, "door"},   {5, PinMode::DigitalOut, 0, "relay1"},
    {13, PinMode::PwmOut, 42, "fan"},     {15, PinMode::DigitalIn, 0, "btn"},
    {16, PinMode::DigitalOut, 0, "relay2"}, {17, PinMode::AdcIn, 0, "soil"},
    {3, PinMode::DigitalIn, 0, "rx0"},    {1, PinMode::DigitalOut, 0, "tx0"},
    {4, PinMode::DigitalIn, 0, "window"}, {13, PinMode::PwmOut, 100, "pump"},
    {5, PinMode::DigitalIn, 0, "pir"},    {17, PinMode::AdcIn, 0, "light"},
};

static unsigned long s_worst_us = 0, s_report_ms = 0;
static uint8_t s_last_stage = 255;

// One-shot actions when entering a stage (index = seconds bucket).
static void applyStage(uint8_t stage) {
    using pnex_status::Step;
    switch (stage) {
        case 0:
            pnex_status::set_step(Step::Wifi);
            break;
        case 1:
            pnex_status::set_wifi(true, 3);
            pnex_status::set_step(Step::Link);
            break;
        case 2:
            pnex_status::set_link(true);
            pnex_status::set_step(Step::Registered);
            pnex_status::set_pins(DEMO_PINS, sizeof(DEMO_PINS) / sizeof(DEMO_PINS[0]));
            break;
        case 3:
            pnex_status::set_link(false);
            pnex_status::set_wifi(true, 1);
            break;
        case 4:
            pnex_status::set_link(true);
            pnex_status::set_wifi(true, 4);
            break;
        default:
            break;
    }
}

static uint8_t stageAt(unsigned long s) {
    if (s < 4) return 0;
    if (s < 9) return 1;
    if (s < 22) return 2;
    if (s < 25) return 3;
    return 4;
}

void setup() {
    Serial.begin(115200);
    pnex_screen::begin();
    pnex_screen::set_device_id("dev-nodemcu-oled");
}

void loop() {
    const unsigned long now = millis();
    const unsigned long s = now / 1000;
    const uint8_t stage = stageAt(s);
    if (stage != s_last_stage) {
        s_last_stage = stage;
        applyStage(stage);
        Serial.printf("[bench] t=%lus stage=%u\n", s, stage);
    }
    // Traffic once registered: sporadic TX, RX bursts.
    if (s >= 10 && random(100) < 2) {
        pnex_status::bump_tx();
    }
    if (s >= 10 && random(100) < 1) {
        pnex_status::bump_rx();
    }
    // OTA phases.
    if (s >= 28 && s < 34) {
        pnex_status::set_ota("downloading", (uint8_t)((now - 28000) / 60));
    } else if (s >= 34 && s < 37) {
        pnex_status::set_ota("flashing", 100);
    } else if (s >= 37 && s < 38) {
        pnex_status::set_ota("failed", 100);
    } else if (s >= 50) {
        ESP.restart();
    }

    const unsigned long t0 = micros();
    pnex_screen::tick();
    const unsigned long dt = micros() - t0;
    if (dt > s_worst_us) {
        s_worst_us = dt;
    }
    if (now - s_report_ms >= 2000) {
        s_report_ms = now;
        Serial.printf("[bench] worst tick %lu us\n", s_worst_us);
        s_worst_us = 0;
    }
    delay(5);
}
