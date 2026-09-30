#!/usr/bin/env bash
# Builds everything the deployed site has and stages it in _site/, for a local preview.
#
#   bash site/scripts/build-local.sh
#   python3 -m http.server 8765 --directory _site      # then open http://localhost:8765/keel/
#
# Steps: generated site files, the playground's wasm core (keel build), the playground web app
# (vite, base /keel/playground/), staging, and the link checker on the staged tree.
# Without binaryen installed, keel build warns and keeps the unoptimised wasm: fine for a preview.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"
# shellcheck disable=SC1091
[ -f scripts/env.sh ] && source scripts/env.sh >/dev/null 2>&1 || true

node site/scripts/build-all.mjs
cargo run -q -p keel-cli -- build --platform web -C examples/playground
( cd examples/playground/web && npm ci --no-audit --no-fund && npx vite build --base=/keel/playground/ )
bash site/scripts/stage.sh
# The playground's asset URLs start with /keel/, so the preview serves the site under that prefix too.
ln -s . _site/keel
node site/scripts/check-links.mjs --root _site
echo "build-local: serve with  python3 -m http.server 8765 --directory _site  and open http://localhost:8765/keel/"
