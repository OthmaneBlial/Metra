#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
METRA_BIN="${METRA_BIN:-$ROOT_DIR/target/release/metra}"
TRANSCRIPT_PATH="${1:-${TMPDIR:-/tmp}/metra-demo-transcript.txt}"
WORK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/metra-demo.XXXXXX")"

if [[ ! -x "$METRA_BIN" ]]; then
  cargo build --release --manifest-path "$ROOT_DIR/Cargo.toml"
fi

run() {
  printf '\n$'
  printf ' %q' "$@"
  printf '\n'
  "$@"
}

{
  printf '%s\n' 'METRA / REAL CLI WALKTHROUGH' 'A bounded, local-first metadata workflow in Rust.'
  printf '%s\n' "Workspace: $WORK_DIR"

  run "$METRA_BIN" --create-png 'Comment=Metra demo' "$WORK_DIR/demo.png"
  run "$METRA_BIN" --set 'PNG:ModificationTime=2026-09-14 12:34:56' "$WORK_DIR/demo.png"
  run "$METRA_BIN" --tag PNG:ModificationTime "$WORK_DIR/demo.png"

  run "$METRA_BIN" --create-tiff 'EXIF:Make=Metra' "$WORK_DIR/camera.tif"
  run "$METRA_BIN" --tag EXIF:Make "$WORK_DIR/camera.tif"

  run "$METRA_BIN" --create-wav 'Title=Metra' --create-wav 'Artist=Metra' "$WORK_DIR/take.wav"
  run "$METRA_BIN" --jsonl "$WORK_DIR/take.wav"

  run "$METRA_BIN" --capabilities
} > "$TRANSCRIPT_PATH" 2>&1

printf 'Transcript written to %s\n' "$TRANSCRIPT_PATH"
printf 'Generated demo files remain in %s\n' "$WORK_DIR"
