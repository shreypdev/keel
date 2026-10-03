#!/usr/bin/env bash
# Sets the release version everywhere it is written down, or checks that it is.
#
#   scripts/bump-version.sh 1.0.0           # edit the files, refresh Cargo.lock, list what changed
#   scripts/bump-version.sh --check 1.0.0   # change nothing; fail and list what does not say 1.0.0
#   scripts/bump-version.sh --check         # the same, against the workspace version in Cargo.toml
#
# The release workflow runs the --check form before building anything, so a tag that disagrees
# with the code, or a version file that was missed, stops the release at the first step.
# scripts/bump-version.test.sh runs it on a copy of the repository with a throwaway version.
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: scripts/bump-version.sh <semver>
       scripts/bump-version.sh --check [<semver>]
       scripts/bump-version.sh -h | --help

Sets the version of the release (for example 1.0.0, or 1.0.0-rc.1) in:

  Cargo.toml                                       [workspace.package] version, and the version of
                                                   each in-workspace dependency (crates inherit it);
                                                   the CLI writes it into every project it makes
  Cargo.lock                                       refreshed with `cargo update --workspace --offline`
  runtimes/ts/@undra/runtime/package.json          @undra/runtime, a release asset (ADR-063)
  runtimes/ts/@undra/testkit/package.json          @undra/testkit, a release asset
  runtimes/rn/@undra/react-native/package.json     @undra/react-native, a release asset
  their package-lock.json                          the two version fields of each
  "@undra/runtime": "^<version>"                   in every package.json and package-lock.json of the
                                                   repository: the peer range of the testkit and the
                                                   React Native host, and of every generated package
  runtimes/ts/@undra/runtime/src/version.ts        RUNTIME_VERSION, and the Kotlin runtime's
  runtimes/kotlin/.../dev/undra/runtime/UndraLog.kt  UNDRA_RUNTIME_VERSION: what each sends in its Hello

and prints every file it changed. Nothing else carries the release's number: the Swift package's
version is the tag, the Kotlin artifacts' version is the tag (JitPack's VERSION), the projects
`undra init` writes take it from the CLI. A version that is not MAJOR.MINOR.PATCH (with an
optional -prerelease) is refused before anything is written.

--check writes nothing: it exits 0 when every file already says <semver> (default: the workspace
version in Cargo.toml) and 1 listing the files that do not.
USAGE
}

die() {
  printf 'bump-version.sh: %s\n' "$*" >&2
  exit 2
}

check=0
version=""
while [ $# -gt 0 ]; do
  case "$1" in
    -h | --help) usage; exit 0 ;;
    --check) check=1; shift ;;
    -*) usage >&2; die "unknown option: $1" ;;
    *)
      [ -z "$version" ] || { usage >&2; die "only one version can be given"; }
      version=$1
      shift
      ;;
  esac
done

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

workspace_version() {
  awk '/^\[/ { section = $0 } section == "[workspace.package]" && /^version = "/ { gsub(/^version = "|"$/, ""); print; exit }' Cargo.toml
}

if [ -z "$version" ]; then
  [ "$check" = 1 ] || { usage >&2; die "a version is required"; }
  version=$(workspace_version)
  [ -n "$version" ] || die "no [workspace.package] version in Cargo.toml"
fi

# MAJOR.MINOR.PATCH without leading zeros, then optionally -prerelease identifiers. Build
# metadata (+...) is refused: it cannot appear in the file names and URLs the release uses.
semver='^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?$'
printf '%s\n' "$version" | grep -Eq "$semver" || die "not a semantic version (MAJOR.MINOR.PATCH, optionally -prerelease): $version"

# --- one rewrite per kind of file: reads <file>, writes the file with the new version ---------------
rewrite_cargo() { # Cargo.toml
  awk -v v="$version" '
    /^\[/ { section = $0 }
    section == "[workspace.package]" && /^version = "/ { sub(/"[^"]*"/, "\"" v "\"") }
    section == "[workspace.dependencies]" && /^undra[A-Za-z0-9_-]* = \{.*path = .*version = "/ { sub(/version = "[^"]*"/, "version = \"" v "\"") }
    { print }
  ' "$1"
}
rewrite_package_json() { # the first top-level "version" of a package.json
  awk -v v="$version" '
    !done && /^  "version": "/ { sub(/"version": "[^"]*"/, "\"version\": \"" v "\""); done = 1 }
    { print }
  ' "$1"
}
rewrite_package_lock() { # the top-level version and the root package'"'"'s ("packages": { "": { ... } })
  awk -v v="$version" '
    !top && /^  "version": "/ { sub(/"version": "[^"]*"/, "\"version\": \"" v "\""); top = 1 }
    /^    "": \{/ { root = 1 }
    root && /^      "version": "/ { sub(/"version": "[^"]*"/, "\"version\": \"" v "\""); root = 0 }
    { print }
  ' "$1"
}
rewrite_runtime_range() { # every `"@undra/runtime": "^<semver>"`: the peer and dev ranges that name the runtime
  sed -E 's/("@undra\/runtime": "\^)[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?"/\1'"$version"'"/g' "$1"
}
rewrite_hello_version() { # the version a runtime reports in its Hello: TypeScript's RUNTIME_VERSION, Kotlin's UNDRA_RUNTIME_VERSION
  sed -E 's/^(export const RUNTIME_VERSION = ")[^"]*(";)$/\1'"$version"'\2/; s/^(internal const val UNDRA_RUNTIME_VERSION: String = ")[^"]*(")$/\1'"$version"'\2/' "$1"
}

# The npm packages of a release (crates/undra-cli/src/dist.rs, NPM_PACKAGES).
packages="runtimes/ts/@undra/runtime runtimes/ts/@undra/testkit runtimes/rn/@undra/react-native"

# Every package manifest and lock file of the repository (git's list when this is a checkout; the
# directories' otherwise), never a dependency's, and not the test fixtures, which are projects of
# older releases on purpose (`undra upgrade`'s).
manifests() {
  if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    git ls-files -- '*package.json' '*package-lock.json'
  else
    find . \( -name node_modules -o -name target -o -name .git \) -prune -o \
      \( -name package.json -o -name package-lock.json \) -type f -print | sed 's|^\./||' | sort
  fi | grep -v '/tests/fixtures/' || true
}

files_and_kinds() {
  printf 'cargo Cargo.toml\n'
  printf 'hello_version runtimes/ts/@undra/runtime/src/version.ts\n'
  printf 'hello_version runtimes/kotlin/undra-runtime/runtime/src/main/kotlin/dev/undra/runtime/UndraLog.kt\n'
  local dir
  for dir in $packages; do
    printf 'package_json %s/package.json\n' "$dir"
    [ ! -f "$dir/package-lock.json" ] || printf 'package_lock %s/package-lock.json\n' "$dir"
  done
  local file
  while read -r file; do
    [ -f "$file" ] || continue
    grep -q '"@undra/runtime": "\^' "$file" && printf 'runtime_range %s\n' "$file"
  done < <(manifests)
  return 0
}

changed=()
while read -r kind file; do
  [ -f "$file" ] || die "missing $file"
  new=$("rewrite_$kind" "$file"; printf x)   # the x keeps the trailing newline through $(...)
  old=$(cat "$file"; printf x)
  if [ "$new" != "$old" ]; then
    changed+=("$file")
    if [ "$check" = 0 ]; then
      printf '%s' "${new%x}" >"$file"
    fi
  fi
done < <(files_and_kinds)

# A file can need more than one rewrite (a package.json with its version and a peer range): list it once.
# (No mapfile: macOS still ships bash 3.2.)
if [ ${#changed[@]} -gt 0 ]; then
  unique=()
  for file in "${changed[@]}"; do
    case " ${unique[*]-} " in *" $file "*) ;; *) unique+=("$file") ;; esac
  done
  changed=("${unique[@]}")
fi

if [ "$check" = 1 ]; then
  if [ ${#changed[@]} -gt 0 ]; then
    printf 'bump-version.sh: these files do not say %s:\n' "$version" >&2
    printf '  %s\n' "${changed[@]}" >&2
    exit 1
  fi
  printf 'every version file says %s\n' "$version"
  exit 0
fi

# Cargo.lock records the version of every workspace crate; `cargo build --locked` (the release)
# fails when it is stale. Updating only the workspace members needs no network.
if [ ${#changed[@]} -gt 0 ] && [ -f Cargo.lock ]; then
  if command -v cargo >/dev/null 2>&1; then
    before=$(cat Cargo.lock)
    cargo update --workspace --offline >/dev/null 2>&1 || die "cargo update --workspace --offline failed; run it by hand and re-run this script"
    [ "$before" = "$(cat Cargo.lock)" ] || changed+=("Cargo.lock")
  else
    printf 'bump-version.sh: cargo is not on PATH; run `cargo update --workspace` before committing, or the release build (--locked) will fail\n' >&2
  fi
fi

if [ ${#changed[@]} -eq 0 ]; then
  printf 'nothing to change: every version file already says %s\n' "$version"
else
  printf 'set the version to %s in:\n' "$version"
  printf '  %s\n' "${changed[@]}"
  printf 'Review `git diff`, commit, open the pull request; tag v%s after it merges (docs/RELEASING.md).\n' "$version"
fi
