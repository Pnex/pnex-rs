// src/actuator_config.h
// Actuator-specific configuration for 4-channel relay
#ifndef ACTUATOR_CONFIG_H
#define ACTUATOR_CONFIG_H

#include <Arduino.h>

// ============================================
// GPIO Pin Configuration (ESP8266 NodeMCU)
// ============================================
// Keep existing pin assignments for compatibility
#define RELAY_PIN_1 D1   // GPIO5 - Channel 1
#define RELAY_PIN_2 D2   // GPIO4 - Channel 2
#define RELAY_PIN_3 D4   // GPIO2 - Channel 3
#define RELAY_PIN_4 D7   // GPIO13 - Channel 4

const uint8_t RELAY_PINS[4] = {
    RELAY_PIN_1,
    RELAY_PIN_2,
    RELAY_PIN_3,
    RELAY_PIN_4
};

// ============================================
// Relay Configuration
// ============================================
// Most relay modules are Active LOW (LOW = ON, HIGH = OFF)
// Set to true if your relay is Active LOW, false if Active HIGH
#define RELAY_ACTIVE_LOW true

// Helper macros for relay control
#if RELAY_ACTIVE_LOW
    #define RELAY_ON LOW
    #define RELAY_OFF HIGH
#else
    #define RELAY_ON HIGH
    #define RELAY_OFF LOW
#endif

// ============================================
// Timing Configuration
// ============================================
#define DEFAULT_TIMEOUT_SECONDS 10           // Safe mode timeout (10 seconds as per guide)
#define WEBSOCKET_RECONNECT_INTERVAL 5000    // 5 seconds
#define STATE_REPORT_INTERVAL 5000           // 5 seconds
#define WIFI_RECONNECT_INTERVAL 10000        // 10 seconds
#define PING_INTERVAL 5000                   // 5 seconds (CRITICAL for device active status)
#define PONG_TIMEOUT 15000                   // 15 seconds - if no PONG, connection is dead
#define CONFIG_REQUEST_INTERVAL 300000       // 5 minutes

// ============================================
// Display Configuration (OLED)
// ============================================
#define USE_DISPLAY true                     // Enable OLED display support
#define DISPLAY_UPDATE_INTERVAL 1000         // Update display every 1 second

// ============================================
// Debug Configuration
// ============================================
#define DEBUG_SERIAL true                    // Enable serial debug output
#define DEBUG_PROTOBUF true                  // Enable detailed protobuf debug
#define DEBUG_STATE_MACHINE true             // Enable state machine debug output

// ============================================
// WebSocket Configuration
// ============================================
#define SERVER_PORT 443                      // WSS port
#define USE_SSL true                         // Use secure WebSocket
#define WS_PATH_PREFIX "/ws/actuator/"       // WebSocket path prefix

// ============================================
// State Machine Configuration
// ============================================
#define DEFAULT_HYSTERESIS_SECONDS 60        // Default hysteresis period (1 minute)
#define MIN_HYSTERESIS_SECONDS 5             // Minimum hysteresis period (5 seconds) - enforced minimum
#define MAX_SENSOR_CACHE_AGE 60000           // Max age for cached sensor data (60 seconds)

#endif // ACTUATOR_CONFIG_H
