---
id: cases
title: Cases — investigate a question over objects, links and time
kind: feature
pages: /cases, /cases/:id, /events
nodes:
err_codes: ontology-not-found, ontology-version-conflict, ontology-link-not-allowed, ontology-already-exists, ontology-write-forbidden
tools: query_ontology, get_object, create_object, update_object, open_link, close_link
tags: case, cases, dossier, dossier d'enquête, enquête, investigation, incident, rca, root cause, cause racine, timeline, chronologie, members, membres, evidence, preuve, hypothesis, hypothèse, actor, acteur, who, when, what happened, que s'est-il passé
---
A **case** (sidebar › Cases) gathers everything that matters for one question to resolve — an incident, an investigation, a story — without copying anything: devices, media, sites, pages, flows or any ontology object become **members** of the case, and the case shows them on one timeline.

## What you can do
- **Cases** page: type a title and press Enter to open a case; filter by status (open, concluded, archived).
- **Case workspace**: edit the title, the question to resolve, the status and the time window in place (saved when you leave the field).
- **Members**: pick a role (subject, actor, evidence, context, hypothesis, note), then type an object's name in the search box and press Enter: it joins the case. The × removes it — the membership is closed, not deleted, so the case remembers what it contained and when.
- **Link two members**: choose *From*, the link type, *To*, and optionally *Valid from* to date the link in the past (for example "sensor C88 measures pump P-12 since March 2"). Only link types that admit both objects are offered.
- **Open a case from an event**: in Events or in a notification channel's journal, expand a row and click **Open a case**: the case opens with a window of 2 hours before and after the event, the flow that raised it as subject and the objects linked to that flow as context.
- **Timeline**: every link opened or closed and every change of the members, the ranges (shifts, broadcasts…) set on a member, and the notifications sent by a member flow, oldest first, grouped by day, within the case window or the last 24 h / 7 d / 30 d / 1 year. Click a member to see only its own timeline.
- From any device, media or site page, or any object page: **Add to a case**.

## Good to know
- Times are in UTC.
- A case is an ontology object: it also has an object page (properties, links, graph, provenance).
- If someone saved the case while you were editing, saving answers *ontology-version-conflict*: reload and redo your change.
- "No link type connects these two objects": create one in Ontology › Schema, or install a pack.
- If the change journal is unavailable, the timeline still shows the links.
