#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
output="$root/assets/social-preview.png"
regular="/System/Library/Fonts/SFNSMono.ttf"
bold="$HOME/Library/Fonts/JetBrainsMono-Bold.ttf"

mkdir -p "$root/assets"

magick -size 1280x640 canvas:'#000000' \
  -font "$bold" -fill '#ffffff' -pointsize 82 \
  -draw "text 72,116 'zz'" \
  -font "$regular" -fill '#888888' -pointsize 24 \
  -draw "text 74,158 'prompt editor for coding agents'" \
  -fill '#080808' -stroke '#383838' -strokewidth 2 \
  -draw "roundrectangle 72,200 1208,566 10,10" \
  -stroke none -font "$regular" -fill '#ffffff' -pointsize 32 \
  -draw "text 112,286 'Fix the draft recovery bug.'" \
  -draw "text 112,338 'Add a regression test for'" \
  -fill '#383838' \
  -draw "roundrectangle 104,358 585,410 4,4" \
  -fill '#ffffff' -pointsize 30 \
  -draw "text 112,395 '@src/storage.rs:128-176'" \
  -fill '#383838' \
  -draw "rectangle 72,492 1208,494" \
  -fill '#888888' -pointsize 22 \
  -draw "text 112,540 '@ context     ZZ accept     ZQ cancel'" \
  -depth 8 -define png:color-type=2 \
  "$output"

printf '%s\n' "$output"
