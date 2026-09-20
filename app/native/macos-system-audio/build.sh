#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
native_dir=$(CDPATH= cd -- "$script_dir/.." && pwd)
output_dir="$native_dir/bin"
mkdir -p "$output_dir"
module_cache="${TMPDIR:-/tmp}/lectureedit-swift-module-cache"
mkdir -p "$module_cache"
if [ -n "${SDKROOT:-}" ]; then
  sdk_path="$SDKROOT"
elif [ -d /Library/Developer/CommandLineTools/SDKs/MacOSX15.4.sdk ]; then
  # The current CLT beta may pair a newer compiler with a slightly older SDK build.
  # The 15.4 SDK contains every API used here and still supports the 13.0 target.
  sdk_path=/Library/Developer/CommandLineTools/SDKs/MacOSX15.4.sdk
else
  sdk_path=$(xcrun --sdk macosx --show-sdk-path)
fi

case "$(uname -m)" in
  arm64) target="arm64-apple-macosx13.0" ;;
  x86_64) target="x86_64-apple-macosx13.0" ;;
  *) echo "Unsupported macOS architecture: $(uname -m)" >&2; exit 1 ;;
esac

xcrun swiftc \
  -O \
  -target "$target" \
  -sdk "$sdk_path" \
  -module-cache-path "$module_cache" \
  -framework CoreMedia \
  -framework ScreenCaptureKit \
  "$script_dir/SystemAudioCapture.swift" \
  -o "$output_dir/lectureedit-capture"

echo "$output_dir/lectureedit-capture"
