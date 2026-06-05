#!/usr/bin/env bash
# flux-voiceover — add a Rocky/ElevenLabs voice-over narration onto a flux-record video.
# Usage: flux-voiceover <input.mp4|mkv> <"narration" | narration.txt> [output.mp4]
# The "host super star" narrates; original audio is ducked under the voice.
set -euo pipefail

KEY_FILE="${ELEVEN_KEY_FILE:-$HOME/.config/flux/eleven_key}"
VOICE="${ELEVEN_VOICE_ID:-nPczCjzI2devNBz1zQrb}"   # Brian — deep, resonant (Rocky)
MODEL="${ELEVEN_MODEL:-eleven_multilingual_v2}"

IN="${1:?input video required}"
NARR="${2:?narration text or .txt file required}"
OUT="${3:-${IN%.*}-voiceover.mp4}"

KEY="$(tr -d '[:space:]' < "$KEY_FILE" 2>/dev/null || true)"
[ -n "$KEY" ] || { echo "Missing ElevenLabs key at $KEY_FILE"; exit 1; }
[ -f "$IN" ]  || { echo "No such video: $IN"; exit 1; }

if [ -f "$NARR" ]; then TEXT="$(cat "$NARR")"; else TEXT="$NARR"; fi

TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
VO="$TMP/vo.mp3"

# 1) build request body (python3 -> safe JSON) and synthesize via ElevenLabs
python3 - "$TEXT" "$MODEL" > "$TMP/body.json" <<'PY'
import json, sys
print(json.dumps({
    "text": sys.argv[1],
    "model_id": sys.argv[2],
    "voice_settings": {"stability": 0.45, "similarity_boost": 0.8, "style": 0.35},
}))
PY

curl -fsS -X POST "https://api.elevenlabs.io/v1/text-to-speech/$VOICE" \
     -H "xi-api-key: $KEY" -H "Content-Type: application/json" \
     --data-binary @"$TMP/body.json" -o "$VO"
[ -s "$VO" ] || { echo "TTS failed (empty audio)"; exit 1; }

# 2) mux the voice-over onto the video
if ffprobe -v error -select_streams a -show_entries stream=index -of csv=p=0 "$IN" | grep -q .; then
  # original has audio -> duck it under the narration
  ffmpeg -y -loglevel error -i "$IN" -i "$VO" -filter_complex \
    "[0:a]volume=0.22[bg];[1:a]volume=1.0[vo];[bg][vo]amix=inputs=2:duration=longest:dropout_transition=0[a]" \
    -map 0:v -map "[a]" -c:v copy -c:a aac -b:a 192k "$OUT"
else
  # no original audio -> attach the narration as the audio track
  ffmpeg -y -loglevel error -i "$IN" -i "$VO" \
    -map 0:v -map 1:a -c:v copy -c:a aac -b:a 192k -shortest "$OUT"
fi

echo "Voice-over ready: $OUT"
