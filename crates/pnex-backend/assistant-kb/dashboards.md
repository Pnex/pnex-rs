---
id: dashboards
title: Dashboards
kind: feature
pages: /dashboards
nodes: control_source, device_write, memory_write, metric, weather
err_codes: dashboard-write-forbidden, dashboard-version-conflict, viz-write-forbidden, control-rate-limited, control-value-invalid
tools: 
tags: dashboard, range bars, barres, per range, par plage, time slice, tranche horaire, aggregation, agrégation, tableau de bord, scada, synoptic, widget, gauge, jauge, mobile, home, maison, domotique, card, carte, template, modèle, room, pièce, weather, météo, switch, interrupteur
---
Dashboards (Visualization › Dashboards) are live screens built from widgets bound to telemetry, flow memory and controls. A **Desktop** dashboard is a free canvas (SCADA synoptic, process symbols, wires); a **Mobile** dashboard is a thumb-friendly stack of cards (home-automation tiles, rooms, pages). A dashboard only reads data and writes *controls*: it never commands a device directly.

## What you can do
- **New dashboard**: name it, choose the format (**Desktop** or **Mobile**, fixed once created) and, for Mobile, **Start from** a template: **Empty**, **Home**, **Energy**, **Security**, **Garden & pool**. Template cards show "To configure" until you pick their source.
- Switch between **Live** and **Edit**. Add widgets from the palette, groups **Start**, **Controls**, **Values & states**, **Charts**, **Home**, **Sensors**, **Energy**, **Industrial**. In the **Inspector**, pick the **Source** (a device, a flow, or **Memory (flows)**) and the **Metric**.
- **Appearance**: card icon, **Colour thresholds**, **States** (value → label, colour, icon), **Stale after (seconds)**.
- Home cards: Light, Thermostat, Fan, Shutter / blind, Gate / garage, Lock, Alarm, Scene, Watering, Door/window/detector, Temperature & humidity, Air quality, Live power, Meter, Energy flow, Appliance, Clock, Weather.
- Mobile layout: **+ Page**, **+ Add a section**, section style **Cards**, **Room (summary + all off)** or **Chips (header summary)**; **Show only when the value is…** hides a card conditionally; **Details and history** shows the last 24 hours.
- **From a device** proposes widgets for a device: outputs become **+ Switch**, **+ Slider** or **+ Light**, metrics become readings or a **Suggested home card**.
- Desktop: **Process symbols** library, **Wire** tool, undo/redo, **Library** of saved templates (**Save as template**).
- The list filters by label (**Filter by label (site:serre)**); the **Labels** button of the editor toolbar edits the dashboard's labels.
- **Save** creates a version; **History** can **Restore this version**. In Live, choose **Refresh every** 1 to 60 s.

## Good to know
- Switches, sliders, buttons, lists, steppers, commands and colours you place become org **controls** once the dashboard is saved. Operating them only stores a value: a deployed flow with a **Control source** node → **Device (write)** makes something happen. Use **Create the flow…** in the inspector (save first). A card marked **No effect** has no deployed flow listening.
- **All off** in a room writes the "off" value of each of its power controls, through the same flows.
- The Weather card reads memory keys written by a flow (**Weather** node → **Memory write**).
- **Bars per range / slice** (Charts group): one row per bucket with its label, start–end, a bar and the value of the series over the bucket. In the Inspector pick the series, the **Aggregation** (Sum, Average, Maximum, Minimum, or **Increase (counter)** for counters such as mentions), the **Window** (last 24 hours or last 7 days) and the **Buckets**: **Time ranges of a stream** (one row per show or segment of the stream, see the ranges card) or **Time slices** (bounds such as `00:00, 06:00, 09:00`, repeated over the days of the window and combined per slice; timezone `Europe/Paris` by default). Hover a range row: it says whether its time is the realigned (actual) or the announced (planned) one. The bars refresh every 15 seconds; with no data a row shows —.
- Series without a device (audio stream supervision, or a **metric** node with **Series labels** such as `stream` = `msg.topic`) appear in the **Source** list under **Labelled series**, shown as `metric · stream=inter`: the widget reads (and sums) the series matching those labels.
- "Stale version" on save: **Reload** or **Overwrite**.
- **Labels** are the single organization-wide labelling mechanism: chips `name` or `name:value` (lowercase letters, digits, `_`, `-`), the same on devices, media, dashboards, flows, 3D tours, map POIs and their folders. Labels set on a folder are inherited by everything stored in it.
- Viewers see dashboards read-only with controls disabled.
