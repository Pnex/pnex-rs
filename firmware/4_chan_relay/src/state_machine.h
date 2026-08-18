// src/state_machine.h
#ifndef STATE_MACHINE_H
#define STATE_MACHINE_H

#include <Arduino.h>
#include "proto/actuator_message.pb.h"
#include "actuator_config.h"

// ============================================
// Channel Runtime State Structure (internal tracking)
// Note: Different from protobuf ChannelState message
// ============================================
struct ChannelRuntimeState {
    // Configuration
    bool enabled;
    ChannelMode mode;
    SafeMode safe_mode;

    // Binary mode config
    float threshold;
    Comparison comparison;
    bool invert_logic;  // Invert ON/OFF logic (for cooling/refrigeration)
    uint32_t hysteresis_seconds;
    float hysteresis_value;  // Value-based hysteresis (creates deadband)

    // PWM mode config (not used for binary actuator)
    float min_sensor_value;
    float max_sensor_value;
    uint32_t min_pwm;
    uint32_t max_pwm;

    // Current state
    bool is_on;
    uint32_t pwm_value;
    unsigned long last_change_time;
    StateReason last_reason;
    float last_sensor_value;  // Store last received sensor value for reporting

    // Hysteresis tracking
    bool hysteresis_active;     // True when hysteresis is blocking a state change
    bool pending_state;         // The state we want to change to (blocked by hysteresis)
    unsigned long hysteresis_start_time;  // When hysteresis period started

    // Constructor
    ChannelRuntimeState() :
        enabled(false),
        mode(ChannelMode_MODE_BINARY),
        safe_mode(SafeMode_SAFE_OFF),
        threshold(0),
        comparison(Comparison_LESS_THAN),
        invert_logic(false),
        hysteresis_seconds(DEFAULT_HYSTERESIS_SECONDS),
        hysteresis_value(0),
        min_sensor_value(0),
        max_sensor_value(100),
        min_pwm(0),
        max_pwm(255),
        is_on(false),
        pwm_value(0),
        last_change_time(0),
        last_reason(StateReason_REASON_DISABLED),
        last_sensor_value(0),
        hysteresis_active(false),
        pending_state(false),
        hysteresis_start_time(0)
    {
    }
};

// ============================================
// Actuator State Machine Class
// ============================================
class ActuatorStateMachine {
public:
    ActuatorStateMachine();

    // Initialization
    void begin();

    // Configuration management
    void handleConfig(const ActuatorConfig& config);
    void updateConfig(const ChannelConfig& config, uint8_t channel_idx);

    // Sensor data management - SIMPLIFIED!
    void handleSensorData(const SensorData& data);
    void processChannelData(uint8_t channel_idx, float value, bool fresh);

    // State machine processing
    void process();

    // Binary mode logic
    void processBinaryChannel(uint8_t channel_idx, float sensor_value);

    // Timeout and safe mode
    void checkTimeout();
    void applySafeMode(uint8_t channel_idx);
    void applyTimeoutSafeMode();
    void forceAllOff();  // Emergency safety: force all relays OFF (for disconnection)

    // GPIO control
    void setChannelState(uint8_t channel_idx, bool state);
    void setChannelOff(uint8_t channel_idx);
    void setChannelOn(uint8_t channel_idx);

    // State reporting
    void buildStateMessage(ActuatorState& state);

    // Getters
    bool isInTimeoutMode() const { return in_timeout_mode; }
    uint32_t getTimeoutSeconds() const { return data_timeout_seconds; }
    const ChannelRuntimeState& getChannelState(uint8_t channel_idx) const {
        return channels[channel_idx];
    }
    bool getChannelPhysicalState(uint8_t channel_idx) const;
    bool getChannelHysteresisActive(uint8_t channel_idx) const;

private:
    // Channel runtime states (4 channels, indexed 0-3)
    ChannelRuntimeState channels[4];

    // Timeout tracking
    uint32_t data_timeout_seconds;
    unsigned long last_sensor_data_time;
    bool in_timeout_mode;

    // Helper functions
    bool shouldActivate(uint8_t channel_idx, float sensor_value);
    bool checkHysteresis(uint8_t channel_idx);
};

#endif // STATE_MACHINE_H
