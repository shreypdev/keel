#!/usr/bin/env bash
# The C++ half of @undra/react-native against the real native core (cpp/test/host_test.cpp), built
# with AddressSanitizer and UndefinedBehaviorSanitizer.
#
#   runtimes/rn/@undra/react-native/cpp/test/run.sh
#
# Needs the playground core for this machine: `undra build -C examples/playground --platform host`
# (built here when missing). UNDRA_CORE_DYLIB points at another core.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
pkg="$(cd "$here/../.." && pwd)"
root="$(cd "$pkg/../../../.." && pwd)"
core="${UNDRA_CORE_DYLIB:-$root/examples/playground/build/host/libundra_core.dylib}"
case "$(uname -s)" in
  Linux) core="${UNDRA_CORE_DYLIB:-$root/examples/playground/build/host/libundra_core.so}" ;;
esac

if [ ! -f "$core" ]; then
  echo "==> building the playground core for this machine" >&2
  undra="${UNDRA_CLI:-$root/target/debug/undra}"
  [ -x "$undra" ] || (cd "$root" && cargo build -p undra-cli >&2)
  "$undra" build -C "$root/examples/playground" --platform host >&2
fi

out="$(mktemp -d)"
trap 'rm -rf "$out"' EXIT
libdir="$(dirname "$core")"
flags=(-std=c++20 -g -O1 -Wall -Wextra -Werror -fsanitize=address,undefined -fno-omit-frame-pointer
  -fno-sanitize-recover=undefined -I "$pkg/cpp")

# 1. The linked shim (UndraApiLinked.cpp, what iOS builds): the core linked into the test.
echo "# linked core (the iOS shim)"
"${CXX:-clang++}" "${flags[@]}" \
  "$pkg/cpp/UndraApiLinked.cpp" "$pkg/cpp/UndraHost.cpp" "$here/host_test.cpp" \
  -L "$libdir" -lundra_core -Wl,-rpath,"$libdir" \
  -o "$out/host_test_linked"
"$out/host_test_linked"

# 2. The dlopen shim (UndraApiAndroid.cpp, what Android builds): the same test, the core opened at
#    run time by path, nothing linked.
echo "# dlopen'ed core (the Android shim)"
"${CXX:-clang++}" "${flags[@]}" -DUNDRA_RN_DLOPEN "-DUNDRA_RN_CORE_LIBRARY=\"$core\"" \
  "$pkg/cpp/UndraApiAndroid.cpp" "$pkg/cpp/UndraHost.cpp" "$here/host_test.cpp" \
  -o "$out/host_test_dlopen"
"$out/host_test_dlopen"
