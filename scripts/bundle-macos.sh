#!/usr/bin/env bash
# Builds Roughcut for Apple Silicon and wraps it in Roughcut.app, zipped for sharing.
# Usage: scripts/bundle-macos.sh [release|debug]
set -euo pipefail

profile="${1:-release}"
case "$profile" in release|debug) ;; *) echo "Use release or debug" >&2; exit 1 ;; esac
target="aarch64-apple-darwin"
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

flags=(-p quill)
binary_dir="target/$profile"
host="$(rustc -vV | sed -n 's/^host: //p')"
if [[ "$host" != "$target" ]]; then
    flags+=(--target "$target")
    binary_dir="target/$target/$profile"
fi
[[ "$profile" == "release" ]] && flags+=(--release)
cargo build "${flags[@]}"

version="$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "quill"))')"
build="$(git rev-list --count HEAD 2>/dev/null || echo 0)"

out="$root/dist"
app="$out/Roughcut.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$binary_dir/roughcut" "$app/Contents/MacOS/roughcut"
cp crates/quill-app/macos/Roughcut.icns "$app/Contents/Resources/Roughcut.icns"
sed -e "s/__VERSION__/$version/" -e "s/__BUILD__/$build/" crates/quill-app/macos/Info.plist > "$app/Contents/Info.plist"

# Ad-hoc signature: required to launch on Apple Silicon. Not notarized, so
# first launch on another Mac needs right-click > Open.
codesign --force --deep --sign - "$app"

zip="$out/Roughcut-$version-macos-arm64.zip"
ditto -c -k --keepParent "$app" "$zip"
echo "$zip"
