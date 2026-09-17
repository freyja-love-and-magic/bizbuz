#!/bin/bash
#
# Produces every App Store screenshot size from the files in originals/.
#
#   ./make-screenshots.sh                 # regenerate from originals/
#   ./make-screenshots.sh ~/Downloads/IMG_*.png   # import new ones first
#
# Sizes, per App Store Connect's current spec:
#
#   iphone-6.9   1320x2868   the only size an iPhone app is REQUIRED to
#                            supply. App Store Connect scales it down for
#                            every smaller iPhone automatically.
#   iphone-6.5   1284x2778   optional; kept because the folder already
#                            existed and some listings still show it.
#   ipad-13      2064x2752   only relevant if the app ships for iPad.
#                            BizBuz sets TARGETED_DEVICE_FAMILY "1"
#                            (iPhone only) in build-ios.cjs, so App Store
#                            Connect won't even offer an iPad slot. Generated
#                            anyway so the set is ready if that changes.
#
# No alpha channel: App Store Connect rejects screenshots with transparency,
# the same rule that applies to the 1024 marketing icon.

set -euo pipefail
cd "$(dirname "$0")"

# Sampled from the screenshots themselves — the app's dark-mode ground. Used
# to pad the iPad canvas, where a portrait phone shot (aspect 0.46) can't fill
# an iPad (0.75) without either huge letterboxing or cropping content away.
PAD="#1a1a1a"

command -v magick >/dev/null || { echo "Need magick (brew install imagemagick)" >&2; exit 1; }

# Importing: copy any files passed in, renumbered 01..NN in argument order.
if [ "$#" -gt 0 ]; then
    echo "Importing $# screenshot(s) into originals/..."
    rm -f originals/*.png
    i=1
    for src in "$@"; do
        printf -v n "%02d" "$i"
        cp "$src" "originals/$n.png"
        echo "  $(basename "$src") -> originals/$n.png"
        i=$((i + 1))
    done
fi

shopt -s nullglob
originals=(originals/*.png)
[ "${#originals[@]}" -gt 0 ] || { echo "No files in originals/" >&2; exit 1; }

# ── TestFlight back-link ─────────────────────────────────────────────────────
#
# Screenshots taken from a TestFlight build carry a "◀ TestFlight" return link
# in the status bar. It's iOS chrome, not part of BizBuz, and it advertises
# that the listing's screenshots came from a beta.
#
# It sits on flat #1a1a1a with nothing behind it, so painting over it is
# pixel-exact rather than a visible patch — verified by sampling the region.
# The originals are untouched, so rerunning without SKIP_TF_MASK=1 restores
# whatever you prefer.
#
# This only removes OS chrome. It doesn't alter anything the app drew, which
# would misrepresent the product.
mask_testflight() { # $1 = file, edited in place
    [ "${SKIP_TF_MASK:-}" = "1" ] && return 0
    magick "$1" -fill "$PAD" -draw "rectangle 0,96 268,146" "$1"
}

# Scale to fill, then crop to the exact canvas. Used for the iPhone sizes,
# whose aspect ratios are within a percent of the source — the crop removes a
# handful of pixels, not content.
fill_crop() { # $1 src, $2 WxH, $3 out
    magick "$1" -resize "${2}^" -gravity center -extent "$2" \
        -background "$PAD" -alpha remove -alpha off "$3"
}

# Scale to fit entirely, then pad. Used for iPad: cropping a phone shot to an
# iPad's aspect would cut off the bottom third of the UI, which on the form
# screens is most of the form.
fit_pad() { # $1 src, $2 WxH, $3 out
    magick "$1" -resize "$2" -gravity center -background "$PAD" -extent "$2" \
        -alpha remove -alpha off "$3"
}

for src in "${originals[@]}"; do
    n=$(basename "$src")
    # Mask on a copy so originals/ stays exactly as imported.
    work=$(mktemp -t shot).png
    cp "$src" "$work"
    mask_testflight "$work"
    src="$work"
    fill_crop "$src" 1320x2868 "iphone-6.9/$n"
    fill_crop "$src" 1284x2778 "iphone-6.5/$n"
    fit_pad   "$src" 2064x2752 "ipad-13/$n"
    rm -f "$work"
    echo "  $n -> 6.9, 6.5, ipad-13"
done

echo
echo "Verifying:"
for d in iphone-6.9 iphone-6.5 ipad-13; do
    for f in "$d"/*.png; do
        printf '  %-22s %s %s\n' "$f" \
            "$(magick identify -format '%wx%h' "$f")" \
            "$(magick identify -format '%[channels]' "$f")"
    done
done
