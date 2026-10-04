---
id: troubleshooting-flow-node-not-running
title: A flow node does not run
kind: troubleshooting
pages: /flows
nodes: inject, debug, display, value
err_codes: flow-no-deployed-version
tools: 
tags: node not running, nothing happens, flow does nothing, not deployed, redeploy, outdated, mon noeud ne s'exécute pas, rien ne se passe
---
A node added or changed in a flow seems to do nothing. Most of the time the engine is still running an older deployed version of the flow: saving never deploys.

## Symptom

- A node you just added or edited never produces output; the debug feed or the node's live badge stays silent.
- The flow's status chip reads "Deployed · to redeploy" (amber) instead of "Deployed".
- After a save, the editor showed "vN saved — the running version is still live. Deploy it now?" and you chose **Later**.

## Cause

The runtime executes only the deployed version. **Save Changes** stores a new version without touching what runs, so new or modified nodes stay inactive until you deploy that version. Other common causes:

- the flow is **Stopped** (status chip "Stopped");
- the chain has no trigger: Json Values, Calc and similar nodes only transform incoming messages and need an Inject (interval or cron) or an event source upstream;
- a wire is missing between two nodes.

## Fix (in the UI)

1. Open the flow from **Flows** → **Open**.
2. If the chip says "Unsaved changes", click **Save Changes** first (Deploy is disabled until the current version is saved).
3. Click **Deploy**. The chip turns to "Deployed".
4. If the flow is "Stopped", click **Start** to resume its last deployed version, or **Deploy** to run the latest one.
5. Check that an Inject (or an event source such as Camera source or Control source) feeds the chain, and that every node is wired.
6. Watch the Debug feed or the Display badges: a deploy restarts from a clean feed, so wait for the next trigger.

The assistant cannot deploy or start a flow: these are your actions in the flow editor.
