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

- **New flow** creates and opens a flow; **Open** edits one, **Delete** removes it with all its versions. Filter by status (Draft, Deployed, Stopped, Error) or by label (**Filter by label**, e.g. `site:serre`).
- In the editor, click **+** (Add a node) to pick a node from the palette (Triggers, Devices, Data & math, Code, Storage & series, AI & predictive, Integrations, Debug…), drag from an output port to the next node to wire it, and configure the selected node in the inspector.
- **Save** stores a new version. **Deploy** runs the saved version; the editor proposes "Deploy vN?" right after a save.
- **Stop** pauses a deployed flow, **Start** resumes its last deployed version, **Restart** reloads the engine as-is and clears what the nodes remember (notification anti-spam, values waiting for a trigger); deploying the same version again keeps it.
- **History** lists versions: **Load** one into the editor, or **Deploy** an earlier version (rollback, no new version).
- **Debug** opens the debug feed (last 100 entries, 5-minute window) when the server enables debug tools; otherwise the full feed is disabled (run mode), but each Debug and Display node still shows the last message it received.

## Good to know

- Saving never deploys. The status chip "Deployed · to redeploy" means the running version is older than the saved one.
- Invalid graphs are refused at save with an "Invalid graph" banner listing each problem (e.g. an Inject without interval or cron, a Notification without its trigger wire).
- An output pin has one writer: deploy is refused if another deployed flow already writes that pin.
- Ports and wires are coloured by what travels on them (number, true/false, text, object, video, audio, detections), and each wire shows that type as a small label.
- To switch an output on or off, check its pin in **Device (write)** and set **Value** to **Always on** or **Always off** (or **Fixed** with a duty for PWM): any message reaching that pin, such as an Inject, then writes that value. **From the message** writes the value the message carries.
- An amber badge under a node ("Nothing written", "Message rejected") counts the messages it refused; click it to read the reason, for example a device that is offline or a value the pin does not accept.
- If another user saved a newer version, choose **Reload from server** or **Overwrite with my version**.
- The assistant can draft flows and edit only stopped flows; deploying, stopping and deleting stay human actions in the editor.
- **Labels** are a plain search aid shared by the whole organization: chips `name` or `name:value` (lowercase letters, digits, `_`, `-`), on every object you can list (devices, media, dashboards, flows, 3D tours, sites and their folders, pages, cases and objects, functions, custom firmware, controls, streams, vision models, notification channels and templates, annotation layers). A label belongs to the object that carries it: it is never inherited from a folder. Filter any list with **Filter by label**, or type `#value` (or `#name:value`) in the search box of the sidebar. In the flow editor, the **Labels** button of the toolbar edits the flow's labels (type `name:value`, Enter, then **Save**).
- Viewers see flows read-only.
