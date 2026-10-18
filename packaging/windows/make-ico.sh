#!/usr/bin/env bash
# Renders an SVG into a Windows .ico (PNG-compressed entries, 16 to 256 px).
#
#   packaging/windows/make-ico.sh ICON.svg OUT.ico
#
# Needs rsvg-convert (librsvg). Works on Linux too.
set -euo pipefail

svg=${1:?usage: make-ico.sh ICON.svg OUT.ico}
ico=${2:?usage: make-ico.sh ICON.svg OUT.ico}
sizes=(16 20 24 32 40 48 64 96 128 256)

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# Little-endian integers as printf %b escapes.
le16() { printf '\\x%02x\\x%02x' $(($1 & 255)) $(($1 >> 8 & 255)); }
le32() { printf '\\x%02x\\x%02x\\x%02x\\x%02x' $(($1 & 255)) $(($1 >> 8 & 255)) $(($1 >> 16 & 255)) $(($1 >> 24 & 255)); }

for size in "${sizes[@]}"; do
    rsvg-convert --width "$size" --height "$size" --keep-aspect-ratio --format png \
        --output "$work/$size.png" "$svg"
done

count=${#sizes[@]}
offset=$((6 + 16 * count))
{
    # ICONDIR: reserved, type 1 (icon), image count.
    printf '%b' "$(le16 0)$(le16 1)$(le16 "$count")"
    for size in "${sizes[@]}"; do
        bytes=$(wc -c <"$work/$size.png")
        dim=$((size >= 256 ? 0 : size)) # 0 means 256
        # ICONDIRENTRY: width, height, palette size, reserved, planes, bpp, size, offset.
        printf '%b' "\\x$(printf %02x "$dim")\\x$(printf %02x "$dim")\\x00\\x00$(le16 1)$(le16 32)$(le32 "$bytes")$(le32 "$offset")"
        offset=$((offset + bytes))
    done
    for size in "${sizes[@]}"; do
        cat "$work/$size.png"
    done
} >"$ico"
