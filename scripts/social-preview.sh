#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
output="$root/assets/social-preview.png"
regular="/System/Library/Fonts/SFNSMono.ttf"
bold="$HOME/Library/Fonts/JetBrainsMono-Bold.ttf"

mkdir -p "$root/assets"

magick -size 1280x640 canvas:'#000000' \
  -gravity center \
  -font "$bold" -fill '#ffffff' -pointsize 210 \
  -annotate +0-62 'zz' \
  -font "$regular" -fill '#888888' -pointsize 27 \
  -annotate +0+118 'modal prompt editor for coding agents' \
  -depth 8 -define png:color-type=2 \
  "$output"

printf '%s\n' "$output"
