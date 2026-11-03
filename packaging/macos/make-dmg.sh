#!/bin/bash
# 把发布用的二进制包成 ffrm.app，再做成 dmg。

set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
bin="${1:-"$root/target/release/ffrm"}"
version="${2:-0.1.6}"
out="${3:-"$root/target/release/ffrm.dmg"}"

if [[ ! -f "$bin" ]]; then
  echo "找不到程序：$bin" >&2
  exit 1
fi

stage="$(mktemp -d)"
mount_point=""
cleanup() {
  if [[ -n "$mount_point" && -d "$mount_point" ]]; then
    hdiutil detach "$mount_point" >/dev/null 2>&1 || hdiutil detach -force "$mount_point" >/dev/null 2>&1 || true
  fi
  rm -rf "$stage"
}
trap cleanup EXIT

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

# 窗口尺寸和图标中心。背景图与 Finder 图标视图共用这组坐标。
win_w=720
win_h=440
icon_size=120
left_x=180
right_x=540
icon_y=230

swift "$root/packaging/macos/dmg-background.swift" \
  "$stage/background.png" "$version" \
  "$win_w" "$win_h" "$left_x" "$right_x" "$icon_y"

if [[ -d /Volumes/ffrm ]]; then
  hdiutil detach /Volumes/ffrm >/dev/null 2>&1 || hdiutil detach -force /Volumes/ffrm >/dev/null 2>&1 || true
fi

rw="$stage/rw.dmg"
hdiutil create -size 64m -fs HFS+ -volname "ffrm" -type UDIF "$rw" >/dev/null
attach_out="$(hdiutil attach -readwrite -noverify -noautoopen "$rw")"
device="$(printf '%s\n' "$attach_out" | awk 'NR==1 {print $1}')"
mount="$(printf '%s\n' "$attach_out" | awk '/\/Volumes\// {print $NF; exit}')"
mount_point="$mount"

mkdir -p "$mount/.background"
cp "$stage/background.png" "$mount/.background/background.png"
cp -R "$app" "$mount/ffrm.app"
cp "$stage/ffrm.icns" "$mount/.VolumeIcon.icns"
chflags hidden "$mount/.background"
if command -v SetFile >/dev/null 2>&1; then
  SetFile -a C "$mount" || true
fi

# 安装包窗口：隐藏工具栏，图标固定在左右两侧，背景画出拖拽箭头。
osascript <<EOF
tell application "Finder"
  tell disk "ffrm"
    open
    delay 1
    set current view of container window to icon view
    set toolbar visible of container window to false
    set statusbar visible of container window to false
    set the bounds of container window to {200, 80, $((200 + win_w)), $((80 + win_h))}
    set viewOptions to the icon view options of container window
    set arrangement of viewOptions to not arranged
    set icon size of viewOptions to $icon_size
    set text size of viewOptions to 12
    set background picture of viewOptions to file ".background:background.png"
    if not (exists item "应用程序" of container window) then
      make new alias file at container window to (POSIX file "/Applications") with properties {name:"应用程序"}
    end if
    set position of item "ffrm.app" of container window to {$left_x, $icon_y}
    set position of item "应用程序" of container window to {$right_x, $icon_y}
    close
    open
    update without registering applications
    delay 2
    close
  end tell
end tell
EOF

sync
sleep 1
hdiutil detach "$device" >/dev/null
mount_point=""

mkdir -p "$(dirname "$out")"
rm -f "$out"
hdiutil convert "$rw" -format UDZO -imagekey zlib-level=9 -o "$out" >/dev/null
echo "已生成 $out"
