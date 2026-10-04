---
id: troubleshooting-notify-not-sent
title: Notification never sent
kind: troubleshooting
pages: /flows, /notifications
nodes: pnex_notify, calc, pnex_function, anomaly, forecast
err_codes: notify-template-render
tools: 
tags: notification not sent, no alert, trigger, boolean, anti-spam, blocked, notification non envoyée, pas d'alerte
---
A flow with a Notification node never sends anything, or the save is refused. The Notification node only sends when its mandatory boolean **trigger** row receives true.

## Symptom

- Saving the flow is refused with an "Invalid graph" banner asking to wire the trigger input.
- The flow is deployed but no message arrives on the channel.
- The Events tab of the Notifications page shows no delivery, or deliveries marked "Blocked (anti-spam)" or "Failed".

## Cause

- The **trigger** row of the Notification node is not wired, or receives something that is never true. It expects a boolean (true/false; 1/0 and "true"/"false" are accepted): the message is sent only while the trigger is true. Without this gate the node would send on every message, so it is mandatory.
- Template variables come in through their own input rows; the node sends once it has the values and the trigger turns true.
- The anti-spam limit of the node blocks bursts.
- The flow runs an older deployed version, or the channel is disabled or misconfigured.

## Fix (in the UI)

1. In the flow editor, produce a boolean: a **Calc** node with a comparison (e.g. `my_device_A0 > 500`), a **Function** returning a bool output, or the boolean port of **Anomaly detection** / **Forecast**.
2. Drag its output onto the **trigger** row of the Notification node, and wire each template variable to its row.
3. **Save Changes**, then **Deploy**.
4. On **Notifications** → **Channels**, check the channel is Enabled and use **Test** to confirm it delivers.
5. Read **Events** for the error detail of failed sends; a "Blocked (anti-spam)" status means the node's rate limit was reached.
