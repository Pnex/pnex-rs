/**
 * ESP8266 4-Channel Relay Actuator with Protocol Buffers
 *
 * Implements intelligent autonomous control using:
 * - Protocol Buffers (Nanopb) for efficient binary communication
 * - Formal state machine with hysteresis and threshold logic
 * - 10-second timeout protection with configurable safe mode
 * - OLED display for status visualization
 *
 * Architecture: Intelligent actuator with autonomous decision-making
 * - ESP8266: Receives sensor data, makes decisions, executes actions
 * - State Machine: Threshold comparison, hysteresis, safe mode
 * - WebSocket: Binary Protobuf messages for config and sensor data
 * - Display: Real-time status visualization
 */

#include <base64.hpp>
#include <ESP8266WiFi.h>
#include <ArduinoWebsockets.h>
#include <U8g2lib.h>
#include <pb_encode.h>
#include <pb_decode.h>

#include "DisplayManager.h"
#include "config.h"
#include "actuator_config.h"
#include "proto/actuator_message.pb.h"
#include "state_machine.h"

using namespace websockets;

// ============================================
// Global Objects
// ============================================
WebsocketsClient client;
DisplayManager displayManager;
ActuatorStateMachine stateMachine;

// ============================================
// Connection Management
// ============================================
bool wifi_connected = false;
bool websocket_connected = false;
bool should_reconnect = true;
bool initial_pong_received = false;      // Track first PONG for loading completion

unsigned long last_reconnect_attempt = 0;
unsigned long reconnect_delay = 1000;  // Start with 1 second
const unsigned long MAX_RECONNECT_DELAY = 60000;  // Max 1 minute

// ============================================
// Timing
// ============================================
unsigned long last_state_report = 0;
unsigned long last_display_update = 0;
unsigned long last_wifi_check = 0;
unsigned long last_ping_time = 0;
unsigned long last_pong_time = 0;        // Track when last PONG received
bool waiting_for_pong = false;           // Flag to track if expecting PONG

// WiFi credentials décodés du base64 au setup() (un SSID avec espaces
// casserait le define -D de platformio.ini) — bornés par la validation API.
char ssid[101];
char password[101];

// WebSocket connection string
char websockets_connection_string[256];
const char* wss_format = "%s://%s/ws/actuator/cast?token=%s&device_id=%s";

// ============================================
// Function Declarations
// ============================================
void setupWiFi();
void connectWiFi();
void setupWebSocket();
void connectWebSocket();
void onMessageCallback(WebsocketsMessage message);
void onEventsCallback(WebsocketsEvent event, String data);
void handleProtobufMessage(uint8_t* buffer, size_t length);
void sendStateReport();
void updateDisplay();
void feedWatchdog();

// ============================================
// Setup
// ============================================
void setup() {
    Serial.begin(115200);
    delay(1000);

    // Decode WiFi credentials (base64 — cf. config.h) avant setupWiFi()
    unsigned int decodedLength = decode_base64((const unsigned char*)WIFI_SSID, (unsigned char*)ssid);
    ssid[decodedLength] = '\0';
    decodedLength = decode_base64((const unsigned char*)WIFI_PASSWORD, (unsigned char*)password);
    password[decodedLength] = '\0';
    delay(1000);

    Serial.println("\n\n");
    Serial.println("===============================================");
    Serial.println("  ESP8266 4-Channel Relay Actuator v2.0");
    Serial.println("  Protocol: WebSocket + Protocol Buffers");
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

    // Initialize state machine
    stateMachine.begin();

    // Animate to 10% - WiFi Setup
    currentProgress = 10;
    displayManager.showLoadingProgressAnimated("pnex.io", 0, currentProgress, "WiFi Setup...", 300);
    setupWiFi();

    // Animate to 20% - WiFi Connecting
    int nextProgress = 20;
    displayManager.showLoadingProgressAnimated("pnex.io", currentProgress, nextProgress, "WiFi Connecting...", 200);
    currentProgress = nextProgress;
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

    // Decode host from base64
    unsigned char decodedHost[64];
    decodedLength = decode_base64((const unsigned char*)host, decodedHost);
    decodedHost[decodedLength] = '\0';

    // Decode device_id from base64
    unsigned char decodedID[64];
    decodedLength = decode_base64((const unsigned char*)device_id, decodedID);
    decodedID[decodedLength] = '\0';

    // Build WebSocket connection string (schéma selon WS_SSL)
    sprintf(websockets_connection_string, wss_format, ws_use_tls() ? "wss" : "ws", decodedHost, token, device_id);
    Serial.print("[WS] Connection string: ");
    Serial.println(websockets_connection_string);

    // Setup WebSocket: Animate to 60%
    nextProgress = 60;
    displayManager.showLoadingProgressAnimated("pnex.io", currentProgress, nextProgress, "WS Setup...", 300);
    currentProgress = nextProgress;
    setupWebSocket();

    // Connect to WebSocket: Animate to 70%
    nextProgress = 70;
    displayManager.showLoadingProgressAnimated("pnex.io", currentProgress, nextProgress, "WS Connecting...", 300);
    currentProgress = nextProgress;
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

    // Initialize watchdog timer (4 second timeout)
    ESP.wdtDisable();
    ESP.wdtEnable(WDTO_4S);

    // Wait for first PING/PONG to confirm active connection: Animate to 85%
    nextProgress = 85;
    displayManager.showLoadingProgressAnimated("pnex.io", currentProgress, nextProgress, "Waiting PING...", 200);
    currentProgress = nextProgress;

    // Send initial PING
    if (websocket_connected) {
        last_ping_time = millis();
        waiting_for_pong = true;
        client.send("PING");
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

    // Display device ID
    displayManager.showDeviceID((char*)decodedID);

    // Force initial display update to establish clean state
    updateDisplay();

    Serial.println("\n[Setup] Complete - Entering main loop\n");
}

// ============================================
// Main Loop
// ============================================
void loop() {
    feedWatchdog();

    unsigned long now = millis();

    // Check WiFi connection periodically
    if (now - last_wifi_check > WIFI_RECONNECT_INTERVAL) {
        last_wifi_check = now;
        if (WiFi.status() != WL_CONNECTED) {
            if (wifi_connected) {
                Serial.println("[WiFi] Connection lost, reconnecting...");
                wifi_connected = false;
                displayManager.wifiDisconnected();

                // Force all relays OFF for safety when WiFi disconnects
                stateMachine.forceAllOff();
            }
            connectWiFi();
        } else if (!wifi_connected) {
            wifi_connected = true;
            displayManager.wifiConnected();
        }
    }

    // WebSocket loop (poll for messages)
    if (client.available()) {
        client.poll();

        // Connection is healthy - reset backoff
        reconnect_delay = 1000;

    } else {
        // Connection lost - attempt reconnection
        if (websocket_connected) {
            // Connection just lost - force all relays OFF for safety
            Serial.println("[WS] Connection lost - forcing all relays OFF");
            displayManager.webSocketDisconnected();
            stateMachine.forceAllOff();
            websocket_connected = false;
        }

        if (should_reconnect && (now - last_reconnect_attempt > reconnect_delay)) {
            Serial.println("[WS] Reconnecting...");
            last_reconnect_attempt = now;
            connectWebSocket();

            // Increase backoff delay (exponential)
            reconnect_delay = min(reconnect_delay * 2, MAX_RECONNECT_DELAY);
        }
    }

    // Send PING every 5 seconds (CRITICAL for device active status)
    if (websocket_connected && (now - last_ping_time > PING_INTERVAL)) {
        // Check if previous PONG timed out
        if (waiting_for_pong && (now - last_ping_time > PONG_TIMEOUT)) {
            Serial.println("[Ping] ✗ PONG TIMEOUT - Connection dead, reconnecting...");
            websocket_connected = false;
            displayManager.webSocketDisconnected();

            // Force all relays OFF for safety
            stateMachine.forceAllOff();

            client.close();
            should_reconnect = true;
            waiting_for_pong = false;
        } else {
            // Send new PING
            last_ping_time = now;
            waiting_for_pong = true;
            client.send("PING");
            #if DEBUG_SERIAL
            Serial.println("[Ping] -> PING sent");
            #endif
        }
    }

    // Process state machine
    stateMachine.process();

    // Send state report periodically (if connected)
    if (websocket_connected && (now - last_state_report > STATE_REPORT_INTERVAL)) {
        last_state_report = now;
        sendStateReport();
    }

    // Update display periodically
    if (now - last_display_update > DISPLAY_UPDATE_INTERVAL) {
        last_display_update = now;
        updateDisplay();
    }

    // Small delay to prevent busy loop
    delay(10);
}

// ============================================
// WiFi Setup
// ============================================
void setupWiFi() {
    WiFi.mode(WIFI_STA);
}

void connectWiFi() {
    Serial.printf("[WiFi] Connecting to %s", ssid);

    WiFi.begin(ssid, password);

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

// ============================================
// WebSocket Setup
// ============================================
void setupWebSocket() {
    // Configure WebSocket client (TLS sans vérification de chaîne, uniquement en wss)
    if (ws_use_tls()) {
        client.setInsecure();
    }
    client.onMessage(onMessageCallback);
    client.onEvent(onEventsCallback);
}

void connectWebSocket() {
    client.connect(websockets_connection_string);

    if (client.available()) {
        Serial.println("[WS] Connected!");
        websocket_connected = true;
        displayManager.webSocketConnected();
        should_reconnect = false;
        reconnect_delay = 1000;

        // Reset PONG tracking
        waiting_for_pong = false;
        last_pong_time = millis();

        // Send initial state report
        sendStateReport();
    } else {
        Serial.println("[WS] Connection failed");
        websocket_connected = false;
        displayManager.webSocketDisconnected();
        should_reconnect = true;
    }
}

// ============================================
// WebSocket Event Handlers
// ============================================
void onMessageCallback(WebsocketsMessage message) {
    displayManager.showArrowDown();

    if (message.isBinary()) {
        #if DEBUG_SERIAL
        Serial.printf("[WS] << Binary message (%d bytes)\n", message.length());
        #endif
        handleProtobufMessage((uint8_t*)message.c_str(), message.length());
    } else {
        String msg = message.data();
        msg.trim();

        // Handle PONG response from server
        if (msg == "PONG") {
            last_pong_time = millis();      // Update last PONG time
            waiting_for_pong = false;        // Reset flag
            initial_pong_received = true;    // Mark first PONG received
            #if DEBUG_SERIAL
            unsigned long rtt = millis() - last_ping_time;
            Serial.printf("[Ping] <- PONG received (RTT: %lu ms)\n", rtt);
            #endif
        } else {
            Serial.print("[WS] << Text: ");
            Serial.println(msg);
        }
    }

    delay(50);  // Brief delay to ensure arrow is visible
    displayManager.hideArrowDown();
}

void onEventsCallback(WebsocketsEvent event, String data) {
    if (event == WebsocketsEvent::ConnectionOpened) {
        Serial.println("[WS] Connection opened");
        websocket_connected = true;
        displayManager.webSocketConnected();
        should_reconnect = false;
        reconnect_delay = 1000;

        // Reset PONG tracking
        waiting_for_pong = false;
        last_pong_time = millis();

    } else if (event == WebsocketsEvent::ConnectionClosed) {
        Serial.println("[WS] Connection closed - forcing all relays OFF");
        websocket_connected = false;
        displayManager.webSocketDisconnected();
        stateMachine.forceAllOff();
        should_reconnect = true;

    } else if (event == WebsocketsEvent::GotPing) {
        Serial.println("[WS] Got Ping");
    } else if (event == WebsocketsEvent::GotPong) {
        Serial.println("[WS] Got Pong");
    }
}

// ============================================
// Protocol Buffer Message Handler
// ============================================
void handleProtobufMessage(uint8_t* buffer, size_t length) {
    // Try to decode as CONFIG first
    ActuatorConfig config = ActuatorConfig_init_zero;
    pb_istream_t config_stream = pb_istream_from_buffer(buffer, length);

    bool config_success = pb_decode(&config_stream, ActuatorConfig_fields, &config);
    if (config_success) {
        Serial.println("[Protobuf] Decoded CONFIG message");
        stateMachine.handleConfig(config);
        return;
    }

    // Log CONFIG decode error
    Serial.printf("[Protobuf] CONFIG decode failed: %s\n", PB_GET_ERROR(&config_stream));

    // Try to decode as SENSOR_DATA
    SensorData sensor_data = SensorData_init_zero;
    pb_istream_t sensor_stream = pb_istream_from_buffer(buffer, length);

    bool sensor_success = pb_decode(&sensor_stream, SensorData_fields, &sensor_data);
    if (sensor_success) {
        #if DEBUG_SERIAL
        Serial.println("[Protobuf] Decoded SENSOR_DATA message");
        #endif
        stateMachine.handleSensorData(sensor_data);
        return;
    }

    // Log SENSOR_DATA decode error
    Serial.printf("[Protobuf] SENSOR_DATA decode failed: %s\n", PB_GET_ERROR(&sensor_stream));
    Serial.println("[Protobuf] Failed to decode message");
}

// ============================================
// Send State Report
// ============================================
void sendStateReport() {
    if (!websocket_connected) {
        return;
    }

    // Build state message
    ActuatorState state = ActuatorState_init_zero;
    stateMachine.buildStateMessage(state);

    // Encode to buffer
    uint8_t buffer[512];
    pb_ostream_t stream = pb_ostream_from_buffer(buffer, sizeof(buffer));
    bool status = pb_encode(&stream, ActuatorState_fields, &state);

    if (status) {
        // Send via WebSocket
        displayManager.showArrowUp();
        client.sendBinary((const char*)buffer, stream.bytes_written);
        delay(50);
        displayManager.hideArrowUp();

        #if DEBUG_SERIAL
        Serial.printf("[State] Sent report (%d bytes)\n", stream.bytes_written);
        #endif
    } else {
        Serial.println("[State] Failed to encode state report");
    }
}

// ============================================
// Update Display
// ============================================
void updateDisplay() {
    // Get channel states and hysteresis states
    bool relay_states[4];
    bool hysteresis_active[4];
    for (uint8_t i = 0; i < 4; i++) {
        relay_states[i] = stateMachine.getChannelPhysicalState(i);
        hysteresis_active[i] = stateMachine.getChannelHysteresisActive(i);
    }

    #if DEBUG_STATE_MACHINE
    // Debug output for hourglass display
    for (uint8_t i = 0; i < 4; i++) {
        if (hysteresis_active[i]) {
            Serial.printf("[Display] CH%u: Showing hourglass (relay=%s)\n",
                i + 1, relay_states[i] ? "ON" : "OFF");
        }
    }
    #endif

    displayManager.show4RelayStatusWithHysteresis(
        relay_states[0],
        relay_states[1],
        relay_states[2],
        relay_states[3],
        hysteresis_active[0],
        hysteresis_active[1],
        hysteresis_active[2],
        hysteresis_active[3]
    );
}

// ============================================
// Watchdog
// ============================================
void feedWatchdog() {
    ESP.wdtFeed();
}
