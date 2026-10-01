# Source this file to put the Undra toolchain on PATH.
#   source scripts/env.sh
#
# Two layouts are supported:
#   1. A portable toolchain beside the repo in ../.tools (the Cowork Linux VM layout).
#   2. Native macOS with rustup (~/.cargo) and Homebrew (openjdk@17, kotlin) installs.
# Whatever is found wins, .tools first so the VM keeps working unchanged.
_UNDRA_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")/.." && pwd)"
_UNDRA_TOOLS="$(cd "$_UNDRA_ROOT/.." && pwd)/.tools"
# A worktree under <repos>/.work/<slug> finds the toolchain two levels up.
[ -d "$_UNDRA_TOOLS" ] || _UNDRA_TOOLS="$(cd "$_UNDRA_ROOT/../.." && pwd)/.tools"
# A desktop-app worktree under undra/.claude/worktrees/<name> finds it three levels up.
[ -d "$_UNDRA_TOOLS" ] || _UNDRA_TOOLS="$(cd "$_UNDRA_ROOT/../../.." && pwd)/.tools"

# --- Rust ------------------------------------------------------------------------------------
# Only use the portable toolchain when its binaries actually run here (they are Linux
# ELF executables; on macOS they would shadow a working cargo and break every build).
if [ -d "$_UNDRA_TOOLS/rust/bin" ] && "$_UNDRA_TOOLS/rust/bin/cargo" --version >/dev/null 2>&1; then
  export PATH="$_UNDRA_TOOLS/rust/bin:$PATH"
  # This toolchain builds wasm32 std from source (rust-src is installed).
  export UNDRA_WASM_BUILD_STD="-Z build-std=std,panic_abort"
elif [ -d "$HOME/.cargo/bin" ]; then
  export PATH="$HOME/.cargo/bin:$PATH"
  # rustup stable ships a prebuilt wasm32-unknown-unknown std; no build-std needed.
  export UNDRA_WASM_BUILD_STD=""
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
[ -d "$_UNDRA_TOOLS/kotlinc/bin" ] && export PATH="$_UNDRA_TOOLS/kotlinc/bin:$PATH"
if [ -f "$_UNDRA_TOOLS/lib/kotlinx-coroutines-core-jvm-1.6.4.jar" ]; then
  export UNDRA_KOTLINX_COROUTINES="$_UNDRA_TOOLS/lib/kotlinx-coroutines-core-jvm-1.6.4.jar"
fi
# Homebrew kotlinc is a wrapper script, which defeats test-local.sh's stdlib guess; point at
# the Cellar jar directly.
if [ -z "${UNDRA_KOTLIN_STDLIB:-}" ]; then
  for _undra_jar in /opt/homebrew/Cellar/kotlin/*/libexec/lib/kotlin-stdlib.jar; do
    [ -f "$_undra_jar" ] && export UNDRA_KOTLIN_STDLIB="$_undra_jar"
  done
fi
unset _undra_jar _UNDRA_ROOT _UNDRA_TOOLS

# Gradle and the Android SDK: the macOS Java stub answers `java` when JAVA_HOME is unset, and the
# playground's Gradle build needs ANDROID_HOME (or sdk.dir in local.properties). Only set when present.
if [ -z "${JAVA_HOME:-}" ] && [ -d /opt/homebrew/opt/openjdk@17/libexec/openjdk.jdk/Contents/Home ]; then
  export JAVA_HOME=/opt/homebrew/opt/openjdk@17/libexec/openjdk.jdk/Contents/Home
fi
if [ -z "${ANDROID_HOME:-}" ] && [ -d /opt/homebrew/share/android-commandlinetools ]; then
  export ANDROID_HOME=/opt/homebrew/share/android-commandlinetools
fi
