---
id: access
title: Access (roles, teams, assignments, service accounts)
kind: feature
pages: /access
nodes: 
err_codes: app-not-found, app-version-conflict, access-denied, iam-escalation, iam-role-key-taken, iam-role-not-found, iam-team-name-taken, iam-team-not-found, iam-assignment-duplicate, iam-assignment-not-found, iam-version-conflict, iam-service-account-not-found, iam-service-account-name-taken, iam-access-key-not-found, iam-access-key-limit, access-key-not-allowed
tools: 
tags: access, application, app, scope, change feed, role, custom role, permission, team, assignment, service account, access key, api key, external application, accès, rôle, équipe, attribution, compte de service, clé d'accès
---
The Access page (Settings › Access) decides who can do what in the selected organization. Only Owners and Admins see it and change it; the assistant never changes access.

## Roles
- Every action needs a **permission** `service:verb` (`flows:read`, `flows:deploy`, `dashboards:write`…).
- The four **managed roles** — Viewer, Member, Admin, Owner — are the membership roles of the Organizations page. They cannot be edited; open one with **View** to see its permissions.
- A **custom role** is a list of permissions. In its grid, one click on a permission allows it, a second click denies it, a third clears it. A denial wins over every other role the person holds (useful for "everything except…"). A custom role never includes access management itself.
- You can only create or give a role whose permissions you hold yourself (*iam-escalation* otherwise).

## Teams and assignments
- A **team** groups members to give them a role at once; manage its members with **Members**.
- An **assignment** gives a role to a member, a team or a service account, on top of the membership role, over the whole organization (**Give a role**). **Take back** removes it. Changes apply on the person's next action, no new sign-in needed.
- A member who leaves the organization loses their teams and assignments there.

## Service accounts and access keys
- A **service account** lets an external application (a website server, a script on a server) read data of the organization. It holds nothing until it is given a role, and it only ever reads: it never writes, never manages access, never uses the assistant.
- Open **Access keys** to create a key (expiry within a year, two active keys at most for a replacement without a cut). The key is shown **once**: copy it then, keep it on the application's server, never in a browser page.
- **Revoke** stops a key at once; deleting the service account stops all its keys.

## Applications
- An **application** is an external program built on the organization's data (a site, a business tool). Declare it under **Applications** with a key, the **service account** it signs in with (one application per account) and its **scope**: the object types, link types and media kinds it reads, and the largest media it copies.
- Its **object sets** are named queries it runs (one type of its scope each, links followed only inside the scope); a value "$code" is a parameter it gives when it runs the set.
- The application follows the organization's changes through the change feed and only receives what its scope holds, even if its role would let it read more.
- Deleting an application keeps its service account and keys: delete them under **Service accounts** to cut it off.
