#!/usr/bin/env bash
# Stitch all saga videos (each already carries background text/subtitles) into ONE long film.
set -uo pipefail
OUT="${1:-/tmp/flux-all.mp4}"
ORDER="saga7 saga8 saga9 saga10 saga11 trilogy interview summit"
: > /tmp/clist.txt
i=0
for v in $ORDER; do
  src="$(ls /tmp/flux-$v.mkv /tmp/flux-$v.mp4 2>/dev/null | head -1)"
  if [ -z "$src" ]; then echo "skip $v (missing)"; continue; fi
  i=$((i+1))
  ffmpeg -nostdin -y -loglevel error -i "$src" -map 0:v:0 -map 0:a:0 \
    -vf "scale=1280:720:force_original_aspect_ratio=decrease,pad=1280:720:-1:-1,setsar=1,fps=25" \
    -c:v libx264 -preset veryfast -pix_fmt yuv420p -ar 44100 -ac 2 -c:a aac -b:a 192k "/tmp/nf$i.mp4"
  echo "file '/tmp/nf$i.mp4'" >> /tmp/clist.txt
  echo "normalized $v -> nf$i.mp4"
done
ffmpeg -nostdin -y -loglevel error -f concat -safe 0 -i /tmp/clist.txt -c copy "$OUT"
echo "FILM: $OUT ($(du -h "$OUT" | cut -f1)) dur=$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$OUT")s"
