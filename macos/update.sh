#!/bin/bash
# Puts the latest release build into ~/Applications/Signalbox.app and signs it ad hoc.
set -euo pipefail
repo="$(cd "$(dirname "$0")/.." && pwd)"
app="$HOME/Applications/Signalbox.app"
cp -f "$repo/target/release/signalbox" "$app/Contents/MacOS/Signalbox.new"
mv -f "$app/Contents/MacOS/Signalbox.new" "$app/Contents/MacOS/Signalbox"
codesign --force --sign - "$app" 2> /dev/null
