#!/bin/zsh
# Builds the macOS release (DMG + ZIP) into app/dist-release/, then removes the intermediate
# LectureEdit.app so Spotlight and Launchpad only ever show the copy installed in /Applications.
set -euo pipefail

app_dir=${0:A:h:h:h}
cd "$app_dir"
version=$(node -p "require('./package.json').version")
bundle=src-tauri/target/release/bundle
out=dist-release
lsregister=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister

APPLE_SIGNING_IDENTITY="${APPLE_SIGNING_IDENTITY:--}" npm run tauri build -- --bundles app,dmg

# Without this entitlement the hardened runtime records silence from the microphone.
if ! codesign -d --entitlements - "$bundle/macos/LectureEdit.app" 2>/dev/null | grep -q device.audio-input; then
  echo "error: LectureEdit.app is missing com.apple.security.device.audio-input" >&2
  exit 1
fi

mkdir -p "$out"
name="LectureEdit-$version-macOS-Apple-Silicon"
cp "$bundle/dmg/LectureEdit_${version}_aarch64.dmg" "$out/$name.dmg"
ditto -c -k --keepParent "$bundle/macos/LectureEdit.app" "$out/$name.zip"

"$lsregister" -u "$PWD/$bundle/macos/LectureEdit.app" 2>/dev/null || true
/bin/rm -r "$bundle/macos/LectureEdit.app"
# The DMG step mounts a temporary /Volumes/dmg.XXXXXX volume; its app stays registered after unmount.
"$lsregister" -dump 2>/dev/null | sed -n 's|^path: *\(/Volumes/dmg\.[^/]*/LectureEdit\.app\).*|\1|p' | sort -u | while read -r stale; do
  "$lsregister" -u "$stale" 2>/dev/null || true
done
(cd "$out" && shasum -a 256 "$name.dmg" "$name.zip")
echo "Release files: $PWD/$out"
