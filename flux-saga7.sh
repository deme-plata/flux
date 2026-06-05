#!/usr/bin/env bash
# Flux Saga 7 - "The Forge, Live & Benchmarked": a flux-programming use case.
# Grace hosts; the ensemble explains live coding + REAL fluxc benchmark numbers.
# Outputs WAV + SRT + MKV (waveform video + soft subtitles).
set -euo pipefail
KEY="$(tr -d '[:space:]' < "${ELEVEN_KEY_FILE:-$HOME/.config/flux/eleven_key}")"
WAV="${1:-/tmp/flux-saga7.wav}"
MKV="${2:-/tmp/flux-saga7.mkv}"
SRT="${3:-/tmp/flux-saga7.srt}"
WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT

GRACE=EXAVITQu4vr4xnSDxMaL
ROCKY=nPczCjzI2devNBz1zQrb
MATILDA=XrExE9yKIg1WjnnlVkGX
ELENA=pFZP5JQG7iQjIQuC4Bku
CHARLIE=IKne3meq5aSn9XLyUdCD
LAURA=FGY2WhTYpPnrIDTdsKH5
HARRY=SOYHLrjzK2X1ezoPC6cr

# voice|style|label|text   (real fluxc benchmarks woven in)
lines=(
"$ROCKY|0.35|Rocky (camera above the camera)|Grace. Saga seven. We go inside the forge, live - the camera above the camera. Watch Flux compile itself, and time every breath."
"$GRACE|0.3|Grace|Matilda, show us. What is flux programming, really?"
"$MATILDA|0.3|Matilda|Flux programming is build aware coding, Grace. You write Rust, but fluxc watches every byte. Change one file, and only that file rebuilds."
"$CHARLIE|0.6|Charlie|Watch! I touch one line in flux-sigil, and I run the combo - fluxc build!"
"$GRACE|0.3|Grace|And the time?"
"$CHARLIE|0.65|Charlie|Incremental build - two point two five seconds! Only the changed crate, nothing else!"
"$MATILDA|0.3|Matilda|Compare a full build: eighteen seconds. The cache holds forty six gigabytes, across three hundred and nine builds."
"$ELENA|0.4|Elena Voss|I have watched scribes copy whole libraries by hand, Grace. This machine copies nothing twice. It remembers."
"$LAURA|0.65|Laura|The all time average is twenty five seconds a build - but a content hash hit is near instant! Same code, same hash, zero work!"
"$HARRY|0.7|Harry|Two hours of compute, saved and cached! The forge never repeats itself!"
"$GRACE|0.3|Grace|So the use case?"
"$ROCKY|0.35|Rocky|Thousands of tiny rebuilds a day, each one fast, each one cached. That is how a swarm of agents ships code every hour. That is flux programming."
"$ROCKY|0.35|Rocky|Grace. The numbers are real, the forge is live, and the saga is benchmarked. Good. Good. Good."
)

ms2srt(){ local ms=$1; printf "%02d:%02d:%02d,%03d" $((ms/3600000)) $(((ms/60000)%60)) $(((ms/1000)%60)) $((ms%1000)); }
ffmpeg -y -loglevel error -f lavfi -i anullsrc=r=44100:cl=stereo -t 0.4 "$WORK/sil.wav"
GAP=400; list="$WORK/list.txt"; : > "$list"; : > "$SRT"; t=0; idx=0
for entry in "${lines[@]}"; do
  idx=$((idx+1))
  voice="${entry%%|*}"; r1="${entry#*|}"; style="${r1%%|*}"; r2="${r1#*|}"; label="${r2%%|*}"; text="${r2#*|}"
  body="$(python3 -c 'import json,sys; print(json.dumps({"text":sys.argv[1],"model_id":"eleven_multilingual_v2","voice_settings":{"stability":0.42,"similarity_boost":0.85,"style":float(sys.argv[2])}}))' "$text" "$style")"
  curl -fsS -X POST "https://api.elevenlabs.io/v1/text-to-speech/$voice" \
       -H "xi-api-key: $KEY" -H "Content-Type: application/json" --data-binary "$body" -o "$WORK/seg$idx.mp3"
  ffmpeg -y -loglevel error -i "$WORK/seg$idx.mp3" -ar 44100 -ac 2 "$WORK/seg$idx.wav"
  dur=$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$WORK/seg$idx.wav" | awk '{printf "%d", $1*1000}')
  start=$t; end=$((t+dur))
  { echo "$idx"; echo "$(ms2srt $start) --> $(ms2srt $end)"; echo "$label: $text"; echo ""; } >> "$SRT"
  t=$((end+GAP)); echo "file '$WORK/seg$idx.wav'" >> "$list"; echo "file '$WORK/sil.wav'" >> "$list"
done
ffmpeg -y -loglevel error -f concat -safe 0 -i "$list" -ar 44100 -ac 2 -c:a pcm_s16le "$WAV"
echo "WAV: $WAV ($(du -h "$WAV" | cut -f1))"
ffmpeg -y -loglevel error -i "$WAV" -i "$SRT" -filter_complex \
  "[0:a]showwaves=s=1280x720:mode=cline:rate=25:colors=0x8b5cf6|0xc084fc,format=yuv420p[v]" \
  -map "[v]" -map 0:a -map 1 -c:v libx264 -preset veryfast -c:a aac -b:a 192k -c:s srt "$MKV"
echo "MKV: $MKV ($(du -h "$MKV" | cut -f1))"
