---
id: events
title: Events
kind: feature
pages: /events
nodes: event_log, vision_detect
err_codes: events-unavailable, event-stream-invalid
tools: 
tags: events, événements, journal, log, history, historique, audit, search, json
---
Events (Automation › Events) searches the JSON events written by your flows with the **Event log** node, stored in OpenObserve. Use it as a searchable history: detections, door openings, alarms, anything a flow decides to record.

## What you can do
- Pick the **Stream** (each Event log node writes to a named stream, "events" by default).
- Filter by level (**All levels**, **Debug**, **Info**, **Warning**, **Error**) and **Period**: **Last hour**, **Last 24 hours**, **Last 7 days** or **Custom period** (**From** / **To**).
- **Search in events…** for any text.
- Each row shows **Time**, **Level**, **Message**, **Topic**, **Flow** and **Node**; expand it to read the payload as a list of fields (nested fields shown as `parent.child`, lists joined).

## Good to know
- Nothing appears until a deployed flow contains an **Event log** node that receives messages. Typical chain: **Object detection** → **Event log** to keep who/what was seen.
- The stream list also shows `ev_controls`, the journal of commands sent from dashboards and annotations (who, value, which surface), once a control has been operated.
- "No event yet" can also mean OpenObserve is not configured on this server.
- "No event in this period": widen the period or relax the filters.
