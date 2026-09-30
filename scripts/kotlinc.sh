#!/usr/bin/env bash
# Kotlin/JVM compiler shim. Uses `kotlinc` when on PATH; otherwise the compiler embedded in a Gradle
# distribution (GRADLE_HOME or /opt/gradle). Any -cp/-classpath argument is merged with the stdlib.
set -euo pipefail
if command -v kotlinc >/dev/null 2>&1; then
  exec kotlinc "$@"
fi
GRADLE_LIB="${GRADLE_HOME:-/opt/gradle}/lib"
STDLIB="$GRADLE_LIB/kotlin-stdlib-2.0.21.jar"
COMPILER_CP="$GRADLE_LIB/kotlin-compiler-embeddable-2.0.21.jar:$STDLIB:$GRADLE_LIB/kotlin-reflect-2.0.21.jar:$GRADLE_LIB/kotlin-script-runtime-2.0.21.jar:$GRADLE_LIB/kotlin-daemon-embeddable-2.0.21.jar:$GRADLE_LIB/trove4j-1.0.20200330.jar:$GRADLE_LIB/annotations-24.0.1.jar:$GRADLE_LIB/kotlinx-coroutines-core-jvm-1.6.4.jar"
USER_CP=""
ARGS=()
while [ $# -gt 0 ]; do
  case "$1" in
    -cp|-classpath) USER_CP="$2"; shift 2 ;;
    *) ARGS+=("$1"); shift ;;
  esac
done
CP="$STDLIB"; [ -n "$USER_CP" ] && CP="$STDLIB:$USER_CP"
exec java -cp "$COMPILER_CP" org.jetbrains.kotlin.cli.jvm.K2JVMCompiler -no-stdlib -no-reflect -cp "$CP" "${ARGS[@]}"
