#!/usr/bin/env bash
# Download the Qwen3-ASR-0.6B Q8 decoder + matching mmproj into the native cache.
# Does not start a server and does not send audio.
set -euo pipefail

CACHE="${LECTUREEDIT_CACHE:-$HOME/.cache/lectureedit}"
DEST="$CACHE/models/qwen3-asr-0.6b-q8"
REPO="ggml-org/Qwen3-ASR-0.6B-GGUF"
HF_CLI="${HF_CLI:-/opt/homebrew/Caskroom/miniforge/base/bin/huggingface-cli}"

DECODER="Qwen3-ASR-0.6B-Q8_0.gguf"
MMPROJ="mmproj-Qwen3-ASR-0.6B-Q8_0.gguf"
DECODER_SHA="bca259818b50ca7c4c05e9bdb35a5dc04fa039653a6d6f3f0f331f96f6aa1971"
MMPROJ_SHA="41a342b5e4c514e968cb756de6cd1b7be39eff43c44c57a2ef5fc6522e36603d"
DECODER_BYTES=804749248
MMPROJ_BYTES=214392480

mkdir -p "$DEST"

need() {
  local path="$1" sha="$2" bytes="$3"
  if [[ -f "$path" ]]; then
    local actual_bytes actual_sha
    actual_bytes=$(stat -f%z "$path")
    if [[ "$actual_bytes" == "$bytes" ]]; then
      actual_sha=$(shasum -a 256 "$path" | awk '{print $1}')
      if [[ "$actual_sha" == "$sha" ]]; then
        echo "ok $path"
        return 1
      fi
    fi
    echo "mismatch $path; re-downloading"
    rm -f "$path"
  fi
  return 0
}

if ! command -v "$HF_CLI" >/dev/null 2>&1; then
  echo "huggingface-cli not found at $HF_CLI" >&2
  exit 1
fi

echo "source https://huggingface.co/$REPO"
echo "planned files:"
echo "  $DECODER  $DECODER_BYTES bytes  sha256:$DECODER_SHA"
echo "  $MMPROJ   $MMPROJ_BYTES bytes  sha256:$MMPROJ_SHA"
echo "  pair total $((DECODER_BYTES + MMPROJ_BYTES)) bytes (~1.019 GB decimal)"

download_one=0
if need "$DEST/$DECODER" "$DECODER_SHA" "$DECODER_BYTES"; then download_one=1; fi
if need "$DEST/$MMPROJ" "$MMPROJ_SHA" "$MMPROJ_BYTES"; then download_one=1; fi

if [[ "$download_one" -eq 1 ]]; then
  "$HF_CLI" download "$REPO" "$DECODER" "$MMPROJ" --local-dir "$DEST"
fi

fail=0
for spec in "$DECODER:$DECODER_SHA:$DECODER_BYTES" "$MMPROJ:$MMPROJ_SHA:$MMPROJ_BYTES"; do
  IFS=: read -r name sha bytes <<<"$spec"
  path="$DEST/$name"
  actual_bytes=$(stat -f%z "$path")
  actual_sha=$(shasum -a 256 "$path" | awk '{print $1}')
  echo "$name bytes=$actual_bytes sha256=$actual_sha"
  if [[ "$actual_bytes" != "$bytes" || "$actual_sha" != "$sha" ]]; then
    echo "VERIFY FAILED $name" >&2
    fail=1
  fi
done
exit "$fail"
