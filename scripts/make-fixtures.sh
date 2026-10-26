#!/usr/bin/env bash
# Generate the small media fixtures used by examples/plans/media.json.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FFMPEG="${FFMPEG_PATH:-ffmpeg}"
mkdir -p "$ROOT/fixtures"

"$FFMPEG" -y -v error -f lavfi -i testsrc2=size=640x360:rate=30:duration=2 \
  -f lavfi -i sine=frequency=440:sample_rate=48000:duration=2 \
  -c:v libx264 -pix_fmt yuv420p -c:a aac -shortest "$ROOT/fixtures/sample.mp4"
"$FFMPEG" -y -v error -f lavfi -i color=c=0x1f6feb:s=640x360:r=1 \
  -frames:v 1 "$ROOT/fixtures/sample.png"
"$FFMPEG" -y -v error -f lavfi -i sine=frequency=220:sample_rate=48000:duration=2 \
  -ac 2 -c:a pcm_s16le "$ROOT/fixtures/tone.wav"
echo "fixtures written to $ROOT/fixtures"
