#!/usr/bin/env bash
# Builds the npm packages of the `undra` command from a local binary, packs them with `npm pack`,
# installs them into a scratch prefix with `npm install -g --prefix` and checks that the installed
# `undra` runs, hands over its output and exit code, and explains itself where it cannot run.
#
#   bash packaging/npm/test.sh                      # builds undra-cli (debug) and tests that
#   UNDRA_BIN=/path/to/undra bash packaging/npm/test.sh   # tests an existing binary
#   bash packaging/npm/test.sh --tarballs <dir>     # tests packed packages (the release workflow
#                                                   #   passes the ones it is about to publish)
#
# Nothing is downloaded: the install runs with --offline. Only this machine's platform package is
# built and installed; the other three are optional dependencies npm skips.
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: bash packaging/npm/test.sh [--tarballs <dir>] [-h | --help]

With no option: take the binary from $UNDRA_BIN, or build `cargo build -p undra-cli` and use
target/debug/undra; pack it as a release asset; build the npm packages from it; npm pack;
install the launcher and this machine's platform package into a scratch prefix; run them.

  --tarballs <dir>  skip building: <dir> holds the packed tarballs (undra-cli-<version>.tgz, undra-
                    <version>.tgz and undra-cli-<platform>-<version>.tgz for this machine)
  -h, --help        print this text

Environment: UNDRA_BIN (an existing undra binary), KEEP_TMP=1 (keep the scratch directory).
USAGE
}

tarballs=""
while [ $# -gt 0 ]; do
  case "$1" in
    --tarballs) [ $# -ge 2 ] || { echo "test.sh: --tarballs needs a directory" >&2; exit 2; }; tarballs=$2; shift 2 ;;
    -h | --help) usage; exit 0 ;;
    *) usage >&2; echo "test.sh: unknown argument: $1" >&2; exit 2 ;;
  esac
done

root=$(cd "$(dirname "$0")/../.." && pwd)
if [ -n "$tarballs" ]; then
  [ -d "$tarballs" ] || { echo "test.sh: no such directory: $tarballs" >&2; exit 2; }
  tarballs=$(cd "$tarballs" && pwd)  # the installs below run from another directory
fi
tmp=$(mktemp -d "${TMPDIR:-/tmp}/undra-npm-test.XXXXXX")
cleanup() { if [ "${KEEP_TMP:-0}" = 1 ]; then echo "kept $tmp"; else rm -rf "$tmp"; fi; }
trap cleanup EXIT

fail() { echo "FAIL: $*" >&2; exit 1; }
pass() { echo "ok: $*"; }

command -v node >/dev/null || fail "node is required"
command -v npm >/dev/null || fail "npm is required"
node -e 'process.exit(Number(process.versions.node.split(".")[0]) >= 20 ? 0 : 1)' \
  || fail "Node 20 or newer is required (found $(node --version))"

version=$(node -p 'require(process.argv[1]).version' "$root/packaging/npm/templates/cli/package.json")
platform=$(node -p 'process.platform + "-" + process.arch')
case "$platform" in
  darwin-arm64) target=aarch64-apple-darwin ;;
  darwin-x64) target=x86_64-apple-darwin ;;
  linux-x64) target=x86_64-unknown-linux-gnu ;;
  linux-arm64) target=aarch64-unknown-linux-gnu ;;
  *) fail "no prebuilt undra for $platform, so there is nothing to test here" ;;
esac

if [ -z "$tarballs" ]; then
  binary=${UNDRA_BIN:-}
  if [ -z "$binary" ]; then
    echo "Building undra-cli (debug)..."
    (cd "$root" && cargo build -p undra-cli >&2)
    binary=${CARGO_TARGET_DIR:-$root/target}/debug/undra
  fi
  [ -x "$binary" ] || fail "no executable at $binary"
  # The packages must hold a file called `undra`, whatever the build called it.
  mkdir -p "$tmp/bin" "$tmp/artifacts" "$tmp/packages"
  cp "$binary" "$tmp/bin/undra"
  sh "$root/packaging/pack-release.sh" --version "$version" --target "$target" \
    --binary "$tmp/bin/undra" --out "$tmp/artifacts" >/dev/null
  node "$root/packaging/npm/build.mjs" --artifacts "$tmp/artifacts" --out "$tmp/packages" \
    --only "$platform" >/dev/null
  mkdir -p "$tmp/tgz"
  for dir in "cli-$platform" cli undra; do
    (cd "$tmp/packages/$dir" && npm pack --pack-destination "$tmp/tgz" --silent >/dev/null)
  done
  tarballs=$tmp/tgz
  pass "built and packed the packages for $platform from $binary"
fi

cli_tgz=$tarballs/undra-cli-$version.tgz
bare_tgz=$tarballs/undra-$version.tgz
plat_tgz=$tarballs/undra-cli-$platform-$version.tgz
for f in "$cli_tgz" "$bare_tgz" "$plat_tgz"; do
  [ -f "$f" ] || fail "missing $f"
done

# --- what is inside the tarballs --------------------------------------------------------------
list=$(tar -tzf "$cli_tgz" | sort | tr '\n' ' ')
[ "$list" = "package/LICENSE-APACHE package/LICENSE-MIT package/README.md package/bin/undra.js package/package.json " ] \
  || fail "unexpected files in @undra/cli: $list"
tar -tvzf "$plat_tgz" | grep -Eq '^-rwxr-xr-x.* package/undra$' \
  || fail "the binary in $plat_tgz is not executable (mode 0755)"
tar -xzOf "$cli_tgz" package/package.json | node -e '
  const pkg = JSON.parse(require("fs").readFileSync(0, "utf8"));
  const want = ["darwin-arm64", "darwin-x64", "linux-x64", "linux-arm64"].map((p) => "@undra/cli-" + p).sort();
  const have = Object.keys(pkg.optionalDependencies || {}).sort();
  const same = JSON.stringify(want) === JSON.stringify(have);
  const exact = Object.values(pkg.optionalDependencies || {}).every((v) => v === pkg.version);
  if (!same || !exact) { console.error("optionalDependencies: " + JSON.stringify(pkg.optionalDependencies)); process.exit(1); }
  if (pkg.bin.undra !== "bin/undra.js" || pkg.engines.node !== ">=20") process.exit(1);
' || fail "@undra/cli package.json: optionalDependencies must be the four platform packages at its own version, with bin and engines"
tar -xzOf "$plat_tgz" package/package.json | node -e '
  const pkg = JSON.parse(require("fs").readFileSync(0, "utf8"));
  const [os, cpu] = process.argv[1].split("-");
  if (pkg.os.join() !== os || pkg.cpu.join() !== cpu) process.exit(1);
' "$platform" || fail "the platform package's os/cpu fields do not say $platform"
pass "tarball contents: files, exec bit, optionalDependencies, os/cpu"

# --- install and run --------------------------------------------------------------------------
check_install() { # <label> <launcher tarball> <prefix>
  local label=$1 launcher=$2 prefix=$3
  (cd "$tmp" && npm install -g --offline --no-audit --no-fund --prefix "$prefix" "$launcher" "$plat_tgz" >"$tmp/install.log" 2>&1) \
    || { cat "$tmp/install.log" >&2; fail "npm install -g of $label failed"; }
  local undra=$prefix/bin/undra
  [ -x "$undra" ] || fail "$label: $undra was not created"

  local out
  out=$("$undra" --version) || fail "$label: undra --version failed"
  case "$out" in *"$version"*) ;; *) fail "$label: undra --version printed '$out', expected it to contain $version" ;; esac
  pass "$label: undra --version -> $out"

  # stdout is the binary's own: same text through the launcher as directly.
  local direct=$prefix/lib/node_modules/@undra/cli-$platform/undra
  [ "$("$undra" --help 2>&1)" = "$("$direct" --help 2>&1)" ] || fail "$label: --help differs between the launcher and the binary"

  # the exit code is the binary's own.
  local want got
  "$direct" definitely-not-a-command >/dev/null 2>&1 && want=0 || want=$?
  "$undra" definitely-not-a-command >/dev/null 2>&1 && got=0 || got=$?
  [ "$want" -ne 0 ] || fail "$label: the binary accepted a bogus command, so the exit code check proves nothing"
  [ "$got" = "$want" ] || fail "$label: exit code $got through the launcher, $want from the binary"
  pass "$label: output and exit code ($want) pass through"
}

check_install "@undra/cli" "$cli_tgz" "$tmp/prefix-cli"
check_install "undra (unscoped)" "$bare_tgz" "$tmp/prefix-bare"

# --- where it cannot run ----------------------------------------------------------------------
launcher=$tmp/prefix-cli/lib/node_modules/@undra/cli/bin/undra.js
err=$(node -e '
  Object.defineProperty(process, "platform", { value: "win32" });
  require(process.argv[1]);
' "$launcher" 2>&1) && fail "an unsupported platform must fail" || status=$?
[ "$status" = 1 ] || fail "unsupported platform exited $status, expected 1"
case "$err" in
  "undra: there is no prebuilt binary for win32-"*"https://shreypdev.github.io/undra/install.sh"*) ;;
  *) fail "unsupported-platform message is wrong: $err" ;;
esac
[ "$(printf '%s\n' "$err" | wc -l | tr -d ' ')" = 1 ] || fail "the unsupported-platform error must be one line: $err"
pass "unsupported platform: one line naming the platform and the installer"

# The launcher alone (no platform package next to it), as after --omit=optional.
mkdir -p "$tmp/alone" && tar -xzf "$cli_tgz" -C "$tmp/alone"
err=$(node "$tmp/alone/package/bin/undra.js" --version 2>&1) && fail "a missing platform package must fail" || status=$?
[ "$status" = 1 ] || fail "missing platform package exited $status, expected 1"
case "$err" in
  "undra: @undra/cli-$platform is not installed"*"install.sh"*) ;;
  *) fail "missing-platform-package message is wrong: $err" ;;
esac
pass "missing platform package: says which package and the way out"

echo "npm packaging: all checks passed"
