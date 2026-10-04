---
id: edge-refs
title: Edge referentials
kind: feature
pages: /edges/refs
nodes: 
err_codes: edge-host-locked
tools: 
tags: wifi, ssid, password, server, host, referentials, provisioning, lan, référentiels, mot de passe
---
The Referentials page (Edges menu, titled Edge referentials) stores the WiFi credentials and PNeX server addresses that device registration uses to compile firmware. Device registration needs at least one WiFi credential and one PNeX server.

## What you can do

- Two tabs: **WiFi credentials** and **PNeX servers**.
- **Add WiFi credential**: SSID and password. The password can be typed (stored encrypted) or picked from an existing secret of the Secrets page.
- **Add PNeX server**: the host the boards will connect to. **Detect PNeX servers on the LAN** scans your local network (optional LAN prefix, empty = auto) and lets you **Register** a server it finds.
- **Edit** or **Delete** an entry from its row.
- The same entries appear as pickers in the WiFi step of the Register wizard on Devices / Agents, where you can also add one on the fly.

## Good to know

- A device cannot reach "localhost": enter the server's LAN address (for example 192.168.1.16:5150).
- Some deployments impose their server: it is then shown as "Server imposed by this deployment" and cannot be added or changed.
- Every firmware is unique to its device. Changing a WiFi password rebuilds nothing: devices already flashed keep the old credentials until you rebuild and reflash them one by one.
- Deleting an entry only affects future builds; devices already flashed keep working.
- Secrets only transit the build queue; they never appear in the UI after saving.
- Viewers see the page read-only.
