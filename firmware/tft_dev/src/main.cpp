#include <base64.hpp>
#include <U8g2lib.h>
#include "DisplayManager.h"
#include "config.h"

DisplayManager displayManager;

bool shouldReconnect = true;  // Flag to control reconnection attempts
unsigned long lastReconnectAttempt = 0;
const unsigned long reconnectInterval = 5000;  // Reconnect interval in milliseconds

char websockets_connection_string[256];
const char* wss_format = "ws://%s/ws/actuator/cast?token=%s&pred_dev=%s&device_id=%s&metadata=%s";

void setup() {
    Serial.begin(115200);
    displayManager.init();

    // Show loading animation with title "pnex.io"
    // Duration: 5000ms (5 seconds), Update interval: 50ms (smooth animation)
    displayManager.showLoadingAnimation("pnex.io", 5000, 50);

    // After loading animation, show other information
    displayManager.clear();
    displayManager.showMessage("System Ready");
    delay(1000);

    // Demo: Show connection icons
    displayManager.wifiConnected();
    delay(1000);
    displayManager.webSocketConnected();
    delay(2000);
}

void loop() {
}
