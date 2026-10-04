---
id: controls
title: Controls
kind: feature
pages: /controls
nodes: control_source, device_write
err_codes: control-write-forbidden, control-key-taken, control-value-invalid, control-rate-limited, control-in-use, control-unknown, control-store-unavailable
tools: 
tags: controls, contrôles, switch, interrupteur, slider, curseur, button, bouton, command, commande, setpoint, consigne, operate, piloter, actuator
---
Controls (Data › Controls) lists the organization's controls: the switches, sliders, buttons and inputs operated from dashboards and annotations. A control drives nothing by itself: it stores the last value a person sent, and a deployed flow with a **Control source** node decides the effect on the devices.

## What you can do
- See every control with **Control**, **Declared by** (the dashboard or annotation set that placed it, or **Standalone controls**), **Kind**, **Last value** and **Listened by** (the deployed flows using it).
- **New control** creates a standalone control: **Label**, **Key** (becomes `msg.topic` in flows: letters, digits, ".", "_", "-", up to 64 characters) and a kind: **Switch** (on/off values), **Slider** or **Number** (min, max, step, unit), **Button** (sent value), **List of choices**, **Stepper (− / +)**, **Commands**, **Colour**.
- Tick **Ask for a confirmation before sending (sensitive actuator)** for gates, locks and similar.
- **Edit** or **Delete control**.

## Good to know
- Most controls are created automatically: placing a switch or a home card on a dashboard (or a control item on an annotation) and saving declares its control, keyed like `dash-xxxxxxxx.w-0001`. Removing the widget releases it.
- The same control can be linked on several surfaces (**Link to an existing control…** in the inspector): they share one state and one flow.
- The badge **No effect** means no deployed flow listens to the control: operating it does nothing until a flow with **Control source** is deployed.
- A control listened to by a deployed flow cannot be deleted; stop the flow or remove the control from its node first.
- Sending values too quickly is refused ("Too fast"); sliders only send when released.
- Every command is logged with who, when and from which surface (stream `ev_controls` on the **Events** page).
- Owners, Admins and Members operate controls; Viewers see them disabled.
- The assistant cannot operate controls.
