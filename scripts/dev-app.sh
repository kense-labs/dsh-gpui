#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
export PATH="$(dirname "$(rustup which rustc)"):$PATH"

npm run ui:build
cargo build -p dsh-gpui

APP="$ROOT/.tools/dev-app/DeepSeek Harness.app"
CONTENTS="$APP/Contents"
mkdir -p "$CONTENTS/MacOS" "$CONTENTS/Resources"
cp "$ROOT/target/debug/DeepSeekHarness" "$CONTENTS/MacOS/DeepSeekHarness"
cp "$ROOT/src-tauri/icons/icon.icns" "$CONTENTS/Resources/icon.icns"
cat > "$CONTENTS/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key><string>zh_CN</string>
  <key>CFBundleDisplayName</key><string>DeepSeek Harness</string>
  <key>CFBundleExecutable</key><string>DeepSeekHarness</string>
  <key>CFBundleIconFile</key><string>icon</string>
  <key>CFBundleIdentifier</key><string>com.kense-labs.dsh-gpui.dev</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleName</key><string>DeepSeek Harness</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

cd "$ROOT"
if [[ "${DSH_DEV_APP_BUNDLE_ONLY:-0}" == "1" ]]; then
    exit 0
fi
exec "$CONTENTS/MacOS/DeepSeekHarness"
