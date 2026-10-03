#!/usr/bin/env bash
# Tests the npm packages a release attaches (ADR-063) the way an app installs them: by URL, from a web
# server, with no registry at all.
#
# It serves <release-dir> as GitHub serves a release (v<version>/undra-runtime-<version>.tgz, ...) with
# `python3 -m http.server` on 127.0.0.1, makes a project that depends on the three tarballs by URL (and on a
# stand-in for react-native, the React Native host's other peer), and installs it with npm pointed at a registry
# that does not exist. Checked: the install needs no registry (so nothing names @undra/runtime by a range a
# registry would have to answer), `npm ls --all` finds every peer satisfied (@undra/react-native and @undra/testkit
# name the runtime as a peer, ^<version>), the runtime imports and has the release's version.
#
#   bash packaging/test-npm-assets.sh --release-dir <dir>   # <dir> holds undra-{runtime,testkit,react-native}-<v>.tgz
# Options: --version <v> (default: the workspace version). Environment: KEEP_TMP=1 keeps the scratch directory.
set -euo pipefail

usage() {
  sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'
}

release_dir=""
version=""
while [ $# -gt 0 ]; do
  case "$1" in
    --release-dir) [ $# -ge 2 ] || { usage >&2; exit 2; }; release_dir=$2; shift 2 ;;
    --version) [ $# -ge 2 ] || { usage >&2; exit 2; }; version=$2; shift 2 ;;
    -h | --help) usage; exit 0 ;;
    *) usage >&2; echo "test-npm-assets.sh: unknown argument: $1" >&2; exit 2 ;;
  esac
done
[ -n "$release_dir" ] && [ -d "$release_dir" ] || { usage >&2; echo "test-npm-assets.sh: --release-dir <dir> is required" >&2; exit 2; }
release_dir=$(cd "$release_dir" && pwd)
root=$(cd "$(dirname "$0")/.." && pwd)
[ -n "$version" ] || version=$(awk '/^\[/ { s = $0 } s == "[workspace.package]" && /^version = "/ { gsub(/^version = "|"$/, ""); print; exit }' "$root/Cargo.toml")

tmp=$(mktemp -d "${TMPDIR:-/tmp}/undra-npm-assets.XXXXXX")
server_pid=""
cleanup() {
  if [ -n "$server_pid" ]; then
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
  fi
  if [ "${KEEP_TMP:-0}" = 1 ]; then echo "kept $tmp"; else rm -rf "$tmp"; fi
}
trap cleanup EXIT
fail() { echo "FAIL: $*" >&2; exit 1; }
pass() { echo "ok: $*"; }

for tool in python3 node npm; do command -v "$tool" >/dev/null || fail "$tool is required"; done

# --- the release layout, served -----------------------------------------------------------------
mkdir -p "$tmp/www/v$version"
for stem in undra-runtime undra-testkit undra-react-native; do
  file=$release_dir/$stem-$version.tgz
  [ -f "$file" ] || fail "$file is missing"
  cp "$file" "$tmp/www/v$version/"
done
python3 -u -m http.server 0 --bind 127.0.0.1 --directory "$tmp/www" >"$tmp/server.log" 2>&1 &
server_pid=$!
port=""
for _ in $(seq 1 100); do
  port=$(sed -n 's/^Serving HTTP on 127.0.0.1 port \([0-9]*\).*/\1/p' "$tmp/server.log" | head -n 1)
  [ -z "$port" ] || break
  kill -0 "$server_pid" 2>/dev/null || fail "the web server exited: $(cat "$tmp/server.log")"
  sleep 0.1
done
[ -n "$port" ] || fail "the web server did not start: $(cat "$tmp/server.log")"
base=http://127.0.0.1:$port/v$version

# --- a project that depends on them by URL --------------------------------------------------------
app=$tmp/app
mkdir -p "$app/react-native-stub"
# react-native is the React Native host's other peer: a stand-in with a version in its range, so nothing is fetched.
printf '{ "name": "react-native", "version": "0.87.0" }\n' >"$app/react-native-stub/package.json"
cat >"$app/package.json" <<JSON
{
  "name": "undra-npm-assets-test",
  "private": true,
  "type": "module",
  "dependencies": {
    "@undra/runtime": "$base/undra-runtime-$version.tgz",
    "@undra/react-native": "$base/undra-react-native-$version.tgz",
    "@undra/testkit": "$base/undra-testkit-$version.tgz",
    "react-native": "file:./react-native-stub"
  }
}
JSON
# No registry: port 9 on this machine answers nothing. npm still asks it about the runtime's optional peers (react,
# vue, ...) and goes on without them; a lookup of anything it needs, an @undra package above all, fails the install.
npm_flags=(--no-audit --no-fund --no-update-notifier --registry "http://127.0.0.1:9/" --cache "$tmp/npm-cache"
  --fetch-retries=0 --fetch-timeout=5000 --loglevel=http)
(cd "$app" && npm install "${npm_flags[@]}" >"$tmp/install.log" 2>&1) || {
  cat "$tmp/install.log" >&2
  fail "npm install of the three tarballs by URL failed (a dependency it could not resolve without a registry?)"
}
if grep -E 'GET http://127\.0\.0\.1:9/@undra' "$tmp/install.log" >&2; then
  fail "npm looked an @undra package up in the registry"
fi
pass "npm installs the three tarballs by URL with no registry, and never looks an @undra package up by name"

(cd "$app" && npm ls --all --registry "http://127.0.0.1:9/" >"$tmp/ls.log" 2>&1) || {
  cat "$tmp/ls.log" >&2
  fail "npm ls finds an unmet or invalid dependency"
}
grep -q "@undra/runtime@$version" "$tmp/ls.log" || fail "npm ls does not show @undra/runtime@$version: $(cat "$tmp/ls.log")"
pass "every peer is satisfied (@undra/react-native and @undra/testkit name the runtime ^$version)"

installed=$(cd "$app" && node -p 'require("./node_modules/@undra/runtime/package.json").version')
[ "$installed" = "$version" ] || fail "the installed runtime is $installed, not $version"
(cd "$app" && node --input-type=module -e 'const m = await import("@undra/runtime"); if (typeof m.UndraCore !== "function") { console.error("no UndraCore export"); process.exit(1); }') ||
  fail "@undra/runtime does not import"
pass "@undra/runtime $version imports and exports UndraCore"
echo "test-npm-assets.sh: all checks passed ($version)"
