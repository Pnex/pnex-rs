---
id: troubleshooting-access-denied
title: Action refused, access denied
kind: troubleshooting
pages: /orgs
nodes: 
err_codes: access-denied, org-not-member
tools: 
tags: access denied, permission, forbidden, 403, refused, read-only, viewer, role, accès refusé, permission, droits, refusé, lecteur
---
An action is refused with "Access denied: your role does not allow … on …", followed by the permission in brackets. Your role in the selected organization does not include that permission.

## Symptom

- A button, a save or a deploy answers "Access denied" and names a permission such as `flows:write`, `devices:operate` or `secrets:write`.
- The assistant answers that it cannot do the change for you, with the same permission.

## Cause

Every action in PNeX needs a permission of the form `service:verb` (`flows:read`, `flows:deploy`, `dashboards:write`…). Your role in the organization — Viewer, Member, Admin or Owner — holds a fixed set of them: a Viewer only reads, a Member edits and deploys content, Admin and Owner also govern the organization (members, vault, providers, data deletion). The refusal is about the organization selected in the top bar: the same person can be Owner of their personal organization and Viewer in a shared one.

## Fix (in the UI)

1. Check the organization selected in the top bar: switch to the one where you have the right role if the change belongs there.
2. Open **Organizations** and look at your role in that organization (Manage → Members).
3. Ask an Owner or Admin of the organization to give you a role that holds the permission: Member for content changes, Admin for governance. Only an Owner can grant Owner.
