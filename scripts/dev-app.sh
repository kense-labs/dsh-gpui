#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
export PATH="$(dirname "$(rustup which rustc)"):$PATH"

npm run ui:build
# blaze is an opt-in accelerator for the native Host only (never for the WASM
# frontend or release builds). The wrapper probes compatibility once and falls
# back to cargo for good when blaze cannot build this workspace yet.
if [[ "${KWORK_USE_BLAZE:-0}" == "1" ]]; then
    node scripts/blaze-build.mjs
else
    cargo build -p dsh-gpui
fi

APP="$ROOT/.tools/dev-app/KWork.app"
CONTENTS="$APP/Contents"
mkdir -p "$CONTENTS/MacOS" "$CONTENTS/Resources"
# Publish a fresh inode so macOS keeps the prior executable and signature
# intact if a previous development build is still running.
APP_EXECUTABLE_TMP="$(mktemp "$CONTENTS/MacOS/.KWork.XXXXXX")"
trap 'rm -f "$APP_EXECUTABLE_TMP"' EXIT
cp -p "$ROOT/target/debug/KWork" "$APP_EXECUTABLE_TMP"
mv -f "$APP_EXECUTABLE_TMP" "$CONTENTS/MacOS/KWork"
trap - EXIT
cp "$ROOT/src-tauri/icons/icon.icns" "$CONTENTS/Resources/icon.icns"
cat > "$CONTENTS/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key><string>zh_CN</string>
  <key>CFBundleDisplayName</key><string>KWork</string>
  <key>CFBundleExecutable</key><string>KWork</string>
  <key>CFBundleIconFile</key><string>icon</string>
  <key>CFBundleIdentifier</key><string>com.kense-labs.dsh-gpui.dev</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleName</key><string>KWork</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSEnvironment</key>
  <dict>
    <key>PATH</key>
    <string>/Users/leon/.local/share/fnm/aliases/default/bin:/Users/leon/.local/share/fnm/current/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin</string>
  </dict>
</dict>
</plist>
PLIST

cd "$ROOT"
if [[ "${DSH_DEV_APP_BUNDLE_ONLY:-0}" == "1" ]]; then
    exit 0
fi
exec "$CONTENTS/MacOS/KWork"
