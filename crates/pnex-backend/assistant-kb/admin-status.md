---
id: admin-status
title: Platform status
kind: feature
pages: /admin/status
nodes: 
err_codes: platform-admin-required, retention-out-of-range, retention-locked-by-plan
tools: 
tags: platform admin, health, status, infrastructure, monitoring, retention, master key, rekey, uptime, disk, database, administrateur plateforme
---
Platform status is the platform administrator's page: the health of every infrastructure component of the PneX installation, the platform-wide telemetry retention settings per organization, and the secrets master-key rotation. It appears in the menu only for platform administrators; anyone else gets "Reserved to platform administrators".

## What you can do
- **Component cards** (auto-refreshed every 30 s, or with the refresh button): Database, Rauthy (identity), OpenObserve (telemetry), Valkey (cache), Object storage, Local storage, Host machine, AI service, Secrets vault. Each shows a status (Operational, Degraded, Down, Not configured), its latency and metrics such as database size, stored points, used memory, disk, CPU cores, load and uptime. The header shows the version and the mode (Self-hosted or SaaS).
- **Organizations**:
  - **Platform default (days)**: retention applied to every organization without a specific value; **Save**, or **Reset** to inherit.
  - Per organization: **Subscription**, effective **Retention**, **Specific value (days)** (Save / Reset) and **Telemetry storage (on disk)**.
- **Master key rotation**: once the operator has installed a new master key on every server, **Re-encrypt the secrets** (then **Re-encrypt now**) rewrites the secrets still encrypted under an older key; the result tells how many were re-encrypted, unreadable, or left.

## Good to know
- Retention must be between 1 and 3650 days. In SaaS mode retention follows the subscription and cannot be changed here.
- Organization members see their effective retention, read-only, on the System page.
- This page is about the installation; the AI service card does not configure any LLM — each organization brings its own in its organization detail.
- Component details are runtime diagnostics shown as the server reports them.
