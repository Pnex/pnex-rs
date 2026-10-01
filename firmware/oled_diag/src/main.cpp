// oled_diag — diagnostic OLED 0.96" sur NodeMCU V3 OLED (SDA=D6/12, SCL=D5/14)
// Trois tests en séquence, résultats sur le port série 115200 :
//   1. scan Wire sur 12/14 puis 4/5 (qui ACK ? à quelle adresse ?)
//   2. U8g2 SW_I2C (config prouvée soil_sensor : clock 14, data 12)
//   3. U8g2 HW_I2C (config pnex_screen : Wire.begin(12,14) puis HW)
// L'écran finit sur le test HW — si "HW ok" n'apparaît pas, HW est coupable.
#include <Arduino.h>
#include <Wire.h>
#include <U8g2lib.h>

static void scan(const char* tag, int sda, int scl) {
    Wire.begin(sda, scl);
    Wire.setClock(100000);
    Serial.printf("[SCAN] %s (sda=%d scl=%d) :", tag, sda, scl);
    int found = 0;
    for (uint8_t a = 1; a < 127; a++) {
        Wire.beginTransmission(a);
        if (Wire.endTransmission() == 0) {
            Serial.printf(" 0x%02X", a);
            found++;
        }
    }
    if (found == 0) {
        Serial.print(" rien");
    }
    Serial.println();
}

static void swTest() {
    Serial.println("[SW] U8G2 SW_I2C clock=14 data=12 (soil_sensor) ...");
    U8G2_SSD1306_128X64_NONAME_F_SW_I2C u8g2(U8G2_R0, /*clock*/ 14, /*data*/ 12,
                                             U8X8_PIN_NONE);
    bool ok = u8g2.begin();
    Serial.printf("[SW] begin()=%d\n", ok ? 1 : 0);
    u8g2.clearBuffer();
    u8g2.setFont(u8g2_font_ncenB14_tr);
    u8g2.drawStr(5, 40, "SW ok");
    u8g2.sendBuffer();
}

static void hwTest() {
    Serial.println("[HW] Wire.begin(12,14) + U8G2 HW_I2C (pnex_screen) ...");
    Wire.begin(12, 14);
    U8G2_SSD1306_128X64_NONAME_F_HW_I2C u8g2(U8G2_R0, U8X8_PIN_NONE);
    bool ok = u8g2.begin();
    Serial.printf("[HW] begin()=%d\n", ok ? 1 : 0);
    u8g2.clearBuffer();
    u8g2.setFont(u8g2_font_ncenB14_tr);
    u8g2.drawStr(5, 40, "HW ok");
    u8g2.sendBuffer();
}

void setup() {
    Serial.begin(115200);
    delay(2000);
    Serial.println("\n[oled_diag] boot");
    scan("12/14", 12, 14);
    scan("4/5", 4, 5);
    swTest();
    delay(5000);  // 5 s pour lire "SW ok" à l'écran
    hwTest();
    Serial.println("[oled_diag] fin — l'écran doit montrer 'HW ok'");
}

void loop() {
    delay(1000);
}
