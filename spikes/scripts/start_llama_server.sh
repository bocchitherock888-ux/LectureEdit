#!/usr/bin/env bash
# Loopback Qwen ASR server with a 16 GB-sane context.
set -euo pipefail
CACHE="${LECTUREEDIT_CACHE:-$HOME/.cache/lectureedit}"
BIN="$CACHE/builds/llama.cpp/bin/llama-server"
MODEL="$CACHE/models/qwen3-asr-0.6b-q8"
KEY="$CACHE/audio/local-api-key.txt"
LOG="$CACHE/logs/llama-server.c4096.log"
CTX="${LLAMA_CTX:-4096}"
HOST="${LLAMA_HOST:-127.0.0.1}"
PORT="${LLAMA_PORT:-8765}"

if [[ ! -x "$BIN" ]]; then
  echo "missing $BIN" >&2
  exit 1
fi
mkdir -p "$CACHE/logs"
exec "$BIN" \
  -m "$MODEL/Qwen3-ASR-0.6B-Q8_0.gguf" \
  --mmproj "$MODEL/mmproj-Qwen3-ASR-0.6B-Q8_0.gguf" \
  --host "$HOST" \
  --port "$PORT" \
  --api-key-file "$KEY" \
  --no-webui \
  --jinja \
  -ngl 99 \
  -c "$CTX" \
  -np 1 \
  > "$LOG" 2>&1
