---
id: devices
title: Devices / Agents
kind: feature
pages: /devices
nodes: device_read, device_write
err_codes: device-write-forbidden, build-in-progress, build-create-forbidden, pin-flow-conflict, pin-reserved-by-flow, pin-device-offline, board-reserved-screen
tools: 
tags: device, devices, agent, board, pinout, pins, firmware, ota, flash, provisioning, token, capteur, carte, appareil
---
The Devices / Agents page (Edges menu) is the registry of the organization's boards and edge agents: register them, build and flash their firmware, update them over the air, and configure their pins from an interactive board pinout.

## What you can do

- **Register** opens a 4-step wizard: Identifier (unique device id, 16 characters max, optional metadata), Model (Generic PneX, Predefined boards or Agents), WiFi (credential and PNeX server picked from the Edge referentials), Review. **Create & build** starts the firmware build right away; you can close the window, the build runs on the server.
- The list filters by type (Sensor, Actuator, Mixed), status and capability, and shows the firmware state: Up to date, Update available, Build in progress, Build failed, Offline.
- Row actions: **Flash** (Web Serial in Chrome/Edge, or the desktop app), **Update over the air**, **Rebuild**, **Download**, **Detail**. Select several rows for **Build selected** or **OTA selected**.
- **Detail** shows capabilities, the provisioning token and encryption key (Reveal / Copy, never share them), labels, and the board pinout. Click a pin to set its Mode (digital in/out, PWM, analog), Safe state and **Read interval**, or to write HIGH/LOW or a PWM duty by hand.
- An agent (computer or Raspberry Pi) shows an install card with a single-use enrollment code and its discovered keys.

## Good to know

- A board publishes nothing until at least one input pin has a read interval: an amber banner warns when the device is connected but no pin is subscribed.
- Changing the mode of a pin used by deployed flows asks to **Stop flows and apply**.
- An output pin driven by a deployed flow is greyed out: it cannot be written by hand (one writer per output pin).
- An offline device is read-only; an OTA update to an offline device is queued until its next connection.
- Building does not update a device: deploy over the air (or flash) afterwards. Each build gets a new build number; deploying the version the device already runs is forced automatically ("Same version").
- The Android app cannot flash over USB: flash from a computer, then update over the air.
- Viewers see the page read-only.
