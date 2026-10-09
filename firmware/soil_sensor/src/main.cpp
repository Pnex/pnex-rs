#include <ESP8266WiFi.h>
#include <OneWire.h>
#include <DallasTemperature.h>
#include <U8g2lib.h>
#include "DisplayManager.h"
#include "pnex_transport.h"

// Soil moisture sensor
#define SOIL_MOISTURE_PIN A0
// OneWire temperature sensor
#define ONE_WIRE_BUS D6

DisplayManager displayManager;

// temperature sensor
OneWire oneWire(ONE_WIRE_BUS);
DallasTemperature DS18B20(&oneWire);

bool shouldReconnect = true;  // Flag to control reconnection attempts
unsigned long lastReconnectAttempt = 0;
const unsigned long reconnectInterval = 5000;  // Reconnect interval in milliseconds

// Ping timing
unsigned long lastPing = 0;
const unsigned long PING_INTERVAL = 5000;  // 5 seconds

// Connection tracking for loading animation
bool wifi_connected = false;
bool websocket_connected = false;
bool initial_pong_received = false;  // Track first PONG for loading completion

// Function prototypes
void connectWiFi();
void connectWebSocket();
void feedWatchdog();
static void on_ws_message(const String& plain);
static void on_ws_pong();
static void on_ws_opened();
static void on_ws_closed();
void sendPing();
int readSoilMoisturePercentage(int analogPin);

// ─────────────────────── Callbacks transport ───────────────────────

static void on_ws_opened() {
    displayManager.showArrowDown();
    Serial.println("[WS] Connection Opened");
    websocket_connected = true;
    displayManager.webSocketConnected();
    shouldReconnect = false;  // Disable reconnection attempts
    displayManager.hideArrowDown();
}

static void on_ws_closed() {
    displayManager.showArrowDown();
    Serial.println("[WS] Connection Closed");
    websocket_connected = false;
    displayManager.webSocketDisconnected();
    shouldReconnect = true;  // Enable reconnection attempts
    displayManager.hideArrowDown();
}

static void on_ws_message(const String& plain) {
    displayManager.showArrowDown();
    if (plain.length() == 0) {
        Serial.println("[WS] Got Message: <frame illisible>");
    } else {
        Serial.print("[WS] Got Message: ");
        Serial.println(plain);
    }
    delay(50);  // Brief delay to ensure arrow is visible
    displayManager.hideArrowDown();
}

static void on_ws_pong() {
    displayManager.showArrowDown();
    initial_pong_received = true;  // Mark first PONG received
    Serial.println("[Ping] <- PONG received");
    delay(50);  // Brief delay to ensure arrow is visible
    displayManager.hideArrowDown();
}

void setup() {
    Serial.begin(115200);
    delay(1000);

    // Compiled config, Noise key, CA pin, client identity and wss URL set
    // up by pnex-transport (F1): without a valid key the device never
    // connects (no clear-text mode).
    PnexTransportInit ti;
    ti.ws_path = "/ws/sensor/ingest";
    ti.pong_timeout_ms = 0;     // pas de timeout PONG dans ce firmware
    ti.reply_ws_ping = false;   // ne répond pas aux pings WS du serveur
    ti.on_connected = on_ws_opened;
    ti.on_message = on_ws_message;
    ti.on_pong = on_ws_pong;
    ti.on_closed = on_ws_closed;
    pnex_transport_setup(ti);
    if (pnex_crypto_ready()) {
        Serial.println("[Crypto] ChaCha20 active (ENCRYPTION_KEY chargee)");
    } else {
        Serial.println("[Crypto] ENCRYPTION_KEY absente/invalide — frames EN CLAIR (mock local uniquement)");
    }

    Serial.println("\n\n");
    Serial.println("===============================================");
    Serial.println("  ESP8266 Soil Sensor v2.0");
    Serial.println("  Protocol: WebSocket + Text/JSON");
    Serial.println("===============================================");
    Serial.printf("Device ID: %s\n", pnex_device_id());
    Serial.printf("Server: %s\n", pnex_host());
    Serial.println("===============================================\n");

    // Initialize display
    displayManager.init();

    // Track current progress for smooth animations
    int currentProgress = 0;

    // Show loading animation: 0% - Starting
    displayManager.showLoadingProgress("pnex.io", currentProgress, "Starting...");
    delay(300);

    // Initialize sensors
    DS18B20.begin();

    // Animate to 10% - Sensor Setup
    currentProgress = 10;
    displayManager.showLoadingProgressAnimated("pnex.io", 0, currentProgress, "Sensor Setup...", 300);
    delay(200);

    // Animate to 20% - WiFi Connecting
    int nextProgress = 20;
    displayManager.showLoadingProgressAnimated("pnex.io", currentProgress, nextProgress, "WiFi Connecting...", 200);
    currentProgress = nextProgress;

    // Connect to WiFi
    connectWiFi();

    // WiFi connected: 50%
    nextProgress = 50;
    if (wifi_connected) {
        displayManager.showLoadingProgressAnimated("pnex.io", currentProgress, nextProgress, "WiFi Connected!", 400);
        currentProgress = nextProgress;
        delay(300);
    } else {
        displayManager.showLoadingProgressAnimated("pnex.io", currentProgress, nextProgress, "WiFi Failed!", 400);
        currentProgress = nextProgress;
        delay(1000);
    }

    // Connection string, built by pnex-transport at setup (wss, token in
    // the Authorization header).
    Serial.print("[WS] Connection string: ");
    Serial.println(pnex_conn_string());

    // Setup WebSocket: Animate to 60%
    nextProgress = 60;
    displayManager.showLoadingProgressAnimated("pnex.io", currentProgress, nextProgress, "WS Setup...", 300);
    currentProgress = nextProgress;

    // Connect to WebSocket: Animate to 70%
    nextProgress = 70;
    displayManager.showLoadingProgressAnimated("pnex.io", currentProgress, nextProgress, "WS Connecting...", 300);
    currentProgress = nextProgress;

    // Connect to WebSocket
    connectWebSocket();

    // WebSocket connected: Animate to 80%
    nextProgress = 80;
    if (websocket_connected) {
        displayManager.showLoadingProgressAnimated("pnex.io", currentProgress, nextProgress, "WS Connected!", 350);
        currentProgress = nextProgress;
        delay(300);
    } else {
        displayManager.showLoadingProgressAnimated("pnex.io", currentProgress, nextProgress, "WS Failed!", 350);
        currentProgress = nextProgress;
        delay(1000);
    }

    // Initialize and feed the watchdog timer
    ESP.wdtDisable();
    ESP.wdtEnable(WDTO_4S);

    // Wait for first PING/PONG to confirm active connection: Animate to 85%
    nextProgress = 85;
    displayManager.showLoadingProgressAnimated("pnex.io", currentProgress, nextProgress, "Waiting PING...", 200);
    currentProgress = nextProgress;

    // Send initial PING
    if (websocket_connected) {
        lastPing = millis();
        pnex_ws_send_ping();
        Serial.println("[Setup] Initial PING sent");

        // Wait for PONG (max 5 seconds)
        unsigned long pong_wait_start = millis();
        while (!initial_pong_received && (millis() - pong_wait_start < 5000)) {
            pnex_ws_poll();
            ESP.wdtFeed();
            delay(100);
        }

        if (initial_pong_received) {
            // Animate to 100%
            displayManager.showLoadingProgressAnimated("pnex.io", currentProgress, 100, "PONG Received!", 400);
            Serial.println("[Setup] Initial PONG received - connection validated");
            delay(500);
        } else {
            // Animate to 90% on timeout
            displayManager.showLoadingProgressAnimated("pnex.io", currentProgress, 90, "PONG Timeout!", 200);
            Serial.println("[Setup] Initial PONG timeout - proceeding anyway");
            delay(1000);
        }
    }

    // Clear display completely and ensure buffer is clean before showing normal status
    displayManager.clear();
    delay(100);  // Small delay to ensure display clears completely

    if (wifi_connected) {
        displayManager.wifiConnected();
    } else {
        displayManager.wifiDisconnected();
    }

    if (websocket_connected) {
        displayManager.webSocketConnected();
    } else {
        displayManager.webSocketDisconnected();
    }

    // Display the decoded device ID
    char decodedId[65];
    strlcpy(decodedId, pnex_device_id(), sizeof(decodedId));
    displayManager.showDeviceID(decodedId);

    Serial.println("\n[Setup] Complete - Entering main loop\n");
}

void loop() {
    feedWatchdog();

    unsigned long now = millis();

    // Poll the WebSocket client
    if (pnex_ws_available()) {
        pnex_ws_poll();

        // Send periodic ping
        if (now - lastPing > PING_INTERVAL) {
            sendPing();
            lastPing = now;
        }
    } else {
        // If the client is not available, try to reconnect
        Serial.println("[WS] WebSocket client disconnected. Reconnecting...");
        connectWebSocket();
    }

    // Attempt to reconnect if the connection is lost
    if (shouldReconnect && !pnex_ws_available() && millis() - lastReconnectAttempt > reconnectInterval) {
        Serial.println("Attempting to reconnect...");
        lastReconnectAttempt = millis();
        connectWebSocket();
	}

    if (pnex_ws_available()){
	Serial.println("[Sensor] Reading Moisture");
	int soilMoisturePercentage = readSoilMoisturePercentage(SOIL_MOISTURE_PIN);
	Serial.print("[Sensor] Moisture: ");
	Serial.println(soilMoisturePercentage);
	String data = "soil_moisture=" + String(soilMoisturePercentage);
        displayManager.showArrowUp();
	pnex_ws_send(data.c_str());
        displayManager.hideArrowUp();
        displayManager.showValue("Moisture", float(soilMoisturePercentage), "%", 0, 30);
    } else {
        // If the client is not available, try to reconnect
        Serial.println("[WS] WebSocket client disconnected. Reconnecting...");
        connectWebSocket();
    }

    if (pnex_ws_available()) {
        Serial.println("[Sensor] Requesting temperatures");
        DS18B20.requestTemperatures();  // Send the command to get temperatures
        Serial.println("[Sensor] Done");
        // After we got the temperatures, we can print them here.
        // We use the function ByIndex, and as an example get the temperature from the first sensor only.
        float tempC = DS18B20.getTempCByIndex(0);
        // Check if reading was successful
        if (tempC != DEVICE_DISCONNECTED_C) {
            Serial.print("[Sensor] Temperature found: ");
            Serial.println(tempC);
            // Send a message every 200 milliseconds
            char tempCStr[8];
            dtostrf(tempC, 4, 2, tempCStr);
            String data = "soil_temperature=" + String(tempCStr);
            displayManager.showArrowUp();
            pnex_ws_send(data.c_str());
            displayManager.hideArrowUp();
            displayManager.showValue("Temp.", tempC, "°C", 0, 40);
        } else {
            Serial.println("[Sensor] Error temperature not found");
        }
    } else {
        // If the client is not available, try to reconnect
        Serial.println("[WS] WebSocket client disconnected. Reconnecting...");
        connectWebSocket();
    }
    delay(200);
}

void connectWiFi() {
    bool ok = pnex_wifi_connect(20);
    wifi_connected = ok;

    if (ok) {
        displayManager.wifiConnected();
        Serial.printf("[WiFi] IP Address: %s\n", WiFi.localIP().toString().c_str());
        Serial.printf("[WiFi] Signal: %d dBm\n", WiFi.RSSI());
    } else {
        displayManager.wifiDisconnected();
    }
}

void connectWebSocket() {
    int attemptCount = 0;
    const int maxAttempts = 3;

    while (!pnex_ws_available() && attemptCount < maxAttempts) {
        attemptCount++;
        Serial.printf("[WS] Connection attempt %d/%d\n", attemptCount, maxAttempts);
        Serial.print("[WS] Free heap before: ");
        Serial.println(ESP.getFreeHeap());

        // Fermeture préalable incluse dans pnex_ws_connect (anti-fuite).
        Serial.println("[WS] Connecting...");
        Serial.print("[WS] Connection string: ");
        Serial.println(pnex_conn_string());

        bool connected = pnex_ws_connect();
        Serial.print("[WS] Connection attempt result: ");
        Serial.println(connected ? "SUCCESS" : "FAILED");

        Serial.print("[WS] Free heap after: ");
        Serial.println(ESP.getFreeHeap());

        // Wait a bit for connection to establish
        delay(2000);

        if (pnex_ws_available()) {
            Serial.println("[WS] Connected");
            websocket_connected = true;
            displayManager.webSocketConnected();
            lastPing = millis();
            return;
        } else {
            Serial.println("[WS] Connection failed. Checking WiFi status...");
            Serial.print("[WS] WiFi status: ");
            Serial.println(WiFi.status());

            websocket_connected = false;
            displayManager.webSocketDisconnected();

            // Force close and cleanup
            pnex_ws_close();
            delay(1000);

            // Check if WiFi is still connected
            if (WiFi.status() != WL_CONNECTED) {
                Serial.println("[WS] WiFi disconnected, reconnecting...");
                connectWiFi();
            }

            if (attemptCount < maxAttempts) {
                Serial.println("[WS] Waiting before retry...");
                delay(5000);
            }
        }
    }

    if (attemptCount >= maxAttempts) {
        Serial.println("[WS] Max connection attempts reached.");
        websocket_connected = false;
    }
}

void feedWatchdog() {
    // Reset the watchdog timer
    ESP.wdtFeed();
}

void sendPing() {
    Serial.println("[PROTO] >> PING");
    pnex_ws_send_ping();
    displayManager.showArrowUp();
    delay(50);
    displayManager.hideArrowUp();
}

int readSoilMoisturePercentage(int analogPin) {
	int rawValue = analogRead(analogPin);
	int percentage = map(rawValue, 0, 1023, 100, 0);
	return percentage;
}
