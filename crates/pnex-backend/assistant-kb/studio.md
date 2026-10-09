---
id: studio
title: Studio (3D tours)
kind: feature
pages: /studio
nodes: 
err_codes: tour-write-forbidden, tour-publish-forbidden, tour-share-forbidden, tour-version-conflict, tour-not-published
tools: 
tags: studio, tour, visite virtuelle, virtual tour, 360, panorama, floor plan, plan, hotspot, scene, share, public link, lien public
---
Studio (Data › Studio Tour) builds 3D virtual tours: 360° panoramas from the media library placed as scenes on floor plans, linked by hotspots, versioned, published, and shareable through a public link that needs no login.

## What you can do
- **New tour** (name, description), then **Open** it from the list (columns Tour, Version, Publication, Updated).
- **Floors**: **Add floor** with a name, a **Level (0 = ground)** and the **North (°)**; give each floor a **Floor plan** (**Import / pick a plan…**) with its width, height and **Scale (m/px)**.
- **Add scene** on the plan and choose its **Panorama (media library)**; set **Label**, **Yaw**, **Pitch**, **FOV**, and **Set as start scene**.
- Link scenes: **Link mode** (click the source scene, then the target) or **Add** in the scene's **Links**. Links are **Same floor** or **Across floors**; new scenes are linked to the nearest one automatically. Give each link a **Hotspot label**.
- **Preview** the tour, with its published annotations (read-only: annotations are edited in Data › Annotations).
- **Save** creates a version. **Publish latest** or publish an older version from **History**; **Unpublish** to withdraw it.
- The tour list filters by label (**Filter by label (site:serre)**); the **Labels** button of the editor toolbar edits the tour's labels.
- **Public link**: **Copy link** to share the published version, **Revoke link** to cut access.

## Good to know
- New panoramas and plans are uploaded on the **Media** page; Studio only picks from the library.
- A public link requires a published version ("Publish the tour first").
- Deleting a tour deletes all its versions; the media stay in the library.
- A floor that still has scenes cannot be removed: delete its scenes first.
- "Stale version": the tour was saved elsewhere; reload (discards your local edits) or overwrite.
- A tour can be attached to a POI on the **Map** (tab **3D tour**).
- **Labels** are the single organization-wide labelling mechanism: chips `name` or `name:value` (lowercase letters, digits, `_`, `-`), the same on devices, media, dashboards, flows, 3D tours, map POIs and their folders. Labels set on a folder are inherited by everything stored in it.
- Only Owners, Admins and Members can edit, publish and share tours.
