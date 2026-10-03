#!/usr/bin/env bash
# Publishes the Kotlin runtime's modules into a local Maven repository, as JitPack builds a release (ADR-063).
#
# jitpack.yml at the repository root runs this when someone first asks JitPack for a tag
# (com.github.shreypdev.undra:<module>:v1.0.0); JitPack then serves what it finds in ~/.m2/repository.
# packaging/rehearse-launch.sh runs it too, into a scratch repository, to rehearse a release offline.
#
# Environment (JitPack sets the first three: https://docs.jitpack.io/building/):
#   GROUP             the GitHub owner's group, default com.github.shreypdev
#   ARTIFACT          the repository's name, default undra; the modules' group is $GROUP.$ARTIFACT
#   VERSION           the tag being built (v1.0.0): the modules' version. Required.
#   MAVEN_REPO_LOCAL  where to publish, default ~/.m2/repository (Maven's own default, which JitPack reads)
#
# The Android modules are built only where the Android SDK is (settings.gradle.kts); JitPack sets ANDROID_HOME. A
# module that is missing afterwards fails the build here, by name, rather than as an unresolved dependency in an app.
set -euo pipefail

: "${VERSION:?VERSION is not set: the tag to publish, for example v1.0.0 (JitPack sets it)}"
group="${GROUP:-com.github.shreypdev}.${ARTIFACT:-undra}"
repo_local="${MAVEN_REPO_LOCAL:-$HOME/.m2/repository}"
build="$(cd "$(dirname "$0")/.." && pwd)"

echo "Publishing the Undra Kotlin runtime as $group:<module>:$VERSION into $repo_local"
(
  cd "$build"
  ./gradlew --no-daemon --console=plain \
    -PundraGroup="$group" -PundraVersion="$VERSION" -Dmaven.repo.local="$repo_local" \
    publishToMavenLocal
)

# Every module an app can depend on (crates/undra-cli/src/dist.rs, MAVEN_MODULES).
missing=()
for module in runtime testkit android-adapters android-work undra-compose okhttp-adapters; do
  pom="$repo_local/${group//.//}/$module/$VERSION/$module-$VERSION.pom"
  [ -f "$pom" ] || missing+=("$module")
done
if [ ${#missing[@]} -gt 0 ]; then
  echo "error: not published: ${missing[*]}. The Android modules need an Android SDK (ANDROID_HOME, ANDROID_SDK_ROOT or" >&2
  echo "       sdk.dir in local.properties); without them an app's android-adapters dependency cannot resolve." >&2
  exit 1
fi
echo "Published: runtime testkit android-adapters android-work undra-compose okhttp-adapters ($group, $VERSION)"
