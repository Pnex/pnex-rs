---
id: home
title: Home
kind: feature
pages: /
nodes: 
err_codes: 
tools: 
tags: home, accueil, overview, summary, quotas, tier, devices online, latest measurements
---
The Home page is the organization overview: device counts and online status, firmware build success rate, the active organization and its tier capacities, and the latest measurements received from devices.

## What you can do
- Read the summary tiles: **Total devices**, **Devices online**, **Organizations**, **Current tier** and **Build success rate**.
- **Device status** lists each device with its last-seen time ("never" when it has never connected).
- **Latest measurements** shows the newest value per device and metric (columns Device, Metric, Value, Time).
- **Tier capacities** shows the quotas of the active organization (sensor, actuator and mixed devices); **Devices by type** breaks the fleet down.
- The page refreshes itself (every 15 s by default); pick another rate with **Refresh every** (1 to 60 s) or refresh immediately.

## Good to know
- Everything shown belongs to the **active organization**: switch organization from the sidebar to see another one.
- On a self-hosted server the tier shows **Self-hosted**: there is no subscription.
- "Telemetry unavailable (OpenObserve not configured or unreachable)" means the measurement store cannot be reached; counts and statuses still work, only the measurements block is empty.
- "No measurements yet" on a connected generic-firmware device usually means no pin is subscribed: open **Devices**, then the device's **Pins** tab, and pick a read interval.
- For curves rather than last values, use **Visualization › Quick charts**; for a composed screen, use **Dashboards**.
