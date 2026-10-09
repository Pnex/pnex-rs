---
id: map
title: Map and POIs
kind: feature
pages: /map
nodes: 
err_codes: poi-write-forbidden, poi-link-write-forbidden, poi-position-write-forbidden, poi-device-already-placed, poi-link-duplicate
tools: 
tags: map, carte, poi, point of interest, location, gps, emplacement, site, attach, cluster, géolocalisation
---
The Map (Visualization › Map) places the organization's points of interest (POIs) on a world map. A POI has a label, a pictogram, a free location text and coordinates; devices, media, 3D tours and dashboards are attached to it, so the map is the geographic entry point to everything installed somewhere. **List view** (top of the panel) opens Sites, the same tree without the map.

## What you can do
- **＋ Add POI**, then click on the map to place it; fill **Label**, **Pictogram**, **Location (building, floor, room…)**, **Latitude** and **Longitude**. **Cancel adding** leaves the placing mode.
- Use the side panel: **Search a POI…**, filters **Attached to a device**, **With GPS position** and **Type** (pictogram), **Show all**. **Collapse panel** gives the map more room.
- Click a POI to open its detail: **Edit**, **Recenter**, **Delete**, and the **Attached objects** list (**Open**, **Detach**).
- The POI detail has a **Labels** section for the POI itself; the tag icon of a folder in **Attached objects** edits that folder's labels, inherited by the objects stored in it.
- **＋ Attach an object** opens a picker with tabs **Media**, **3D tour**, **Dashboard** and **Device**.
- **Pin as default preview** chooses which attached object is previewed when the POI opens (**Unpin** to undo).
- The preview of an attached media or tour shows its published annotations: dots, and the control and reading cards (switch **Hide** / **Dots** / **Cards**). Controls can be operated there; annotations are edited in Data › Annotations.

## Good to know
- Markers are grouped into clusters when zoomed out; zoom in to separate them.
- A device sits on **one POI only**. Attaching a device already placed elsewhere asks "Move the device?": confirming moves it (**Move it here**).
- Deleting a POI deletes its links; the attached devices must then be placed again. The media, tours and dashboards themselves are not deleted.
- The **GPS** badge marks a POI whose placed device reports a GPS position.
- **Labels** are the single organization-wide labelling mechanism: chips `name` or `name:value` (lowercase letters, digits, `_`, `-`), the same on devices, media, dashboards, flows, 3D tours, map POIs and their folders. Labels set on a folder are inherited by everything stored in it.
- Viewers can browse the map but cannot create, edit or attach (only Owners, Admins and Members can).
- On a phone the side panel takes most of the screen: collapse it to see the map.
