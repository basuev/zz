#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
output="$root/assets/social-preview.png"
regular="/System/Library/Fonts/SFNSMono.ttf"
bold="$HOME/Library/Fonts/JetBrainsMono-Bold.ttf"

mkdir -p "$root/assets"

magick -size 1280x640 canvas:'#000000' \
  -font "$bold" -fill '#ffffff' -pointsize 166 \
  -draw "text 72,205 'zz'" \
  -font "$regular" -fill '#888888' -pointsize 24 \
  -draw "text 404,105 'MODAL PROMPT EDITOR'" \
  -fill '#ffffff' -pointsize 43 \
  -draw "text 400,165 'Write serious prompts'" \
  -draw "text 400,220 'for coding agents.'" \
  -fill '#1d1d1d' -stroke '#383838' -strokewidth 2 \
  -draw "roundrectangle 72,300 1208,558 10,10" \
  -fill '#888888' -stroke none \
  -draw "circle 104,332 109,332 circle 128,332 133,332 circle 152,332 157,332" \
  -font "$regular" -fill '#888888' -pointsize 22 \
  -draw "text 98,392 '$ VISUAL=zz your-agent-command'" \
  -fill '#ffffff' -pointsize 25 \
  -draw "text 98,447 '> Fix draft recovery in @src/storage.rs:128-176'" \
  -fill '#888888' -pointsize 20 \
  -draw "text 98,515 'LOCAL HISTORY   CRASH RECOVERY   EXACT FILE RANGES'" \
  -fill '#888888' -pointsize 18 \
  -draw "text 72,606 'github.com/basuev/zz'" \
  "$output"

printf '%s\n' "$output"
