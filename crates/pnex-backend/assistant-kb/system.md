---
id: system
title: System (telemetry data)
kind: feature
pages: /system
nodes: metric
err_codes: system-data-forbidden, o2-not-configured, o2-delete-failed, o2-time-range-unsupported, o2-time-range-invalid, confirmation-mismatch
tools: 
tags: retention, telemetry, storage, disk, purge, delete data, cleanup, openobserve, quota, measurements, rétention, nettoyage
---
The System page shows the active organization's telemetry storage: how long measurements are kept (retention) and how much space each metric stream takes on disk, and lets Owners and Admins delete data.

## What you can do
- **Data retention**: shows the effective retention in days and where it comes from (Organization specific, Platform default, Subscription or Server default). It is read-only here; platform administrators change it in Platform status (link "Change it in Platform status" when you have that right).
- **Telemetry data (OpenObserve)**: one row per metric stream with **Points**, **Size on disk**, **Period (UTC)** and **Retention**. Use **Search a stream…** to filter.
- Owners and Admins can:
  - **Delete a period** of a stream: start and end in UTC; the range is widened to full hours and the deletion is processed asynchronously.
  - **Delete stream**, or select several rows (**Select all**) and **Delete selected**.
  - **Purge all data**: deletes every metrics stream; you must type the organization name to confirm.

## Good to know
- Deletions are permanent.
- Sizes are on-disk (compressed) sizes. In SaaS, a gauge "Storage used (subscription quota)" appears; above quota, collection continues but you should clean data or upgrade.
- "No data received yet: the OpenObserve space is created with the first measurement." means the organization has not sent any telemetry: connect a device and subscribe a pin (or write a metric from a flow).
- "OpenObserve is not configured on this server." means telemetry storage is not available on this installation; ask the platform administrator.
- Members and Viewers see the page read-only.

## Assistant conversations

The **Assistant conversations** card shows how long conversations without a new message are kept before automatic erasure. An owner or admin can shorten it for the organization (never beyond the platform value) or click **Follow the platform**.
