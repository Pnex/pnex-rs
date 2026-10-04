---
id: annotations
title: Annotations
kind: feature
pages: /annotations
nodes: control_source
err_codes: annot-write-forbidden, annot-publish-forbidden, annot-version-conflict
tools: 
tags: annotations, annotation set, layer, couche, panorama, 360, photo, overlay, label, pastille, reading, live value
---
Annotations places live information on top of photos and 360° panoramas: device values, pin states, status, notes, control buttons and readings. Annotations are grouped in versioned **annotation sets** (layers) that you publish to make them visible in the viewers.

## What you can do
- **New annotation set**: give a name (e.g. "Machine room — ground floor") and choose the media (a photo or a 360 panorama), then **Create**.
- Filter the list by **Media**, search, and see each set's **Status** (published or draft).
- In the editor, use **Place** and click the image to add an annotation, then set its **Target type** in the **Inspector**: **Device**, **Pin**, **Status**, **Note**, **Control** or **Reading** (optional **Sparkline** of recent history). Adjust **Label**, **Text** and **Color**.
- **Save** creates a version; **Publish** makes it visible to viewers, **Unpublish** hides it; **History** can load or publish an earlier version.
- A set attached to a 3D tour shows **Open in Studio**: it is edited in the tour editor.

## Good to know
- A **Control** item behaves like a dashboard switch: once the set is saved it becomes a source of the **Control source** node of flows. It only stores a value; a deployed flow acts on devices.
- The same control can appear on a dashboard and an annotation, with one shared state.
- "Target not found (dead reference)" means the device, pin or control was deleted.
- If the media or tour of a set was deleted, the set shows "(missing target)": recreate it on another media.
- Conflicting saves ("Save conflict") offer **Reload** or **Overwrite**.
- Only Owners, Admins and Members can edit and publish annotation sets.
