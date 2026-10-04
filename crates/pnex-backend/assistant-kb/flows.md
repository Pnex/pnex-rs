---
id: flows
title: Flows
kind: feature
pages: /flows
nodes: inject, device_read, device_write, calc, metric, pnex_notify, debug, display
err_codes: pin-already-assigned, flow-stale-version, flow-debug-disabled, flow-deploy-forbidden, flow-start-not-stopped, flow-no-deployed-version, ai-flow-running
tools: 
tags: flow, flows, etl, automation, node-red, deploy, pipeline, nodes, debug, automatisation, déployer
---
The Flows page (Automation menu) lists the organization's flows: visual pipelines of nodes wired on a canvas (Node-RED style) that read devices, compute, store metrics, notify and drive outputs. Only a flow deployed by a human acts on devices.

## What you can do

- **New flow** creates and opens a flow; **Open** edits one, **Delete** removes it with all its versions. Filter by status: Draft, Deployed, Stopped, Error.
- In the editor, click **+** (Add a node) to pick a node from the palette (Triggers, Devices, Data & math, Code, Storage & series, AI & predictive, Integrations, Debug…), drag from an output port to the next node to wire it, and configure the selected node in the inspector.
- **Save Changes** stores a new version. **Deploy** runs the saved version; the editor proposes "Deploy vN?" right after a save.
- **Stop** pauses a deployed flow, **Start** resumes its last deployed version, **Restart** reloads the engine as-is.
- **History** lists versions: **Load** one into the editor, or **Deploy** an earlier version (rollback, no new version).
- **Debug** opens the debug feed (last 100 entries, 5-minute window) when the server enables debug tools; otherwise the full feed is disabled (run mode), but each Debug and Display node still shows the last message it received.

## Good to know

- Saving never deploys. The status chip "Deployed · to redeploy" means the running version is older than the saved one.
- Invalid graphs are refused at save with an "Invalid graph" banner listing each problem (e.g. an Inject without interval or cron, a Notification without its trigger wire).
- An output pin has one writer: deploy is refused if another deployed flow already writes that pin.
- If another user saved a newer version, choose **Reload from server** or **Overwrite with my version**.
- The assistant can draft flows and edit only stopped flows; deploying, stopping and deleting stay human actions in the editor.
- Viewers see flows read-only.
