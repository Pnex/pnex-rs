---
id: ranges
title: Time ranges (shows, segments, shifts)
kind: feature
pages: 
nodes: range_upsert
err_codes: time-range-not-found, time-range-write-forbidden, time-range-import-too-large, time-range-import-invalid, time-range-scope-unknown
tools: 
tags: ranges, timeline, frise, plages, time range, plage horaire, show, émission, programme, schedule, grille, epg, ics, icalendar, csv, import, planned, actual, prévu, réel, recalage, shift, poste, icy, stream title, metadata, métadonnées
---
A time range is a named interval of a stream (or of the whole organization): a show, a news segment, a work shift, a production batch. Each range has a **planned** time (announced by a schedule) and/or an **actual** time (what really happened). Statistics use the actual time when it exists, otherwise the planned one.

## What you can do
- **Ranges** tab (Audio streams › Ranges): pick a stream or the whole organization and a day (arrows, date picker, Today). A **timeline** above the table draws two bars per range on the hours of the day: the light one is the planned time, the dark one the actual time, so a late start or an early end shows at a glance (hover a row for the gap in minutes). The table shows each range with its planned and actual times, its origin (Schedule, Detected, EPG, Manual, Import) and a **Source** link when the range has one.
- **New range** / **Edit** / **Delete** (writers): label, category, external id, planned start and end, actual start and end, source link. At least one complete pair (start and end), the end after the start.
- **Import a schedule** (writers): a CSV file with a header row (`label` required, plus `external_id`, `planned_start`, `planned_end`, `actual_start`, `actual_end`, `category`, `source_url`; times like `2026-10-12T07:00:00+02:00`; comma or semicolon separated) or an iCalendar `.ics` file (each event's `UID`, `SUMMARY`, `DTSTART`, `DTEND` or `DURATION`). The result says how many ranges were created, updated and rejected, with the line and the reason of each rejected one.
- In a flow, the **Time range writer** node (`range_upsert`) stores each message as a range of the stream picked in the node: for example a schedule fetched every night (Inject → HTTP fetch → Function → Time range writer, origin Schedule), or an announcement recognized in the transcriptions (Media source → Function → Time range writer, origin Detected, with the actual times).

## Good to know
- The **external id** identifies a range in its schedule: importing the same file again, or a flow writing the same id again, updates the range instead of creating a duplicate. Every imported row needs one (CSV `external_id` column, ICS `UID`).
- An empty cell in a CSV leaves the stored value as it is: re-importing a planned schedule never erases actual times written by a flow.
- A file over 1 MiB or 5000 rows is refused (*time-range-import-too-large*); a file that is neither a CSV with a `label` column nor an iCalendar file is refused (*time-range-import-invalid*).
- The source link must start with `http://` or `https://`, otherwise the range is refused.
- Deploying a flow whose Time range writer targets a deleted stream, or a stream of another organization, is refused (*time-range-scope-unknown*): pick the stream again in the node.
- Viewers see the ranges but cannot change them (*time-range-write-forbidden*).
- Radio streams (icecast) that announce the current song or show title in the stream itself are recorded too: each title change is kept as an event of the stream. It is not a range by itself: a flow decides what becomes one.
- Statistics per range: add a **Bars per range / slice** widget to a dashboard (Charts group), pick the series and **Time ranges of a stream** (see the dashboards card).
- Not available yet: TV program guides (EPG from a tuner) and XMLTV schedules.
