---
id: voice-id
title: Voice profiles — recognize a consenting person by their voice
kind: feature
pages: /voice-id, /ontology/object/:id, /media
nodes:
err_codes: access-denied, voice-id-not-person, voice-id-profile-exists, voice-id-profile-not-found, voice-id-sample-not-found, voice-id-no-model, voice-id-audio-unsupported, voice-id-not-enough-speech, voice-id-not-ready, voice-id-failed
tools:
tags: voice, voix, voice profile, empreinte vocale, speaker recognition, reconnaissance vocale, reconnaissance du locuteur, speaker verification, identification, person, personne, enrolment, enrôlement, consent, consentement, biometric, biométrie, rgpd, gdpr, speaker embedding, empreinte de locuteur
---
A **voice profile** recognizes one person in another recording. It is attached to a **Person** object of the ontology (pack Investigation) and lives in its **Voice** tab; **Media › Voice profiles** lists every profile of the organization and recognizes a recording among them; its **New voice profile** button (owners and admins) asks which person to enrol and opens their Voice tab.

A voice profile is biometric data: it exists only with the person's **explicit consent**, and only owners and admins create, feed or delete one. The assistant never reads a profile and cannot create one.

## What you can do
- **Create the profile** (Person › Voice): pick the **speaker embedding model**, enter the **consent date**, its **scope** (purpose, duration) and where the **proof** is kept, then tick the confirmation. Without a valid speaker embedding model in Streams › Models, nothing can be created.
- **Add enrolment clips**: recordings of the person **alone** (WAV, MP3, Ogg/Opus, FLAC, M4A), at least about 30 seconds of speech in total. After choosing a file, a player opens: **Start here** / **End here** set the span to enrol (3 minutes at most), useful for a long show or interview. Only that span is sent for analysis and kept, as an **Audio** item of the media library, so the profile can be generated again with another model.
- **Generate the profile**: each clip is cut into 8-second stretches; stretches with too little voice, or with another voice, are left out. The profile gets a **threshold** calibrated on how consistent the clips are. Each clip shows its score and whether it was used.
- **Test with another audio** (Voice tab, profile ready): drop a recording; the score says whether it is the same voice. The tested audio is **not kept**.
- **Recognize a voice** (Media › Voice profiles): compares a recording with every ready profile of the chosen model. The result is "recognized", "ambiguous" (two voices too close) or "none".
- **Withdraw the consent**: deletes the profile, its clips and their audio files at once.

## Good to know
- A result is a **proposal**, never a certain identification: a human checks before drawing a conclusion. Nothing is written anywhere from a match.
- Every generation and every test is logged (who, when, which profiles, scores) — never the audio or the voice print itself.
- The speakers of streams (S1, S2…) stay anonymous: voice profiles are never used to name them.
- Changing the model of a profile sends it back to draft: generate it again.
