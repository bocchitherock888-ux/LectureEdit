#!/usr/bin/env bash
# Clone or update the freeze-candidate llama.cpp and build llama-server + llama-mtmd-cli with Metal.
set -euo pipefail

CACHE="${LECTUREEDIT_CACHE:-$HOME/.cache/lectureedit}"
SRC="$CACHE/vendor/llama.cpp"
BUILD="$CACHE/builds/llama.cpp"
TAG="${LLAMA_CPP_TAG:-v0.4.1}"
COMMIT="${LLAMA_CPP_COMMIT:-391fac16460f15233a7740550d858ac96df3419d}"
CMAKE="${CMAKE:-/opt/homebrew/Caskroom/miniforge/base/bin/cmake}"
JOBS="${JOBS:-8}"

mkdir -p "$CACHE/vendor" "$CACHE/builds" "$CACHE/logs"

if [[ ! -d "$SRC/.git" ]]; then
  git clone --filter=blob:none https://github.com/ggml-org/llama.cpp.git "$SRC"
fi

git -C "$SRC" fetch --tags --force origin
git -C "$SRC" checkout --detach "$COMMIT"
HEAD=$(git -C "$SRC" rev-parse HEAD)
if [[ "$HEAD" != "$COMMIT" ]]; then
  echo "checkout mismatch: $HEAD != $COMMIT" >&2
  exit 1
fi

echo "llama.cpp $HEAD (requested $TAG)"
echo "cmake $CMAKE"

"$CMAKE" -S "$SRC" -B "$BUILD" \
  -DCMAKE_BUILD_TYPE=Release \
  -DGGML_METAL=ON \
  -DGGML_NATIVE=ON \
  -DLLAMA_BUILD_TESTS=OFF \
  -DLLAMA_BUILD_EXAMPLES=OFF \
  -DLLAMA_BUILD_TOOLS=ON \
  -DLLAMA_CURL=ON

# Target names have moved before; request both current and legacy names.
set +e
"$CMAKE" --build "$BUILD" --config Release -j "$JOBS" --target llama-server llama-mtmd-cli
status=$?
if [[ $status -ne 0 ]]; then
  echo "named targets failed; building default tools" >&2
  "$CMAKE" --build "$BUILD" --config Release -j "$JOBS"
  status=$?
fi
set -e
if [[ $status -ne 0 ]]; then
  exit "$status"
fi

echo "binaries:"
find "$BUILD" -type f \( -name 'llama-server' -o -name 'llama-mtmd-cli' -o -name 'llama-cli' \) -perm +111 -print
git -C "$SRC" rev-parse HEAD > "$CACHE/builds/llama.cpp.commit"
git -C "$SRC" describe --tags --always > "$CACHE/builds/llama.cpp.tag" || true
