#!/usr/bin/env bash
# Builds libkeel_ffi and runs smoke.c, a C host written against keel.h (-Wall -Wextra -Werror).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../../../.." && pwd)"
HEADER="$REPO/runtimes/swift/KeelRuntime/Sources/KeelFFI/include"
OUT="${KEEL_C_SCRATCH:-$(mktemp -d)}"

cargo build --manifest-path "$REPO/Cargo.toml" -p keel-ffi
LIBDIR="${CARGO_TARGET_DIR:-$REPO/target}/debug"
"${CC:-cc}" -std=c11 -Wall -Wextra -Werror -I"$HEADER" "$HERE/smoke.c" -L"$LIBDIR" -lkeel_ffi \
  -Wl,-rpath,"$LIBDIR" -o "$OUT/smoke"
"$OUT/smoke"
