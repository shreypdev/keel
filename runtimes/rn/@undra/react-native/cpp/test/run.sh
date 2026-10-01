#!/usr/bin/env bash
# The C++ half of @undra/react-native against real native cores (cpp/test/host_test.cpp), built
# with AddressSanitizer and UndefinedBehaviorSanitizer.
#
#   runtimes/rn/@undra/react-native/cpp/test/run.sh
#
# Needs two cores for this machine, built here when missing: the playground core
# (`undra build -C examples/playground --platform host`, namespace playground_core) and the same crate
# under a second namespace (`undra build -C examples/two-cores/a --platform host`, playground_a), so
# the module is tested with two cores in one process (ADR-044). UNDRA_CORE_DYLIB and
# UNDRA_SECOND_CORE_DYLIB point at other builds of those two (the file names must stay
# lib<namespace>.<ext>). CXX and CC pick the compilers (clang++, and clang, or CXX's clang).
#
#   1. the linked shim (UndraApiLinked.cpp, what iOS builds; macOS only, it needs the Objective-C
#      runtime): the cores linked into the test, found through their `UndraCoreTable_<namespace>`
#      classes, the CLI's `PlaygroundCoreTable.m` when `undra build --platform rn` wrote one;
#   2. the dlopen shim (UndraApiAndroid.cpp, what Android builds): the same test, every core opened
#      at run time by its namespace, nothing linked;
#   3. the JSI layer compiled against React Native's headers, skipped when the playground app's
#      dependencies are not installed; UNDRA_RN_REQUIRE_JSI=1 (CI) makes that a failure instead.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
pkg="$(cd "$here/../.." && pwd)"
root="$(cd "$pkg/../../../.." && pwd)"
ext=dylib
case "$(uname -s)" in
  Linux) ext=so ;;
esac
core="${UNDRA_CORE_DYLIB:-$root/examples/playground/build/host/libplayground_core.$ext}"
second="${UNDRA_SECOND_CORE_DYLIB:-$root/examples/two-cores/a/build/host/libplayground_a.$ext}"

build_core() { # build_core <file> <project>
  [ -f "$1" ] && return 0
  echo "==> building $(basename "$1") for this machine" >&2
  local undra="${UNDRA_CLI:-$root/target/debug/undra}"
  [ -x "$undra" ] || (cd "$root" && cargo build -p undra-cli >&2)
  "$undra" build -C "$2" --platform host >&2
}
build_core "$core" "$root/examples/playground"
build_core "$second" "$root/examples/two-cores/a"

namespace_of() { # lib<namespace>.<ext> -> <namespace>
  local name
  name="$(basename "$1")"
  name="${name#lib}"
  echo "${name%.*}"
}
cc="${CC:-${CXX:+${CXX/clang++/clang}}}"
cc="${cc:-clang}"
ns1="$(namespace_of "$core")"
ns2="$(namespace_of "$second")"

out="$(mktemp -d)"
trap 'rm -rf "$out"' EXIT
flags=(-std=c++20 -g -O1 -Wall -Wextra -Werror -fsanitize=address,undefined -fno-omit-frame-pointer
  -fno-sanitize-recover=undefined -I "$pkg/cpp" "-DUNDRA_TEST_NAMESPACE=\"$ns1\"" "-DUNDRA_TEST_SECOND_NAMESPACE=\"$ns2\"")
cflags=(-std=c11 -g -O1 -Wall -Wextra -Werror -fsanitize=address,undefined -fno-omit-frame-pointer -I "$pkg/cpp")
sources=("$pkg/cpp/UndraApi.cpp" "$pkg/cpp/UndraHost.cpp" "$here/host_test.cpp")

# The fake cores (fake_cores.c): one object for the linked test, one library that the dlopen test
# opens under each fake namespace.
"$cc" "${cflags[@]}" -c "$here/fake_cores.c" -o "$out/fake_cores.o"
"$cc" "${cflags[@]}" -shared -fPIC "$here/fake_cores.c" -o "$out/libfake_cores.$ext"

# The class a core's pod compiles (`undra build --platform rn` writes <Bundle>Table.m; this is the
# same class for a core that was only built for the host).
table_class() { # table_class <namespace>
  cat <<EOF
#import <Foundation/Foundation.h>
const void *$1_undra_api(void);
@interface UndraCoreTable_$1 : NSObject
+ (const void *)api;
@end
@implementation UndraCoreTable_$1
+ (const void *)api {
  return $1_undra_api();
}
@end
EOF
}

# 1. The linked shim (UndraApiLinked.cpp, what iOS builds): the cores linked into the test.
if [ "$(uname -s)" = Darwin ]; then
  echo "# linked cores (the iOS shim)"
  cli_table="$root/examples/playground/build/ios/PlaygroundCoreTable.m"
  if [ "$ns1" = playground_core ] && [ -f "$cli_table" ]; then
    echo "# ($ns1 through the CLI's $(basename "$cli_table"))"
    cp "$cli_table" "$out/table_$ns1.m"
  else
    table_class "$ns1" >"$out/table_$ns1.m"
  fi
  table_class "$ns2" >"$out/table_$ns2.m"
  objc=(-g -O1 -Wall -Werror -fsanitize=address -fno-omit-frame-pointer)
  for m in "$out/table_$ns1.m" "$out/table_$ns2.m" "$here/fake_classes.m"; do
    "$cc" "${objc[@]}" -c "$m" -o "$out/$(basename "$m" .m).o"
  done
  "${CXX:-clang++}" "${flags[@]}" \
    "$pkg/cpp/UndraApiLinked.cpp" "${sources[@]}" \
    "$out/table_$ns1.o" "$out/table_$ns2.o" "$out/fake_classes.o" "$out/fake_cores.o" \
    -L "$(dirname "$core")" -l"$ns1" -Wl,-rpath,"$(dirname "$core")" \
    -L "$(dirname "$second")" -l"$ns2" -Wl,-rpath,"$(dirname "$second")" \
    -framework Foundation -lobjc \
    -o "$out/host_test_linked"
  "$out/host_test_linked"
else
  echo "# skipped the linked shim (what iOS builds): it finds cores through the Objective-C runtime (macOS)"
fi

# 2. The dlopen shim (UndraApiAndroid.cpp, what Android builds): every core opened at run time from
#    one directory by its namespace, nothing linked.
echo "# dlopen'ed cores (the Android shim)"
libs="$out/lib"
mkdir -p "$libs"
ln -s "$core" "$libs/lib$ns1.$ext"
ln -s "$second" "$libs/lib$ns2.$ext"
for fake in bad_abi short_table wrong_ns null_entry not_a_core; do
  ln -s "$out/libfake_cores.$ext" "$libs/lib$fake.$ext"
done
"${CXX:-clang++}" "${flags[@]}" -DUNDRA_RN_DLOPEN "-DUNDRA_RN_LIBRARY_DIR=\"$libs/\"" "-DUNDRA_RN_LIBRARY_SUFFIX=\".$ext\"" \
  "$pkg/cpp/UndraApiAndroid.cpp" "${sources[@]}" -ldl \
  -o "$out/host_test_dlopen"
"$out/host_test_dlopen"

# 3. The JSI layer (UndraJsi.cpp, UndraTurboModule.cpp) runs only on a device, but it must compile
#    against the React Native headers: checked when the playground app's dependencies are installed.
rn="$root/examples/playground/rn/node_modules/react-native/ReactCommon"
if [ -f "$rn/jsi/jsi/jsi.h" ]; then
  echo "# the JSI layer against React Native's headers"
  for file in UndraJsi.cpp UndraTurboModule.cpp; do
    "${CXX:-clang++}" -std=c++20 -fsyntax-only -Wall -Wextra -Werror -I "$rn/jsi" -I "$rn/callinvoker" -I "$rn" \
      -I "$rn/react/nativemodule/core" -I "$pkg/cpp" "$pkg/cpp/$file"
    echo "ok - $file compiles"
  done
elif [ "${UNDRA_RN_REQUIRE_JSI:-}" = 1 ]; then
  echo "not ok - the JSI compile check needs React Native's headers: npm ci in examples/playground/rn first" >&2
  exit 1
else
  echo "# skipped the JSI compile check: npm install in examples/playground/rn first"
fi
