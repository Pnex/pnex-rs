---
id: troubleshooting-stream-silent
title: Audio stream silent or transcription late
kind: troubleshooting
pages: /streams
nodes: 
err_codes: media-asr-model-invalid
tools: 
tags: stream silent, flux muet, no audio, pas de son, transcription late, retard de transcription, backoff, capture error, unreachable, stalled, model too slow, modèle trop lent
---
An enabled audio stream brings no new audio, or its transcriptions arrive late.

## Symptom

- An in-app notification (and a message on the stream's alert channel) says "Stream X is silent": no audio for 2 minutes.
- On **Audio streams**, the stream shows **Backoff** or **Starting** instead of **Running**, with a short reason (stream unreachable, stopped delivering audio, format not supported, encrypted stream, access secret unreadable…).
- Or the stream runs, but the newest transcription is minutes old and segments pile up as **queued**; some end as **skipped (backlog)**.

## Cause

- **Silent**: the source is down or moved, it refuses the access secret, its format is not supported, or the server cannot reach it. A stream set to a **capture worker** stays idle when the platform runs no capture worker.
- **Late**: the transcription model is too slow for the number of streams (each model shows how many real-time streams it sustains), or the transcription queue is shared with long jobs. Segments older than the platform's maximum lag are skipped on purpose so that the text keeps up with live.

## Fix (in the UI)

1. Open **Audio streams**, read the reason under the stream, then **Edit** it: check the **Stream URL** (open it in a browser), the **Kind** (`hls` for an `.m3u8` playlist, `icecast` for a continuous stream) and the **Access secret**.
2. If the stream is captured by a **capture worker**, switch **Capture location** to **This server** unless the platform confirmed a worker runs.
3. For late transcriptions, open the **Models and profiles** tab: pick a model whose sustained streams exceed your enabled streams, set it in the profile, or disable some streams.
4. Open **Segments** on the stream to see which slices failed and why; **Retry failed segments** sends back those whose audio is still kept.

Capture restarts by itself with a growing delay; one alert is sent per silence and the alert re-arms once audio is back.
