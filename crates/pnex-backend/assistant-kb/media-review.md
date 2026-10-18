---
id: media-review
title: Review a media with its transcript and speakers
kind: feature
pages: /media/:id/review
nodes: 
err_codes: media-attribution-conflict, media-write-forbidden, media-analysis-no-asr
tools: 
tags: review, relecture, relire, transcript, transcription, subtitles, sous-titres, speakers, locuteurs, who speaks, qui parle, attribution, attribuer, person, personne, intervenant, participant, appears in, apparaît dans, timeline, frise
---
The review screen of an audio or video file (Media › open a file › Analyses card › **Review with the transcript**) shows the media in the centre, its transcript on the right and the speakers on the timeline below, with one cursor across the three.

## What you can do
- **Play** the media with the shared player (play/pause, speed); the word being spoken is lit in the transcript.
- **Click a word**, a time in the transcript or a place on the timeline: the media jumps there.
- **Choose the run** when there are several: a PNeX transcription of a version of the file, or the subtitles the platform gave with a post captured by a browser agent.
- **Say who speaks** (owners and admins): attribute a span of the media to a person of the ontology, without any voice recognition ("from 12:30 to 14:10, this is Alice"). Spans of the same person that overlap join into one. The person then **appears in** the media in the ontology (link *appears in*, kind *speaking*).
- Proposals (from a voice profile, the transcript or the programme grid) are shown apart: **confirm** or **reject** them; a rejected proposal is not made again.

## Good to know
- Speakers S1, S2… are anonymous voices told apart by the transcription; they are never named automatically.
- No transcript yet: start **Transcription** from the Analyses card of the media (it needs a valid transcription profile in Streams › Models and profiles, *media-analysis-no-asr*).
- A post captured without its video shows its text and timeline, without a player.
- Two people editing the same span: the second save is refused with *media-attribution-conflict*; reload the page and do it again.
- Viewers can review and read the attributions; only owners and admins attribute (*media-write-forbidden*).
