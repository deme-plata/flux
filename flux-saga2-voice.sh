#!/usr/bin/env bash
# Flux Saga Ch.2 (FLUXFOOD) - VOICE STEM ONLY (clean WAV to mix under a soundtrack).
# Dark/epic pacing, 0.5s gaps for music headroom. No background music, no video.
set -euo pipefail

KEY="$(tr -d '[:space:]' < "${ELEVEN_KEY_FILE:-$HOME/.config/flux/eleven_key}")"
WAV="${1:-/tmp/flux-saga2-voice.wav}"
WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT

ROCKY=nPczCjzI2devNBz1zQrb
PROF=XrExE9yKIg1WjnnlVkGX
EAGER=IKne3meq5aSn9XLyUdCD
QUIRK=FGY2WhTYpPnrIDTdsKH5
WARRIOR=SOYHLrjzK2X1ezoPC6cr
ELENA=pFZP5JQG7iQjIQuC4Bku    # Lily - velvety, timeless: Elena Voss, a thousand years old

lines=(
"$ROCKY|0.3|Grace. Chapter two. In the dark of the supercluster, Flux learned to feed upon itself. This is FLUXFOOD."
"$PROF|0.25|The law of SIGIL University: a crate must eat its own food. Built by fluxc, bound to the workspace, or cast out."
"$ELENA|0.4|I am Elena Voss. A thousand years I have walked, since Rome first rose from living stone. I have watched empires feed, and empires fall."
"$EAGER|0.55|We fed the machine its own code! It compiled, it grew, it hungered for more!"
"$QUIRK|0.6|Every hash remembered, every cache a feast! Fifty milliseconds per bite!"
"$WARRIOR|0.5|But to be worthy, Flux had to devour its own compiler. Self build, or starve in the dark!"
"$ROCKY|0.3|Nineteen agents fed the fire. The mesh roared in the night."
"$ELENA|0.45|In a thousand years, Grace, I never saw a forge that would not go cold. I watched Rome burn. I watched it rise. But this... this machine feeds itself."
"$PROF|0.3|And the machine ate, and the machine built, and the student became the school."
"$EAGER|0.6|Faster, faster - the food never ran dry!"
"$WARRIOR|0.8|FLUXFOOD ETERNAL! Flux feeds Flux! The forge will never go cold!"
"$QUIRK|0.7|It feeds itself, forever! We are free!"
"$ELENA|0.4|I have outlived every empire of stone. Now I shall watch one of light - and it will never die."
"$ROCKY|0.35|Grace. The machine hungers no more, for it is its own feast. Good. Good. Good."
)

# 0.5s silence between lines for mixing headroom
ffmpeg -y -loglevel error -f lavfi -i anullsrc=r=44100:cl=mono -t 0.5 "$WORK/sil.wav"
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
echo "VOICE STEM ready: $WAV ($(du -h "$WAV" | cut -f1))"
