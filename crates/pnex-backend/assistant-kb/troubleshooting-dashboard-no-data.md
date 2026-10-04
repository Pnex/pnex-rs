---
id: troubleshooting-dashboard-no-data
title: Dashboard widget shows no data or stale data
kind: troubleshooting
pages: /dashboards, /visualisation
nodes: metric, memory_write
err_codes: 
tools: 
tags: no data, pas de données, empty widget, widget vide, stale, périmé, greyed, grisé, frozen, figé, telemetry unavailable, to configure, dashboard empty
---
A dashboard widget or card stays empty, shows "No data over the window", "No recent data" (greyed card), "Telemetry unavailable for …" or "To configure", or its values do not move.

## Symptom
- Empty gauge, value or chart; "No data over the window".
- Greyed card with "No recent data".
- "Telemetry unavailable for <widget>".
- Home card showing "To configure: pick its source in the editor".
- Values frozen while editing.

## Cause
- **Edit mode** shows a frozen preview ("values are not refreshed").
- The card comes from a template or was never bound: no source picked.
- The device publishes nothing: a generic-firmware device sends only the pins that are subscribed with a read interval.
- The window is too short for a slow source, or the source stopped sending: the card greys out after its **Stale after (seconds)** delay.
- A **Memory (flows)** source is empty: the writing flow is not deployed, or its **Lifetime** expired because the value was not rewritten in time.
- The telemetry store (OpenObserve) is unreachable or the organization has not sent any data yet.

## Fix (in the UI)
1. Switch the dashboard to **Live**.
2. In **Edit**, select the widget and check **Source** and **Metric** in the **Inspector**; for home cards, pick a source for each value read.
3. In **Devices**, open the device, tab **Pins**, and choose a read interval (e.g. "Read every 5 s") for the pins you display. Check the device is online.
4. Confirm the series exists in **Visualization › Quick charts**: if it is missing there, the problem is upstream of the dashboard.
5. For memory sources, open the flow in **Automation › Flows**, check it is deployed and that the **Memory write** lifetime is longer than the time between two messages.
6. Raise **Stale after (seconds)** (or set it to **Never**) if the source legitimately reports rarely.
7. If every widget says "Telemetry unavailable", check **System**: the organization's telemetry space is created with the first measurement; otherwise ask the platform administrator.
