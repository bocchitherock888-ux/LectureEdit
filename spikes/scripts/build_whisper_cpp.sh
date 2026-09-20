#!/usr/bin/env bash
# Clone whisper.cpp, build the CLI, and download the small English fallback model.
set -euo pipefail

CACHE="${LECTUREEDIT_CACHE:-$HOME/.cache/lectureedit}"
SRC="$CACHE/vendor/whisper.cpp"
BUILD="$CACHE/builds/whisper.cpp"
CMAKE="${CMAKE:-/opt/homebrew/Caskroom/miniforge/base/bin/cmake}"
JOBS="${JOBS:-8}"
MODEL="${WHISPER_MODEL:-base.en}"

mkdir -p "$CACHE/vendor" "$CACHE/builds" "$CACHE/models/whisper"

if [[ ! -d "$SRC/.git" ]]; then
  git clone --filter=blob:none https://github.com/ggml-org/whisper.cpp.git "$SRC"
fi

git -C "$SRC" fetch --tags --force origin
git -C "$SRC" checkout --detach origin/master

"$CMAKE" -S "$SRC" -B "$BUILD" -DCMAKE_BUILD_TYPE=Release -DWHISPER_METAL=ON
"$CMAKE" --build "$BUILD" --config Release -j "$JOBS"

# Official helper writes into the source models/ directory.
if [[ -x "$SRC/models/download-ggml-model.sh" ]]; then
  (cd "$SRC" && bash models/download-ggml-model.sh "$MODEL")
  if [[ -f "$SRC/models/ggml-${MODEL}.bin" ]]; then
    cp -n "$SRC/models/ggml-${MODEL}.bin" "$CACHE/models/whisper/" || true
  fi
fi

git -C "$SRC" rev-parse HEAD > "$CACHE/builds/whisper.cpp.commit"
echo "whisper.cpp $(cat "$CACHE/builds/whisper.cpp.commit")"
find "$BUILD" -type f -name 'whisper-cli' -o -name 'main' | head
