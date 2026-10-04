---
id: visualisation
title: Quick charts
kind: feature
pages: /visualisation
nodes: metric
err_codes: 
tools: 
tags: quick charts, visualization, visualisation, courbes, graph, chart, series, telemetry, history, openobserve
---
Quick charts (Visualization › Quick charts) plots the measurements stored in OpenObserve, sensor by sensor, without building a dashboard: pick a metric and a sensor, add the series, and compare up to six curves.

## What you can do
- **Available series** lists every series of the organization with the age of its last point.
- Pick a **Metric** and a **Sensor**, then **Add**: the curve appears in the **Chart**. Up to 6 series can be overlaid.
- Choose the **Window**: 1 h, 6 h or 24 h (24 h by default).
- The chart refreshes automatically every 15 s ("Auto · 15 s").

## Good to know
- Series come from two places: device telemetry (pins a device publishes) and flow results written with a **Metric** node (series named `etl_…`).
- "No telemetry data in this organization" means nothing has been stored yet: a generic device publishes only the pins subscribed in **Devices › Pins**, and a flow publishes only once a Metric node is deployed and receiving messages.
- "Telemetry unavailable" means OpenObserve is unreachable or the organization is not provisioned yet; it is provisioned automatically on the first ingestion.
- Quick charts are not saved. To keep a layout, build a dashboard (**Visualization › Dashboards**) with mini charts, gauges and values.
- Values from the org shared memory (**Memory write** node) have no history and do not appear here.
