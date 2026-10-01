#!/usr/bin/env bash
# Builds libundra_ffi and runs the C hosts written against undra.h, with -Wall -Wextra -Werror:
#
#   smoke.c     the ABI end to end (init, sync port, calls, snapshot layout, shutdown, re-init)
#   lifetime.c  the host contract: a port's `user` is free()d the moment its removal returns, callbacks
#               run concurrently, a late Log answer does not loop. Built with AddressSanitizer when the
#               compiler has it (UNDRA_C_SANITIZE=1 makes that a requirement, =0 turns it off), so a
#               callback that outlives its registration is a heap-use-after-free report.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../../../.." && pwd)"
HEADER="$REPO/runtimes/swift/UndraRuntime/Sources/UndraFFI/include"
OUT="${UNDRA_C_SCRATCH:-$(mktemp -d)}"
CC="${CC:-cc}"

cargo build --manifest-path "$REPO/Cargo.toml" -p undra-ffi
LIBDIR="${CARGO_TARGET_DIR:-$REPO/target}/debug"
CFLAGS=(-std=c11 -Wall -Wextra -Werror -I"$HEADER")
LINK=(-L"$LIBDIR" -lundra_ffi -Wl,-rpath,"$LIBDIR" -pthread)

"$CC" "${CFLAGS[@]}" "$HERE/smoke.c" "${LINK[@]}" -o "$OUT/smoke"
"$OUT/smoke"

SAN=()
case "${UNDRA_C_SANITIZE:-auto}" in
  0) ;;
  *)
    if echo 'int main(void){return 0;}' | "$CC" -x c -fsanitize=address - -o "$OUT/asan-probe" 2>/dev/null; then
      SAN=(-fsanitize=address -fno-omit-frame-pointer -g)
    elif [ "${UNDRA_C_SANITIZE:-auto}" = "1" ]; then
      echo "UNDRA_C_SANITIZE=1 but $CC cannot build with -fsanitize=address" >&2
      exit 1
    else
      echo "note: $CC has no AddressSanitizer; lifetime.c runs without it" >&2
    fi
    ;;
esac
"$CC" "${CFLAGS[@]}" ${SAN[@]+"${SAN[@]}"} "$HERE/lifetime.c" "${LINK[@]}" -o "$OUT/lifetime"
# LeakSanitizer (Linux) would judge the Rust runtime's process-lifetime statics; the point here is
# use-after-free, so it stays off.
if [ "$(uname -s)" = "Linux" ]; then export ASAN_OPTIONS="${ASAN_OPTIONS:+$ASAN_OPTIONS:}detect_leaks=0"; fi
"$OUT/lifetime"
