#!/bin/bash
#
# Regenerates BizBuz's icon sets from src-tauri/icons/bizbuz-icon.svg.
#
# The previous generator lived in /tmp and was lost, so this one lives in the
# repo next to the source SVG. Run it after editing the SVG:
#
#   ./scripts/make-icons.sh
#
# ── The alpha trap ───────────────────────────────────────────────────────────
#
# The two icon sets need OPPOSITE alpha treatment. Do not "simplify" this by
# processing them the same way:
#
#   Top-level Tauri icons (32x32, 64x64, 128x128, 128x128@2x, icon.png)
#     must be RGBA. tauri's generate_context!() reads them at compile time and
#     panics with "icon <path> is not RGBA" if any is stored as plain RGB.
#
#   iOS AppIcon set (icons/ios/AppIcon-*.png)
#     must be RGB. App Store server-side validation rejects any alpha channel
#     on the 1024x1024 marketing icon, and build-ios.cjs already flattens
#     these, so generating them flat keeps the two in agreement.
#
# Verify with:  magick identify -format "%[channels]" <file>
#   top-level -> srgba      ios -> srgb

set -euo pipefail

cd "$(dirname "$0")/.."
SVG="src-tauri/icons/bizbuz-icon.svg"
ICONS="src-tauri/icons"

[ -f "$SVG" ] || { echo "Missing $SVG" >&2; exit 1; }
command -v rsvg-convert >/dev/null || { echo "Need rsvg-convert (brew install librsvg)" >&2; exit 1; }
command -v magick >/dev/null       || { echo "Need magick (brew install imagemagick)" >&2; exit 1; }

render() { # $1 = px, $2 = out
    rsvg-convert -w "$1" -h "$1" "$SVG" -o "$2"
}

echo "Top-level Tauri icons (RGBA)..."
for spec in "32:32x32.png" "64:64x64.png" "128:128x128.png" "256:128x128@2x.png" "512:icon.png"; do
    px="${spec%%:*}"; name="${spec#*:}"
    render "$px" "$ICONS/$name"
    # Force an explicit, fully-opaque alpha channel so the file is stored as
    # PNG32/srgba rather than being optimised down to RGB.
    magick "$ICONS/$name" -alpha on -channel A -evaluate set 100% "PNG32:$ICONS/$name"
done

echo "iOS AppIcon set (RGB, no alpha)..."
ios_icons=(
    "20:AppIcon-20x20@1x.png"      "40:AppIcon-20x20@2x.png"     "40:AppIcon-20x20@2x-1.png"
    "60:AppIcon-20x20@3x.png"      "29:AppIcon-29x29@1x.png"     "58:AppIcon-29x29@2x.png"
    "58:AppIcon-29x29@2x-1.png"    "87:AppIcon-29x29@3x.png"     "40:AppIcon-40x40@1x.png"
    "80:AppIcon-40x40@2x.png"      "80:AppIcon-40x40@2x-1.png"   "120:AppIcon-40x40@3x.png"
    "120:AppIcon-60x60@2x.png"     "180:AppIcon-60x60@3x.png"    "76:AppIcon-76x76@1x.png"
    "152:AppIcon-76x76@2x.png"     "167:AppIcon-83.5x83.5@2x.png" "1024:AppIcon-512@2x.png"
)
for spec in "${ios_icons[@]}"; do
    px="${spec%%:*}"; name="${spec#*:}"
    render "$px" "$ICONS/ios/$name"
    magick "$ICONS/ios/$name" -background '#1F2933' -alpha remove -alpha off "$ICONS/ios/$name"
done

echo "Windows/Store logos (RGBA)..."
for spec in "30:Square30x30Logo.png" "44:Square44x44Logo.png" "71:Square71x71Logo.png" \
            "89:Square89x89Logo.png" "107:Square107x107Logo.png" "142:Square142x142Logo.png" \
            "150:Square150x150Logo.png" "284:Square284x284Logo.png" "310:Square310x310Logo.png" \
            "50:StoreLogo.png"; do
    px="${spec%%:*}"; name="${spec#*:}"
    render "$px" "$ICONS/$name"
    magick "$ICONS/$name" -alpha on -channel A -evaluate set 100% "PNG32:$ICONS/$name"
done

echo
echo "Verifying alpha treatment:"
printf '  %-34s %s\n' "icon.png" "$(magick identify -format '%[channels]' "$ICONS/icon.png")  (want srgba)"
printf '  %-34s %s\n' "ios/AppIcon-512@2x.png" "$(magick identify -format '%[channels]' "$ICONS/ios/AppIcon-512@2x.png")  (want srgb)"
echo
echo "Done. icon.icns and icon.ico are not regenerated (desktop only, unused on iOS)."
