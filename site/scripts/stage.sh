#!/usr/bin/env bash
# Stages the deployable site into _site/: site/ plus the built playground at _site/playground/.
# Expects examples/playground/web/dist (npx vite build --base=/keel/playground/) to exist.
#
#   bash site/scripts/stage.sh
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"
[ -d examples/playground/web/dist ] || { echo "stage: examples/playground/web/dist is missing; build the playground first" >&2; exit 1; }
rm -rf _site
mkdir -p _site/playground
cp -R site/. _site/
cp -R examples/playground/web/dist/. _site/playground/
rm -rf _site/scripts _site/data _site/og
touch _site/.nojekyll
echo "stage: _site/ ready ($(find _site -type f | wc -l | tr -d ' ') files)"
