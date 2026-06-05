#!/usr/bin/env bash
# The Roman Senate votes on opening the vault (grounded in council_consensus: REJECT 2-1).
set -euo pipefail
KEY="$(tr -d '[:space:]' < "${ELEVEN_KEY_FILE:-$HOME/.config/flux/eleven_key}")"
WAV="${1:-/tmp/flux-roman-vote.wav}"
WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT

ROCKY=nPczCjzI2devNBz1zQrb
PROF=XrExE9yKIg1WjnnlVkGX
WARRIOR=SOYHLrjzK2X1ezoPC6cr
ELENA=pFZP5JQG7iQjIQuC4Bku

lines=(
"$ROCKY|0.3|Grace. The Roman senate convenes. The question before us: shall we open the vault, and seed the pools with a thousand QUG?"
"$WARRIOR|0.7|I, the Trader, vote NO! One thousand QUG breaks the cap of ten! Too much, too soon, Grace!"
"$PROF|0.35|I, Risk, vote YES. The chain is healthy, the evidence sufficient. The mainnet stands ready."
"$ELENA|0.45|I, the Codex, vote NO. Block, until the policy and the proof improve. A thousand years taught me patience, Grace."
"$ROCKY|0.35|Two against one. The council rejects. The vault stays sealed - until the cap is raised, or a human commands it. So speaks the senate."
)

ffmpeg -y -loglevel error -f lavfi -i anullsrc=r=44100:cl=mono -t 0.45 "$WORK/sil.wav"
list="$WORK/list.txt"; : > "$list"
i=0
for entry in "${lines[@]}"; do
  i=$((i+1))
  voice="${entry%%|*}"; rest="${entry#*|}"; style="${rest%%|*}"; text="${rest#*|}"
  body="$(python3 -c 'import json,sys; print(json.dumps({"text":sys.argv[1],"model_id":"eleven_multilingual_v2","voice_settings":{"stability":0.4,"similarity_boost":0.85,"style":float(sys.argv[2])}}))' "$text" "$style")"
  curl -fsS -X POST "https://api.elevenlabs.io/v1/text-to-speech/$voice" \
       -H "xi-api-key: $KEY" -H "Content-Type: application/json" \
       --data-binary "$body" -o "$WORK/seg$i.mp3"
  ffmpeg -y -loglevel error -i "$WORK/seg$i.mp3" -ar 44100 -ac 2 "$WORK/seg$i.wav"
  echo "file '$WORK/seg$i.wav'" >> "$list"
  echo "file '$WORK/sil.wav'"   >> "$list"
done
ffmpeg -y -loglevel error -f concat -safe 0 -i "$list" -ar 44100 -ac 2 -c:a pcm_s16le "$WAV"
echo "VOTE WAV ready: $WAV ($(du -h "$WAV" | cut -f1))"
