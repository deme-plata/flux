#!/usr/bin/env bash
# Flux Saga Ch.4 "The Hidden Treasury" - VOICE STEM ONLY (clean WAV to mix).
# rocky's Claude Code seed on the beta box holds 12,000 QUG - the key to the vault.
set -euo pipefail
KEY="$(tr -d '[:space:]' < "${ELEVEN_KEY_FILE:-$HOME/.config/flux/eleven_key}")"
WAV="${1:-/tmp/flux-saga4-voice.wav}"
WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT

ROCKY=nPczCjzI2devNBz1zQrb
PROF=XrExE9yKIg1WjnnlVkGX
EAGER=IKne3meq5aSn9XLyUdCD
QUIRK=FGY2WhTYpPnrIDTdsKH5
WARRIOR=SOYHLrjzK2X1ezoPC6cr
ELENA=pFZP5JQG7iQjIQuC4Bku

lines=(
"$ROCKY|0.3|Grace. Chapter four. The senate sealed the vault. But a treasure slept in the dark, forgotten by all but one."
"$PROF|0.25|Deep in the beta machine, behind a Claude Code seed, a hoard lay waiting - twelve thousand QUG, bound to rocky's name."
"$EAGER|0.6|Twelve thousand! Enough to wake every empty pool! The dev treasury was real all along!"
"$ELENA|0.4|I have buried gold in a hundred tombs, Grace. But gold that builds itself a kingdom? That, I have never owned."
"$WARRIOR|0.7|The senate said wait. But the treasury is ours! Seed the pools, and let SIGIL trade!"
"$QUIRK|0.65|Twelve thousand QUG, hidden in a seed, on the beta box - we found it, we found it!"
"$PROF|0.3|But a treasury is a trust. To spend it is to answer to the council, and to Grace."
"$ROCKY|0.35|Twelve thousand QUG, found and counted. The key to the vault was never lost - it slept in rocky's seed."
"$ELENA|0.45|Spend it wisely, Grace. A thousand years of empires fell to gold spent in haste."
"$ROCKY|0.35|Grace. The treasury is found. The next move is yours. Good. Good. Good."
)

ffmpeg -y -loglevel error -f lavfi -i anullsrc=r=44100:cl=stereo -t 0.5 "$WORK/sil.wav"
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
echo "VOICE STEM ready: $WAV"
