#!/bin/zsh
set -euo pipefail

repo_root="${0:A:h:h}"
profile="${1:-release}"
app_root="$repo_root/dist/岛仔.app"
contents="$app_root/Contents"
resources="$contents/Resources"
binary="$repo_root/target/$profile/daozai"
framework="$repo_root/macos/mediaremote-adapter/build/MediaRemoteAdapter.framework"

if [[ ! -x "$binary" ]]; then
  echo "找不到 $binary，请先运行 cargo build --$profile --bin daozai" >&2
  exit 1
fi

if [[ ! -d "$framework" ]]; then
  cmake -S "$repo_root/macos/mediaremote-adapter" \
    -B "$repo_root/macos/mediaremote-adapter/build" \
    -DCMAKE_BUILD_TYPE=Release
  cmake --build "$repo_root/macos/mediaremote-adapter/build" --config Release -j 6
fi

rm -rf "$app_root"
mkdir -p "$contents/MacOS" "$resources"
cp "$repo_root/macos/Info.plist" "$contents/Info.plist"
cp "$binary" "$contents/MacOS/daozai"
cp "$repo_root/assets/health-reminder.wav" "$resources/"
cp "$repo_root/macos/mediaremote-adapter/bin/mediaremote-adapter.pl" "$resources/"
cp -R "$framework" "$resources/"
cp "$repo_root/macos/mediaremote-adapter/LICENSE" "$resources/MediaRemoteAdapter.LICENSE"
chmod +x "$contents/MacOS/daozai"

iconset="$(mktemp -d)/Daozai.iconset"
mkdir -p "$iconset"
source_icon="$repo_root/assets/daozai-logo.png"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" "$source_icon" --out "$iconset/icon_${size}x${size}.png" >/dev/null
  double=$((size * 2))
  sips -z "$double" "$double" "$source_icon" --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$resources/Daozai.icns"

codesign --force --deep --sign - "$app_root" >/dev/null
archive="$repo_root/dist/Daozai-macOS-arm64.zip"
rm -f "$archive"
ditto -c -k --sequesterRsrc --keepParent "$app_root" "$archive"
echo "$app_root"
echo "$archive"
