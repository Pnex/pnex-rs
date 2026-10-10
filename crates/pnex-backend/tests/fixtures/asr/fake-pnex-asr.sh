#!/bin/bash
# Fake `pnex-asr serve` for the backend tests: the JSON-lines protocol of
# crates/pnex-asr/src/protocol.rs without any native runtime.
# - speech-to-text: the same French sentence and two timed words;
# - with --vad-dir: a WAV whose samples are all zero is silent
#   (`speech_ms: 0`, no text), any other has 2.5 s of speech;
# - with --seg-dir (+ --emb-dir): two local speakers, each with a fixed
#   embedding, so the server links them to the same labels every segment;
# - family silero / pyannote_segmentation / speaker_embedding: the model
#   check answers of a VAD, segmentation or embedding model alone.
if [ "$1" != "serve" ]; then
  echo '{"fatal":"fake pnex-asr only serves"}'
  exit 2
fi
shift
family="" vad="" seg="" emb=""
while [ $# -gt 0 ]; do
  case "$1" in
    --family) family="$2"; shift ;;
    --vad-dir) vad="$2"; shift ;;
    --seg-dir) seg="$2"; shift ;;
    --emb-dir) emb="$2"; shift ;;
  esac
  shift
done
if [ "$family" = "pyannote_segmentation" ] && [ -z "$emb" ]; then
  echo '{"fatal":"a segmentation model needs --emb-dir"}'
  exit 2
fi
echo "{\"ready\":\"fake:${family}\",\"load_ms\":3}"
turns='"turns":[{"spk":0,"s":0,"e":400},{"spk":1,"s":400,"e":1000}]'
voices='"voices":[{"spk":0,"v":[1.0,0.0,0.1]},{"spk":1,"v":[0.0,1.0,0.1]}]'
words='"text":"La surface de la lune est constituée de pierres.","words":[{"w":"La","s":0,"e":200},{"w":"surface","s":200,"e":700}]'
while IFS= read -r line; do
  id="${line#*\"id\":}"
  id="${id%%,*}"
  data="${line#*\"wav_b64\":\"}"
  data="${data%%\"*}"
  # Past the 44-byte header (60 base64 chars): only zero bytes = silence.
  silent=1
  [ -n "$(printf '%s' "${data:60}" | tr -d 'A=' | head -c 1)" ] && silent=0
  case "$family" in
    silero)
      if [ $silent = 1 ]; then speech=0; else speech=2500; fi
      echo "{\"id\":${id},\"speech_ms\":${speech},\"ms\":1}"
      continue ;;
    pyannote_segmentation)
      echo "{\"id\":${id},${turns},${voices},\"ms\":2}"
      continue ;;
    speaker_embedding)
      echo "{\"id\":${id},\"voices\":[{\"spk\":0,\"v\":[0.5,0.5,0.0]}],\"ms\":2}"
      continue ;;
  esac
  out="{\"id\":${id}"
  if [ -n "$vad" ]; then
    if [ $silent = 1 ]; then
      echo "${out},\"speech_ms\":0,\"ms\":1}"
      continue
    fi
    out="${out},\"speech_ms\":2500"
  fi
  out="${out},${words}"
  [ -n "$seg" ] && out="${out},${turns},${voices}"
  echo "${out},\"ms\":7}"
done
