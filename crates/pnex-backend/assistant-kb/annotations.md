---
id: annotations
title: Annotations
kind: feature
pages: /annotations
nodes: control_source
err_codes: annot-write-forbidden, annot-publish-forbidden, annot-version-conflict
tools: 
tags: annotations, annotation set, layer, couche, panorama, 360, photo, floor plan, splat, 3D, tour, overlay, label, pastille, card, mini chart, gauge, reading, live value
---
Annotations places live information on top of photos, floor plans, 360° panoramas, 3D splats and 360 tours: device values, pin states, status, notes, control buttons and readings shown as small dashboard cards. Annotations are grouped in versioned **annotation sets** (layers) that you publish to make them visible in the viewers. They are edited only on this page; the Map and Studio show them read-only, and the media Library shows the raw media without them.

## What you can do
- **New annotation set**: give a name (e.g. "Machine room — ground floor") and choose what it is attached to: a media (photo, floor plan, 360 panorama or 3D splat) or a whole **360 tour**, then **Create**.
- Filter the list by **Media**, search, and see each set's **Status** (published or draft).
- In the editor, use **Place** and click the image (on a splat: click the object, the point lands on its surface) to add an annotation, then set its **Target type** in the **Inspector**: **Device**, **Pin**, **Status**, **Note**, **Control** or **Reading**. A reading picks its **Mini chart**: **Value**, **Sparkline**, **Gauge** (with **Minimum** and **Maximum**) or **Indicator**. Adjust **Label**, **Text** and **Color**; drag a marker to move it.
- A set attached to a 360 tour opens the real tour: navigate from scene to scene and place annotations on each one.
- Sets stay in their context: a set attached to a panorama shows on that panorama alone; a set attached to a tour shows only inside that tour, even when the tour uses the same panorama.
- **Save** creates a version; **Publish** (at the top of the panel) makes the latest saved version visible to viewers; the panel warns while saved changes are not published. **History** can load, publish an earlier version or **Unpublish**.
- **Preview** shows the published view, exactly as the Map and tours display it. The media Library always shows the raw media, without annotations.
- Viewers switch between **Hide**, **Dots** (click a dot to open its card) and **Cards** (controls and readings shown next to their dot, following the view).

## Good to know
- A **Control** item behaves like a dashboard switch: once the set is saved it becomes a source of the **Control source** node of flows. It only stores a value; a deployed flow acts on devices.
- The same control can appear on a dashboard and an annotation, with one shared state.
- "Target not found (dead reference)" means the device, pin or control was deleted.
- If the media or tour of a set was deleted, the set shows "(missing target)": recreate it on another media.
- Nothing shows on the Map or in a tour? The set is probably saved but not published: open it and **Publish**.
- On a 3D splat, a marker stays visible through the object (no hiding behind surfaces).
- Conflicting saves ("Save conflict") offer **Reload** or **Overwrite**.
- Only Owners, Admins and Members can edit and publish annotation sets.
