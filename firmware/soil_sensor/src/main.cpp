#include <ESP8266WiFi.h>
#include <ArduinoWebsockets.h>
#include <OneWire.h>
#include <DallasTemperature.h>
#include <U8g2lib.h>
#include "DisplayManager.h"
#include "chacha_crypto.h"
#include "config.h"

// Soil moisture sensor
#define SOIL_MOISTURE_PIN A0
// OneWire temperature sensor
#define ONE_WIRE_BUS D6

using namespace websockets;
WebsocketsClient client;
DisplayManager displayManager;

// temperature sensor
OneWire oneWire(ONE_WIRE_BUS);
DallasTemperature DS18B20(&oneWire);

bool shouldReconnect = true;  // Flag to control reconnection attempts
unsigned long lastReconnectAttempt = 0;
const unsigned long reconnectInterval = 5000;  // Reconnect interval in milliseconds

// WiFi credentials décodés du base64 au setup() (espaces/quotes d'un SSID
// littéral casseraient le define -D de platformio.ini) — bornés par la
// validation API (100 caractères).
char ssid[101];
char password[101];

// Ping timing
unsigned long lastPing = 0;
const unsigned long PING_INTERVAL = 5000;  // 5 seconds

char websockets_connection_string[256];
const char* wss_format = "%s://%s/ws/sensor/ingest?token=%s&device_id=%s";

// Connection tracking for loading animation
bool wifi_connected = false;
bool websocket_connected = false;
bool initial_pong_received = false;      // Track first PONG for loading completion
unsigned long last_pong_time = 0;
bool waiting_for_pong = false;

// Function prototypes
void connectWiFi();
void connectWebSocket();
void feedWatchdog();
void onMessageCallback(WebsocketsMessage message);
void onEventsCallback(WebsocketsEvent event, String data);
void sendPing();
int readSoilMoisturePercentage(int analogPin);

void setup() {
    Serial.begin(115200);
    delay(1000);

    // Decode WiFi credentials (base64 — cf. config.h ; via le wrap du
    // module crypto, seul endroît où le header densaugeo est inclus)
    unsigned int decodedLength = cryptoB64Decode(WIFI_SSID, (unsigned char*)ssid);
    ssid[decodedLength] = '\0';
    decodedLength = cryptoB64Decode(WIFI_PASSWORD, (unsigned char*)password);
    password[decodedLength] = '\0';

    // Clé de chiffrement des frames WS (base64 → 32 o, -D ENCRYPTION_KEY).
    // Absente → frames EN CLAIR, réservé au mock local ws-server/ : le
    // serveur réel répondrait ERROR:decryption_failed à tout et le device
    // ne passerait jamais « actif ».
    if (cryptoSetKey(encryption_key)) {
        Serial.println("[Crypto] ChaCha20 active (ENCRYPTION_KEY chargee)");
    } else {
        Serial.println("[Crypto] ENCRYPTION_KEY absente/invalide — frames EN CLAIR (mock local uniquement)");
    }

    Serial.println("\n\n");
    Serial.println("===============================================");
    Serial.println("  ESP8266 Soil Sensor v2.0");
    Serial.println("  Protocol: WebSocket + Text/JSON");
    Serial.println("===============================================");
    Serial.printf("Device ID: %s\n", device_id);
    Serial.printf("Server: %s\n", host);
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

    // decode host
    unsigned char decodedHost[64];  // Ensure this is large enough to hold the decoded string
    // decode_base64 does not place a null terminator, because the output is not always a string
    decodedLength = cryptoB64Decode(host, decodedHost);
    decodedHost[decodedLength] = '\0';  // Null-terminate the string

    // decode device_id
    unsigned char decodedID[64];  // Ensure this is large enough to hold the decoded string
    decodedLength = cryptoB64Decode(device_id, decodedID);
    decodedID[decodedLength] = '\0';  // Null-terminate the string

    // Build the simplified connection string using sprintf (schéma selon WS_SSL)
    sprintf(websockets_connection_string, wss_format, ws_use_tls() ? "wss" : "ws", decodedHost, token, device_id);
    Serial.print("[WS] Connection string: ");
    Serial.println(websockets_connection_string);

    // Setup WebSocket: Animate to 60%
    nextProgress = 60;
    displayManager.showLoadingProgressAnimated("pnex.io", currentProgress, nextProgress, "WS Setup...", 300);
    currentProgress = nextProgress;

    // Use TLS for WebSocket, but do not verify the chain (uniquement en
    // wss — en ws local le client reste en TCP simple)
    if (ws_use_tls()) {
        client.setInsecure();
    }
    // Run callback when messages are received
    client.onMessage(onMessageCallback);
    // Run callback when events are occurring
    client.onEvent(onEventsCallback);

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
        waiting_for_pong = true;
        client.send(cryptoEncryptFrame("PING"));
        Serial.println("[Setup] Initial PING sent");

        // Wait for PONG (max 5 seconds)
        unsigned long pong_wait_start = millis();
        while (!initial_pong_received && (millis() - pong_wait_start < 5000)) {
            if (client.available()) {
                client.poll();
            }
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
    displayManager.showDeviceID((char*)decodedID);

    Serial.println("\n[Setup] Complete - Entering main loop\n");
}

void loop() {
    feedWatchdog();

    unsigned long now = millis();

    // Poll the WebSocket client
    if (client.available()) {
        client.poll();

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
    if (shouldReconnect && !client.available() && millis() - lastReconnectAttempt > reconnectInterval) {
        Serial.println("Attempting to reconnect...");
        lastReconnectAttempt = millis();
        connectWebSocket();
	}

    if (client.available()){
	Serial.println("[Sensor] Reading Moisture");
	int soilMoisturePercentage = readSoilMoisturePercentage(SOIL_MOISTURE_PIN);
	Serial.print("[Sensor] Moisture: ");
	Serial.println(soilMoisturePercentage);
	String data = "soil_moisture=" + String(soilMoisturePercentage);
        displayManager.showArrowUp();
	client.send(cryptoEncryptFrame(data.c_str()));
        displayManager.hideArrowUp();
        displayManager.showValue("Moisture", float(soilMoisturePercentage), "%", 0, 30);
    } else {
        // If the client is not available, try to reconnect
        Serial.println("[WS] WebSocket client disconnected. Reconnecting...");
        connectWebSocket();
    }

    if (client.available()) {
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
            client.send(cryptoEncryptFrame(data.c_str()));
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
    Serial.printf("[WiFi] Connecting to %s", ssid);
    WiFi.begin(ssid, password);

    // Wait some time to connect to WiFi
    int attempts = 0;
    while (WiFi.status() != WL_CONNECTED && attempts < 20) {
        delay(500);
        Serial.print(".");
        attempts++;
    }

    if (WiFi.status() == WL_CONNECTED) {
        wifi_connected = true;
        displayManager.wifiConnected();
        Serial.println(" Connected!");
        Serial.printf("[WiFi] IP Address: %s\n", WiFi.localIP().toString().c_str());
        Serial.printf("[WiFi] Signal: %d dBm\n", WiFi.RSSI());
    } else {
        wifi_connected = false;
        displayManager.wifiDisconnected();
        Serial.println(" Failed!");
    }
}

void connectWebSocket() {
    int attemptCount = 0;
    const int maxAttempts = 3;

    while (!client.available() && attemptCount < maxAttempts) {
        attemptCount++;
        Serial.printf("[WS] Connection attempt %d/%d\n", attemptCount, maxAttempts);
        Serial.print("[WS] Free heap before: ");
        Serial.println(ESP.getFreeHeap());

        // Close any existing connection first to prevent memory leaks
        if (client.available()) {
            client.close();
            delay(100);
        }

        Serial.println("[WS] Connecting...");
        Serial.print("[WS] Connection string: ");
        Serial.println(websockets_connection_string);

        bool connected = client.connect(websockets_connection_string);
        Serial.print("[WS] Connection attempt result: ");
        Serial.println(connected ? "SUCCESS" : "FAILED");

        Serial.print("[WS] Free heap after: ");
        Serial.println(ESP.getFreeHeap());

        // Wait a bit for connection to establish
        delay(2000);

        if (client.available()) {
            Serial.println("[WS] Connected");
            websocket_connected = true;
            displayManager.webSocketConnected();
            lastPing = millis();

            // Reset PONG tracking
            waiting_for_pong = false;
            last_pong_time = millis();
            return;
        } else {
            Serial.println("[WS] Connection failed. Checking WiFi status...");
            Serial.print("[WS] WiFi status: ");
            Serial.println(WiFi.status());

            websocket_connected = false;
            displayManager.webSocketDisconnected();

            // Force close and cleanup
            client.close();
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

void onMessageCallback(WebsocketsMessage message) {
    displayManager.showArrowDown();

    // Frame serveur chiffrée base64(nonce‖ct) → plaintext ; sans clé
    // chargée (mock local), la passe-passe est transparente.
    String msg = cryptoDecryptFrame(message.data().c_str());
    msg.trim();

    // Handle PONG response from server
    if (msg == "PONG") {
        last_pong_time = millis();           // Update last PONG time
        waiting_for_pong = false;            // Reset flag
        initial_pong_received = true;        // Mark first PONG received
        Serial.println("[Ping] <- PONG received");
    } else if (msg.length() == 0) {
        Serial.println("[WS] Got Message: <frame illisible>");
    } else {
        Serial.print("[WS] Got Message: ");
        Serial.println(msg);
    }

    delay(50);  // Brief delay to ensure arrow is visible
    displayManager.hideArrowDown();
}

void onEventsCallback(WebsocketsEvent event, String data) {
    displayManager.showArrowDown();
    if (event == WebsocketsEvent::ConnectionOpened) {
        Serial.println("[WS] Connection Opened");
        websocket_connected = true;
        displayManager.webSocketConnected();
        shouldReconnect = false;  // Disable reconnection attempts

        // Reset PONG tracking
        waiting_for_pong = false;
        last_pong_time = millis();

    } else if (event == WebsocketsEvent::ConnectionClosed) {
        Serial.println("[WS] Connection Closed");
        websocket_connected = false;
        displayManager.webSocketDisconnected();
        shouldReconnect = true;  // Enable reconnection attempts
    } else if (event == WebsocketsEvent::GotPing) {
        Serial.println("[WS] Got Ping");
    } else if (event == WebsocketsEvent::GotPong) {
        Serial.println("[WS] Got Pong");
    }
    displayManager.hideArrowDown();
}

void sendPing() {
    Serial.println("[PROTO] >> PING");
    client.send(cryptoEncryptFrame("PING"));
    displayManager.showArrowUp();
    delay(50);
    displayManager.hideArrowUp();
}

int readSoilMoisturePercentage(int analogPin) {
	int rawValue = analogRead(analogPin);
	int percentage = map(rawValue, 0, 1023, 100, 0);
	return percentage;
}
