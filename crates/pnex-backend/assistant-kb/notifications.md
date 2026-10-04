---
id: notifications
title: Notifications
kind: feature
pages: /notifications
nodes: pnex_notify
err_codes: notify-write-forbidden, notify-channel-name-conflict, notify-template-name-conflict, notify-template-render, notify-journal-unavailable
tools: 
tags: notification, alert, alarm, ntfy, telegram, slack, discord, email, smtp, webhook, template, alerte, courriel
---
The Notifications page (Automation menu) manages how flows notify people: channels (where messages go), templates (what they say) and the journal of every delivery. A flow sends a notification through its Notification node.

## What you can do

- **Channels** tab: **New channel** opens a picker: WebSocket (internal bus, in-app, no settings), Webhook (generic HTTP POST), ntfy (push), Telegram, Slack, Discord, Email (SMTP). Each channel can be enabled or disabled, sent a **Test**, edited, deleted; **Journal** jumps to its deliveries. The table shows the last delivery status.
- **Templates** tab: **New template** with a name, an optional subject and a body. **+ Variable** inserts a placeholder in the body and declares it; give each variable an example value to see the **Preview**, and send a test to a channel.
- **Events** tab: the delivery journal (flows, Test buttons, OTA), filterable by channel, status (Sent, Failed, Blocked by anti-spam) and source, with the error detail of each failed send.
- In a flow, add a **Notification** node, select channels and a template; each template variable becomes an input row of the node.

## Good to know

- The Notification node has a mandatory boolean **trigger** row: the save is refused while it is not wired, and the message is sent only while the trigger is true.
- An optional anti-spam limit (max messages per time window) blocks bursts; blocked sends appear in the journal.
- Templates and channels are resolved at deploy: after changing or deleting a channel or template used by a flow, redeploy the flow (the editor warns about stale references).
- Channel and template names are unique in the organization.
- Viewers see the page read-only.
- The assistant can read, preview and write message templates, never channels, and it never sends a message.
