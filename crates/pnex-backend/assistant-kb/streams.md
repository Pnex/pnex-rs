---
id: streams
title: Audio streams
kind: feature
pages: /streams
nodes: 
err_codes: asr-test-audio-unsupported, asr-test-failed, media-stream-not-found, media-stream-write-forbidden, media-stream-limit, media-url-has-credentials, media-stream-unreachable, media-asr-model-invalid, media-capture-unsupported, asr-profile-not-found, asr-profile-name-taken, asr-model-not-found, asr-model-name-taken, asr-model-unsupported, media-transcripts-unavailable
tools: 
tags: streams, flux, radio, audio, icecast, hls, podcast, transcription, transcriptions, asr, speech to text, parole, capture, retention, tdm, parakeet, canary, whisper, sherpa, profile, profil, search, recherche
---
Audio streams (Data › Audio streams) lists the radios and live streams the organization captures around the clock. Each stream is cut into short audio segments, transcribed into timestamped text, and only the text is kept for good.

## What you can do
- **Add a stream**: give it a **Name**, a **Kind** (`icecast` for a continuous radio stream, `hls` for an `.m3u8` playlist, `http_file` for a single file such as a podcast episode) and the **Stream URL**.
- **Access secret**: when the stream needs a header or credentials, put them in the secret (typed or picked from the vault). It is sent only to the host of the URL. Only owners and admins can set it.
- **Transcription profile**: the speech-to-text model used for new segments. A stream cannot be enabled without a profile whose model passed its check.
- **Capture location**: this server (default), or a capture worker, a separate machine run by the platform. A stream set to a capture worker stays idle until the platform starts one.
- **Alert channel**: when an enabled stream brings no audio for 2 minutes, the organization gets an in-app notification, and a message on this channel when one is picked (email, ntfy, Slack…). One alert per silence; it re-arms once audio is back.
- **Segment length** (10 to 120 s) and **Audio retention**: none (audio deleted once transcribed, the default), a number of days, or kept (only when the organization holds the rights).
- **Enable / Disable** capture, **Edit**, **Delete** (pending segments are erased; transcriptions already written stay in the history).
- **Segments** (on each stream, every member): every captured slice with its state (captured, queued, transcribing, transcribed, failed, silence or too late), newest first, filterable by state. **Retry failed segments** sends back to transcription the failed ones whose audio is still kept.
- Under the state of an enabled stream: how long ago the last audio arrived, how old the oldest segment waiting for transcription is, and the share of the last hour that was transcribed.
- **Transcriptions** tab: search the text of every stream (or one), newest first.
- **Models and profiles** tab: **Import a model** (upload a sherpa-onnx `.tar.bz2` archive or a whisper.cpp GGML file, or pick a `model` file from the media library) with its **Model license**; the server reads the family from the files, loads the model and transcribes a French reference clip: the row shows how many real-time streams it sustains and its error rate on the clip. **Check** runs it again; when the platform runs a dedicated transcription worker, the model is also checked there and the row adds one line per worker ("On worker:…: ≈ N real-time streams" or "does not run"). **Test** (on a checked model) transcribes an audio file you drop (MP3, AAC, Ogg/Opus, FLAC, WAV or the sound of an MP4; the first 2 minutes) and shows the text and how long it took. Then **Add the profile** (name, model, language) and pick that profile in the stream.

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
