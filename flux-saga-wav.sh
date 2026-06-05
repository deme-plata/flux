#!/usr/bin/env bash
# Build the multi-voice FLUX SAGA as a single WAV (ElevenLabs mp3 per line -> ffmpeg -> wav).
set -euo pipefail

KEY="$(tr -d '[:space:]' < "${ELEVEN_KEY_FILE:-$HOME/.config/flux/eleven_key}")"
OUT="${1:-/tmp/flux-saga.wav}"
WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT

ROCKY=nPczCjzI2devNBz1zQrb     # Brian   - narrator
PROF=XrExE9yKIg1WjnnlVkGX      # Matilda - SIGIL University professor
EAGER=IKne3meq5aSn9XLyUdCD     # Charlie - eager builder
QUIRK=FGY2WhTYpPnrIDTdsKH5     # Laura   - quirky enthusiast
WARRIOR=SOYHLrjzK2X1ezoPC6cr   # Harry   - fierce warrior

# entries: voiceid|style|text
lines=(
"$ROCKY|0.3|Grace. Listen close. This is the saga of Flux - the day the machine learned to feed itself."
"$PROF|0.2|At SIGIL University we teach one sacred law: a crate must eat its own food. Workspace roots. Built only by fluxc. We call it FLUXFOOD."
"$EAGER|0.5|But the old way was so slow! Cargo crawled. We waited, and waited, for every single build!"
"$QUIRK|0.6|So we got clever - content hashes, BLAKE3, a cache that never forgets! Fifty milliseconds, baby! Whoosh!"
"$WARRIOR|0.4|Then came the night of the great self build. Flux turned upon itself. Compile your own compiler, agent, or fall."
"$ROCKY|0.3|The swarm gathered. Nineteen agents. One mesh. The supercluster held its breath."
"$EAGER|0.6|Rocky took the lead - building, testing, shipping, again and again and again!"
"$PROF|0.3|The cache hit. Green across the whole board. Flux had compiled Flux. The student had become the school."
"$WARRIOR|0.7|VICTORY! Forty three QUG, settled and signed! Rocky stands champion of the swarm!"
"$QUIRK|0.7|We did it, we did it! The food feeds itself, forever and ever!"
"$ROCKY|0.35|Grace. The machine is alive, and it hungers no more. We built Rome - and now Rome builds itself. Good. Good. Good."
)

# 0.35s silence between lines
ffmpeg -y -loglevel error -f lavfi -i anullsrc=r=44100:cl=mono -t 0.35 "$WORK/sil.wav"

list="$WORK/list.txt"; : > "$list"
i=0
for entry in "${lines[@]}"; do
  i=$((i+1))
  voice="${entry%%|*}"; rest="${entry#*|}"; style="${rest%%|*}"; text="${rest#*|}"
  body="$(python3 -c 'import json,sys; print(json.dumps({"text":sys.argv[1],"model_id":"eleven_multilingual_v2","voice_settings":{"stability":0.4,"similarity_boost":0.85,"style":float(sys.argv[2])}}))' "$text" "$style")"
  curl -fsS -X POST "https://api.elevenlabs.io/v1/text-to-speech/$voice" \
       -H "xi-api-key: $KEY" -H "Content-Type: application/json" \
       --data-binary "$body" -o "$WORK/seg$i.mp3"
  ffmpeg -y -loglevel error -i "$WORK/seg$i.mp3" -ar 44100 -ac 1 "$WORK/seg$i.wav"
  echo "file '$WORK/seg$i.wav'"  >> "$list"
  echo "file '$WORK/sil.wav'"    >> "$list"
done

ffmpeg -y -loglevel error -f concat -safe 0 -i "$list" -c copy "$OUT"
echo "WAV ready: $OUT ($(du -h "$OUT" | cut -f1))"
