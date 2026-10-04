---
id: troubleshooting-deploy-skipped-tabs
title: Deploy toast about skipped tabs
kind: troubleshooting
pages: /flows
nodes: 
err_codes: 
tools: 
tags: skipped tabs, unreadable graph, deploy warning, old flow, removed node, onglets ignorés, graphe illisible
---
After a deploy, a toast says "Deployed — N tab(s) from other flows skipped: unreadable graph, rebuild or delete them." Your flow did deploy; the message is about other deployed flows of the organization.

## Symptom

- Every deploy, of any flow, shows the "tab(s) from other flows skipped" toast.
- One or more older flows listed as Deployed on the Flows page no longer do anything.

## Cause

All deployed flows of the organization are loaded together into the flow engine. When a deployed flow's saved graph can no longer be read, typically because it uses a node type that no longer exists in this version of PNeX, it is left out of the engine so that the other flows still deploy. That flow is not running anymore, and the toast repeats on every deploy until it is cleaned up.

## Fix (in the UI)

1. On **Flows**, filter on **Deployed** and look for old flows that should be running but produce nothing (often flows created long ago).
2. Delete the unreadable flow with **Delete** in its row (deleting works even when its graph cannot be read).
3. Recreate it with **New flow**, using the current nodes from the palette (for example Device (read) and Device (write) for device access), then **Save Changes** and **Deploy**.
4. Deploy any flow again: the toast no longer appears once no unreadable flow remains deployed.

Never try to repair such a flow outside the UI. The assistant cannot delete or stop flows: these are your actions.
