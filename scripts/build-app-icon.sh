#!/usr/bin/env bash
# Convert the checked-in artwork into macOS icon sizes; preserves PNG alpha.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
brand="$root/crates/quill-app/assets/brand"
iconset="$root/target/Roughcut.iconset"
mkdir -p "$iconset"
sips -z 1024 1024 "$brand/roughcut-icon-master.png" --out "$brand/roughcut-icon.png" >/dev/null
sips -z 128 128 "$brand/roughcut-icon.png" --out "$brand/roughcut-mark.png" >/dev/null
for size in 16 32 128 256 512; do
    sips -z "$size" "$size" "$brand/roughcut-icon.png" --out "$iconset/icon_${size}x${size}.png" >/dev/null
    retina=$((size * 2))
    sips -z "$retina" "$retina" "$brand/roughcut-icon.png" --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$root/crates/quill-app/macos/Roughcut.icns"
