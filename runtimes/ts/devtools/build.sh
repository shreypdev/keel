#!/usr/bin/env bash
# Builds the devtools page into crates/undra-cli/assets/devtools/ (committed, so `cargo build` needs no Node).
#
#   bash build.sh            rebuild and write the assets
#   bash build.sh --check    rebuild into a temp dir and fail if it differs from the committed assets (CI)
#
# One bundle (esbuild, pinned in package.json), the stylesheet and the page, no source maps, no
# absolute paths in the output, so the same sources give the same bytes on any machine. The size
# budget (150 KB gzipped, in all) is checked here and by a test of undra-cli.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
dest="$here/../../../crates/undra-cli/assets/devtools"
mode="${1:-write}"
out="$dest"
if [ "$mode" = "--check" ]; then out="$(mktemp -d)"; fi
cd "$here"
[ -d node_modules ] || npm ci --no-audit --no-fund >/dev/null
mkdir -p "$out"
node_modules/.bin/esbuild src/main.ts --bundle --format=esm --target=es2022 --minify --legal-comments=none \
  --alias:@undra/runtime/wire=../@undra/runtime/src/wire/index.ts \
  --log-level=warning --outfile="$out/app.js"
cp static/app.css "$out/app.css"
cp static/index.html "$out/index.html"
gz=0
for f in app.js app.css index.html; do gz=$((gz + $(gzip -9 -n -c "$out/$f" | wc -c))); done
echo "devtools assets: $gz bytes gzipped (budget 153600)"
[ "$gz" -le 153600 ] || { echo "over the size budget" >&2; exit 1; }
if [ "$mode" = "--check" ]; then
  if diff -r "$out" "$dest" >/dev/null 2>&1; then echo "committed assets match a rebuild"; rm -rf "$out"; exit 0; fi
  diff -r "$out" "$dest" | head -20 >&2 || true
  echo "the committed assets differ from a rebuild: run \`bash runtimes/ts/devtools/build.sh\` and commit the result" >&2
  rm -rf "$out"
  exit 1
fi
