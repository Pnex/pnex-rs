---
id: notifications
title: Notifications
kind: feature
pages: /notifications
nodes: pnex_notify
err_codes: notify-write-forbidden, notify-channel-name-conflict, notify-template-name-conflict, notify-template-render, notify-journal-unavailable, notify-channel-config-invalid, notify-channel-field-required, notify-test-failed, notify-dynamic-recipients-admin, notify-dynamic-recipients-off
tools: 
tags: notification, alert, alarm, ntfy, telegram, slack, discord, email, smtp, webhook, template, alerte, courriel
---
The Notifications page (Automation menu) manages how flows notify people: channels (where messages go), templates (what they say) and the journal of every delivery. A flow sends a notification through its Notification node.

## What you can do

- **Channels** tab: **New channel** opens a picker: In-app notifications (no settings), Webhook (generic HTTP POST), ntfy (push), Telegram, Slack, Discord, Email (SMTP). Each channel can be enabled or disabled, sent a **Test**, edited, deleted; **Journal** jumps to its deliveries. The table shows the last delivery status.
- **Templates** tab: **New template** with a name, an optional subject and a body. **+ Variable** inserts a placeholder in the body and declares it; give each variable an example value to see the **Preview**, and send a test to a channel.
- **Delivery log** tab: the delivery journal (flows, Test buttons, OTA), filterable by channel, status (Sent, Failed, Blocked by anti-spam) and source, with the error detail of each failed send.
- In a flow, add a **Notification** node, select channels and a template; each template variable becomes an input row of the node.

## Recipients read from the message
- An e-mail channel can let flows send to **one address read from the message** (for example the compliance contact of a partner): tick *Recipients from the flow* in the channel. Only owners and admins can turn it on (*notify-dynamic-recipients-admin*).
- In the Notification node, *Recipient from the message* names the field holding the address (and *Copy from the message* one address in copy). The message always uses the node's template, never a body taken from the message.
- Limits: 3 messages a day per recipient and 200 a day per channel; a list of addresses is refused. Deploying a node reading its recipient from the message through a channel that does not allow it is refused (*notify-dynamic-recipients-off*). The delivery journal keeps a fingerprint of the address, never the address.

## Signed webhooks
- A Webhook channel can sign its messages instead of sending the secret: *Signature* = **hmac**. Each request then carries `x-pnex-timestamp` and `x-pnex-signature` (`sha256=` HMAC of "timestamp.body" with the secret value). The receiving application recomputes it and refuses a timestamp older than 5 minutes.

## Good to know

- The Notification node has a mandatory boolean **trigger** row: the save is refused while it is not wired, and the message is sent only while the trigger is true.
- An optional anti-spam limit (max messages per time window) blocks bursts; blocked sends appear in the journal.
- Templates and channels are resolved at deploy: after changing or deleting a channel or template used by a flow, redeploy the flow (the editor warns about stale references).
- Channel and template names are unique in the organization.
- Viewers see the page read-only.
- The assistant can read, preview and write message templates, never channels, and it never sends a message.
