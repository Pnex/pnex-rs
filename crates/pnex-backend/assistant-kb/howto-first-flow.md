---
id: howto-first-flow
title: Build your first flow
kind: howto
pages: /flows
nodes: inject, device_read, calc, metric, display, debug
err_codes: 
tools: 
tags: first flow, getting started, tutorial, read sensor, compute, metric, deploy, premier flow, tutoriel, démarrer
---
Recipe for the canonical pipeline: Inject → Device (read) → Calc → Metric. It reads a device pin periodically, converts the value, and stores the result as a series you can chart on the Visualization page or in dashboards.

## Before you start

The device must be online and the pin you read must have a **Read interval** (Devices / Agents → **Detail** → click the pin → Read interval → **Apply**); otherwise the flow only sees empty payloads.

## Steps

1. Go to **Flows** and click **New flow**. Rename it from the editor title if you like.
2. Click **+** and add an **Inject** node. Set **Interval (s)**, e.g. 5: the flow then runs continuously.
3. Add a **Device (read)** node. Pick the device, then tick the pins to read (e.g. A0) under "Pins to read". Each pin gets its own output port, plus an "all" port carrying every pin in one object.
4. Add a **Calc** node. Its variables are the payload keys: device identifier and pin label joined by an underscore, with case preserved (device `soil-sensor`, pin `A0` → `soil_sensor_A0`). The inspector lists the "Detected variables". Write an expression such as `soil_sensor_A0 * 0.01`.
5. Add a **Metric** node and give it a **Metric name**, e.g. `soil_volt`. The inspector previews the written series (`etl_soil_volt`, recorded under the flow as its device).
6. Wire the nodes: drag from Inject's output to Device (read), from the pin's port to Calc, from Calc to Metric. Optionally branch a **Display** or **Debug** node after Calc to watch the values.
7. Click **Save Changes**. Fix anything listed in the "Invalid graph" banner.
8. Click **Deploy**. The status chip shows "Deployed".
9. Open **Visualization** → Quick charts and look for the new `etl_` series.

## Good to know

- Saving never deploys; after each change, deploy again.
- Writing an output pin goes through a **Device (write)** node, and only from a deployed flow.
- The assistant can draft this flow for you, but you deploy it yourself.
