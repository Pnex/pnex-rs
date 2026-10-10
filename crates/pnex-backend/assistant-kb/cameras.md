---
id: cameras
title: Cameras
kind: feature
pages: /cameras
nodes: camera_source, video_record, vision_detect, event_log, pnex_notify
err_codes: camera-write-forbidden, camera-unknown, camera-no-frame, camera-offline, camera-range-invalid, camera-export-too-large, camera-export-empty, camera-already-recorded, camera-stream-unknown, video-segment-invalid, video-store-unavailable
tools: 
tags: camera, caméra, esp32-cam, video, vidéo, live, stream, recording, enregistrement, playback, detection, détection, surveillance, rtsp, ip camera, caméra ip
---
Cameras (Data › Cameras) lists every device that announced a camera (for example an ESP32-CAM on the generic camera firmware) with its streaming status, capture settings and last frame. From there you watch the live view, change capture settings and play back recordings. Recording and object detection are done by flows.

## What you can do
- **Live** opens the stream in the browser; no flow is needed and nothing is stored.
- **Settings**: **Resolution**, **JPEG quality**, **Frames per second**, **Vertical flip**, **Horizontal mirror** and **Capture mode**: **On demand** (streams only while someone watches, sleeps 30 s after) or **Continuous** (streams as long as it is connected).
- **Flash**: switch the camera's flash LED when the board has one.
- **Recordings**: one day at a time (**Previous day** / **Next day**), a timeline of recorded periods, a player that chains segments, **Download** the hour under the playhead as one AVI file, **Delete this day**. **Detection layers** draws the boxes recorded by detection nodes over the video.

## Recording and detection (in a flow)
In **Automation › Flows**: the **Camera source** node reads a camera device (source **Camera (device)**) or the video track of an IP camera declared in **Data › Audio streams** (source **Media stream**, kind `rtsp` or another stream with tracks Video). **Camera source** → **Video recording** stores video; **Camera source** → **Object detection** (a model from **Data › Models**) detects objects, optionally kept as a camera layer (**Record the detections as a camera layer**). Chain detection to **Notification** or **Event log**. Deploy the flow.

## Good to know
- No **Video recording** node = nothing is stored. Recordings show "No recording" until such a flow is deployed.
- One camera has exactly one recorder in the organization; a second Video recording on the same camera is refused.
- A deployed flow with a Camera source keeps its camera awake even in On demand; Continuous is the right mode for round-the-clock recording.
- "Connected, waiting for frames" or "No frame received yet": check the camera's power and Wi-Fi.
- An ESP32-CAM in VGA at 5 fps records roughly 360 MB per hour.
- Only Owners, Admins and Members change settings or delete recordings.
- IP cameras (RTSP) are not listed here: they are streams of **Data › Audio streams** with a video track. Their recordings are under **Recordings** on the stream's row; detection layers are kept for camera devices only.
- In **Video recording**, **Label** names the recording (default: the camera or stream name).
