---
id: org-detail
title: Organization detail
kind: feature
pages: /orgs/current, /orgs
nodes: 
err_codes: org-rename-forbidden, org-delete-forbidden, org-not-empty, org-member-add-forbidden, org-owner-grant-forbidden, org-member-unknown-user, org-member-duplicate, org-member-role-forbidden, org-owner-edit-forbidden, org-last-owner, org-member-remove-forbidden, org-owner-remove-forbidden, org-name-duplicate
tools: 
tags: members, invite, add member, team, role, rename organization, delete organization, membres, inviter, owner, admin, viewer
---
The organization detail page (Organizations → **Manage**, or directly the active organization) shows one organization: its members and their roles, its LLM providers for the assistant, and the rename and delete actions.

## What you can do
- **Rename** (Owner or Admin): type the new name and click **Rename**.
- **Add member** (Owner or Admin): enter the person's email, pick a **Role** (Viewer is preselected, least privilege) and click **Add member**. The person must have signed in to PneX at least once, otherwise the server answers "This user has never signed in, so they cannot be added yet."
- **Change a role** (Owner or Admin): use the role selector on the member row.
- **Remove member** (Owner or Admin): trash button on the member row.
- **LLM providers**: configure the organization's own LLM for the assistant (see the LLM providers card).
- **Delete** (Owner only): button at the top right, confirmed by a dialog. It is irreversible and deletes the organization's data.

Users with the Member or Viewer role see the member list and role badges, without the editing controls.

## Good to know
- Only an Owner can grant the Owner role, modify another Owner or remove an Owner.
- An organization must keep at least one Owner: the last one cannot be demoted or removed.
- Deletion is refused while other members remain: remove them first.
- A legend under **Members** summarizes the four roles: Owner (everything, incl. deleting the organization and managing owners), Admin (members, secrets, LLM providers, telemetry data, plus all content), Member (edits flows, devices, dashboards and channels; picks existing secrets without seeing them), Viewer (read-only).
- **Back to organizations** returns to the list.
