---
id: calendar
title: Calendar — maintenance plans and their occurrences
kind: feature
pages: /calendar
nodes: range_upsert
err_codes: access-denied, time-range-not-found
tools: list_calendar, propose_occurrence_change, create_object, update_object
tags: calendar, calendrier, agenda, planning, planification, maintenance plan, plan de maintenance, préventif, preventive, gmao, cmms, recurrence, récurrence, periodicity, périodicité, every, tous les, floating, flottant, fixed, fixe, occurrence, late, en retard, overdue, done, réalisé, cancel, annuler, move, déplacer, reschedule, replanifier, work order, ordre de travail, intervention
---
The Calendar page (Knowledge › Calendar) shows every time range of the organization by month, and in particular the **occurrences of maintenance plans**.

## Planning a recurring job
1. Install the *Augmented maintenance* pack (Ontology › Packs). It adds the **Maintenance plan** type.
2. Ontology › Objects › *Maintenance plan* › create one. In **Schedule**, set:
   - **Every N** days, weeks, months or years;
   - the first date;
   - an optional end date;
   - the length of one job in days;
   - how many days before to **prepare** it: the occurrence turns *to prepare* (amber) that many days ahead.
   
   The form previews the next dates.
3. Tick **Count from the last completion** for a *floating* plan: a job done late pushes the next one back. Unticked (*fixed*), the dates follow the plan whatever happens.
4. Optionally link the plan to what it maintains (Links › *maintains*): a pump, a machine, or any device on its own.

The occurrences of the next 90 days appear by themselves, in the Calendar and in the *Time* tab of the plan and of every object linked to it (the pump shows its next greasing). They follow the plan when you change it, and disappear when you archive it.

## Working on an occurrence
Click an occurrence in the calendar and pick a change:
- **Done**: the day it was done and the time spent. For a floating plan, the next dates are recomputed from this completion.
- **Move**: a new day, for this occurrence only.
- **Cancel**: this occurrence only; the plan is unchanged.

Done, cancelled and moved occurrences are history: changing the plan never rewrites them.

Colours: blue planned, amber **to prepare**, red **late** (its end has passed and it is not done), green done, struck through cancelled.

## With the assistant
Ask for example "what maintenance is late?" or "plan a greasing of pump P12 every 3 months". The assistant reads the calendar, creates or updates plans, and **proposes** a change to an occurrence (done, move, cancel): a *Review* button under its answer opens the same dialog, and you apply it.

## Good to know
- Viewers read the calendar but change nothing (*access-denied*).
- Flows can add ranges too (node *Time range writer*), for example a corrective job opened by an anomaly; they show in the same calendar.
- The calendar never starts or stops a machine. To act at the start of a job, run the type's action (for example *Start maintenance* on a pump): a deployed flow decides what happens.
