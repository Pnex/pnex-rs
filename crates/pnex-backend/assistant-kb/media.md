---
id: media
title: Media library
kind: feature
pages: /media
nodes: 
err_codes: media-write-forbidden, media-last-version, media-file-too-large, media-format-unsupported
tools: 
tags: media, médias, library, bibliothèque, video, vidéo, mp4, photo, panorama, 360, splat, floor plan, plan, upload, version, take 360, onnx, glb, 3d model, modèle 3d
---
The Media library (Media › Library) stores the organization's versioned files: photos, 360° panoramas, 3D splats, floor plans, videos (MP4, MOV), 3D models (GLB, shown in a 3D viewer with their animations and skeleton) and vision model files. They are reused by object galleries (Ontology › object › Gallery), 3D tours (Studio), annotations, map POIs, the Vision Lab (frames of a video) and object detection models.

## What you can do
- **Upload** a file: choose a **Name** and a **Type** (auto-detected when "auto": a 360 panorama is recognised automatically).
- Filter by type (**All**, **Photo**, **360° Panorama**, **3D Splat**, **Floor plan**, **Vision model**), search by name, or filter by label.
- Open a media to preview it as is (interactive 360 viewer for panoramas, 3D splat viewer; annotations are not shown here, see the Annotations page); see **Versions**, **Add a version**, **View** or **Restore** an older one, **Download**, **Delete asset**.
- In the installed apps (not the browser): **Take a photo**; on Android also **Take 360** to capture a panorama with the phone, stitched on the device for a quick preview; the server then builds an HD version that replaces the preview automatically.

## Good to know
- Versions are kept: adding a version never overwrites the previous one. The last remaining version cannot be deleted alone; delete the whole asset instead.
- Files above the server's size limit are refused ("File is too large").
- Deleting a media used by a tour, an annotation layer or a POI leaves a "(deleted)" or "(missing target)" reference there.
- Previews of some formats are only available in the web interface.
- **Labels** are a plain search aid shared by the whole organization: chips `name` or `name:value` (lowercase letters, digits, `_`, `-`), on every object you can list (devices, media, dashboards, flows, 3D tours, sites and their folders, pages, cases and objects, functions, custom firmware, controls, streams, vision models, notification channels and templates, annotation layers). A label belongs to the object that carries it: it is never inherited from a folder. Filter any list with **Filter by label**, or type `#value` (or `#name:value`) in the search box of the sidebar. Edit them in the **Labels** card of a media.
- Only Owners, Admins and Members can upload, version and delete.
- Only known formats are accepted, and the file's extension must match its content: images (JPEG, PNG, WebP, GIF, AVIF, HEIC), splats (.ply, .splat, .ksplat, .spz), models (.onnx, GGML/GGUF, sherpa-onnx .tar/.tar.gz/.tar.bz2/.zip archives) and documents or tables (PDF, Word, text, Markdown, CSV, Excel, ODS). Anything else, a renamed file or a format that does not fit the chosen kind is refused (*media-format-unsupported*).
