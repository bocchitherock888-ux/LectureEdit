#!/bin/sh
set -eu

llama_commit=391fac16460f15233a7740550d858ac96df3419d
llama_url=https://github.com/ggml-org/llama.cpp.git
script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
app_dir=$(CDPATH= cd -- "$script_dir/.." && pwd)
source_dir="$app_dir/.native-build/source/llama"
build_dir="$app_dir/.native-build/llama"

for command_name in git cmake node xcrun; do
  if ! command -v "$command_name" >/dev/null 2>&1; then
    echo "Required build command is missing: $command_name" >&2
    exit 1
  fi
done

"$app_dir/native/macos-system-audio/build.sh"

if [ ! -d "$source_dir/.git" ]; then
  if [ -e "$source_dir" ]; then
    echo "Expected a llama.cpp Git checkout at $source_dir" >&2
    exit 1
  fi
  mkdir -p "$source_dir"
  git -C "$source_dir" init
  git -C "$source_dir" remote add origin "$llama_url"
fi

if ! git -C "$source_dir" cat-file -e "$llama_commit^{commit}" 2>/dev/null; then
  git -C "$source_dir" fetch --depth 1 origin "$llama_commit"
fi
git -C "$source_dir" checkout --detach "$llama_commit"
actual_commit=$(git -C "$source_dir" rev-parse HEAD)
if [ "$actual_commit" != "$llama_commit" ]; then
  echo "llama.cpp checkout mismatch: expected $llama_commit, found $actual_commit" >&2
  exit 1
fi

if [ -f "$build_dir/CMakeCache.txt" ]; then
  cached_source=$(sed -n 's/^CMAKE_HOME_DIRECTORY:INTERNAL=//p' "$build_dir/CMakeCache.txt")
  if [ "$cached_source" != "$source_dir" ]; then
    echo "Replacing stale llama.cpp build cache from $cached_source"
    cmake -E remove_directory "$build_dir"
  fi
fi

cmake \
  -S "$source_dir" \
  -B "$build_dir" \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_OSX_DEPLOYMENT_TARGET=13.0 \
  -DGGML_METAL=ON \
  -DGGML_METAL_EMBED_LIBRARY=ON \
  -DGGML_METAL_MACOSX_VERSION_MIN=13.0 \
  -DGGML_BLAS=OFF \
  -DGGML_NATIVE=OFF \
  -DGGML_OPENMP=OFF \
  -DLLAMA_OPENSSL=OFF \
  -DLLAMA_CURL=OFF \
  -DLLAMA_BUILD_UI=OFF \
  -DLLAMA_USE_PREBUILT_UI=OFF \
  -DLLAMA_BUILD_SERVER=ON \
  -DLLAMA_BUILD_APP=OFF \
  -DLLAMA_BUILD_EXAMPLES=OFF \
  -DLLAMA_BUILD_TESTS=OFF \
  -DLLAMA_BUILD_TOOLS=ON \
  -DBUILD_SHARED_LIBS=ON
cmake -E remove_directory "$build_dir/bin"
cmake -E make_directory "$build_dir/bin"
cmake -E remove_directory "$build_dir/tools/ui/dist"
cmake --build "$build_dir" --config Release --target llama-server --parallel

if [ ! -x "$build_dir/bin/llama-server" ]; then
  echo "llama-server build output is missing: $build_dir/bin/llama-server" >&2
  exit 1
fi

cmake -E remove_directory "$app_dir/src-tauri/resources/native"
node "$app_dir/scripts/package-native.mjs"
