#!/usr/bin/env bash
# Builds and packs the npm packages a release attaches to its GitHub Release (ADR-063):
# undra-runtime-<v>.tgz (@undra/runtime), undra-testkit-<v>.tgz (@undra/testkit) and
# undra-react-native-<v>.tgz (@undra/react-native), exactly as `npm pack` names them.
#
#   bash packaging/pack-npm.sh --out <dir>
#
# The release workflow and packaging/rehearse-launch.sh both use it, so what a release attaches and what the
# rehearsal installs are built the same way. The runtime is built first (the other two compile against its
# declarations). `npm ci --ignore-scripts`: no dependency's install script runs on a release build. Each
# package's own version must be the workspace version (scripts/bump-version.sh keeps them so); the script
# fails, naming the file, when a tarball it expects is not there.
set -euo pipefail

out=""
while [ $# -gt 0 ]; do
  case "$1" in
    --out) [ $# -ge 2 ] || { echo "pack-npm.sh: --out needs a directory" >&2; exit 2; }; out=$2; shift 2 ;;
    -h | --help) sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "pack-npm.sh: unknown argument: $1 (usage: --out <dir>)" >&2; exit 2 ;;
  esac
done
[ -n "$out" ] || { echo "pack-npm.sh: --out <dir> is required" >&2; exit 2; }
root=$(cd "$(dirname "$0")/.." && pwd)
mkdir -p "$out"
out=$(cd "$out" && pwd)
version=$(awk '/^\[/ { s = $0 } s == "[workspace.package]" && /^version = "/ { gsub(/^version = "|"$/, ""); print; exit }' "$root/Cargo.toml")

for dir in runtimes/ts/@undra/runtime runtimes/ts/@undra/testkit runtimes/rn/@undra/react-native; do
  echo "==> $dir"
  (
    cd "$root/$dir"
    npm ci --ignore-scripts --no-audit --no-fund --prefer-offline >/dev/null
    npm run build >/dev/null
    npm pack --pack-destination "$out" --silent >/dev/null
  )
done

for stem in undra-runtime undra-testkit undra-react-native; do
  [ -f "$out/$stem-$version.tgz" ] || {
    echo "pack-npm.sh: $out/$stem-$version.tgz is missing: is the package's version $version? (scripts/bump-version.sh --check)" >&2
    exit 1
  }
  echo "$out/$stem-$version.tgz"
done
