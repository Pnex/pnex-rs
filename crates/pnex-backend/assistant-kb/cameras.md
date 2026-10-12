---
id: cameras
title: Cameras
kind: feature
pages: /cameras
nodes: camera_source, video_record, vision_detect, vision_track, vision_zone, vision_caption, media_fuse, text_decide, media_source, event_log, pnex_notify
err_codes: camera-write-forbidden, camera-unknown, camera-no-frame, camera-offline, camera-range-invalid, camera-export-too-large, camera-export-empty, camera-already-recorded, camera-stream-unknown, video-segment-invalid, video-store-unavailable
tools: 
tags: camera, caméra, esp32-cam, video, vidéo, live, stream, recording, enregistrement, playback, detection, détection, surveillance, rtsp, ip camera, caméra ip, tracking, suivi, zone, comptage, counting, présence, presence, description, caption, légende, vlm, what happens, ce qui se passe, résumé vidéo, video summary, decision, décision, laya, incident, classification, probability, probabilité
---
Cameras (Data › Cameras) lists every device that announced a camera (for example an ESP32-CAM on the generic camera firmware) with its streaming status, capture settings and last frame. From there you watch the live view, change capture settings and play back recordings. Recording and object detection are done by flows.

## What you can do
- **Live** opens the stream in the browser; no flow is needed and nothing is stored.
- **Settings**: **Resolution**, **JPEG quality**, **Frames per second**, **Vertical flip**, **Horizontal mirror** and **Capture mode**: **On demand** (streams only while someone watches, sleeps 30 s after) or **Continuous** (streams as long as it is connected).
- **Flash**: switch the camera's flash LED when the board has one.
- **Recordings**: one day at a time (**Previous day** / **Next day**), a timeline of recorded periods, a player that chains segments, **Download** the hour under the playhead as one AVI file, **Delete this day**. **Detection layers** draws the boxes recorded by detection nodes over the video.

## Recording and detection (in a flow)
In **Automation › Flows**: the **Camera source** node reads a camera device (source **Camera (device)**) or the video track of an IP camera declared in **Data › Audio streams** (source **Media stream**, kind `rtsp` or another stream with tracks Video). **Camera source** → **Video recording** stores video; **Camera source** → **Object detection** (a model from **Data › Models**) detects objects, optionally kept as a camera layer (**Record the detections as a camera layer**). Chain detection to **Notification** or **Event log**. Deploy the flow.

## Tracking and zones (in a flow)
- **Object detection** → **Object tracking** gives each object a stable id while it stays in view. Output 1 lists the current tracks, output 2 emits **appear** / **disappear** events (with the duration and average confidence) — wire it to **Event log** to search them on the Events page.
- **Object tracking** output 1 → **Image zones**: in the node panel, pick a camera to show behind the drawing, click on the image to place the points of a zone, **Close the zone**, name it. Each object entering or leaving a zone emits an **enter** / **exit** event with the number of objects now in the zone (counting, occupancy). **Min presence** delays the enter event ("a person in the zone for more than 10 s"); **Labels counted** keeps only some classes.
- For an alert, wire **Image zones** → **Notification** (filter on the event in between if only entries matter), or → **Event log** and create the alert on the Events page. Neither node ever drives a device.
- Tracking at 1 frame per second: keep **Lost after** at 3 s or more and lower **Min overlap** if fast objects get a new id at every frame. Set the detection **Min score** below the tracking **Score to open a track** so weak detections keep a track alive.
- Zones apply to every camera reaching the node: use one Image zones node per camera viewpoint.

## Describing what happens (in a flow)
- **Image description** asks the organization's AI model (Organizations → organization detail → LLM providers; the default provider must accept images) to describe a frame in words. Feed it with **Camera source**, **Object detection** or **Object tracking**; **Describe the best detection only** crops the most confident object. The image leaves PneX for that provider: pick a local model if images must stay on site.
- **Video description** collects, over a window (30 s by default), transcripts from **Media source**, detection, tracking and zone events and image descriptions, and writes a short description where every sentence cites its inputs. Detections count as facts, speech as reliable, image descriptions as indicative; a sentence without a source is dropped.
- Typical chain: Camera source → Object detection → Object tracking → Image zones and Image description → Video description → Event log (searchable on the Events page).
- Both nodes share a daily cap of model calls per organization (2000 by default). "AI model call failed" on the node: no LLM provider, the provider refused the image (text-only model), the cap was reached, or the frame expired (keep the node right after the camera chain).
- The descriptions are AI-generated and indicative: show or store them, never let them drive a device.
- **Text decision** answers typed questions about a text (a Video description, a transcript) with a probability per answer — e.g. "incident" or "routine", or an urgency level — without writing any text. The model runs on the PneX server, nothing leaves it. Route with a **Switch** on the best answer and its probability, or put it before Video description so the AI model only describes the windows that matter. Out of the box the probabilities are a ranking, not a calibrated truth: check them on real examples before alerting on a threshold. "AI model call failed" with decide-unavailable: the decision model is not installed on this server (ask the platform administrator).

## Good to know
- No **Video recording** node = nothing is stored. Recordings show "No recording" until such a flow is deployed.
- One camera has exactly one recorder in the organization; a second Video recording on the same camera is refused.
- A deployed flow with a Camera source keeps its camera awake even in On demand; Continuous is the right mode for round-the-clock recording.
- "Connected, waiting for frames" or "No frame received yet": check the camera's power and Wi-Fi.
- An ESP32-CAM in VGA at 5 fps records roughly 360 MB per hour.
- Only Owners, Admins and Members change settings or delete recordings.
- IP cameras (RTSP) are not listed here: they are streams of **Data › Audio streams** with a video track. Their recordings are under **Recordings** on the stream's row; detection layers are kept for camera devices only.
- In **Video recording**, **Label** names the recording (default: the camera or stream name).
