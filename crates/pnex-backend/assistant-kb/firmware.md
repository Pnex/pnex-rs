---
id: firmware
title: Custom firmware
kind: feature
pages: /firmware
nodes: device_read, device_write
err_codes: firmware-custom-disabled, firmware-chip-mismatch, firmware-family-locked, firmware-lib-unknown, firmware-source-refused, firmware-project-in-use, firmware-include-parent, firmware-source-too-large, firmware-version-conflict
tools: 
tags: firmware, ide, c++, arduino, main.cpp, sketch, library, verify, compile, revision, micrologiciel, code embarqué
---
The Custom firmware page (Edges menu) is a C++ editor in the browser: you write one `main.cpp` per project against the PneX library, and the server compiles it per device. Use it when a generic board is not enough (I²C/SPI/1-Wire sensors, local logic, your own metrics and commands).

## What you can do

- **New project**: name, **Chip family** (ESP32, ESP32-C3, ESP32-C6, ESP32-S3, ESP8266) and description. The exact board comes from each attached device.
- Edit `main.cpp`. The **PneX API** menu inserts documented calls (declare and publish a metric, handle a command, declare pins, run code periodically without blocking). The **Lib** menu adds libraries from a curated catalog with pinned versions.
- **Save** creates an immutable revision; **History** lists revisions and can load an older one into the editor.
- **Verify** (or **Save and verify**) compiles the saved revision for the chip family without any device or real secret; compiler errors are mapped to your lines.
- Attach a project to devices in the Register wizard (Devices / Agents), step Model: only projects for the board's chip are listed. **Rebuild** on the device row compiles the latest revision; then flash it or update over the air.

## Good to know

- Call the PneX loop on every iteration and never block in long delays, or the server marks the device offline and outputs fall back to their safe state.
- WiFi, device token and encryption key never appear in your code: they are injected at build time.
- Metrics published with `addMetric` / `publish` show up in Quick charts, and in a flow through the **Device (read)** node: pick the device and check the metric, listed as "(metric)" after the board pins (it appears once the device has published).
- Commands registered with `onCommand` are driven from a flow through the **Device (write)** node: pick the device and check the command under **Firmware commands** (it appears once the device has announced it, after the update). The handler receives the wired value in `args["value"]`; keep it short and do the work in `loop()`.
- Libraries outside the catalog are refused. Absolute or ".." includes, embedded files and sketches over 256 KiB are refused too.
- Only generic models accept a custom firmware; predefined boards keep their PneX firmware.
- A project cannot be deleted while devices use it.
- Custom firmware can be disabled by the server administrator; the page then refuses builds.
- If someone saved a newer revision first, the editor reloads.
