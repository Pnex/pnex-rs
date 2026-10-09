//
// pnex-camera — optional camera module of the PneX lib (camera-video.md
// D73/D76/D77). Compiled only when PNEX_CAMERA_ENABLE == 1 on a classic
// ESP32 (AI-Thinker ESP32-CAM pinout, OV2640 + PSRAM); every other build
// gets the inline no-op stubs below and never pulls esp_camera.
//
// Wiring into a sketch (firmware/generic_esp32cam):
//
//   PnexDevice pnex;
//   void setup() { pnex_camera_begin(pnex); pnex.begin(); }
//   void loop()  { pnex.loop(); pnex_camera_loop(); }
//
// pnex_camera_begin() must run BEFORE pnex.begin(): it registers the
// `camera`/`video` announce cap (only when the sensor initialized) and the
// server message hook consuming ServerMsg::CameraConfig.
//
// Runtime:
// - boot state = not streaming; a `camera_config` with enabled=true opens
//   a SECOND WebSocket `/ws/camera?device_id=` (token in the Authorization
//   header, D154; same scheme, host
//   and TLS posture as the control WS) and frames are pushed at `fps`;
// - after a binary Noise handshake (D156), each frame = one binary WS
//   message: `PXC1 header(16) || JPEG` sealed on the camera's Noise link
//   (pnex_camera_frame.h + cryptoSealBinaryInPlace);
// - enabled=false, or the control WS going down, closes the camera WS and
//   resets the state to "not streaming" (the server re-pushes the config
//   after the next announce);
// - reconnect backoff 1 s → 30 s; a sealed "PING" text every 15 s keeps
//   the server watchdog (45 s) quiet when frames are sparse.
//
#ifndef PNEX_CAMERA_H
#define PNEX_CAMERA_H

#include <Arduino.h>  // brings sdkconfig.h (CONFIG_IDF_TARGET_*)

class PnexDevice;

// Single gate shared by the header and pnex_camera.cpp: classic ESP32 only
// (the AI-Thinker pin map below does not exist on C3/S3/8266).
#if defined(PNEX_CAMERA_ENABLE) && PNEX_CAMERA_ENABLE == 1 && defined(ESP32) && \
    defined(CONFIG_IDF_TARGET_ESP32)
#define PNEX_CAMERA_ACTIVE 1
#else
#define PNEX_CAMERA_ACTIVE 0
#endif

#if PNEX_CAMERA_ACTIVE

/// Initializes the sensor, registers the announce cap and the message
/// hook. False when the camera failed to initialize (no cap announced;
/// camera_config commands are then refused with an Ack error).
bool pnex_camera_begin(PnexDevice& device);

/// Call on every loop() turn after pnex.loop(): camera WS connect /
/// reconnect / keepalive and frame pacing.
void pnex_camera_loop();

#else

inline bool pnex_camera_begin(PnexDevice&) { return false; }
inline void pnex_camera_loop() {}

#endif

#endif  // PNEX_CAMERA_H
