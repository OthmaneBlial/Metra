#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FRAME_DIR="$(mktemp -d "${TMPDIR:-/tmp}/metra-frames.XXXXXX")"
ASSET_DIR="$ROOT_DIR/site/assets"
mkdir -p "$ASSET_DIR"

index=0
for scene in "$ROOT_DIR"/demo/scenes/*.svg; do
  raw="$FRAME_DIR/$(basename "$scene").png"
  frame="$FRAME_DIR/$(printf '%02d' "$index").png"
  qlmanage -t -s 1280 -o "$FRAME_DIR" "$scene" >/dev/null 2>&1
  sips --cropToHeightWidth 720 1280 "$raw" --out "$frame" >/dev/null
  index=$((index + 1))
done

ffmpeg -y -hide_banner -loglevel error \
  -framerate 1/7 -i "$FRAME_DIR/%02d.png" \
  -vf "fps=30,format=yuv420p" \
  -c:v libx264 -crf 19 -preset medium -movflags +faststart \
  "$ASSET_DIR/metra-demo.mp4"

cp "$FRAME_DIR/03.png" "$ASSET_DIR/metra-demo-poster.png"
ffprobe -v error -show_entries format=duration,size \
  -of default=noprint_wrappers=1 "$ASSET_DIR/metra-demo.mp4"
