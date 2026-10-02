#!/usr/bin/env bash
# Compiles the Swift, Kotlin and TypeScript lines the cookbook pages show, against the bindings
# `undra bindgen` generates from examples/cookbook (../generated) and the platform runtimes in this
# checkout. The pages quote the regions between `docs:begin` / `docs:end` markers in these files
# (site/scripts/build-cookbook.mjs), so what the pages say is what compiled.
#
#   bash examples/cookbook/snippets/check.sh [swift|kotlin|ts ...]
#
# Needs the toolchains of docs/ONBOARDING.md (`source scripts/env.sh`): Xcode's `swift` (Swift 6),
# `kotlinc` (or a Gradle distribution; see scripts/kotlinc.sh) with the kotlinx-coroutines jar in
# UNDRA_KOTLINX_COROUTINES, and `tsc` from `runtimes/ts/@undra/runtime` (`npm ci` there).
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
want=("$@"); [ ${#want[@]} -gt 0 ] || want=(swift kotlin ts)
scratch="$(mktemp -d)"; trap 'rm -rf "$scratch"' EXIT

for target in "${want[@]}"; do
  case "$target" in
    swift)
      echo "== swift build (Swift 6 language mode)"
      (cd "$here/swiftui" && swift build --scratch-path "$scratch/swift")
      ;;
    kotlin)
      echo "== kotlinc -Werror"
      : "${UNDRA_KOTLINX_COROUTINES:?set UNDRA_KOTLINX_COROUTINES to the kotlinx-coroutines-core jar (source scripts/env.sh)}"
      "$root/scripts/kotlinc.sh" -cp "$UNDRA_KOTLINX_COROUTINES" -d "$scratch/kotlin" -jvm-target 11 -Werror \
        "$root/runtimes/kotlin/undra-runtime/runtime/src/main/kotlin" \
        "$here/../generated/kotlin/src/main/kotlin" \
        "$here/kotlin"
      ;;
    ts)
      echo "== tsc --strict"
      tsc="$root/runtimes/ts/@undra/runtime/node_modules/.bin/tsc"
      (cd "$here/ts" && "$tsc" -p tsconfig.json)
      ;;
    *) echo "unknown target $target (swift, kotlin, ts)" >&2; exit 2 ;;
  esac
done
echo "check.sh: ok (${want[*]})"
