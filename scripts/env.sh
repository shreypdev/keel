# Source this file to put the Keel toolchain on PATH.
#   source scripts/env.sh
#
# Two layouts are supported:
#   1. A portable toolchain beside the repo in ../.tools (the Cowork Linux VM layout).
#   2. Native macOS with rustup (~/.cargo) and Homebrew (openjdk@17, kotlin) installs.
# Whatever is found wins, .tools first so the VM keeps working unchanged.
_KEEL_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")/.." && pwd)"
_KEEL_TOOLS="$(cd "$_KEEL_ROOT/.." && pwd)/.tools"
# A worktree under <repos>/.work/<slug> finds the toolchain two levels up.
[ -d "$_KEEL_TOOLS" ] || _KEEL_TOOLS="$(cd "$_KEEL_ROOT/../.." && pwd)/.tools"
# A desktop-app worktree under keel/.claude/worktrees/<name> finds it three levels up.
[ -d "$_KEEL_TOOLS" ] || _KEEL_TOOLS="$(cd "$_KEEL_ROOT/../../.." && pwd)/.tools"

# --- Rust ------------------------------------------------------------------------------------
# Only use the portable toolchain when its binaries actually run here (they are Linux
# ELF executables; on macOS they would shadow a working cargo and break every build).
if [ -d "$_KEEL_TOOLS/rust/bin" ] && "$_KEEL_TOOLS/rust/bin/cargo" --version >/dev/null 2>&1; then
  export PATH="$_KEEL_TOOLS/rust/bin:$PATH"
  # This toolchain builds wasm32 std from source (rust-src is installed).
  export KEEL_WASM_BUILD_STD="-Z build-std=std,panic_abort"
elif [ -d "$HOME/.cargo/bin" ]; then
  export PATH="$HOME/.cargo/bin:$PATH"
  # rustup stable ships a prebuilt wasm32-unknown-unknown std; no build-std needed.
  export KEEL_WASM_BUILD_STD=""
fi

# --- Swift / Xcode ---------------------------------------------------------------------------
# XCTest and the simulators need full Xcode. If it is installed but xcode-select still points at
# CommandLineTools, DEVELOPER_DIR routes swift/xcodebuild there without needing sudo.
if [ -z "${DEVELOPER_DIR:-}" ] && [ -d "/Applications/Xcode.app/Contents/Developer" ]; then
  case "$(xcode-select -p 2>/dev/null)" in
    */CommandLineTools) export DEVELOPER_DIR="/Applications/Xcode.app/Contents/Developer" ;;
  esac
fi

# --- JVM -------------------------------------------------------------------------------------
if [ -d "/opt/homebrew/opt/openjdk@17/bin" ] && ! command -v java >/dev/null 2>&1; then
  export PATH="/opt/homebrew/opt/openjdk@17/bin:$PATH"
fi

# --- Kotlin ----------------------------------------------------------------------------------
[ -d "$_KEEL_TOOLS/kotlinc/bin" ] && export PATH="$_KEEL_TOOLS/kotlinc/bin:$PATH"
if [ -f "$_KEEL_TOOLS/lib/kotlinx-coroutines-core-jvm-1.6.4.jar" ]; then
  export KEEL_KOTLINX_COROUTINES="$_KEEL_TOOLS/lib/kotlinx-coroutines-core-jvm-1.6.4.jar"
fi
# Homebrew kotlinc is a wrapper script, which defeats test-local.sh's stdlib guess; point at
# the Cellar jar directly.
if [ -z "${KEEL_KOTLIN_STDLIB:-}" ]; then
  for _keel_jar in /opt/homebrew/Cellar/kotlin/*/libexec/lib/kotlin-stdlib.jar; do
    [ -f "$_keel_jar" ] && export KEEL_KOTLIN_STDLIB="$_keel_jar"
  done
fi
unset _keel_jar _KEEL_ROOT _KEEL_TOOLS
