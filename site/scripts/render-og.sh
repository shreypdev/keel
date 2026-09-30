#!/usr/bin/env bash
# Renders site/og/index.html (1200x630) to site/assets/og.png with headless Chrome.
# The card loads Geist from Google Fonts, so run this with network access, then commit the PNG.
#
#   bash site/scripts/render-og.sh
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CHROME="${CHROME:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"
SRC="$ROOT/site/og/index.html"
OUT="$ROOT/site/assets/og.png"
[ -x "$CHROME" ] || { echo "render-og: Chrome not found at $CHROME (set CHROME=...)" >&2; exit 1; }
"$CHROME" --headless=new --hide-scrollbars --window-size=1200,630 --virtual-time-budget=8000 \
  --screenshot="$OUT" "file://$SRC" >/dev/null 2>&1
[ -s "$OUT" ] || { echo "render-og: no image written" >&2; exit 1; }
echo "render-og: wrote $OUT ($(wc -c < "$OUT" | tr -d ' ') bytes)"
