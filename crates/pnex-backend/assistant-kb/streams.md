---
id: streams
title: Audio streams
kind: feature
pages: /streams
nodes: media_source, range_upsert, camera_source, video_record
err_codes: asr-test-audio-unsupported, asr-test-failed, media-stream-not-found, media-stream-write-forbidden, media-stream-limit, media-url-has-credentials, media-stream-unreachable, media-asr-model-invalid, media-capture-unsupported, asr-profile-not-found, asr-profile-name-taken, asr-model-not-found, asr-model-name-taken, asr-model-unsupported, media-transcripts-unavailable, media-stream-unknown, camera-stream-unknown
tools: 
tags: streams, flux, radio, audio, icecast, hls, podcast, transcription, transcriptions, asr, speech to text, parole, capture, retention, tdm, parakeet, canary, whisper, sherpa, profile, profil, search, recherche, vad, voice detection, silence, diarization, diarisation, speaker, locuteur, temps de parole, speaking time, rtsp, ip camera, caméra ip, video, vidéo, tracks, pistes, fps
---
Audio streams (Data › Audio streams) lists the radios, live streams and IP cameras the organization captures around the clock. The audio of a stream is cut into short segments, transcribed into timestamped text, and only the text is kept for good; its video, when asked for, feeds the flows' camera nodes.

## What you can do
- **Add a stream**: give it a **Name**, a **Kind** (`icecast` for a continuous radio stream, `hls` for an `.m3u8` playlist, `http_file` for a single file such as a podcast episode, `rtsp` for an IP camera, URL `rtsp://…`) and the **Stream URL**.
- **Tracks**: **Audio** (transcription, the default), **Video** (camera bus) or **Audio and video**. With video, pick the **Frames per second** (1 to 5, default 1): the server samples the video at that rate, scales each frame to at most 1280 px wide and hands it to the **Camera source** flow node (source "Media stream"), which then feeds **Video recording** and **Object detection** like a camera device. Video works with RTSP, HLS and MPEG-TS / MP4 sources in H.264, H.265 or MJPEG.
- **IP camera (RTSP)**: put "user:password" in the access secret, never in the URL. The connection is TCP only and redirects are refused; a camera that only offers UDP or redirects to another address cannot be captured.
- **Access secret**: when the stream needs a header or credentials, put them in the secret (typed or picked from the vault). It is sent only to the host of the URL. Only owners and admins can set it.
- **Transcription profile**: the speech-to-text model used for new segments. A stream with audio cannot be enabled without a profile whose model passed its check; a video-only stream needs none. Audio and video without a profile captures the video only.
- **Capture location**: this server (default), or a capture worker, a separate machine run by the platform. A stream set to a capture worker stays idle until the platform starts one.
- **Alert channel**: when an enabled stream brings no audio for 2 minutes, the organization gets an in-app notification, and a message on this channel when one is picked (email, ntfy, Slack…). One alert per silence; it re-arms once audio is back.
- **Segment length** (10 to 120 s) and **Audio retention**: none (audio deleted once transcribed, the default), a number of days, or kept (only when the organization holds the rights).
- **Test** (writers): the server captures the first 10 seconds of the stream's audio and transcribes them with its profile, nothing stored, to check the URL and the profile before enabling. The test does not check video: a camera without sound reports a capture error there while its video still works once enabled.
- **Enable / Disable** capture, **Edit**, **Delete** (pending segments are erased; transcriptions already written stay in the history).
- **Recordings** (streams with video, every member): the video segments written by a flow Camera source (source "Media stream") → Video recording, newest first, each downloadable as an AVI file. Without such a flow nothing is recorded.
- **Segments** (on each stream, every member): every captured slice with its state (captured, queued, transcribing, transcribed, failed, silence or too late), newest first, filterable by state. **Retry failed segments** sends back to transcription the failed ones whose audio is still kept.
- Under the state of an enabled stream: how long ago the last audio arrived, how old the oldest segment waiting for transcription is, and the share of the last hour that was transcribed.
- **Ranges** tab: the shows and segments of each stream, announced (planned) and realigned (actual), with a planned-vs-actual timeline of the day and CSV / iCalendar import (see the card ranges). Per-show or per-time-slice statistics go on a dashboard (Bars per range / slice widget).
- **Taxonomies** tab: versioned lists of topics that the **Topic classifier** flow node tags transcriptions with (see the card taxonomies).
- **Transcriptions** tab: search the text of every stream (or one), newest first.
- **Models and profiles** tab: **Import a model** (upload a sherpa-onnx `.tar.bz2` archive or a whisper.cpp GGML file, or pick a `model` file from the media library) with its **Model license**; the server reads the family from the files, loads the model and transcribes a French reference clip: the row shows how many real-time streams it sustains and its error rate on the clip. **Check** runs it again; when the platform runs a dedicated transcription worker, the model is also checked there and the row adds one line per worker ("On worker:…: ≈ N real-time streams" or "does not run"). **Test** (on a checked model) transcribes an audio file you drop (MP3, AAC, Ogg/Opus, FLAC, WAV or the sound of an MP4; the first 2 minutes) and shows the text and how long it took. Voice detection and diarization models are imported the same way, as a single `.onnx` file (Silero VAD, pyannote segmentation, speaker embedding such as 3D-Speaker): the row shows their role (Voice detection, Diarization: segmentation, Diarization: speaker embedding); import the speaker embedding model before the segmentation one, which is checked with it. **Test** only exists for transcription models. Then **Add the profile** (name, transcription model, optional **Voice detection**, optional diarization = **segmentation** + **speaker embedding**, language) and pick that profile in the stream.

## Good to know
- The URL never contains a token or a password: such a URL is refused, use the access secret instead.
- Addresses inside the server's own network (localhost, cloud metadata, internal service names) are refused with the same "cannot be reached" message on purpose.
- Moving a stream that holds a secret to another host, scheme or port needs an owner or admin.
- The short name under the stream (its slug) never changes, even when the stream is renamed: it names the stored transcriptions.
- Before capturing a third-party stream, check that the publisher did not opt out of text and data mining (terms, robots.txt, notices) and tick **Text and data mining opt-out checked**; until then the list shows a warning.
- Each organization has a limited number of streams (3 by default, set by the platform).
- A model that does not load (transcription runtime missing on the server, unsupported layout) is kept as **Error** with the reason; once the platform installs the runtime, **Check** it again.
- Large models (Parakeet, Whisper turbo) can exceed the platform's upload size limit: the platform raises it.
- Deleting a model deletes the profiles that use it; their streams stop being transcribed until another profile is set.
- Only the text is kept: once transcribed, audio is deleted unless the stream keeps it.
- With **Voice detection** in the profile, a segment without speech is not transcribed: it shows as **silence** in Segments and its audio follows the retention. The speech duration of each segment becomes the series `media_speech_seconds` (label `stream`) for dashboards.
- With **diarization**, each transcription carries speaker turns labelled `S1`, `S2`… These labels are local to the stream: they say "the same voice as a moment ago", never who is speaking, and PNEX never identifies a person by their voice. They restart from `S1` after a restart of the transcription worker, when another worker takes over, or after 30 minutes without hearing that voice. Name speakers from external sources (schedules, announcements, ranges). Speaking time per label is written as `media_speaker_seconds` (labels `stream`, `speaker`).
- Diarization or voice detection needs both models of the profile to have passed their check; a missing or failed model stops the stream's transcription (segments end failed).
- A flow can read the transcriptions with the **Media source** node: one message per transcribed segment (or per sentence) of the streams it lists, nothing during silences. With diarization the message carries `speakers` (turns with their label) and, per sentence, `speaker`. A stream must exist and be enabled with a transcription profile; a stream that is not in the organization refuses the deploy.
- A flow reads the video of a stream with the **Camera source** node, source **Media stream**: only streams with a video track are offered, and a stream that is not a video stream of the organization refuses the deploy. The stream must be enabled to produce frames; there is no wake-up as for camera devices, an enabled stream is captured continuously. A camera without audio in "Audio and video" simply has no transcription.
- The silence alert only watches the audio: a video-only stream never raises it.
