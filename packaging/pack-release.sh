#!/bin/sh
# Packs one release asset: undra-v<version>-<target>.tar.gz, flat, holding `undra` (mode 0755) and
# the LICENSE-* files, plus <asset>.sha256 in `sha256sum` format ("<hash>  <file>").
#
# The release workflow and the local tests (packaging/npm/test.sh, packaging/test-install.sh)
# all use this script, so what CI publishes and what the tests install have the same layout.
set -eu

usage() {
  cat <<'USAGE'
Usage: packaging/pack-release.sh --version <semver> --target <triple> --binary <path> --out <dir>

Writes <dir>/undra-v<semver>-<triple>.tar.gz and <dir>/undra-v<semver>-<triple>.tar.gz.sha256
and prints the path of the tarball.

  --version  the release version, without a leading "v" (for example 1.0.0)
  --target   the Rust target triple (for example aarch64-apple-darwin)
  --binary   the built `undra` executable (any file name)
  --out      the output directory (created when missing)
  -h, --help print this text
USAGE
}

die() {
  printf 'pack-release.sh: %s\n' "$*" >&2
  exit 2
}

version=""
target=""
binary=""
out=""
while [ $# -gt 0 ]; do
  case "$1" in
    --version) [ $# -ge 2 ] || die "--version needs a value"; version=$2; shift 2 ;;
    --target) [ $# -ge 2 ] || die "--target needs a value"; target=$2; shift 2 ;;
    --binary) [ $# -ge 2 ] || die "--binary needs a value"; binary=$2; shift 2 ;;
    --out) [ $# -ge 2 ] || die "--out needs a value"; out=$2; shift 2 ;;
    -h | --help) usage; exit 0 ;;
    *) usage >&2; die "unknown argument: $1" ;;
  esac
done
[ -n "$version" ] && [ -n "$target" ] && [ -n "$binary" ] && [ -n "$out" ] \
  || { usage >&2; die "--version, --target, --binary and --out are all required"; }

case "$version" in
  v*) die "--version takes the bare version (1.0.0), not the tag (v1.0.0)" ;;
esac
# Both values end up in a file name: keep them to what a version and a target triple use.
case "$version" in
  *[!0-9A-Za-z.+-]*) die "--version contains characters a version does not use: $version" ;;
esac
case "$target" in
  *[!0-9A-Za-z_-]*) die "--target contains characters a target triple does not use: $target" ;;
esac
[ -f "$binary" ] || die "binary not found: $binary"

root=$(cd "$(dirname "$0")/.." && pwd)
for license in LICENSE-MIT LICENSE-APACHE; do
  [ -f "$root/$license" ] || die "missing $root/$license"
done

if command -v shasum >/dev/null 2>&1; then
  sha256() { shasum -a 256 "$1" | awk '{print $1}'; }
elif command -v sha256sum >/dev/null 2>&1; then
  sha256() { sha256sum "$1" | awk '{print $1}'; }
else
  die "neither shasum nor sha256sum is available"
fi

asset="undra-v${version}-${target}.tar.gz"
mkdir -p "$out"
out=$(cd "$out" && pwd)
stage=$(mktemp -d "${TMPDIR:-/tmp}/undra-pack.XXXXXX")
trap 'rm -rf "$stage"' EXIT INT TERM

cp "$binary" "$stage/undra"
chmod 755 "$stage/undra"
cp "$root/LICENSE-MIT" "$root/LICENSE-APACHE" "$stage/"

# COPYFILE_DISABLE keeps macOS tar from adding AppleDouble (._*) entries for extended
# attributes; gzip -n leaves the timestamp out of the header.
(
  cd "$stage"
  COPYFILE_DISABLE=1 tar -cf - undra LICENSE-APACHE LICENSE-MIT | gzip -9n >"$out/$asset"
)
printf '%s  %s\n' "$(sha256 "$out/$asset")" "$asset" >"$out/$asset.sha256"
printf '%s\n' "$out/$asset"
