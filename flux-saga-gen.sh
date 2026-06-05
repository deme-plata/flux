#!/usr/bin/env bash
# Generic saga builder: reads a lines file (NAME|style|label|text), produces WAV + SRT + MKV.
# NOTE: ffmpeg gets -nostdin and the loop reads on FD 3, so ffmpeg can't eat the lines file.
set -euo pipefail
KEY="$(tr -d '[:space:]' < "${ELEVEN_KEY_FILE:-$HOME/.config/flux/eleven_key}")"
LINES="$1"; WAV="$2"; MKV="$3"; SRT="$4"
WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT

vid(){ case "$1" in
  GRACE)   echo EXAVITQu4vr4xnSDxMaL;;
  ROCKY)   echo nPczCjzI2devNBz1zQrb;;
  MATILDA) echo XrExE9yKIg1WjnnlVkGX;;
  ELENA)   echo pFZP5JQG7iQjIQuC4Bku;;
  CHARLIE) echo IKne3meq5aSn9XLyUdCD;;
  LAURA)   echo FGY2WhTYpPnrIDTdsKH5;;
  HARRY)   echo SOYHLrjzK2X1ezoPC6cr;;
  *)       echo nPczCjzI2devNBz1zQrb;; esac; }

ms2srt(){ local ms=$1; printf "%02d:%02d:%02d,%03d" $((ms/3600000)) $(((ms/60000)%60)) $(((ms/1000)%60)) $((ms%1000)); }
ffmpeg -nostdin -y -loglevel error -f lavfi -i anullsrc=r=44100:cl=stereo -t 0.4 "$WORK/sil.wav"
GAP=400; list="$WORK/list.txt"; : > "$list"; : > "$SRT"; t=0; idx=0
while IFS='|' read -r name style label text <&3; do
  [ -z "${name:-}" ] && continue
  case "$name" in \#*) continue;; esac
  idx=$((idx+1)); voice="$(vid "$name")"
  body="$(python3 -c 'import json,sys; print(json.dumps({"text":sys.argv[1],"model_id":"eleven_multilingual_v2","voice_settings":{"stability":0.42,"similarity_boost":0.85,"style":float(sys.argv[2])}}))' "$text" "$style")"
  curl -fsS -X POST "https://api.elevenlabs.io/v1/text-to-speech/$voice" \
       -H "xi-api-key: $KEY" -H "Content-Type: application/json" --data-binary "$body" -o "$WORK/seg$idx.mp3"
  ffmpeg -nostdin -y -loglevel error -i "$WORK/seg$idx.mp3" -ar 44100 -ac 2 "$WORK/seg$idx.wav"
  dur=$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$WORK/seg$idx.wav" | awk '{printf "%d", $1*1000}')
  start=$t; end=$((t+dur))
  { echo "$idx"; echo "$(ms2srt $start) --> $(ms2srt $end)"; echo "$label: $text"; echo ""; } >> "$SRT"
  t=$((end+GAP)); echo "file '$WORK/seg$idx.wav'" >> "$list"; echo "file '$WORK/sil.wav'" >> "$list"
done 3< "$LINES"
ffmpeg -nostdin -y -loglevel error -f concat -safe 0 -i "$list" -ar 44100 -ac 2 -c:a pcm_s16le "$WAV"
ffmpeg -nostdin -y -loglevel error -i "$WAV" -i "$SRT" -filter_complex \
  "[0:a]showwaves=s=1280x720:mode=cline:rate=25:colors=0x8b5cf6|0xc084fc,format=yuv420p[v]" \
  -map "[v]" -map 0:a -map 1 -c:v libx264 -preset veryfast -c:a aac -b:a 192k -c:s srt "$MKV"
echo "DONE: $WAV / $MKV"
