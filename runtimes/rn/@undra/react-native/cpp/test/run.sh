#!/usr/bin/env bash
# The C++ half of @undra/react-native against the real native core (cpp/test/host_test.cpp), built
# with AddressSanitizer and UndefinedBehaviorSanitizer.
#
#   runtimes/rn/@undra/react-native/cpp/test/run.sh
#
# Needs the playground core for this machine: `undra build -C examples/playground --platform host`
# (built here when missing, or older than the playground core's sources: the native-defaults checks
# call its `platform` module). UNDRA_CORE_DYLIB points at another core. CXX picks the compiler (clang++).
# The JSI compile check (step 3) is skipped when the playground app's dependencies are not installed;
# UNDRA_RN_REQUIRE_JSI=1 (CI) makes that a failure instead of a skip. The Db port's checks (step 0b and the
# Db part of steps 1 and 2) run against the system SQLite (`sqlite3.h` and `-lsqlite3`: macOS has both;
# Linux needs `libsqlite3-dev`) and are skipped without it; UNDRA_RN_REQUIRE_SQLITE=1 makes that a failure.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
pkg="$(cd "$here/../.." && pwd)"
root="$(cd "$pkg/../../../.." && pwd)"
core="${UNDRA_CORE_DYLIB:-$root/examples/playground/build/host/libundra_core.dylib}"
case "$(uname -s)" in
  Linux) core="${UNDRA_CORE_DYLIB:-$root/examples/playground/build/host/libundra_core.so}" ;;
esac

stale=0
if [ -f "$core" ] && [ -z "${UNDRA_CORE_DYLIB:-}" ] && [ -n "$(find "$root/examples/playground/core/src" -newer "$core" -name '*.rs' 2>/dev/null | head -n 1)" ]; then
  stale=1
fi
if [ ! -f "$core" ] || [ "$stale" = 1 ]; then
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

# 0. The portable Kv and Fs (UndraStores.cpp), on this machine's file system: no core needed.
echo "# the portable Kv and Fs stores"
"${CXX:-clang++}" "${flags[@]}" "$pkg/cpp/UndraStores.cpp" "$here/stores_test.cpp" -o "$out/stores_test"
"$out/stores_test"

# 0b. The Db port (UndraDb.cpp, the binding; UndraDbSqlite.cpp, the sqlite3 C API backend of iOS) against
#     the system SQLite, with no core: the binding's semantics, the typed errors, the SQL lexer against sqlite3.
host=("$pkg/cpp/UndraHost.cpp" "$pkg/cpp/UndraDefaults.cpp" "$pkg/cpp/UndraDb.cpp" "$pkg/cpp/UndraStores.cpp" "$here/host_test.cpp")
sqlite=()
printf '#include <sqlite3.h>\nint main() { return sqlite3_libversion_number() > 0 ? 0 : 1; }\n' >"$out/has_sqlite.cpp"
if "${CXX:-clang++}" "$out/has_sqlite.cpp" -lsqlite3 -o "$out/has_sqlite" >/dev/null 2>&1 && "$out/has_sqlite"; then
  echo "# the Db port against the system SQLite"
  "${CXX:-clang++}" "${flags[@]}" "$pkg/cpp/UndraStores.cpp" "$pkg/cpp/UndraDb.cpp" "$pkg/cpp/UndraDbSqlite.cpp" "$here/db_test.cpp" \
    -lsqlite3 -o "$out/db_test"
  "$out/db_test"
  # Steps 1 and 2 also check the Db port through the core.
  host+=("$pkg/cpp/UndraDbSqlite.cpp")
  sqlite=(-DUNDRA_RN_TEST_SQLITE -lsqlite3)
elif [ "${UNDRA_RN_REQUIRE_SQLITE:-}" = 1 ]; then
  echo "not ok - the Db checks need the system SQLite (sqlite3.h and -lsqlite3; on Linux: apt-get install libsqlite3-dev)" >&2
  exit 1
else
  echo "# skipped the Db checks: no system SQLite to build against (sqlite3.h and -lsqlite3; on Linux: libsqlite3-dev)"
fi

# 1. The linked shim (UndraApiLinked.cpp, what iOS builds): the core linked into the test.
echo "# linked core (the iOS shim)"
"${CXX:-clang++}" "${flags[@]}" \
  "$pkg/cpp/UndraApiLinked.cpp" "${host[@]}" ${sqlite[@]+"${sqlite[@]}"} \
  -L "$libdir" -lundra_core -Wl,-rpath,"$libdir" \
  -o "$out/host_test_linked"
"$out/host_test_linked"

# 2. The dlopen shim (UndraApiAndroid.cpp, what Android builds): the same test, the core opened at
#    run time by path, nothing linked.
echo "# dlopen'ed core (the Android shim)"
"${CXX:-clang++}" "${flags[@]}" -DUNDRA_RN_DLOPEN "-DUNDRA_RN_CORE_LIBRARY=\"$core\"" \
  "$pkg/cpp/UndraApiAndroid.cpp" "${host[@]}" ${sqlite[@]+"${sqlite[@]}"} \
  -o "$out/host_test_dlopen"
"$out/host_test_dlopen"

# 3. The JSI layer (UndraJsi.cpp) runs only on a device, but it must compile against the React
#    Native headers: checked when the playground app's dependencies are installed.
rn="$root/examples/playground/rn/node_modules/react-native/ReactCommon"
if [ -f "$rn/jsi/jsi/jsi.h" ]; then
  echo "# the JSI layer against React Native's headers"
  "${CXX:-clang++}" -std=c++20 -fsyntax-only -Wall -Wextra -Werror -I "$rn/jsi" -I "$rn/callinvoker" -I "$rn" -I "$pkg/cpp" \
    "$pkg/cpp/UndraJsi.cpp"
  echo "ok - UndraJsi.cpp compiles"
elif [ "${UNDRA_RN_REQUIRE_JSI:-}" = 1 ]; then
  echo "not ok - the JSI compile check needs React Native's headers: npm ci in examples/playground/rn first" >&2
  exit 1
else
  echo "# skipped the JSI compile check: npm install in examples/playground/rn first"
fi

# 4. The Apple platform of the default ports (ios/UndraPlatformApple.mm: the Keychain, nw_path_monitor)
#    runs only on a device, but it must compile against the iOS SDK: checked on macOS with Xcode.
if [ "$(uname -s)" = Darwin ] && xcrun --sdk iphonesimulator --show-sdk-path >/dev/null 2>&1; then
  echo "# the Apple platform against the iOS SDK"
  xcrun --sdk iphonesimulator clang++ -std=c++20 -fobjc-arc -x objective-c++ -fsyntax-only -Wall -Wextra -Werror \
    -target arm64-apple-ios17.0-simulator -I "$pkg/cpp" "$pkg/ios/UndraPlatformApple.mm"
  echo "ok - UndraPlatformApple.mm compiles"
  # The Db backend over the iOS SDK's sqlite3.h (the functions it calls exist at the pod's deployment target).
  xcrun --sdk iphonesimulator clang++ -std=c++20 -fsyntax-only -Wall -Wextra -Werror \
    -target arm64-apple-ios17.0-simulator -I "$pkg/cpp" "$pkg/cpp/UndraDbSqlite.cpp" "$pkg/cpp/UndraDb.cpp"
  echo "ok - UndraDbSqlite.cpp and UndraDb.cpp compile against the iOS SDK"
else
  echo "# skipped the Apple platform compile check: no iOS SDK on this machine"
fi
