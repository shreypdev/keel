#!/usr/bin/env bash
# Stages the deployable site into _site/ (or the directory given): site/, the built playground at
# playground/ and the Rust API reference at reference/rust/.
# Expects examples/playground/web/dist (npx vite build --base=/undra/playground/) and the rustdoc
# output (bash site/scripts/build-rustdoc.sh) to exist.
#
#   bash site/scripts/stage.sh [out-dir]
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"
OUT="${1:-_site}"
case "$OUT" in ""|/|.|..|"$ROOT"|"$HOME") echo "stage: refusing to empty \"$OUT\"" >&2; exit 1;; esac
DOC="${CARGO_TARGET_DIR:-target}/doc"
[ -d examples/playground/web/dist ] || { echo "stage: examples/playground/web/dist is missing; build the playground first" >&2; exit 1; }
[ -f "$DOC/undra/index.html" ] || { echo "stage: $DOC/undra/index.html is missing; run bash site/scripts/build-rustdoc.sh first" >&2; exit 1; }
rm -rf "$OUT"
mkdir -p "$OUT/playground" "$OUT/reference/rust"
cp -R site/. "$OUT/"
cp -R examples/playground/web/dist/. "$OUT/playground/"
cp -R "$DOC/." "$OUT/reference/rust/"
# rustdoc writes no index of its own for a set of crates: send /reference/rust/ to the facade crate.
cat > "$OUT/reference/rust/index.html" <<'HTML'
<!doctype html>
<meta charset="utf-8">
<title>Undra Rust API reference</title>
<meta http-equiv="refresh" content="0; url=undra/index.html">
<link rel="canonical" href="undra/index.html">
<p><a href="undra/index.html">The undra crate</a></p>
HTML
rm -rf "$OUT/scripts" "$OUT/data" "$OUT/og" "$OUT/reference/rust/.lock"
touch "$OUT/.nojekyll"
echo "stage: $OUT/ ready ($(find "$OUT" -type f | wc -l | tr -d ' ') files)"
