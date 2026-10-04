---
id: orgs
title: Organizations
kind: feature
pages: /orgs
nodes: 
err_codes: org-name-duplicate, org-delete-forbidden, org-not-empty
tools: 
tags: organization, organisation, org, tenant, workspace, team, switch org, active org, create org
---
The Organizations page lists every organization you belong to, with your role in each, lets you create a new one and choose which one is active. Everything you see in PneX (devices, flows, dashboards, secrets, telemetry) belongs to the active organization.

## What you can do
- **Create an organization**: type a name in "New organization name…" and click **Create**. You become its Owner and it becomes the active organization.
- **Switch organization**: click **Set active** on a row. The active one carries the green **Active** badge; all other pages then show that organization's data.
- **Manage**: opens the organization detail (rename, members, LLM providers, delete). The same detail for the active organization is reachable directly at Organizations → detail (/orgs/current).
- **Refresh** the list with the refresh button in the header.

Columns: **Name**, **Your role** (Owner, Admin, Member or Viewer), **Tier** (subscription, "—" when none).

## Good to know
- Every account gets a personal organization at its first sign-in, where the user is Owner. Create more organizations to separate projects or share with a team.
- Organization names must be unique (error "This organization name is already taken.").
- Data never crosses organizations: a device, flow or secret of one organization is invisible from another, even for the same user.
- Deleting an organization is done from its detail page, by an Owner only, and only once no other member remains (see the organization detail card).
- What each role may do is described in the roles card and in the legend under the member list.
