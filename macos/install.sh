#!/bin/bash
# Builds Signalbox and installs ~/Applications/Signalbox.app (Spotlight, Dock, Finder) and the
# `signalbox` command. The bundle holds a copy of the release build, signed ad hoc so macOS lets
# it post notifications. Run this again after building, or let the `signalbox` command do it.
set -euo pipefail
repo="$(cd "$(dirname "$0")/.." && pwd)"
app="$HOME/Applications/Signalbox.app"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

(cd "$repo" && cargo build --release)

# The icon: macos/icon.svg, drawn by Quick Look, at every size the iconset wants.
qlmanage -t -s 1024 -o "$work" "$repo/macos/icon.svg" > /dev/null 2>&1
mkdir -p "$work/Signalbox.iconset"
for px in 16 32 128 256 512; do
    sips -z $px $px "$work/icon.svg.png" --out "$work/Signalbox.iconset/icon_${px}x${px}.png" > /dev/null
    sips -z $((px * 2)) $((px * 2)) "$work/icon.svg.png" --out "$work/Signalbox.iconset/icon_${px}x${px}@2x.png" > /dev/null
done
iconutil -c icns "$work/Signalbox.iconset" -o "$work/Signalbox.icns"

mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$repo/macos/Info.plist" "$app/Contents/Info.plist"
cp "$work/Signalbox.icns" "$app/Contents/Resources/Signalbox.icns"
# A sound for each kind of notification (needs you, finished, news), from macOS's own set.
for sound in Glass Pop Purr; do
    cp "/System/Library/Sounds/$sound.aiff" "$app/Contents/Resources/"
done
"$repo/macos/update.sh"
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$app"

# The command, pointed at this repo for newer builds.
mkdir -p "$HOME/.local/bin"
sed "s|__REPO__|$repo|" "$repo/macos/signalbox" > "$HOME/.local/bin/signalbox"
chmod +x "$HOME/.local/bin/signalbox"
echo "Installed $app and ~/.local/bin/signalbox"
