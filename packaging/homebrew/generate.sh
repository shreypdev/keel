#!/bin/sh
# Writes the Homebrew formula for one release from packaging/homebrew/undra.rb.tmpl and the
# release's checksums.txt. There is no committed formula on purpose: the hashes are those of the
# tarballs the release workflow has just built, and a checked-in copy could only hold stale ones.
set -eu

usage() {
  cat <<'USAGE'
Usage: packaging/homebrew/generate.sh --version <semver> --checksums <file> [--base-url <url>] [--out <file>]

  --version    the release version, without a leading "v" (for example 1.0.0)
  --checksums  checksums.txt of the release ("<sha256>  undra-v<version>-<target>.tar.gz" lines)
  --base-url   where the tarballs are; default
               https://github.com/shreypdev/undra/releases/download/v<version>
  --out        write the formula here instead of to standard output
  -h, --help   print this text

Fails when a platform's tarball has no hash in the checksums file.
USAGE
}

die() {
  printf 'generate.sh: %s\n' "$*" >&2
  exit 2
}

version=""
checksums=""
base_url=""
out=""
while [ $# -gt 0 ]; do
  case "$1" in
    --version) [ $# -ge 2 ] || die "--version needs a value"; version=$2; shift 2 ;;
    --checksums) [ $# -ge 2 ] || die "--checksums needs a value"; checksums=$2; shift 2 ;;
    --base-url) [ $# -ge 2 ] || die "--base-url needs a value"; base_url=$2; shift 2 ;;
    --out) [ $# -ge 2 ] || die "--out needs a value"; out=$2; shift 2 ;;
    -h | --help) usage; exit 0 ;;
    *) usage >&2; die "unknown argument: $1" ;;
  esac
done
[ -n "$version" ] && [ -n "$checksums" ] || { usage >&2; die "--version and --checksums are required"; }
case "$version" in
  v*) die "--version takes the bare version (1.0.0), not the tag (v1.0.0)" ;;
  *[!0-9A-Za-z.+-]*) die "--version contains characters a version does not use: $version" ;;
esac
[ -f "$checksums" ] || die "no such file: $checksums"
[ -n "$base_url" ] || base_url="https://github.com/shreypdev/undra/releases/download/v$version"

here=$(cd "$(dirname "$0")" && pwd)
template=$here/undra.rb.tmpl
[ -f "$template" ] || die "missing $template"

# sha256 of one tarball, from the checksums file; exactly one 64-hex entry for that file name.
hash_of() {
  file="undra-v$version-$1.tar.gz"
  found=$(awk -v f="$file" '$2 == f || $2 == "*" f { print $1 }' "$checksums")
  count=$(printf '%s\n' "$found" | grep -c . || true)
  [ "$count" = 1 ] || die "$checksums has $count entries for $file, expected exactly 1"
  case "$found" in
    *[!0-9a-f]*) die "the hash for $file is not lowercase hex: $found" ;;
  esac
  [ "${#found}" = 64 ] || die "the hash for $file is not 64 characters: $found"
  printf '%s' "$found"
}

sha_arm_mac=$(hash_of aarch64-apple-darwin)
sha_intel_mac=$(hash_of x86_64-apple-darwin)
sha_arm_linux=$(hash_of aarch64-unknown-linux-gnu)
sha_intel_linux=$(hash_of x86_64-unknown-linux-gnu)

# Literal replacement (awk index/substr, no regular expressions), so a URL with `&` or `\` is safe.
formula=$(awk \
  -v version="$version" -v base="$base_url" \
  -v arm_mac="$sha_arm_mac" -v intel_mac="$sha_intel_mac" \
  -v arm_linux="$sha_arm_linux" -v intel_linux="$sha_intel_linux" '
  function replace(line, from, to,    out, i) {
    out = ""
    while ((i = index(line, from)) > 0) {
      out = out substr(line, 1, i - 1) to
      line = substr(line, i + length(from))
    }
    return out line
  }
  {
    line = $0
    line = replace(line, "@@BASE_URL@@", base)
    line = replace(line, "@@VERSION@@", version)
    line = replace(line, "@@SHA_AARCH64_APPLE_DARWIN@@", arm_mac)
    line = replace(line, "@@SHA_X86_64_APPLE_DARWIN@@", intel_mac)
    line = replace(line, "@@SHA_AARCH64_UNKNOWN_LINUX_GNU@@", arm_linux)
    line = replace(line, "@@SHA_X86_64_UNKNOWN_LINUX_GNU@@", intel_linux)
    print line
  }' "$template")

case "$formula" in
  *@@*) die "a placeholder survived in the formula; the template and this script disagree" ;;
esac

if [ -n "$out" ]; then
  printf '%s\n' "$formula" >"$out"
else
  printf '%s\n' "$formula"
fi
