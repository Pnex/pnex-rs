---
id: streams
title: Audio streams
kind: feature
pages: /streams
nodes: 
err_codes: media-stream-not-found, media-stream-write-forbidden, media-stream-limit, media-url-has-credentials, media-stream-unreachable, media-asr-model-invalid, media-capture-unsupported, asr-profile-not-found, asr-profile-name-taken
tools: 
tags: streams, flux, radio, audio, icecast, hls, podcast, transcription, asr, speech to text, parole, capture, retention, tdm
---
Audio streams (Data › Audio streams) lists the radios and live streams the organization captures around the clock. Each stream is cut into short audio segments, transcribed into timestamped text, and only the text is kept for good.

## What you can do
- **Add a stream**: give it a **Name**, a **Kind** (`icecast` for a continuous radio stream, `hls` for an `.m3u8` playlist, `http_file` for a single file such as a podcast episode) and the **Stream URL**.
- **Access secret**: when the stream needs a header or credentials, put them in the secret (typed or picked from the vault). It is sent only to the host of the URL. Only owners and admins can set it.
- **Transcription profile**: the speech-to-text model used for new segments. A stream cannot be enabled without a profile whose model passed its check.
- **Segment length** (10 to 120 s) and **Audio retention**: none (audio deleted once transcribed, the default), a number of days, or kept (only when the organization holds the rights).
- **Enable / Disable** capture, **Edit**, **Delete** (pending segments are erased; transcriptions already written stay in the history).

## Good to know
- The URL never contains a token or a password: such a URL is refused, use the access secret instead.
- Addresses inside the server's own network (localhost, cloud metadata, internal service names) are refused with the same "cannot be reached" message on purpose.
- Moving a stream that holds a secret to another host, scheme or port needs an owner or admin.
- The short name under the stream (its slug) never changes, even when the stream is renamed: it names the stored transcriptions.
- Before capturing a third-party stream, check that the publisher did not opt out of text and data mining (terms, robots.txt, notices) and tick **Text and data mining opt-out checked**; until then the list shows a warning.
- Each organization has a limited number of streams (3 by default, set by the platform).
