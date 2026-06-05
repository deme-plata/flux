#!/usr/bin/env bash
# Flux Saga - Chapter 2: "The Awakening of the DEX". Builds a multi-voice WAV
# and a YouTube-ready MP4 (purple waveform visualizer + audio).
set -euo pipefail

KEY="$(tr -d '[:space:]' < "${ELEVEN_KEY_FILE:-$HOME/.config/flux/eleven_key}")"
WAV="${1:-/tmp/flux-saga2.wav}"
MP4="${2:-/tmp/flux-saga2.mp4}"
WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT

ROCKY=nPczCjzI2devNBz1zQrb
PROF=XrExE9yKIg1WjnnlVkGX
EAGER=IKne3meq5aSn9XLyUdCD
QUIRK=FGY2WhTYpPnrIDTdsKH5
WARRIOR=SOYHLrjzK2X1ezoPC6cr

lines=(
"$ROCKY|0.3|Grace. Chapter two. The saga of the silent pools, and the day they came alive."
"$PROF|0.2|On the mainnet, eighteen million blocks deep, the DEX was built. Twenty six pools, carved and ready."
"$EAGER|0.55|But every pool was empty! Zero, zero, zero! No liquidity, no trades - only echoes!"
"$QUIRK|0.6|The premine slept in the vault, dev only, waiting for its moment. Patience, patience!"
"$WARRIOR|0.45|For real trades to flow, the premine must wake. Seed the pools, or the market stays a ghost town!"
"$ROCKY|0.3|Some whispered two hundred days. But the chain was already alive - caught up, healthy, one hundred percent accepted."
"$PROF|0.3|Hear me: the wait is not the chain. The wait is the will - to open the vault and pour the first liquidity."
"$EAGER|0.6|And when it pours - slippage falls, depth rises, the first real SIGIL trade clears!"
"$WARRIOR|0.75|THE POOLS AWAKEN! Liquidity flows! The first trade settles on mainnet!"
"$QUIRK|0.7|From you versus yourself, to a living market! We did it again!"
"$ROCKY|0.35|Grace. The DEX breathes now. When the vault opens, the saga becomes real. Good. Good. Good."
)

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
  echo "file '$WORK/seg$i.wav'" >> "$list"
  echo "file '$WORK/sil.wav'"   >> "$list"
done

ffmpeg -y -loglevel error -f concat -safe 0 -i "$list" -c copy "$WAV"
echo "WAV ready: $WAV ($(du -h "$WAV" | cut -f1))"

# YouTube-ready MP4: purple waveform visualizer over the audio
ffmpeg -y -loglevel error -i "$WAV" -filter_complex \
  "[0:a]showwaves=s=1280x720:mode=cline:rate=25:colors=0x8b5cf6|0xc084fc,format=yuv420p[v]" \
  -map "[v]" -map 0:a -c:v libx264 -preset veryfast -c:a aac -b:a 192k -shortest "$MP4"
echo "MP4 ready: $MP4 ($(du -h "$MP4" | cut -f1))"
