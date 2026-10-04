---
id: troubleshooting-no-telemetry
title: Device connected but no telemetry
kind: troubleshooting
pages: /devices, /visualisation, /flows
nodes: device_read
err_codes: pin_not_subscribed
tools: 
tags: no data, no telemetry, empty chart, telemetry unavailable, subscribe, read interval, pas de données, mesures, capteur muet
---
A generic board shows as connected but no measurement ever arrives: charts stay empty, flows reading it get an empty payload. The usual cause is that no input pin has a read interval, so the board publishes nothing.

## Symptom

- The device is online on Devices / Agents, yet the Visualization page shows no series for it, or "Telemetry unavailable (OpenObserve unreachable or organization not provisioned)" for a brand-new organization.
- A Device (read) node in a flow outputs `{}` (no value is ever invented), and the flow editor banner says the pin "is not subscribed — the device publishes nothing".
- On the device detail, an amber banner says: "Connected, but no pin is subscribed: the device publishes nothing".

## Cause

A generic PneX board only sends the pins you subscribed. Without any read interval on an input pin, nothing is published, and the organization's telemetry storage is only created with the first measurement it receives, so a first device with no subscription also leaves the whole telemetry unavailable.

## Fix (in the UI)

1. Open **Devices / Agents**, find the device and click **Detail**.
2. On the board pinout, click an input pin (digital or analog input).
3. In the pin panel, choose a **Read interval** (Read every 1 s, 5 s, 15 s or 60 s) instead of "Manual read" and click **Apply**.
4. Values appear within seconds; the Visualization page and Device (read) nodes then see the pin.

Repeat for every input pin you need. Output pins are not subscribed. Note that deleting and re-registering a device under a new identifier starts from zero: subscriptions must be set again and older series stay under the old identifier.
