#!/usr/bin/env bash
# Builds Quill for Apple Silicon and wraps it in Quill.app, zipped for sharing.
# Usage: scripts/bundle-macos.sh [release|debug]
set -euo pipefail

profile="${1:-release}"
target="aarch64-apple-darwin"
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

flags=(--target "$target" -p quill)
[[ "$profile" == "release" ]] && flags+=(--release)
cargo build "${flags[@]}"

version="$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "quill"))')"
build="$(git rev-list --count HEAD 2>/dev/null || echo 0)"

out="$root/dist"
app="$out/Quill.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "target/$target/$profile/quill" "$app/Contents/MacOS/quill"
sed -e "s/__VERSION__/$version/" -e "s/__BUILD__/$build/" crates/quill-app/macos/Info.plist > "$app/Contents/Info.plist"

# Ad-hoc signature: required to launch on Apple Silicon. Not notarized, so
# first launch on another Mac needs right-click > Open.
codesign --force --deep --sign - "$app"

zip="$out/Quill-$version-macos-arm64.zip"
rm -f "$zip"
ditto -c -k --keepParent "$app" "$zip"
echo "$zip"
