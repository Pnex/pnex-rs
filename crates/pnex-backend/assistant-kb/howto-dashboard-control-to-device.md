---
id: howto-dashboard-control-to-device
title: Drive a device output from a dashboard switch
kind: howto
pages: /dashboards, /flows, /controls
nodes: control_source, device_write
err_codes: pin-already-assigned, control-unknown, control-in-use
tools: 
tags: switch, interrupteur, relay, relais, led, light, lumière, slider, pwm, dimmer, variateur, command device, piloter, allumer, turn on, dashboard button, colour, couleur, rgb, ws2812, neopixel, custom firmware, onCommand
---
A dashboard switch, slider or home card never touches a device itself: it writes an org **control**. To make it act on a device output, a flow with a **Control source** node wired to a **Device (write)** node must be deployed by a person. This recipe goes from "I wired a relay" to "my switch turns it on".

## Steps
1. **Configure the output pin**: in **Devices**, open the device, tab **Pins**, set the pin to **Output (digital_out)** (relay, LED) or **PWM output (pwm_out)** (dimmer, fan speed).
2. **Place the command**: in **Dashboards**, open or create a dashboard, **Edit**. Either use **From a device** (pick the device: **+ Switch**, **+ Slider** or **+ Light** per output), or add a Switch / Slider / home card (Light, Fan…) from the palette.
3. **Save** the dashboard: each command becomes a control, listed under this dashboard in the **Control source** node and on the **Controls** page.
4. **Create the flow**: select the widget, open **Create the flow…** in the inspector, choose the **Target device…** and **Output pin or firmware command…**, then **Create and open the flow**. A draft **Control source → Device (write)** opens in the flow editor. You can also build it yourself in **Automation › Flows**.
5. Review the draft (add conditions, schedules or several devices if needed), then click **Deploy**. Nothing runs before you deploy.
6. Back on the dashboard in **Live**, operate the switch: the flow's debug shows the message and the output follows. The **No effect** badge disappears once a deployed flow listens.

## Good to know
- A switch sends 1/0 and a slider 0..100 by default, which **Device (write)** accepts as-is (digital 1/0, PWM duty 0..100).
- A **colour**, an option list or a setpoint cannot go to a pin: it goes to a **firmware command**. Write a custom firmware that declares it (`pnex.onCommand("color", …)` in **Edges › Custom firmware**), update the device, and the command appears in **Device (write)** under **Firmware commands** and in **Create the flow…**. The firmware receives the value in `args["value"]` (a colour = 0xRRGGBB as a number).
- One output pin has one writer: deploying is refused if another deployed flow already drives that pin. Stop the other flow or pick another pin.
- To show the real state rather than the last command, tick **Show the actual state** and choose the pin's telemetry or a memory key.
- Tick **Resend the last value at start** in the Control source node so outputs recover their state after a restart or redeploy.
- The assistant can draft this flow for you, but only you can deploy it.
