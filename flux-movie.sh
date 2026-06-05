#!/usr/bin/env bash
# flux-movie - stitch a lines file into ONE rich video: full-screen styled text per
# scene, speaker label, fade-shift between scenes, voices per character. Also emits WAV.
set -euo pipefail
KEY="$(tr -d '[:space:]' < "${ELEVEN_KEY_FILE:-$HOME/.config/flux/eleven_key}")"
LINES="$1"; MP4="$2"; WAV="$3"
WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
FONT=/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf

vid(){ case "$1" in
  GRACE) echo EXAVITQu4vr4xnSDxMaL;; ROCKY) echo nPczCjzI2devNBz1zQrb;;
  MATILDA) echo XrExE9yKIg1WjnnlVkGX;; ELENA) echo pFZP5JQG7iQjIQuC4Bku;;
  CHARLIE) echo IKne3meq5aSn9XLyUdCD;; LAURA) echo FGY2WhTYpPnrIDTdsKH5;;
  HARRY) echo SOYHLrjzK2X1ezoPC6cr;; *) echo nPczCjzI2devNBz1zQrb;; esac; }

vlist="$WORK/v.txt"; alist="$WORK/a.txt"; : > "$vlist"; : > "$alist"; idx=0
while IFS='|' read -r name style label text <&3; do
  [ -z "${name:-}" ] && continue
  idx=$((idx+1)); voice="$(vid "$name")"
  body="$(python3 -c 'import json,sys; print(json.dumps({"text":sys.argv[1],"model_id":"eleven_multilingual_v2","voice_settings":{"stability":0.42,"similarity_boost":0.85,"style":float(sys.argv[2])}}))' "$text" "$style")"
  curl -fsS -X POST "https://api.elevenlabs.io/v1/text-to-speech/$voice" \
       -H "xi-api-key: $KEY" -H "Content-Type: application/json" --data-binary "$body" -o "$WORK/seg$idx.mp3"
  ffmpeg -nostdin -y -loglevel error -i "$WORK/seg$idx.mp3" -ar 44100 -ac 2 "$WORK/seg$idx.wav"
  dur=$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$WORK/seg$idx.wav")
  # wrap body to a textfile (avoids drawtext escaping), uppercase label sanitized
  python3 -c 'import textwrap,sys; open(sys.argv[2],"w").write(textwrap.fill(sys.argv[1],36))' "$text" "$WORK/t$idx.txt"
  spk="$(printf '%s' "$label" | sed "s/[:\x27\\]/ /g")"
  ffmpeg -nostdin -y -loglevel error -f lavfi -i "color=c=0x0a0a0f:s=1280x720:r=25" -i "$WORK/seg$idx.wav" \
    -filter_complex "[0:v]drawtext=fontfile=$FONT:text='$spk':fontcolor=0xfbbf24:fontsize=30:x=70:y=70:borderw=0,drawtext=fontfile=$FONT:textfile=$WORK/t$idx.txt:fontcolor=0xc084fc:fontsize=46:line_spacing=18:x=(w-text_w)/2:y=(h-text_h)/2,drawtext=fontfile=$FONT:text='FLUX':fontcolor=0x2a2140:fontsize=22:x=w-110:y=h-50,fade=t=in:st=0:d=0.35[v]" \
    -map "[v]" -map 1:a -t "$dur" -c:v libx264 -preset veryfast -pix_fmt yuv420p -c:a aac -b:a 192k "$WORK/clip$idx.mp4"
  echo "file '$WORK/clip$idx.mp4'" >> "$vlist"
  echo "file '$WORK/seg$idx.wav'" >> "$alist"
done 3< "$LINES"

ffmpeg -nostdin -y -loglevel error -f concat -safe 0 -i "$vlist" -c copy "$MP4" 2>/dev/null \
  || ffmpeg -nostdin -y -loglevel error -f concat -safe 0 -i "$vlist" -c:v libx264 -preset veryfast -c:a aac "$MP4"
ffmpeg -nostdin -y -loglevel error -f concat -safe 0 -i "$alist" -ar 44100 -ac 2 -c:a pcm_s16le "$WAV"
echo "MOVIE: $MP4 ($(du -h "$MP4" | cut -f1))  WAV: $WAV"
