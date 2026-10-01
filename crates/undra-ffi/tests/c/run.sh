#!/usr/bin/env bash
# Builds the fixture core (libundra_fixture, namespace `undra_fixture`) and runs the C hosts written
# against undra.h, with -Wall -Wextra -Werror. Since C ABI version 2 (ADR-044) a host reaches a core
# through the table its one export returns (`undra_fixture_undra_api()`), so both hosts call `api->...`:
#
#   smoke.c     the ABI end to end (the table's header fields, init, sync port, calls, snapshot
#               layout, shutdown, re-init)
#   lifetime.c  the host contract: a port's `user` is free()d the moment its removal returns, callbacks
#               run concurrently, a late Log answer does not loop. Built with AddressSanitizer when the
#               compiler has it (UNDRA_C_SANITIZE=1 makes that a requirement, =0 turns it off), so a
#               callback that outlives its registration is a heap-use-after-free report.
#   two_cores.c two copies of the library dlopen'ed side by side, each its own image: a panic through
#               either table is a status 2 reply, and shutting one down while the other has a stream
#               open and a port call in flight leaves the other working and never calls the first
#               one's callbacks again (also under AddressSanitizer).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../../../.." && pwd)"
HEADER="$REPO/runtimes/swift/UndraRuntime/Sources/UndraFFI/include"
OUT="${UNDRA_C_SCRATCH:-$(mktemp -d)}"
CC="${CC:-cc}"

FIXTURE="$HERE/../fixture"
cargo build --manifest-path "$FIXTURE/Cargo.toml"
LIBDIR="${CARGO_TARGET_DIR:-$FIXTURE/target}/debug"
CFLAGS=(-std=c11 -Wall -Wextra -Werror -I"$HEADER")
LINK=(-L"$LIBDIR" -lundra_fixture -Wl,-rpath,"$LIBDIR" -pthread)

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

# two_cores.c: two copies of the library, each dlopen'ed as its own image (ADR-044), one shut down
# while the other has a stream open and a port call in flight.
case "$(uname -s)" in Darwin) EXT=dylib ;; *) EXT=so ;; esac
cp "$LIBDIR/libundra_fixture.$EXT" "$OUT/libcore_a.$EXT"
cp "$LIBDIR/libundra_fixture.$EXT" "$OUT/libcore_b.$EXT"
DL=()
if [ "$(uname -s)" = "Linux" ]; then DL=(-ldl); fi
"$CC" "${CFLAGS[@]}" ${SAN[@]+"${SAN[@]}"} "$HERE/two_cores.c" -pthread ${DL[@]+"${DL[@]}"} -o "$OUT/two_cores"
"$OUT/two_cores" "$OUT/libcore_a.$EXT" "$OUT/libcore_b.$EXT"
