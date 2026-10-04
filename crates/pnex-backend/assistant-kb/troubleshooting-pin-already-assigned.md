---
id: troubleshooting-pin-already-assigned
title: Output pin already driven by another flow
kind: troubleshooting
pages: /flows, /devices
nodes: device_write, reg_tt_heat, reg_tt_cool, reg_pid
err_codes: pin-already-assigned, pin-reserved-by-flow
tools: 
tags: pin already assigned, deploy refused, output pin, one writer, conflict, relay, pin déjà assigné, sortie, conflit
---
Deploying a flow is refused because one of its output pins is already written by another deployed flow. Each output pin (digital or PWM output) has a single writer in the organization.

## Symptom

- **Deploy** fails with: "Output pin X of device Y is already driven by flow “Z” — stop that flow or choose another pin."
- On the device detail, writing the pin by hand is refused with "Pin X is reserved by flow “Z” (deployed)", and its controls are greyed out.

## Cause

Two writers on the same output would fight silently (a relay switched on by one flow and off by another). PNeX therefore lets only one deployed flow write each output pin: a Device (write) node or the actuator of a regulation card (TT heating, TT cooling, PID) claims the pin. Redeploying the flow that already owns the pin is always allowed; stopping or deleting a flow is never blocked. A pin that flows only read stays free.

## Fix (in the UI)

Pick one of:

1. **Stop the other flow**: open flow Z from **Flows** and click **Stop**, then deploy yours. The pin is freed as soon as Z stops.
2. **Use another pin**: in your Device (write) node or regulation card, select a different output pin, **Save Changes**, then **Deploy**.
3. **Merge the logic**: move the writing into a single flow (for example with a Merge to JSON node feeding one Device (write)), stop the other flow, then deploy.

To drive the pin by hand from the device's pinout, stop the flow that reserves it first. Dashboards never write pins: a dashboard control reaches the pin only through a Control source node in the deployed flow that owns it.
