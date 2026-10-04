---
id: troubleshooting-control-no-effect
title: Dashboard switch does nothing
kind: troubleshooting
pages: /dashboards, /controls, /annotations, /flows
nodes: control_source, device_write
err_codes: control-rate-limited, control-store-unavailable, control-write-forbidden, pin-already-assigned, control-unknown
tools: 
tags: switch does nothing, interrupteur ne marche pas, no effect, sans effet, relay not switching, button not working, control deleted, contrôle supprimé, slider
---
Operating a switch, slider, button or home card on a dashboard (or a control on an annotation) changes the card but nothing happens on the device, or the card shows **No effect** or **Control deleted**.

## Symptom
- Badge **No effect** next to the command.
- The value changes on the card but the relay, LED or motor does not react.
- Card shows "Control deleted".
- Toast "Too fast: wait a moment…" or "The control store (Valkey) is unavailable".
- The command is disabled.

## Cause
- By design a dashboard only writes a control; only a **deployed** flow with **Control source** → **Device (write)** acts on a device. **No effect** means no deployed flow listens to this control.
- The flow exists but was only saved (draft), or the deployed version is older than the one you edited.
- The flow writes the wrong device or pin, or the pin is not configured as an output.
- Deploy was refused because another deployed flow already drives that output pin.
- The device is offline.
- The control was released (widget removed, or an older dashboard version restored) or deleted.
- Your role is Viewer (commands are read-only), or values were sent too fast.

## Fix (in the UI)
1. Select the widget in **Edit** and read **Listened by** in the inspector, or check **Listened by** on the **Controls** page.
2. If nothing listens, use **Create the flow…**, choose the device and output pin, and **Deploy** the draft in the flow editor.
3. If a flow exists, open it in **Automation › Flows**, check the **Control source** node has this control checked and that **Device (write)** targets the right device and pin, then **Deploy** again.
4. In **Devices › Pins**, make sure the pin mode is **Output (digital_out)** or **PWM output (pwm_out)** and the device is connected.
5. For "Control deleted", save the dashboard again: the widget gets a new control, then select it in the flow's Control source node and redeploy.
6. Watch the flow's debug while you operate the switch: a message there means the dashboard side works.
