#!/usr/bin/env bash
# The pure Java of @undra/react-native's Android library on the JVM (android/test/PureTest.java): the
# secure-store seal against an independent AES-GCM vector and the network classification. Needs a JDK 17+
# (`javac`, `java`); no Android SDK, no Gradle.
#
#   runtimes/rn/@undra/react-native/android/test/run.sh
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
src="$here/../src/main/java/dev/undra/reactnative"
out="$(mktemp -d)"
trap 'rm -rf "$out"' EXIT
javac -Werror -Xlint:all -d "$out" "$src/SecureSeal.java" "$src/NetworkClassifier.java" "$here/dev/undra/reactnative/PureTest.java"
java -cp "$out" dev.undra.reactnative.PureTest
