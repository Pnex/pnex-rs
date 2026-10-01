//
// Generic ESP32-CAM firmware (AI-Thinker, Tier 1 preset) — the generic
// ESP32 pin_slave (server-driven pins: flash LED GPIO4, red LED GPIO33)
// plus the PneX camera module (camera-video.md D73/D76/D77): frames are
// pushed on a second WebSocket /ws/camera while the server asks for them
// (ServerMsg::CameraConfig).
//
// Flashing: the AI-Thinker has no USB — use the ESP32-CAM-MB carrier or an
// FTDI adapter with GPIO0 tied to GND at reset.
//
// Device config: -D defines (b64) via ${sysenv.*}, built per device by the
// server (docs/architecture/firmware-build.md §2.1).
//

#include <Pnex.h>
#include <pnex_camera.h>

PnexDevice pnex;

void setup() {
    // Before pnex.begin(): registers the `camera` cap (first announce) and
    // the camera_config hook.
    pnex_camera_begin(pnex);
    pnex.begin();
}

void loop() {
    pnex.loop();
    pnex_camera_loop();
}
