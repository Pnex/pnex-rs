---
id: roles
title: Organization roles
kind: feature
pages: 
nodes: 
err_codes: org-owner-grant-forbidden, org-last-owner, secret-write-forbidden, system-data-forbidden, llm-provider-forbidden, platform-admin-required, flow-write-forbidden, flow-deploy-forbidden, device-write-forbidden, dashboard-write-forbidden
tools: 
tags: role, permission, rights, access, owner, admin, member, viewer, read-only, forbidden, 403, droits, rôle, observateur, membre
---
Each member of an organization has one of four roles: Owner, Admin, Member or Viewer. The role decides what the person can change in that organization; it is set per organization in its detail page (Organizations → Manage → Members).

## What each role can do
- **Viewer**: read-only. Sees devices, flows, dashboards, data and secret names, changes nothing.
- **Member**: edits all content — flows (including deploy), devices and pin commands, dashboards, annotations, tours, media, cameras, functions, firmwares, notification channels, edge references, controls — and uses the assistant. In secret fields a Member can only **pick** an existing secret, never type or see a value.
- **Admin**: everything a Member does, plus governance: rename the organization, add/remove members and change their roles, manage the secrets vault, manage LLM providers, delete telemetry data on the System page.
- **Owner**: everything an Admin does, plus deleting the organization and granting, modifying or removing the Owner role.

## Good to know
- A new member is added as Viewer by default (least privilege); only an Owner can grant Owner.
- An organization always keeps at least one Owner.
- Every user is Owner of their personal organization; roles matter when sharing an organization with others.
- **Platform administrator** is separate from organization roles: it gives access to Platform status (infrastructure health, platform retention, master key rotation) for the whole installation.
- When an action is refused for lack of rights, the message names the role required (e.g. "Owner or admin role required…"); ask an Owner or Admin of the organization to change your role.
- A legend of the four roles is shown under the member list.
