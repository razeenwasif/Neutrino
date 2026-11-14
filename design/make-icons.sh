#!/bin/sh
# Renders the app icons from icon.svg (iOS, full bleed) and icon-macos.svg
# (macOS plate and shadow) into the apps' asset catalogs, once those exist.
# Requires rsvg-convert (brew install librsvg).
set -eu
here=$(cd "$(dirname "$0")" && pwd)
apps="$here/../apps"

# Previews kept next to the sources.
rsvg-convert -w 1024 -h 1024 -b '#0B0A10' "$here/icon.svg" -o "$here/icon-1024.png"
rsvg-convert -w 1024 -h 1024 "$here/icon-macos.svg" -o "$here/icon-macos-1024.png"

ios="$apps/iOS/Assets.xcassets/AppIcon.appiconset"
mac="$apps/Mac/Assets.xcassets/AppIcon.appiconset"
[ -d "$apps/iOS" ] || [ -d "$apps/Mac" ] || { echo "No apps/ yet; wrote the previews only."; exit 0; }

if [ -d "$apps/iOS" ]; then
  mkdir -p "$ios"
  # iOS takes one opaque 1024 image and rounds it itself.
  cp "$here/icon-1024.png" "$ios/icon-1024.png"
  cat > "$ios/Contents.json" <<JSON
{
  "images" : [
    { "filename" : "icon-1024.png", "idiom" : "universal", "platform" : "ios", "size" : "1024x1024" }
  ],
  "info" : { "author" : "xcode", "version" : 1 }
}
JSON
fi

if [ -d "$apps/Mac" ]; then
  mkdir -p "$mac"
  entries=""
  for size in 16 32 128 256 512; do
    for scale in 1 2; do
      px=$((size * scale))
      name="icon-${size}@${scale}x.png"
      rsvg-convert -w "$px" -h "$px" "$here/icon-macos.svg" -o "$mac/$name"
      entries="$entries    { \"filename\" : \"$name\", \"idiom\" : \"mac\", \"scale\" : \"${scale}x\", \"size\" : \"${size}x${size}\" },
"
    done
  done
  entries=$(printf '%s' "$entries" | sed '$ s/,$//')
  cat > "$mac/Contents.json" <<JSON
{
  "images" : [
$entries
  ],
  "info" : { "author" : "xcode", "version" : 1 }
}
JSON
fi
echo "Icons written."
