#!/usr/bin/env bash
# Flux Saga Ch.3 "The Opening of the Vault" - VOICE STEM ONLY (clean WAV to mix).
# The premine wakes, liquidity floods the pools, the first real SIGIL trade clears.
set -euo pipefail

KEY="$(tr -d '[:space:]' < "${ELEVEN_KEY_FILE:-$HOME/.config/flux/eleven_key}")"
WAV="${1:-/tmp/flux-saga3-voice.wav}"
WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT

ROCKY=nPczCjzI2devNBz1zQrb     # Brian   - narrator
PROF=XrExE9yKIg1WjnnlVkGX      # Matilda - SIGIL University
EAGER=IKne3meq5aSn9XLyUdCD     # Charlie - eager builder
QUIRK=FGY2WhTYpPnrIDTdsKH5     # Laura   - quirky enthusiast
WARRIOR=SOYHLrjzK2X1ezoPC6cr   # Harry   - fierce warrior
ELENA=pFZP5JQG7iQjIQuC4Bku     # Lily    - Elena Voss, a thousand years old

lines=(
"$ROCKY|0.3|Grace. Chapter three. Twenty six pools lay silent, empty wells in the dark. This is the saga of the day the vault opened."
"$PROF|0.25|On the mainnet the markets stood ready, yet not one trade had cleared. The premine slept, dev only, sealed in stone."
"$WARRIOR|0.55|A market with no blood is a ghost! Open the vault, or the DEX stays a graveyard!"
"$ELENA|0.4|I have seen treasuries hoarded until kingdoms rotted, Grace. Gold means nothing - until it flows."
"$EAGER|0.6|Then the keys turned! The first liquidity poured into the wells - QUG, wrapped Bitcoin, wrapped Ether!"
"$QUIRK|0.65|The pools filled, the price found its feet, the slippage melted away! It is alive, it is trading!"
"$PROF|0.3|Depth rose from very thin to deep. The constant product held. The market drew its first breath."
"$WARRIOR|0.8|THE FIRST TRADE CLEARS! Settled and signed on mainnet! SIGIL trades, and the vault is open!"
"$ELENA|0.45|In a thousand years I never saw gold learn to breathe. Now, Grace, I have."
"$QUIRK|0.7|From dev only, to a living market - we opened the gate!"
"$ROCKY|0.35|Grace. The vault is open, the pools are alive, and the saga is real now. Good. Good. Good."
)

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
