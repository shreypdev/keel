#!/bin/sh
# The one program every Undra Bazel action runs (ADR-061).
#
# It makes a private, offline, location-independent copy of everything a build reads, then runs one command in it:
#
#   mode=cli     cargo build of the `undra` CLI from the Undra checkout
#   mode=build   `undra build -C <project> --platform <platform>`
#   mode=bindgen `undra bindgen -C <project> --library <the host library>`
#
# Its only argument is a parameter file of `key=value` lines (written by the rules; keys may repeat). Paths in it are
# relative to the execution root, which is the working directory. Nothing is read from the user's home, the network or
# the environment but what the lines say: Cargo's directory source is the crates.io archives named by the vendor manifest,
# and `rustc` is the toolchain Bazel resolved, with its sysroot merged from the toolchains the action was given.
set -eu

PARAMS="$1"
EXECROOT="$(pwd -P)"

# --- parameters -------------------------------------------------------------------------------------------------
MODE=""
CLI=""
RUSTC=""
CARGO=""
VENDOR_MANIFEST=""
PROJECT="."
PLATFORM="host"
RELEASE=0
SYMBOLS=0
WASM_OPT=""
LIBRARY=""
OUT_FILE=""
OUT_DIR=""
OUT_SYMBOLS=""
NAMESPACE=""
UNDRA_ROOT=""
APP_ROOT=""
SYSROOTS=""
OUT_SWIFT=""
OUT_KOTLIN=""
OUT_TS=""
OUT_KOTLIN_SRCJAR=""
ZIPPER=""
SWIFT_FILES=""
BINDGEN_PLATFORMS=""
BINDGEN_DOCS=0
EXTRA_PATH=""
INHERIT_PATH=0
ORIG_PATH="${PATH:-}"
EXTRA_ENV=""
while IFS= read -r line || [ -n "$line" ]; do
  key="${line%%=*}"
  value="${line#*=}"
  case "$key" in
    mode) MODE="$value" ;;
    cli) CLI="$value" ;;
    rustc) RUSTC="$value" ;;
    cargo) CARGO="$value" ;;
    sysroot) SYSROOTS="$SYSROOTS $value" ;;
    vendor) VENDOR_MANIFEST="$value" ;;
    project) PROJECT="$value" ;;
    platform) PLATFORM="$value" ;;
    release) RELEASE="$value" ;;
    symbols) SYMBOLS="$value" ;;
    wasm_opt) WASM_OPT="$value" ;;
    library) LIBRARY="$value" ;;
    out_file) OUT_FILE="$value" ;;
    out_dir) OUT_DIR="$value" ;;
    out_symbols) OUT_SYMBOLS="$value" ;;
    namespace) NAMESPACE="$value" ;;
    undra_root) UNDRA_ROOT="$value" ;;
    app_root) APP_ROOT="$value" ;;
    out_swift) OUT_SWIFT="$value" ;;
    out_kotlin) OUT_KOTLIN="$value" ;;
    out_ts) OUT_TS="$value" ;;
    out_kotlin_srcjar) OUT_KOTLIN_SRCJAR="$value" ;;
    zipper) ZIPPER="$value" ;;
    swift_file) SWIFT_FILES="$SWIFT_FILES
$value" ;;
    bindgen_platforms) BINDGEN_PLATFORMS="$value" ;;
    bindgen_docs) BINDGEN_DOCS="$value" ;;
    path) case "$value" in /*) EXTRA_PATH="$EXTRA_PATH:$value" ;; *) EXTRA_PATH="$EXTRA_PATH:$EXECROOT/$value" ;; esac ;;
    inherit_path) INHERIT_PATH="$value" ;;
    env) EXTRA_ENV="$EXTRA_ENV
$value" ;;
    '') ;;
    *) echo "undra bazel: unknown parameter '$key'" >&2; exit 2 ;;
  esac
done < "$PARAMS"

die() { echo "undra bazel: $*" >&2; exit 1; }
abs() { case "$1" in /*) printf '%s' "$1" ;; *) printf '%s/%s' "$EXECROOT" "$1" ;; esac; }

# --- where the build happens --------------------------------------------------------------------------------------
# Cargo hashes a path dependency that lies outside the workspace (the core, the Undra crates) by its absolute path into every
# symbol's name, and `wasm-opt` and the linker order what they emit by those names: a build in a directory with another name is
# another file, a few bytes different (measured: 274.6 to 275.1 KB for one core). So the directory is named by what is built, not
# by when: the same inputs are built in the same place on every machine and give the same bytes. A lock keeps two builds of
# the same inputs (two Bazel servers, one machine) from sharing it; on Linux the sandbox's /tmp is private and it never waits.
digest_tree() { # <directory>
  (cd "$1" && find . \( -name target -o -name .git -o -name node_modules -o -name '.cargo' -o -name 'bazel-*' \) -prune -o -type f -print |
    LC_ALL=C sort | while IFS= read -r f; do printf '%s\n' "$f"; cat "$f"; done)
}
sha() { if command -v sha256sum >/dev/null 2>&1; then sha256sum; else shasum -a 256; fi; }
DIGEST="$({
  printf 'mode=%s platform=%s release=%s symbols=%s project=%s\n' "$MODE" "$PLATFORM" "$RELEASE" "$SYMBOLS" "$PROJECT"
  "$(abs "$RUSTC")" --version 2>/dev/null || true
  [ -n "$UNDRA_ROOT" ] && digest_tree "$(abs "$UNDRA_ROOT")"
  [ -n "$APP_ROOT" ] && digest_tree "$(abs "$APP_ROOT")"
  [ -n "$VENDOR_MANIFEST" ] && cat "$(abs "$VENDOR_MANIFEST")"
  [ -n "$LIBRARY" ] && cat "$(abs "$LIBRARY")"
  true
} | sha | cut -c1-16)"
STAGE_BASE=/tmp/undra-bazel
mkdir -p "$STAGE_BASE" 2>/dev/null || { STAGE_BASE="${TMPDIR:-/tmp}/undra-bazel"; mkdir -p "$STAGE_BASE"; }
WORK="$STAGE_BASE/$DIGEST"
LOCK="$WORK.lock"
waited=0
until mkdir "$LOCK" 2>/dev/null; do
  owner="$(cat "$LOCK/pid" 2>/dev/null || true)"
  if [ -n "$owner" ] && ! kill -0 "$owner" 2>/dev/null; then rm -rf "$LOCK"; continue; fi # its owner is gone
  if [ -z "$owner" ] && [ -n "$(find "$LOCK" -maxdepth 0 -mmin +2 2>/dev/null)" ]; then rm -rf "$LOCK"; continue; fi
  waited=$((waited + 1))
  [ "$waited" -le 1800 ] || die "waited 30 minutes for $LOCK, held by '$owner'"
  sleep 1
done
echo "$$" > "$LOCK/pid"
trap 'rm -rf "$WORK" "$LOCK"' EXIT HUP INT TERM
rm -rf "$WORK"
mkdir -p "$WORK"

# --- a private copy of the sources -------------------------------------------------------------------------------
# `tar -h` (bsdtar and GNU tar both spell it so) follows symlinks, so what lands in $WORK is real files: a build that writes
# next to its sources (`undra build` writes `build/`) never writes into the user's tree, whatever the sandbox does.
copy_tree() { # <source dir> <destination dir>
  mkdir -p "$2"
  (cd "$1" && tar -chf - --exclude=target --exclude=.git --exclude='bazel-*' --exclude=node_modules .) | (cd "$2" && tar -xf -)
}

# --- the Rust toolchain ------------------------------------------------------------------------------------------
# rustc finds its standard library from `--sysroot`, and the toolchain Bazel resolves for a target is the sysroot of that
# target alone, so a wasm build (whose build scripts and proc macros run on the host) is given the host's sysroot and the
# wasm one, merged here file by file.
SYSROOT="$WORK/sysroot"
mkdir -p "$SYSROOT"
for root in $SYSROOTS; do
  root="$(abs "$root")"
  (cd "$root" && find . \( -type f -o -type l \) -print) | while IFS= read -r file; do
    file="${file#./}"
    [ -e "$SYSROOT/$file" ] || [ -L "$SYSROOT/$file" ] && continue
    mkdir -p "$SYSROOT/$(dirname "$file")"
    ln -s "$root/$file" "$SYSROOT/$file"
  done
done
REAL_RUSTC="$(abs "$RUSTC")"
mkdir -p "$WORK/bin"
cat > "$WORK/bin/rustc" <<EOF
#!/bin/sh
exec "$REAL_RUSTC" --sysroot "$SYSROOT" "\$@"
EOF
chmod +x "$WORK/bin/rustc"
ln -s "$(abs "$CARGO")" "$WORK/bin/cargo"

# --- Cargo, offline ----------------------------------------------------------------------------------------------
export CARGO_HOME="$WORK/cargo-home"
mkdir -p "$CARGO_HOME" "$WORK/home"
if [ -n "$VENDOR_MANIFEST" ]; then
  # Where Cargo's own registry would unpack the crates (`registry/src/<index>-<hash>`), under the name every Cargo of 1.85 or
  # later gives crates.io's index: `undra build` remaps `$CARGO_HOME/registry/src` to a fixed label in a release build, so a
  # crate's path in the binary is the same here as in a build that downloaded it.
  VENDOR_DIR="$CARGO_HOME/registry/src/index.crates.io-1949cf8c6b5b557f"
  VENDOR_BASE="$(dirname "$(abs "$VENDOR_MANIFEST")")"
  mkdir -p "$VENDOR_DIR"
  while IFS='|' read -r name version checksum archive; do
    [ -n "$name" ] || continue
    tar -xzf "$VENDOR_BASE/$archive" -C "$VENDOR_DIR"
    printf '{"files":{},"package":"%s"}\n' "$checksum" > "$VENDOR_DIR/$name-$version/.cargo-checksum.json"
  done < "$(abs "$VENDOR_MANIFEST")"
fi

if [ -n "$UNDRA_ROOT" ]; then
  copy_tree "$(abs "$UNDRA_ROOT")" "$WORK/undra"
  # Only the library crates are members: the examples and benchmarks of the checkout are not inputs of any build here, and
  # Cargo reads every member's manifest when it loads a workspace.
  sed -e '/"examples\//d' -e '/"bench"/d' "$WORK/undra/Cargo.toml" > "$WORK/undra/Cargo.toml.pruned"
  mv "$WORK/undra/Cargo.toml.pruned" "$WORK/undra/Cargo.toml"
fi

{
  echo '[net]'
  echo 'offline = true'
  echo '[term]'
  echo 'color = "never"'
  if [ -n "$VENDOR_MANIFEST" ]; then
    echo '[source.crates-io]'
    echo 'replace-with = "vendored-sources"'
    echo '[source.vendored-sources]'
    echo "directory = \"$VENDOR_DIR\""
  fi
  if [ -n "$UNDRA_ROOT" ] && [ "$MODE" != cli ]; then
    # The core depends on `undra = "0.1"` like any app; this is where the checkout stands in for the registry.
    echo '[patch.crates-io]'
    for crate in "$WORK"/undra/crates/*/; do
      crate="${crate%/}"
      [ -f "$crate/Cargo.toml" ] || continue
      case "$(basename "$crate")" in
        # Never a dependency of an app's core: patching them only makes Cargo warn that the patch is unused.
        undra-cli | undra-bindgen | undra-transport) continue ;;
      esac
      echo "$(basename "$crate") = { path = \"$crate\" }"
    done
  fi
} > "$CARGO_HOME/config.toml"

# The toolchain's cargo and rustc first, then the system's; a platform whose toolchain is the machine's (Xcode, the NDK and
# cargo-ndk) also gets the PATH the build was started with, after them.
export PATH="$WORK/bin${EXTRA_PATH}:/usr/bin:/bin:/usr/sbin:/sbin"
if [ "$INHERIT_PATH" = 1 ] && [ -n "$ORIG_PATH" ]; then export PATH="$PATH:$ORIG_PATH"; fi
export RUSTC="$WORK/bin/rustc"
export HOME="$WORK/home"
export CARGO_TARGET_DIR="$WORK/target"
export CARGO_INCREMENTAL=0
export CARGO_NET_OFFLINE=true
export CARGO_TERM_COLOR=never
export LC_ALL=C
export TZ=UTC
unset RUSTUP_TOOLCHAIN RUSTFLAGS CARGO_ENCODED_RUSTFLAGS RUSTC_WRAPPER RUSTC_WORKSPACE_WRAPPER
if [ -n "$EXTRA_ENV" ]; then
  # key=value lines
  OLD_IFS="$IFS"
  IFS='
'
  for pair in $EXTRA_ENV; do
    [ -n "$pair" ] && export "$pair"
  done
  IFS="$OLD_IFS"
fi

# --- the command -------------------------------------------------------------------------------------------------
case "$MODE" in
  cli)
    (cd "$WORK/undra" && cargo build --offline -p undra-cli --bin undra)
    cp "$WORK/target/debug/undra" "$(abs "$OUT_FILE")"
    ;;
  build)
    [ -n "$APP_ROOT" ] || die "no app_root"
    copy_tree "$(abs "$APP_ROOT")" "$WORK/app"
    PROJECT_DIR="$WORK/app/$PROJECT"
    [ -f "$PROJECT_DIR/undra.toml" ] || die "there is no undra.toml in $PROJECT of the app's files"
    # A lock file in the project seeds the shim's (undra build); the checkout's stands in for it when the app has none.
    if [ ! -f "$WORK/app/Cargo.lock" ] && [ ! -f "$PROJECT_DIR/Cargo.lock" ] && [ -f "$WORK/undra/Cargo.lock" ]; then
      cp "$WORK/undra/Cargo.lock" "$PROJECT_DIR/Cargo.lock"
    fi
    if [ -n "$WASM_OPT" ]; then
      mkdir -p "$WORK/tools"
      ln -s "$(abs "$WASM_OPT")" "$WORK/tools/wasm-opt"
      export PATH="$WORK/tools:$PATH"
    fi
    set -- build -C "$PROJECT_DIR" --platform "$PLATFORM"
    [ "$RELEASE" = 1 ] && set -- "$@" --release
    [ "$SYMBOLS" = 1 ] || set -- "$@" --no-symbols
    "$(abs "$CLI")" "$@"
    BUILD_DIR="$PROJECT_DIR/build"
    case "$PLATFORM" in
      host)
        found=""
        for candidate in "$BUILD_DIR/host/lib$NAMESPACE.dylib" "$BUILD_DIR/host/lib$NAMESPACE.so"; do
          [ -f "$candidate" ] && found="$candidate"
        done
        [ -n "$found" ] || die "undra build did not produce lib$NAMESPACE below build/host (it produced: $(ls "$BUILD_DIR/host" 2>/dev/null | tr '\n' ' ')): undra_core(namespace = \"$NAMESPACE\") must be the [core] namespace of undra.toml"
        cp "$found" "$(abs "$OUT_FILE")"
        ;;
      web)
        found="$BUILD_DIR/web/$NAMESPACE.wasm"
        [ -f "$found" ] || die "undra build did not produce $NAMESPACE.wasm below build/web (it produced: $(ls "$BUILD_DIR/web" 2>/dev/null | tr '\n' ' ')): undra_core(namespace = \"$NAMESPACE\") must be the [core] namespace of undra.toml"
        cp "$found" "$(abs "$OUT_FILE")"
        ;;
      ios)
        mkdir -p "$(abs "$OUT_DIR")"
        cp -R "$BUILD_DIR"/ios/. "$(abs "$OUT_DIR")/"
        ;;
      android)
        mkdir -p "$(abs "$OUT_DIR")"
        cp -R "$BUILD_DIR"/android/. "$(abs "$OUT_DIR")/"
        ;;
      *) die "unknown platform '$PLATFORM'" ;;
    esac
    if [ -n "$OUT_SYMBOLS" ]; then
      mkdir -p "$(abs "$OUT_SYMBOLS")"
      [ -d "$BUILD_DIR/symbols" ] && cp -R "$BUILD_DIR"/symbols/. "$(abs "$OUT_SYMBOLS")/"
    fi
    ;;
  bindgen)
    [ -n "$APP_ROOT" ] || die "no app_root"
    copy_tree "$(abs "$APP_ROOT")" "$WORK/app"
    PROJECT_DIR="$WORK/app/$PROJECT"
    [ -f "$PROJECT_DIR/undra.toml" ] || die "there is no undra.toml in $PROJECT of the app's files"
    GEN="$WORK/generated"
    set -- bindgen -C "$PROJECT_DIR" --library "$(abs "$LIBRARY")" --out "$GEN"
    [ -n "$BINDGEN_PLATFORMS" ] && set -- "$@" --platforms "$BINDGEN_PLATFORMS"
    [ "$BINDGEN_DOCS" = 1 ] && set -- "$@" --docs
    "$(abs "$CLI")" "$@"
    # One declared tree per language: the directory `undra bindgen` wrote for it.
    for language in swift kotlin ts; do
      case "$language" in
        swift) dest="$OUT_SWIFT" ;;
        kotlin) dest="$OUT_KOTLIN" ;;
        ts) dest="$OUT_TS" ;;
      esac
      [ -n "$dest" ] || continue
      [ -d "$GEN/$language" ] || die "undra bindgen wrote no $language tree"
      mkdir -p "$(abs "$dest")"
      cp -R "$GEN/$language"/. "$(abs "$dest")/"
    done
    if [ -n "$SWIFT_FILES" ]; then
      # The Swift files `rules_swift` compiles, one declared output each. The set depends on the schema, so it is checked
      # both ways: a file that is listed and not generated, and one that is generated and not listed.
      LISTED="$WORK/swift.listed"
      ACTUAL="$WORK/swift.actual"
      printf '%s\n' "$SWIFT_FILES" | sed -e '/^$/d' -e 's#|.*##' | LC_ALL=C sort > "$LISTED"
      (cd "$GEN/swift" && find Sources -type f \( -name '*.swift' -o -name '*.c' -o -name '*.h' -o -name module.modulemap \) | LC_ALL=C sort) > "$ACTUAL"
      if ! cmp -s "$LISTED" "$ACTUAL"; then
        echo "undra bazel: the Swift files in swift_files are not the ones undra bindgen generated for this core." >&2
        echo "undra bazel: set swift_files to:" >&2
        sed -e 's#^#    "#' -e 's#$#",#' "$ACTUAL" >&2
        exit 1
      fi
      printf '%s\n' "$SWIFT_FILES" | sed -e '/^$/d' | while IFS='|' read -r rel dest; do
        mkdir -p "$(dirname "$(abs "$dest")")"
        cp "$GEN/swift/$rel" "$(abs "$dest")"
      done
    fi
    if [ -n "$OUT_KOTLIN_SRCJAR" ]; then
      # The Kotlin sources as a source jar (what rules_kotlin takes), entries named by their package path.
      SRC="$GEN/kotlin/src/main/kotlin"
      ENTRIES="$WORK/srcjar.entries"
      (cd "$SRC" && find . -type f -name '*.kt' | sort) | sed -e 's#^\./##' | while IFS= read -r entry; do
        printf '%s=%s\n' "$entry" "$SRC/$entry"
      done > "$ENTRIES"
      # shellcheck disable=SC2046
      "$(abs "$ZIPPER")" cC "$(abs "$OUT_KOTLIN_SRCJAR")" $(cat "$ENTRIES")
    fi
    ;;
  *) die "unknown mode '$MODE'" ;;
esac
