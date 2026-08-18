# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

This is an ESP8266-based 4-channel relay actuator that implements intelligent autonomous control with formal state machine logic. The device receives sensor data from a server via WebSocket and makes local decisions to control relays based on configurable thresholds and hysteresis.

**Key Architecture Components:**
- **Protocol Buffers (Nanopb)**: Binary communication protocol for efficient message exchange
- **State Machine**: Device-based autonomous decision engine with threshold logic, hysteresis, and timeout protection
- **WebSocket Client**: Secure connection to server for receiving configuration and sensor data
- **OLED Display**: Real-time status visualization (128x64 SSD1306)

## Build Commands

### Using build.sh (Recommended)
The project uses `uv` as the Python environment manager. The `build.sh` script loads environment variables and runs PlatformIO commands:

```bash
# Build the firmware
./build.sh run

# Upload to device
./build.sh run --target upload

# Monitor serial output
./build.sh device monitor -b 115200

# Clean build
./build.sh run --target clean
```

### Using Task (Alternative)
```bash
# Upload firmware
task flash-firmware  # or: task fm

# Monitor device
task dev-monitor     # or: task m
```

### Direct PlatformIO Commands
If environment variables are already exported:
```bash
pio run                        # Build
pio run --target upload        # Upload
pio device monitor -b 115200   # Monitor
```

## Protocol Buffers

### Generating Protobuf Files
The project uses Nanopb to generate C files from `.proto` definitions:

```bash
# Manual regeneration (usually automatic during build)
python3 generate_proto.py
```

**Important Files:**
- `proto/actuator_message.proto` - Message definitions
- `proto/actuator_message.options` - Nanopb field size constraints
- `src/proto/actuator_message.pb.h/c` - Generated files (auto-generated, do not edit directly)

**When to regenerate:**
- After modifying `actuator_message.proto`
- After changing `actuator_message.options`
- Usually happens automatically via PlatformIO pre-build hook

## Architecture Details

### Communication Protocol

The device communicates via WebSocket using three message types:

1. **ActuatorConfig** (Server → Device): Configuration with channel settings, thresholds, hysteresis, safe modes
2. **SensorData** (Server → Device): Aggregated sensor values per channel with freshness indicators
3. **ActuatorState** (Device → Server): Current relay states, reasons, and sensor values

**Critical Behavior:**
- Device sends PING every 5 seconds to maintain active status on server
- Server responds with PONG
- Device sends state reports every 5 seconds

### State Machine Logic

Located in `src/state_machine.cpp` and `src/state_machine.h`:

**Key Concepts:**
- **Binary Mode**: Relay ON/OFF based on threshold comparison (LT or GT)
- **Hysteresis**: Two types to prevent oscillation
  - Time-based: Minimum seconds between state changes
  - Value-based: Creates deadband around threshold (e.g., ON at threshold-2°C, OFF at threshold)
- **Safe Mode**: Action when data is stale or timed out (SAFE_OFF, SAFE_ON, SAFE_KEEP)
- **Timeout Protection**: Applies safe mode if no sensor data received within configured timeout (default 10s)

**Decision Flow:**
1. Receive `SensorData` with channel values and freshness flags
2. For each enabled channel:
   - If data is stale → Apply safe mode
   - Check hysteresis conditions (time and value)
   - Evaluate threshold comparison with hysteresis deadband
   - Update relay state if conditions met
3. If no data received within timeout → Apply safe mode to all channels

### GPIO Configuration

Relay pins are defined in `src/actuator_config.h`:
- Channel 1: D1 (GPIO5)
- Channel 2: D2 (GPIO4)
- Channel 3: D4 (GPIO2)
- Channel 4: D7 (GPIO13)

**Relay Mode**: Active LOW by default (LOW = ON, HIGH = OFF)
Change `RELAY_ACTIVE_LOW` in `actuator_config.h` if using Active HIGH relays.

### Common Libraries

The `../common_libs` directory contains shared code across multiple devices:
- `config/config.h` - WiFi and connection configuration defaults
- `display/DisplayManager.h/cpp` - OLED display abstraction for status visualization
- `display/icon_set.h` - Icons for WiFi, WebSocket, arrows, relay status

These libraries are referenced via `lib_extra_dirs = ../common_libs` in `platformio.ini`.

## Environment Variables

Configuration is managed via environment variables (loaded from `.env` or exported in `build.sh`):

- `WIFI_SSID` - WiFi network name
- `WIFI_PASSWORD` - WiFi password
- `HOST` - Server hostname (base64 encoded)
- `TOKEN` - Authentication token (base64 encoded)
- `DEVICE_ID` - Unique device identifier (base64 encoded)

These are injected as build flags in `platformio.ini` and referenced in `config.h`.

## Key Configuration Constants

In `src/actuator_config.h`:

```cpp
#define DEFAULT_TIMEOUT_SECONDS 10        // Safe mode timeout
#define PING_INTERVAL 5000                // PING frequency (critical!)
#define STATE_REPORT_INTERVAL 5000        // State report frequency
#define WIFI_RECONNECT_INTERVAL 10000     // WiFi check interval
#define DEFAULT_HYSTERESIS_SECONDS 60     // Default time hysteresis
```

Debug flags:
```cpp
#define DEBUG_SERIAL true
#define DEBUG_PROTOBUF true
#define DEBUG_STATE_MACHINE true
```

## Important Implementation Notes

### WebSocket Connection Management
- Uses exponential backoff for reconnection (1s → 2s → 4s → ... → 60s max)
- Automatically reconnects on connection loss
- **CRITICAL**: PING must be sent every 5 seconds or device appears offline to server

### State Machine Processing
- Processing happens immediately upon receiving `SensorData` (no periodic polling of cached data)
- Each channel operates independently
- Disabled channels are immediately turned off
- Stale data triggers safe mode per channel
- Global timeout triggers safe mode for all channels

### Protobuf Decoding Strategy
When binary message arrives, the code attempts to decode in order:
1. Try `ActuatorConfig` decode
2. If fails, try `SensorData` decode
3. If both fail, log error

### Display Updates
- Display updates every 1 second (DISPLAY_UPDATE_INTERVAL)
- Shows WiFi status, WebSocket status, device ID, relay states
- Arrows indicate TX/RX activity

## Troubleshooting

**Build fails with protobuf errors:**
- Ensure `nanopb` is installed: `pip install nanopb`
- Manually run `python3 generate_proto.py`

**Device not connecting to server:**
- Check base64 encoding of HOST, TOKEN, DEVICE_ID
- Verify WiFi credentials
- Monitor serial output at 115200 baud

**Relay not switching:**
- Verify relay is Active LOW (or change `RELAY_ACTIVE_LOW`)
- Check channel is enabled in configuration
- Verify sensor data freshness flag
- Check timeout settings

**Device shows offline on server:**
- Ensure PING is sent every 5 seconds (check PING_INTERVAL)
- Verify WebSocket connection is stable
- Check server logs for PING/PONG messages
