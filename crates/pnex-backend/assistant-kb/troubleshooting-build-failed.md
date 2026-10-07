---
id: troubleshooting-build-failed
title: Firmware build failed
kind: troubleshooting
pages: /devices, /firmware
nodes: 
err_codes: 
tools: 
tags: build failed, firmware, compile error, compilation, rebuild, échec du build, build échoué, erreur de compilation, wifi password, certificate
---
A device shows the **Build failed** badge: the server could not produce its firmware, so there is nothing to flash or deploy over the air.

## Symptom

- In **Devices / Agents**, the firmware column shows **Build failed**; hovering the badge shows the reason.
- The device detail shows a red **Why the build failed** box under its header; for a compilation error, **Compiler output** unfolds the last lines of the compiler (device credentials are masked).
- The registration wizard shows the same box when the build launched by **Create & build** fails.

## Cause

The reason shown tells which step failed:

- **The firmware does not compile**: for a custom firmware, an error in its code; the compiler output names the file and line.
- **The WiFi password is missing from the vault**: the WiFi network picked for the device was deleted or its secret removed.
- **The build ran out of time**: the build server is busy or slow.
- **No certificate authority to pin**: the server is missing its device certificate (an installation problem).
- **The device or its token was deleted during the build**, or the custom firmware project has no revision.

## Fix (in the UI)

1. **Compilation error (custom firmware)**: open the project in **Firmware**, fix the code, click **Verify** until it passes, then **Rebuild** the device from its row in **Devices / Agents**.
2. **WiFi password missing**: in **Edge referentials**, save the WiFi network again (or pick another one on the device), then **Rebuild**.
3. **Out of time** or an unexpected server error: click **Rebuild** a little later.
4. **No certificate authority**: ask the platform administrator; nothing can be fixed from the organization.

A failed build never changes what a device already runs: its current firmware keeps working until a new build succeeds and is flashed or deployed.
