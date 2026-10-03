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
OUT_ABIS=""
UNDRA_ROOT=""
APP_ROOT=""
SYSROOTS=""
OUT_SWIFT=""
OUT_KOTLIN=""
OUT_TS=""
OUT_KOTLIN_SRCJAR=""
ZIPPER=""
BINDGEN_PLATFORMS=""
BINDGEN_DOCS=0
EXTRA_PATH=""
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
    abis) OUT_ABIS="$value" ;;
    undra_root) UNDRA_ROOT="$value" ;;
    app_root) APP_ROOT="$value" ;;
    out_swift) OUT_SWIFT="$value" ;;
    out_kotlin) OUT_KOTLIN="$value" ;;
    out_ts) OUT_TS="$value" ;;
    out_kotlin_srcjar) OUT_KOTLIN_SRCJAR="$value" ;;
    zipper) ZIPPER="$value" ;;
    bindgen_platforms) BINDGEN_PLATFORMS="$value" ;;
    bindgen_docs) BINDGEN_DOCS="$value" ;;
    path) EXTRA_PATH="$EXTRA_PATH:$EXECROOT/$value" ;;
    env) EXTRA_ENV="$EXTRA_ENV
$value" ;;
    '') ;;
    *) echo "undra bazel: unknown parameter '$key'" >&2; exit 2 ;;
  esac
done < "$PARAMS"

die() { echo "undra bazel: $*" >&2; exit 1; }
abs() { case "$1" in /*) printf '%s' "$1" ;; *) printf '%s/%s' "$EXECROOT" "$1" ;; esac; }

WORK="$(mktemp -d "${TMPDIR:-/tmp}/undra-bazel.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT HUP INT TERM

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

export PATH="$WORK/bin${EXTRA_PATH}:/usr/bin:/bin:/usr/sbin:/sbin"
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
        for candidate in "$BUILD_DIR"/host/lib*.dylib "$BUILD_DIR"/host/lib*.so; do
          [ -f "$candidate" ] && found="$candidate"
        done
        [ -n "$found" ] || die "undra build produced no host library below $BUILD_DIR/host"
        cp "$found" "$(abs "$OUT_FILE")"
        ;;
      web)
        found=""
        for candidate in "$BUILD_DIR"/web/*.wasm; do
          case "$candidate" in *.debug.wasm | *.dwarf.wasm) ;; *) [ -f "$candidate" ] && found="$candidate" ;; esac
        done
        [ -n "$found" ] || die "undra build produced no wasm module below $BUILD_DIR/web"
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
