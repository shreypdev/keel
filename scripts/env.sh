# Source this file to use the Keel local toolchain that lives beside the repo in ../.tools
# (installed for the Cowork Linux VM on Shrey's Mac). On macOS proper, use your own rustup/Xcode.
#   source scripts/env.sh
_KEEL_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")/.." && pwd)"
_KEEL_TOOLS="$(cd "$_KEEL_ROOT/.." && pwd)/.tools"
[ -d "$_KEEL_TOOLS/rust/bin" ] && export PATH="$_KEEL_TOOLS/rust/bin:$PATH"
[ -d "$_KEEL_TOOLS/kotlinc/bin" ] && export PATH="$_KEEL_TOOLS/kotlinc/bin:$PATH"
[ -f "$_KEEL_TOOLS/lib/kotlinx-coroutines-core-jvm-1.6.4.jar" ] && export KEEL_KOTLINX_COROUTINES="$_KEEL_TOOLS/lib/kotlinx-coroutines-core-jvm-1.6.4.jar"
# wasm32 std is built from source on this toolchain (rust-src is installed):
export KEEL_WASM_BUILD_STD="-Z build-std=std,panic_abort"
unset _KEEL_ROOT _KEEL_TOOLS
