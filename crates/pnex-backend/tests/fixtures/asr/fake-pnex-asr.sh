#!/bin/bash
# Fake `pnex-asr serve` for the backend tests: the JSON-lines protocol of
# crates/pnex-asr/src/protocol.rs without any native runtime. Answers every
# request with the same French sentence and two timed words.
if [ "$1" != "serve" ]; then
  echo '{"fatal":"fake pnex-asr only serves"}'
  exit 2
fi
echo '{"ready":"fake:test","load_ms":3}'
while IFS= read -r line; do
  id="${line#*\"id\":}"
  id="${id%%,*}"
  echo "{\"id\":${id},\"text\":\"La surface de la lune est constituée de pierres.\",\"words\":[{\"w\":\"La\",\"s\":0,\"e\":200},{\"w\":\"surface\",\"s\":200,\"e\":700}],\"ms\":7}"
done
