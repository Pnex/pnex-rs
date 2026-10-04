---
id: troubleshooting-telemetry-unavailable
title: Telemetry unavailable
kind: troubleshooting
pages: /, /visualisation, /dashboards, /system
nodes: metric
err_codes: o2-not-configured
tools: 
tags: telemetry unavailable, no data, empty chart, no measurements, openobserve, not provisioned, données indisponibles, pas de données, graph vide
---
"Telemetry unavailable" appears on the home page, Visualisation or a dashboard when PneX cannot read any measurement for the active organization. Most often the organization has simply never sent data yet; less often the telemetry store is not configured or unreachable.

## Symptom
- Visualisation or a dashboard shows "Telemetry unavailable (OpenObserve unreachable or organization not provisioned)", the home page "Telemetry unavailable (OpenObserve not configured or unreachable)", or a widget "Telemetry unavailable for …".
- The System page may show "No data received yet: the OpenObserve space is created with the first measurement."

## Cause
1. **No measurement received yet** (most common): the organization's telemetry space is only created when its first measurement arrives. A device can be online yet publish nothing if none of its pins is subscribed.
2. **Telemetry store not configured or down** on the installation (System page: "OpenObserve is not configured on this server.").
3. Telemetry is read per organization: data sent under another organization is not visible from the active one.

## Fix (in the UI)
1. Check the active organization in Organizations (green **Active** badge).
2. Make a device send data: in **Devices**, open the device; for a generic board, click an input pin on the board diagram and choose a **Read interval** (e.g. Read every 5 s), then **Apply**. The Pins panel warns "Connected, but no pin is subscribed…" when nothing is published.
3. Or deploy a flow that writes a measurement with a **metric** node.
4. Wait for the first values, then reload the page: the space is created automatically and the message disappears.
5. If the System page says OpenObserve is not configured, or the message persists while data is being sent, the problem is on the server side: ask the platform administrator, who can check the OpenObserve card in Platform status.
