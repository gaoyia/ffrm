#!/bin/bash
# 把发布用的二进制包成 ffrm.app，再做成 dmg。

set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
bin="${1:-"$root/target/release/ffrm"}"
version="${2:-0.1.5}"
out="${3:-"$root/target/release/ffrm.dmg"}"

if [[ ! -f "$bin" ]]; then
  echo "找不到程序：$bin" >&2
  exit 1
fi

stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT

iconset="$stage/ffrm.iconset"
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" "$root/assets/ffrm.png" --out "$iconset/icon_${size}x${size}.png" >/dev/null
  double=$((size * 2))
  sips -z "$double" "$double" "$root/assets/ffrm.png" --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$stage/ffrm.icns"

app="$stage/ffrm.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$bin" "$app/Contents/MacOS/ffrm"
chmod +x "$app/Contents/MacOS/ffrm"
cp "$stage/ffrm.icns" "$app/Contents/Resources/ffrm.icns"
if [[ -f "$root/LICENSE" ]]; then
  cp "$root/LICENSE" "$app/Contents/Resources/LICENSE"
fi

cat > "$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key>
	<string>zh_CN</string>
	<key>CFBundleDisplayName</key>
	<string>文件强制解锁删除工具</string>
	<key>CFBundleExecutable</key>
	<string>ffrm</string>
	<key>CFBundleIconFile</key>
	<string>ffrm</string>
	<key>CFBundleIdentifier</key>
	<string>com.gaoyia.ffrm</string>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
	<key>CFBundleName</key>
	<string>ffrm</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleShortVersionString</key>
	<string>${version}</string>
	<key>CFBundleVersion</key>
	<string>${version}</string>
	<key>LSMinimumSystemVersion</key>
	<string>12.0</string>
	<key>NSHighResolutionCapable</key>
	<true/>
</dict>
</plist>
EOF

codesign --force --sign - "$app"

payload="$stage/payload"
mkdir -p "$payload"
cp -R "$app" "$payload/ffrm.app"
ln -s /Applications "$payload/Applications"

mkdir -p "$(dirname "$out")"
rm -f "$out"
hdiutil create -volname "ffrm" -srcfolder "$payload" -ov -format UDZO "$out"
echo "已生成 $out"
