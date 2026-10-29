---
id: home
title: Home
kind: feature
pages: /
nodes: 
err_codes: 
tools: 
tags: home, accueil, overview, summary, recent, workspaces, quotas, tier, devices online, latest measurements
---
The Home page is the overview of the active organization across every workspace: open cases, pages, the media library, flows and devices, the items you worked on lately, and a shortcut to each workspace.

## What you can do
- Read the summary band: **Open cases**, **Pages**, **Library** (media count), **Deployed flows** (deployed / total) and **Devices online** (online / total). Click a number to open its page.
- Pick up where you left off: **Recent pages** (last edited first), **Open cases** and **Latest flows** with their status. **See all** opens the full list.
- **Workspaces** has one tile per sidebar group: Monitor, Knowledge, Media, Sources and Automation.
- When the organization has devices, **Device status** lists each device with its last-seen time ("never" when it has never connected), with the tier capacities on a SaaS server, and **Latest measurements** shows the newest value per device and metric.
- The page refreshes itself (every 15 s by default); pick another rate with **Refresh every** (1 to 60 s) or refresh immediately.

## Good to know
- Everything shown belongs to the **active organization** (named under the greeting, with its tier): switch organization from the sidebar to see another one.
- A dash (—) means that number could not be read, often because your role does not grant access to that workspace.
- On a self-hosted server the tier shows **Self-hosted**: there is no subscription.
- "Telemetry unavailable (telemetry store not configured or unreachable)" means the measurement store cannot be reached; device statuses still work, only the measurements block is empty.
- "No measurements yet" on a connected generic-firmware device usually means no pin is subscribed: open **Devices**, then the device's **Pins** tab, and pick a read interval.
- For curves rather than last values, use **Monitor › Quick charts**; for a composed screen, use **Dashboards**.
