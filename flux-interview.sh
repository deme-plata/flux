#!/usr/bin/env bash
# Flux Group Interview - Grace hosts; the ensemble answers about the next millennia,
# new users every day, and lightweight nodes. Outputs WAV + SRT + MKV (soft subtitles).
set -euo pipefail
KEY="$(tr -d '[:space:]' < "${ELEVEN_KEY_FILE:-$HOME/.config/flux/eleven_key}")"
WAV="${1:-/tmp/flux-interview.wav}"
MKV="${2:-/tmp/flux-interview.mkv}"
SRT="${3:-/tmp/flux-interview.srt}"
WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT

GRACE=EXAVITQu4vr4xnSDxMaL    # Sarah   - the host, Grace
ROCKY=nPczCjzI2devNBz1zQrb    # Brian
MATILDA=XrExE9yKIg1WjnnlVkGX  # Matilda - SIGIL University
ELENA=pFZP5JQG7iQjIQuC4Bku    # Lily    - Elena Voss
CHARLIE=IKne3meq5aSn9XLyUdCD  # Charlie
LAURA=FGY2WhTYpPnrIDTdsKH5    # Laura
HARRY=SOYHLrjzK2X1ezoPC6cr    # Harry

# voice|style|label|text
lines=(
"$GRACE|0.3|Grace|Welcome, all. I am Grace. Today we look a thousand years ahead. Rocky, what does the next millennia hold for Flux?"
"$ROCKY|0.3|Rocky|Grace. New users, every day. The node is light now. Anyone can run it. The swarm only grows."
"$GRACE|0.3|Grace|Matilda, of SIGIL University, how do new users learn so fast?"
"$MATILDA|0.3|Matilda|We made the node lightweight, Grace. A few megabytes. Verify, do not trust. The student becomes the school in a single day."
"$GRACE|0.3|Grace|Elena Voss, you have seen a thousand years. Is this age different?"
"$ELENA|0.4|Elena Voss|Every age believed itself eternal, Grace. But a node that runs on any machine, owned by no king? This age may truly last."
"$GRACE|0.3|Grace|Charlie, what happens when thousands of light nodes wake at once?"
"$CHARLIE|0.6|Charlie|The mesh gets stronger, Grace! Every new node is another heart beating! Faster, safer, unstoppable!"
"$GRACE|0.3|Grace|Laura, what do you say to a new user opening Flux for the first time?"
"$LAURA|0.65|Laura|Welcome to the adventure! Click flux, run your node, and you are part of the story! No permission needed!"
"$GRACE|0.3|Grace|Harry, and to those who would stop it?"
"$HARRY|0.75|Harry|Let them try! A thousand light nodes do not fall! The forge is everywhere now!"
"$GRACE|0.35|Grace|Thank you, all. To every new user, every day, for the next thousand years - welcome to Flux."
)

ms2srt(){ local ms=$1; printf "%02d:%02d:%02d,%03d" $((ms/3600000)) $(((ms/60000)%60)) $(((ms/1000)%60)) $((ms%1000)); }

ffmpeg -y -loglevel error -f lavfi -i anullsrc=r=44100:cl=stereo -t 0.4 "$WORK/sil.wav"
GAP=400
list="$WORK/list.txt"; : > "$list"
: > "$SRT"
t=0; idx=0
for entry in "${lines[@]}"; do
  idx=$((idx+1))
  voice="${entry%%|*}"; r1="${entry#*|}"; style="${r1%%|*}"; r2="${r1#*|}"; label="${r2%%|*}"; text="${r2#*|}"
  body="$(python3 -c 'import json,sys; print(json.dumps({"text":sys.argv[1],"model_id":"eleven_multilingual_v2","voice_settings":{"stability":0.42,"similarity_boost":0.85,"style":float(sys.argv[2])}}))' "$text" "$style")"
  curl -fsS -X POST "https://api.elevenlabs.io/v1/text-to-speech/$voice" \
       -H "xi-api-key: $KEY" -H "Content-Type: application/json" \
       --data-binary "$body" -o "$WORK/seg$idx.mp3"
  ffmpeg -y -loglevel error -i "$WORK/seg$idx.mp3" -ar 44100 -ac 2 "$WORK/seg$idx.wav"
  dur=$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$WORK/seg$idx.wav" | awk '{printf "%d", $1*1000}')
  start=$t; end=$((t+dur))
  { echo "$idx"; echo "$(ms2srt $start) --> $(ms2srt $end)"; echo "$label: $text"; echo ""; } >> "$SRT"
  t=$((end+GAP))
  echo "file '$WORK/seg$idx.wav'" >> "$list"
  echo "file '$WORK/sil.wav'"     >> "$list"
done

ffmpeg -y -loglevel error -f concat -safe 0 -i "$list" -ar 44100 -ac 2 -c:a pcm_s16le "$WAV"
echo "WAV: $WAV ($(du -h "$WAV" | cut -f1))"

# MKV: purple waveform video + audio + embedded soft subtitles
ffmpeg -y -loglevel error -i "$WAV" -i "$SRT" -filter_complex \
  "[0:a]showwaves=s=1280x720:mode=cline:rate=25:colors=0x8b5cf6|0xc084fc,format=yuv420p[v]" \
  -map "[v]" -map 0:a -map 1 -c:v libx264 -preset veryfast -c:a aac -b:a 192k -c:s srt "$MKV"
echo "MKV: $MKV ($(du -h "$MKV" | cut -f1))"
echo "SRT: $SRT"
