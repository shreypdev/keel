#!/usr/bin/env bash
# Rehearses a release (ADR-063) on this machine, with no account anywhere: everything a project made by
# `undra init` needs comes from a copy of this repository standing in for github.com/shreypdev/undra.
#
#   1. Snapshot the working tree (uncommitted edits included) into a fresh repository, run
#      scripts/bump-version.sh <version> there, commit, tag v<version>, and make a bare clone of it: the
#      "remote" the crates and the Swift package are fetched from by tag.
#   2. Build `undra` from that snapshot (it says `undra <version> (unknown)`).
#   3. Build the npm tarballs the release would attach (packaging/pack-npm.sh), serve them from a local web
#      server as GitHub serves a release's assets, and install them by URL with no registry
#      (packaging/test-npm-assets.sh: every peer satisfied).
#   4. Run the install command of jitpack.yml, as JitPack would on the tag, into a local Maven repository.
#   5. Hide the snapshot, then `undra init rehearsal` (no --undra-path) with UNDRA_DIST_GIT_URL,
#      UNDRA_DIST_RELEASE_URL and UNDRA_DIST_MAVEN_REPO pointing at the three stand-ins, and build the web app
#      (npm install && npm run build), the iOS app for the simulator (xcodebuild, the Swift package resolved
#      from the bare clone) and the Android app (./gradlew :app:assembleDebug).
#
# It fails when a step fails, and when the project or a build names the checkout or the snapshot (a step that
# needed a checkout). The summary says, per platform, whether it built and how long it took.
#
#   bash packaging/rehearse-launch.sh                          # every platform this machine can build
#   bash packaging/rehearse-launch.sh --platforms web,android  # some
#   bash packaging/rehearse-launch.sh --version 1.0.0-rc.1     # the version to rehearse (default <workspace>-rehearsal.1)
#   bash packaging/rehearse-launch.sh --keep                   # keep the scratch directory (its path is printed)
#
# Needs git, cargo, node and npm, python3; iOS needs Xcode (macOS), Android an Android SDK with the NDK
# (ANDROID_HOME), a JDK 17 and Gradle (`undra init` makes the wrapper with it). Cargo resolves the core's
# crates.io dependencies and npm and Gradle the apps' third-party ones as usual (from their caches when they
# have them). It takes minutes (the core is compiled for each platform). CARGO_BUILD_JOBS (default 8) bounds Cargo.
set -euo pipefail

repo=$(cd "$(dirname "$0")/.." && pwd)
workspace_version=$(awk '/^\[/ { s = $0 } s == "[workspace.package]" && /^version = "/ { gsub(/^version = "|"$/, ""); print; exit }' "$repo/Cargo.toml")
version="$workspace_version-rehearsal.1"
platforms=""
keep=0
while [ $# -gt 0 ]; do
  case "$1" in
    --version) [ $# -ge 2 ] || { echo "rehearse-launch.sh: --version needs a value" >&2; exit 2; }; version=$2; shift 2 ;;
    --platforms) [ $# -ge 2 ] || { echo "rehearse-launch.sh: --platforms needs a value" >&2; exit 2; }; platforms=$2; shift 2 ;;
    --keep) keep=1; shift ;;
    -h | --help) sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "rehearse-launch.sh: unknown argument: $1 (--help)" >&2; exit 2 ;;
  esac
done
if [ -z "$platforms" ]; then
  platforms=web
  if [ "$(uname -s)" = Darwin ] && command -v xcodebuild >/dev/null 2>&1; then platforms="$platforms,ios"; fi
  if [ -n "${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}" ] && command -v gradle >/dev/null 2>&1; then platforms="$platforms,android"; fi
fi
has() { case ",$platforms," in *",$1,"*) return 0 ;; *) return 1 ;; esac; }
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-8}"

tmp=$(mktemp -d "${TMPDIR:-/tmp}/undra-rehearsal.XXXXXX")
tmp=$(cd "$tmp" && pwd -P)
server_pid=""
cleanup() {
  if [ -n "$server_pid" ]; then
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
  fi
  if [ "$keep" = 1 ]; then echo "kept $tmp"; else rm -rf "$tmp"; fi
}
trap cleanup EXIT
log() { printf '\n==> %s\n' "$*"; }
fail() { printf '\nrehearse-launch.sh: FAIL: %s\n' "$*" >&2; exit 1; }
now() { date +%s; }
summary=()
record() { summary+=("$(printf '%-28s %-4s %s' "$1" "$2" "$3")"); }

for tool in git cargo node npm python3; do command -v "$tool" >/dev/null || fail "$tool is required"; done
printf 'Rehearsing Undra %s (platforms: %s) in %s\n' "$version" "$platforms" "$tmp"

# --- 1. the remote: a snapshot of the working tree, bumped and tagged ---------------------------------------
log "Snapshot of the working tree, bumped to $version and tagged v$version"
src=$tmp/src
mkdir -p "$src"
(cd "$repo" && git ls-files -z | while IFS= read -r -d '' f; do [ -e "$f" ] && printf '%s\0' "$f"; done) |
  (cd "$repo" && tar --null -T - -cf -) | tar -xf - -C "$src"
git_q() { git -C "$src" -c user.name=rehearsal -c user.email=rehearsal@example.invalid -c commit.gpgsign=false -c tag.gpgsign=false "$@"; }
git_q init -q
git_q add -A
git_q commit -qm "the working tree"
(cd "$src" && bash scripts/bump-version.sh "$version")
git_q add -A
git_q commit -qm "chore(release): $version"
git_q tag "v$version"
mkdir -p "$tmp/remote"
git clone -q --bare "$src" "$tmp/remote/undra.git"
git_url="file://$tmp/remote/undra.git"
record "remote (bare clone, tag)" ok "v$version at $git_url"

# --- 2. the CLI of the release --------------------------------------------------------------------------------
log "Building undra $version from the snapshot"
t0=$(now)
# A target directory of its own, kept between rehearsals so the dependencies are not rebuilt every time.
cli_target=$repo/target/rehearsal-cli
(cd "$src" && CARGO_TARGET_DIR="$cli_target" cargo build -q -p undra-cli)
mkdir -p "$tmp/bin"
cp "$cli_target/debug/undra" "$tmp/bin/undra"
said=$("$tmp/bin/undra" --version)
[ "$said" = "undra $version (unknown)" ] || fail "the rehearsal's undra says '$said', expected 'undra $version (unknown)'"
record "undra CLI" ok "$said ($(($(now) - t0)) s)"

# --- 3. the release's npm assets, served ---------------------------------------------------------------------
log "The npm tarballs the release attaches, served as a GitHub Release serves them"
t0=$(now)
www=$tmp/www
(cd "$src" && bash packaging/pack-npm.sh --out "$www/v$version")
(cd "$src" && bash packaging/test-npm-assets.sh --release-dir "$www/v$version" --version "$version")
python3 -u -m http.server 0 --bind 127.0.0.1 --directory "$www" >"$tmp/server.log" 2>&1 &
server_pid=$!
port=""
for _ in $(seq 1 100); do
  port=$(sed -n 's/^Serving HTTP on 127.0.0.1 port \([0-9]*\).*/\1/p' "$tmp/server.log" | head -n 1)
  [ -z "$port" ] || break
  kill -0 "$server_pid" 2>/dev/null || fail "the web server exited: $(cat "$tmp/server.log")"
  sleep 0.1
done
[ -n "$port" ] || fail "the web server did not start"
release_url=http://127.0.0.1:$port
curl -fsS -o /dev/null "$release_url/v$version/undra-runtime-$version.tgz" || fail "the assets are not served at $release_url"
record "npm assets (3 tarballs)" ok "$release_url/v$version ($(($(now) - t0)) s)"

# --- 4. JitPack's build of the tag, into a local Maven repository ---------------------------------------------
m2=$tmp/m2
if has android; then
  log "jitpack.yml's install command on the tag, into $m2"
  t0=$(now)
  install=$(awk '/^install:/ { f = 1; next } /^[^ ]/ { f = 0 } f && /^  - / { sub(/^  - /, ""); print }' "$src/jitpack.yml")
  [ -n "$install" ] || fail "jitpack.yml has no install command"
  (cd "$src" && GROUP=com.github.shreypdev ARTIFACT=undra VERSION="v$version" MAVEN_REPO_LOCAL="$m2" bash -c "$install")
  record "Kotlin modules (JitPack)" ok "com.github.shreypdev.undra:*:v$version ($(($(now) - t0)) s)"
fi

# --- 5. a project made by the release, built without the checkout -------------------------------------------
# From here on nothing may read the snapshot: it is moved away, and the project is checked for any path to it or to
# the checkout this script runs from.
mv "$src" "$tmp/src-hidden"
apps=$tmp/apps
mkdir -p "$apps"
export PATH="$tmp/bin:$PATH"
# The Vite plugin and the Gradle task take UNDRA_BIN first; Xcode's build phase looks in the installers' directories
# before PATH, so an `undra` installed there would build the iOS core instead of this one.
export UNDRA_BIN="$tmp/bin/undra"
for other in "$HOME/.undra/bin/undra" "$HOME/.cargo/bin/undra" /opt/homebrew/bin/undra /usr/local/bin/undra; do
  if has ios && [ -x "$other" ]; then echo "warning: $other exists: Xcode's build phase will use it for the iOS core" >&2; fi
done
export UNDRA_DIST_GIT_URL="$git_url" UNDRA_DIST_RELEASE_URL="$release_url" UNDRA_DIST_MAVEN_REPO="file://$m2"
log "undra init rehearsal (no --undra-path), platforms $platforms"
(cd "$apps" && undra init rehearsal --platforms "$platforms")
project=$apps/rehearsal
unset UNDRA_DIST_GIT_URL UNDRA_DIST_RELEASE_URL UNDRA_DIST_MAVEN_REPO
checkout_refs() { # <dir>: files that name the checkout or the snapshot
  grep -rIl -e "$repo" -e "$tmp/src" "$1" --exclude-dir=node_modules --exclude-dir=target --exclude-dir=build \
    --exclude-dir=.gradle --exclude-dir=DerivedData 2>/dev/null || true
}
named=$(checkout_refs "$project")
[ -z "$named" ] || fail "the project names a checkout: $named"
grep -q "undra = { git = \"$git_url\", tag = \"v$version\" }" "$project/core/Cargo.toml" || fail "core/Cargo.toml does not pin v$version of the remote"
record "undra init" ok "core pins v$version of the remote"

status=0
build() { # <label> <dir> <log> <command...>
  local label=$1 dir=$2 out=$3
  shift 3
  local t0
  t0=$(now)
  log "$label: $*"
  if (cd "$dir" && "$@") >"$out" 2>&1; then
    if grep -q -e "$repo" -e "$tmp/src" "$out"; then
      record "$label" FAIL "the build named a checkout (see $out)"
      status=1
    else
      record "$label" ok "built without the checkout in $(($(now) - t0)) s"
    fi
  else
    tail -n 40 "$out" >&2
    record "$label" FAIL "after $(($(now) - t0)) s (see $out)"
    status=1
  fi
}

if has web; then
  build "web (npm run build)" "$project/web" "$tmp/web.log" \
    sh -c 'npm install --no-audit --no-fund --prefer-offline && npm run build'
  grep -q "\"@undra/runtime\": \"$release_url/v$version/undra-runtime-$version.tgz\"" "$project/web/package.json" ||
    { record "web: runtime by URL" FAIL "web/package.json"; status=1; }
fi
if has ios; then
  build "iOS (xcodebuild, simulator)" "$project/ios" "$tmp/ios.log" \
    xcodebuild -project Rehearsal.xcodeproj -scheme Rehearsal -destination 'generic/platform=iOS Simulator' \
    -derivedDataPath "$tmp/DerivedData" -clonedSourcePackagesDirPath "$tmp/spm" CODE_SIGNING_ALLOWED=NO build
  resolved=$(find "$project/ios" "$tmp/spm" -name Package.resolved 2>/dev/null | head -n 1)
  if [ -n "$resolved" ] && grep -q "\"version\" *: *\"$version\"" "$resolved" && grep -q "$tmp/remote/undra.git" "$resolved"; then
    record "iOS: Swift package" ok "undra $version from the bare clone ($resolved)"
  else
    record "iOS: Swift package" FAIL "Package.resolved does not pin undra $version from the bare clone"
    status=1
  fi
fi
if has android; then
  sdk=${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}
  [ -x "$project/android/gradlew" ] || fail "undra init made no Gradle wrapper (is gradle installed?)"
  printf 'sdk.dir=%s\n' "$sdk" >"$project/android/local.properties"
  build "Android (assembleDebug)" "$project/android" "$tmp/android.log" \
    ./gradlew --no-daemon --console=plain :app:assembleDebug
  apk=$(find "$project/android/app/build/outputs/apk/debug" -name '*.apk' 2>/dev/null | head -n 1)
  [ -n "$apk" ] || { record "Android: APK" FAIL "no APK"; status=1; }
fi

printf '\n%s\n' "Rehearsal of Undra $version"
printf '  %s\n' "${summary[@]}"
[ "$status" = 0 ] || fail "a platform did not build without the checkout"
printf '\nEvery platform built from the rehearsal release alone.\n'
