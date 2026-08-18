// src/state_machine.cpp
#include "state_machine.h"
#include "actuator_config.h"

// ============================================
// Constructor
// ============================================
ActuatorStateMachine::ActuatorStateMachine() :
    data_timeout_seconds(DEFAULT_TIMEOUT_SECONDS),
    last_sensor_data_time(0),
    in_timeout_mode(false)
{
}

// ============================================
// Initialization
// ============================================
void ActuatorStateMachine::begin() {
    // Initialize GPIO pins
    for (uint8_t i = 0; i < 4; i++) {
        pinMode(RELAY_PINS[i], OUTPUT);
        digitalWrite(RELAY_PINS[i], RELAY_OFF);
        channels[i].is_on = false;
    }

    #if DEBUG_SERIAL
    Serial.println(F("[StateMachine] Initialized"));
    Serial.printf("[StateMachine] Relay mode: %s\n",
        RELAY_ACTIVE_LOW ? "Active LOW" : "Active HIGH");
    #endif
}

// ============================================
// Handle CONFIG Message
// ============================================
void ActuatorStateMachine::handleConfig(const ActuatorConfig& config) {
    #if DEBUG_SERIAL
    Serial.println(F("[StateMachine] Received CONFIG"));
    Serial.printf("  Device ID: %s\n", config.device_id);
    Serial.printf("  Timeout: %u seconds\n", config.data_timeout_seconds);
    Serial.printf("  Channels: %u\n", config.channels_count);
    #endif

    // Update timeout configuration
    if (config.data_timeout_seconds > 0) {
        data_timeout_seconds = config.data_timeout_seconds;
    }

    // Update channel configurations
    for (size_t i = 0; i < config.channels_count && i < 4; i++) {
        const ChannelConfig& ch_config = config.channels[i];
        if (ch_config.number >= 1 && ch_config.number <= 4) {
            uint8_t channel_idx = ch_config.number - 1;  // Convert 1-4 to 0-3
            updateConfig(ch_config, channel_idx);
        }
    }
}

void ActuatorStateMachine::updateConfig(const ChannelConfig& config, uint8_t channel_idx) {
    if (channel_idx >= 4) return;

    ChannelRuntimeState& ch = channels[channel_idx];

    ch.enabled = config.enabled;
    ch.mode = config.mode;
    ch.safe_mode = config.safe_mode;

    if (config.mode == ChannelMode_MODE_BINARY) {
        ch.threshold = config.threshold;
        ch.comparison = config.comparison;
        ch.invert_logic = config.invert_logic;
        // Enforce minimum 5-second hysteresis
        ch.hysteresis_seconds = max(config.hysteresis_seconds, (uint32_t)MIN_HYSTERESIS_SECONDS);
        ch.hysteresis_value = config.hysteresis_value;
    }

    #if DEBUG_SERIAL || DEBUG_STATE_MACHINE
    Serial.printf("[StateMachine] Channel %u configured:\n", channel_idx + 1);
    Serial.printf("  Enabled: %d\n", ch.enabled);
    Serial.printf("  Mode: %d\n", ch.mode);
    Serial.printf("  Threshold: %.2f\n", ch.threshold);
    Serial.printf("  Comparison: %s\n",
        ch.comparison == Comparison_LESS_THAN ? "LT" : "GT");
    Serial.printf("  Invert logic: %s\n", ch.invert_logic ? "YES (cooling)" : "NO (heating)");
    Serial.printf("  Hysteresis: %u sec, %.2f value\n", ch.hysteresis_seconds, ch.hysteresis_value);
    Serial.printf("  Safe mode: %d\n", ch.safe_mode);
    #endif
}

// ============================================
// Handle SENSOR_DATA Message - SIMPLIFIED!
// ============================================
void ActuatorStateMachine::handleSensorData(const SensorData& data) {
    // Reset timeout timer - we received data!
    last_sensor_data_time = millis();

    // Exit timeout mode if we were in it
    if (in_timeout_mode) {
        #if DEBUG_SERIAL
        Serial.println(F("[StateMachine] Sensor data received, exiting timeout mode"));
        #endif
        in_timeout_mode = false;
    }

    // Process channel data directly - no cache!
    for (size_t i = 0; i < data.channel_data_count; i++) {
        const ChannelData& ch_data = data.channel_data[i];

        // Convert channel number (1-4) to index (0-3)
        if (ch_data.channel >= 1 && ch_data.channel <= 4) {
            uint8_t channel_idx = ch_data.channel - 1;
            processChannelData(channel_idx, ch_data.value, ch_data.fresh);

            #if DEBUG_STATE_MACHINE
            Serial.printf("  [Channel %u] value: %.2f (fresh: %d)\n",
                ch_data.channel, ch_data.value, ch_data.fresh);
            #endif
        }
    }

    #if DEBUG_SERIAL || DEBUG_STATE_MACHINE
    Serial.printf("[StateMachine] Processed %u channel data\n", data.channel_data_count);
    #endif
}

void ActuatorStateMachine::processChannelData(uint8_t channel_idx, float value, bool fresh) {
    if (channel_idx >= 4) return;

    ChannelRuntimeState& ch = channels[channel_idx];

    // Store the sensor value
    ch.last_sensor_value = value;

    // Skip disabled channels
    if (!ch.enabled) {
        if (ch.is_on) {
            setChannelOff(channel_idx);
            ch.last_reason = StateReason_REASON_DISABLED;
        }
        return;
    }

    // If data is stale, apply safe mode
    if (!fresh) {
        applySafeMode(channel_idx);
        ch.last_reason = StateReason_REASON_STALE_SENSOR_DATA;
        return;
    }

    // Process based on mode
    if (ch.mode == ChannelMode_MODE_BINARY) {
        processBinaryChannel(channel_idx, value);
    }
    // PWM and Follow modes not implemented for binary actuator
}

// ============================================
// Main Processing Loop
// ============================================
void ActuatorStateMachine::process() {
    // Check for timeout
    checkTimeout();

    // Note: Channel processing now happens in handleSensorData()
    // This function only checks timeout and applies safe mode if needed
}

// ============================================
// Binary Mode Logic
// ============================================
void ActuatorStateMachine::processBinaryChannel(uint8_t channel_idx, float sensor_value) {
    ChannelRuntimeState& ch = channels[channel_idx];

    #if DEBUG_STATE_MACHINE
    Serial.printf("[StateMachine] processBinaryChannel CH%u: sensor=%.2f, threshold=%.2f, comparison=%s, current_state=%s\n",
        channel_idx + 1, sensor_value, ch.threshold,
        ch.comparison == Comparison_GREATER_THAN ? "GT" : "LT",
        ch.is_on ? "ON" : "OFF");
    #endif

    // Determine desired state
    bool should_be_on = shouldActivate(channel_idx, sensor_value);

    // Check if hysteresis period has expired and clear flag if needed
    if (ch.hysteresis_active) {
        unsigned long now = millis();
        unsigned long elapsed = now - ch.hysteresis_start_time;
        unsigned long required = ch.hysteresis_seconds * 1000UL;

        if (elapsed >= required) {
            ch.hysteresis_active = false;
            #if DEBUG_STATE_MACHINE
            Serial.printf("[StateMachine] CH%u: Hysteresis period expired, clearing flag\n", channel_idx + 1);
            #endif
        }
    }

    // Check if state needs to change
    if (should_be_on != ch.is_on) {
        // State change is desired

        // Apply hysteresis ONLY when turning ON (OFF → ON transition)
        // Allow immediate OFF transitions to conserve resources
        if (should_be_on && !ch.is_on) {
            // OFF → ON: Check hysteresis
            if (!checkHysteresis(channel_idx)) {
                // Hysteresis is blocking the state change
                ch.last_reason = StateReason_REASON_HYSTERESIS_ACTIVE;
                #if DEBUG_STATE_MACHINE
                Serial.printf("[StateMachine] CH%u: OFF→ON blocked by hysteresis\n", channel_idx + 1);
                #endif
                return;
            }
            #if DEBUG_STATE_MACHINE
            Serial.printf("[StateMachine] CH%u: OFF→ON hysteresis passed, turning ON\n", channel_idx + 1);
            #endif
        } else {
            // ON → OFF: Allow immediate transition (no hysteresis)
            #if DEBUG_STATE_MACHINE
            Serial.printf("[StateMachine] CH%u: ON→OFF immediate (no hysteresis applied)\n", channel_idx + 1);
            #endif
        }

        // Apply state change
        ch.is_on = should_be_on;
        ch.last_change_time = millis();

        setChannelState(channel_idx, should_be_on);

        // Start hysteresis period ONLY when turning ON
        // This prevents immediate OFF→ON→OFF oscillation
        if (should_be_on) {
            ch.hysteresis_active = true;
            ch.hysteresis_start_time = millis();
            #if DEBUG_STATE_MACHINE
            Serial.printf("[StateMachine] CH%u: Starting %u second hysteresis timer\n",
                channel_idx + 1, ch.hysteresis_seconds);
            #endif
        }

        // Update reason
        if (ch.comparison == Comparison_LESS_THAN) {
            ch.last_reason = should_be_on ?
                StateReason_REASON_SENSOR_BELOW_THRESHOLD :
                StateReason_REASON_SENSOR_ABOVE_THRESHOLD;
        } else {
            ch.last_reason = should_be_on ?
                StateReason_REASON_SENSOR_ABOVE_THRESHOLD :
                StateReason_REASON_SENSOR_BELOW_THRESHOLD;
        }

        #if DEBUG_SERIAL || DEBUG_STATE_MACHINE
        if (ch.hysteresis_value > 0.0) {
            float on_threshold, off_threshold;

            if (!ch.invert_logic) {
                // Normal logic
                if (ch.comparison == Comparison_GREATER_THAN) {
                    on_threshold = ch.threshold + ch.hysteresis_value;
                    off_threshold = ch.threshold;
                } else {
                    on_threshold = ch.threshold - ch.hysteresis_value;
                    off_threshold = ch.threshold;
                }
            } else {
                // Inverted logic
                if (ch.comparison == Comparison_GREATER_THAN) {
                    on_threshold = ch.threshold;
                    off_threshold = ch.threshold - ch.hysteresis_value;
                } else {
                    on_threshold = ch.threshold;
                    off_threshold = ch.threshold + ch.hysteresis_value;
                }
            }

            Serial.printf("[StateMachine] Channel %u: %s (sensor: %.2f, ON@%.2f, OFF@%.2f) %s\n",
                channel_idx + 1,
                should_be_on ? "ON" : "OFF",
                sensor_value,
                on_threshold,
                off_threshold,
                ch.invert_logic ? "[INVERTED]" : ""
            );
        } else {
            Serial.printf("[StateMachine] Channel %u: %s (sensor: %.2f, threshold: %.2f)\n",
                channel_idx + 1,
                should_be_on ? "ON" : "OFF",
                sensor_value,
                ch.threshold
            );
        }
        #endif
    }
    // Note: hysteresis flag is now cleared by time expiration, not by state stabilization
}

bool ActuatorStateMachine::shouldActivate(uint8_t channel_idx, float sensor_value) {
    const ChannelRuntimeState& ch = channels[channel_idx];

    // If value-based hysteresis is enabled, use deadband logic
    if (ch.hysteresis_value > 0.0) {
        if (!ch.invert_logic) {
            // NORMAL LOGIC (Heating mode)
            if (ch.comparison == Comparison_LESS_THAN) {
                // LT mode with hysteresis (e.g., heater):
                // Turn ON when: sensor < (threshold - hysteresis)
                // Turn OFF when: sensor > threshold
                if (ch.is_on) {
                    // Currently ON, check if should turn OFF
                    return sensor_value <= ch.threshold;
                } else {
                    // Currently OFF, check if should turn ON
                    return sensor_value < (ch.threshold - ch.hysteresis_value);
                }
            } else {
                // GT mode with hysteresis:
                // Turn ON when: sensor > (threshold + hysteresis)
                // Turn OFF when: sensor < threshold
                if (ch.is_on) {
                    // Currently ON, check if should turn OFF
                    return sensor_value >= ch.threshold;
                } else {
                    // Currently OFF, check if should turn ON
                    return sensor_value > (ch.threshold + ch.hysteresis_value);
                }
            }
        } else {
            // INVERTED LOGIC (Cooling mode)
            if (ch.comparison == Comparison_GREATER_THAN) {
                // GT mode with inverted hysteresis (e.g., cooler):
                // Turn ON when: sensor > threshold
                // Turn OFF when: sensor < (threshold - hysteresis)
                if (ch.is_on) {
                    // Currently ON, check if should turn OFF
                    return sensor_value >= (ch.threshold - ch.hysteresis_value);
                } else {
                    // Currently OFF, check if should turn ON
                    return sensor_value > ch.threshold;
                }
            } else {
                // LT mode with inverted hysteresis:
                // Turn ON when: sensor < threshold
                // Turn OFF when: sensor > (threshold + hysteresis)
                if (ch.is_on) {
                    // Currently ON, check if should turn OFF
                    return sensor_value <= (ch.threshold + ch.hysteresis_value);
                } else {
                    // Currently OFF, check if should turn ON
                    return sensor_value < ch.threshold;
                }
            }
        }
    } else {
        // No value-based hysteresis, use simple threshold comparison
        // Invert logic doesn't affect this case (no deadband to invert)
        if (ch.comparison == Comparison_LESS_THAN) {
            return sensor_value < ch.threshold;
        } else {
            return sensor_value > ch.threshold;
        }
    }
}

bool ActuatorStateMachine::checkHysteresis(uint8_t channel_idx) {
    const ChannelRuntimeState& ch = channels[channel_idx];
    unsigned long now = millis();
    unsigned long elapsed = now - ch.last_change_time;
    unsigned long required = ch.hysteresis_seconds * 1000UL;

    #if DEBUG_STATE_MACHINE
    Serial.printf("[StateMachine] CH%u: Hysteresis check: elapsed=%lu ms, required=%lu ms, passes=%d\n",
        channel_idx + 1, elapsed, required, (elapsed >= required));
    #endif

    return elapsed >= required;
}

// ============================================
// Timeout Handling
// ============================================
void ActuatorStateMachine::checkTimeout() {
    unsigned long now = millis();

    // No timeout if we never received data
    if (last_sensor_data_time == 0) {
        return;
    }

    unsigned long elapsed = now - last_sensor_data_time;
    unsigned long timeout_ms = data_timeout_seconds * 1000UL;

    if (elapsed > timeout_ms) {
        if (!in_timeout_mode) {
            #if DEBUG_SERIAL
            Serial.println(F("[StateMachine] TIMEOUT: Entering safe mode"));
            #endif
            in_timeout_mode = true;
            applyTimeoutSafeMode();
        }
    }
}

void ActuatorStateMachine::applyTimeoutSafeMode() {
    for (uint8_t i = 0; i < 4; i++) {
        applySafeMode(i);
        channels[i].last_reason = StateReason_REASON_TIMEOUT;
    }
}

void ActuatorStateMachine::forceAllOff() {
    // Emergency safety: turn all relays OFF regardless of configuration
    // Used when WiFi/WebSocket disconnects to ensure safe state
    #if DEBUG_SERIAL
    Serial.println(F("[StateMachine] EMERGENCY: Forcing all relays OFF"));
    #endif

    for (uint8_t i = 0; i < 4; i++) {
        if (channels[i].is_on) {
            setChannelOff(i);
            channels[i].last_reason = StateReason_REASON_TIMEOUT;
            #if DEBUG_STATE_MACHINE
            Serial.printf("[StateMachine] Channel %u forced OFF\n", i + 1);
            #endif
        }
    }
}

void ActuatorStateMachine::applySafeMode(uint8_t channel_idx) {
    if (channel_idx >= 4) return;

    ChannelRuntimeState& ch = channels[channel_idx];

    switch (ch.safe_mode) {
        case SafeMode_SAFE_OFF:
            if (ch.is_on) {
                setChannelOff(channel_idx);
                ch.last_reason = StateReason_REASON_SAFE_MODE;
            }
            break;
        case SafeMode_SAFE_ON:
            if (!ch.is_on) {
                setChannelOn(channel_idx);
                ch.last_reason = StateReason_REASON_SAFE_MODE;
            }
            break;
        case SafeMode_SAFE_KEEP:
            // Keep current state, do nothing
            break;
    }
}

// ============================================
// GPIO Control
// ============================================
void ActuatorStateMachine::setChannelState(uint8_t channel_idx, bool state) {
    if (channel_idx >= 4) return;

    // Set GPIO based on relay type (active LOW or HIGH)
    digitalWrite(RELAY_PINS[channel_idx], state ? RELAY_ON : RELAY_OFF);
    channels[channel_idx].is_on = state;

    #if DEBUG_STATE_MACHINE
    Serial.printf("[GPIO] Channel %u pin %u set to %s\n",
        channel_idx + 1,
        RELAY_PINS[channel_idx],
        state ? "ON" : "OFF"
    );
    #endif
}

void ActuatorStateMachine::setChannelOff(uint8_t channel_idx) {
    setChannelState(channel_idx, false);
}

void ActuatorStateMachine::setChannelOn(uint8_t channel_idx) {
    setChannelState(channel_idx, true);
}

bool ActuatorStateMachine::getChannelPhysicalState(uint8_t channel_idx) const {
    if (channel_idx >= 4) return false;
    return channels[channel_idx].is_on;
}

bool ActuatorStateMachine::getChannelHysteresisActive(uint8_t channel_idx) const {
    if (channel_idx >= 4) return false;
    return channels[channel_idx].hysteresis_active;
}

// ============================================
// State Reporting
// ============================================
void ActuatorStateMachine::buildStateMessage(ActuatorState& state) {
    // Note: device_id should be set by caller
    state.timestamp = millis() / 1000;
    state.channels_count = 4;

    for (uint8_t i = 0; i < 4; i++) {
        const ChannelRuntimeState& ch = channels[i];
        ChannelState& ch_state = state.channels[i];

        ch_state.number = i + 1;
        ch_state.state = ch.is_on ? ChannelStateValue_STATE_ON : ChannelStateValue_STATE_OFF;
        ch_state.reason = ch.last_reason;
        ch_state.pwm_value = 0;  // Not used for binary actuator
        ch_state.sensor_value = ch.last_sensor_value;
        ch_state.threshold = ch.threshold;
    }
}
