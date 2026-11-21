#!/usr/bin/env bash
# Local development bundle only; does not sign, notarize, install or publish.
set -euo pipefail
if [[ "$(uname -s)" != Darwin ]]; then
  echo 'This helper builds a local macOS development app.' >&2
  exit 1
fi
repo_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_dir"
cargo build --locked --bin video-toolkit
build_dir="${CARGO_TARGET_DIR:-$repo_dir/target}"
app_dir="$build_dir/local/Video Toolkit.app"
mkdir -p "$app_dir/Contents/MacOS"
mkdir -p "$app_dir/Contents/Resources/Fonts"
cp "$build_dir/debug/video-toolkit" "$app_dir/Contents/MacOS/video-toolkit"
cp "$repo_dir/assets/fonts/OFL.txt" "$app_dir/Contents/Resources/Fonts/DM-Sans-OFL.txt"
cat > "$app_dir/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>video-toolkit</string>
<key>CFBundleIdentifier</key><string>io.github.alaneel.video-toolkit.local</string>
<key>CFBundleName</key><string>Video Toolkit</string>
<key>CFBundleDisplayName</key><string>Video Toolkit</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleVersion</key><string>1</string>
<key>CFBundleShortVersionString</key><string>0.1</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
echo "$app_dir"
