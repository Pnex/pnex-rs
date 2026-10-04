---
id: assistant
title: The PneX assistant
kind: feature
pages: 
nodes: 
err_codes: ai-flow-running, ai-conversation-busy
tools: list_devices, get_device_pins, list_flows, get_flow, list_notifications, query_telemetry, describe_node_types, search_knowledge, read_knowledge, diagnose_device, diagnose_flow, list_dashboards, get_dashboard, validate_dashboard_layout, create_dashboard, update_dashboard, get_notification_template, preview_notification_template, create_notification_template, update_notification_template, list_functions, get_function, validate_function, test_function, create_function, update_function, list_annotation_sets, list_tours, list_pois, list_controls, read_memory, validate_flow_graph, validate_calc_expression, create_flow, update_flow
tags: assistant, function, starlark, javascript, template, ai, llm, chat, conversation, history, privacy, gdpr, rgpd, help, aide, what can you do
---
The assistant (chat button at the bottom right of every page) answers questions about PneX, diagnoses devices and flows, and drafts or edits flows. It never acts on the physical world: only a flow that you deploy yourself drives devices.

## What it can do

- **Read** your organization: devices and their pins (`list_devices`, `get_device_pins`), flows (`list_flows`, `get_flow`), notification channels and templates (`list_notifications`), telemetry series (`query_telemetry`).
- **Explain** PneX: the flow node catalogue (`describe_node_types`) and the knowledge cards (`search_knowledge`, `read_knowledge`).
- **Diagnose**: `diagnose_device` (online state, subscribed pins, last values, firmware, last OTA update) and `diagnose_flow` (saved vs deployed version, engine error, last debug/display messages).
- **Build dashboards**: `list_dashboards`, `get_dashboard`, `validate_dashboard_layout`, `create_dashboard`, `update_dashboard`. A dashboard save is live at once. A widget whose control feeds a deployed flow can only be moved or resized until you stop that flow (*ai-flow-running* lists the flows); a new control widget only declares a control — a flow you deploy makes it act.
- **Notification templates**: read (`get_notification_template`), preview with example values (`preview_notification_template`, never sends), create and rewrite them (`create_notification_template`, `update_notification_template`). Channels stay yours: the assistant never creates, edits or tests a channel and never sends a message.
- **Functions (JavaScript / Starlark)**: list and read them with the flows using them (`list_functions`, `get_function`), check code without running it (`validate_function`), run it in the Test sandbox (`test_function`: no network, no device), create a function or save a new version (`create_function`, `update_function`). A deployed flow keeps the function version it pins until you select the new one and redeploy.
- **Browse** annotation sets, virtual tours and map points of interest (`list_annotation_sets`, `list_tours`, `list_pois`), the controls with their last value and the flows listening to them (`list_controls`), and the shared memory (`read_memory`) — read-only.
- **Write flows as drafts**: it checks a graph (`validate_flow_graph`, `validate_calc_expression`), creates a new flow (`create_flow`, never deployed) or saves a new version of a flow (`update_flow`).

## What it never does

- Deploy, stop or delete a flow; delete anything.
- Send a command to a device, start an OTA update or a flash, write a control or the shared memory.
- Edit a **deployed** flow: it answers *ai-flow-running*. Stop the flow in the flow editor, then ask again; after its change, review and redeploy yourself.
- Overwrite your work: it saves on top of the version it read; if someone saved in between, it reloads and redoes its change.
- Read a secret value: secrets appear by name only.
- Delete a dashboard or write a control value.

Write tools need the owner, admin or member role; a viewer can chat and read.

## Conversations

Open **Conversations** in the chat panel to resume, rename, delete or export (JSON) your conversations, or **Erase all**. A conversation is visible to you only, in the organization where you started it — nobody else, not even an owner, can read it. Conversations are erased automatically after a period of inactivity (180 days by default; see the **Assistant conversations** card on the System page, where an owner or admin can shorten it), and when you leave the organization. One reply at a time per conversation (*ai-conversation-busy* while one is being written).

Your messages are sent to the LLM provider configured by your organization (see the card llm-providers); what the provider keeps is governed by its own terms.
