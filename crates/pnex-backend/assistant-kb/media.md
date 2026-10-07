---
id: media
title: Media library
kind: feature
pages: /media
nodes: 
err_codes: media-write-forbidden, media-last-version, media-file-too-large
tools: 
tags: media, médias, library, bibliothèque, photo, panorama, 360, splat, floor plan, plan, upload, version, take 360, onnx
---
The Media library (Data › Library) stores the organization's versioned files: photos, 360° panoramas, 3D splats, floor plans and vision model files. They are reused by 3D tours (Studio), annotations, map POIs and object detection models.

## What you can do
- **Upload** a file: choose a **Name** and a **Type** (auto-detected when "auto": a 360 panorama is recognised automatically).
- Filter by type (**All**, **Photo**, **360° Panorama**, **3D Splat**, **Floor plan**, **Vision model**), search by name, or filter by label.
- Open a media to preview it as is (interactive 360 viewer for panoramas, 3D splat viewer; annotations are not shown here, see the Annotations page); see **Versions**, **Add a version**, **View** or **Restore** an older one, **Download**, **Delete asset**.
- In the installed apps (not the browser): **Take a photo**; on Android also **Take 360** to capture a panorama with the phone, stitched on the device for a quick preview; the server then builds an HD version that replaces the preview automatically.

## Good to know
- Versions are kept: adding a version never overwrites the previous one. The last remaining version cannot be deleted alone; delete the whole asset instead.
- Files above the server's size limit are refused ("File is too large").
- Deleting a media used by a tour, an annotation set or a POI leaves a "(deleted)" or "(missing target)" reference there.
- Previews of some formats are only available in the web interface.
- Only Owners, Admins and Members can upload, version and delete.
