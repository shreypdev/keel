#!/usr/bin/env bash
# Run this before pushing a branch: every step of the CI, Bench, Two cores and Site workflows, here, from a clone of
# what you committed. A branch that passes it has used up what this machine can tell you; what is left for the hosted
# run is named at the end of the output (Linux behaviour, macOS 15's own frameworks, emulators).
#
#   scripts/ci-local.sh                      all jobs, in a clone of HEAD (committed files only: CI sees nothing else)
#   scripts/ci-local.sh --slow               the slow-runner pass: the timing-sensitive test suites, throttled
#                                            (taskpolicy -b, 16 CPU burners, four test threads), three rounds
#   scripts/ci-local.sh --only ci/ts,ci/rust    some jobs (a job is <workflow>/<id>, or just <id>)
#   scripts/ci-local.sh --skip bench/budgets    all but some
#   scripts/ci-local.sh --list               what would run, and what is skipped and why; runs nothing
#   scripts/ci-local.sh --cold               start from a new clone (a cold target and node_modules)
#   scripts/ci-local.sh --here               run in this checkout instead of a clone (quick; dirty trees allowed;
#                                            not what CI sees: a clean tree is what makes it a proof)
#   scripts/ci-local.sh -v                   stream every step's output (the default prints it on failure, and keeps logs)
#
# The steps are read from .github/workflows/*.yml by scripts/ci-local.rb, so they cannot drift from CI. Provisioning that
# only a hosted runner needs (apt, sudo, SDK installs) is skipped and listed; the pinned Rust, Node 24 and JDK 17 are
# checked, never installed. The clone lives in $UNDRA_CI_LOCAL_DIR (default: $TMPDIR/undra-ci-local/<branch>) and is
# reused (incremental, like a cache-restored run) until --cold. The machine needs ruby (macOS has it), `source scripts/env.sh`
# is done for you, and nothing here touches the branch or the network beyond what the workflow steps do.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cold=0
in_place=0
pass=()
while [ $# -gt 0 ]; do
  case "$1" in
    --cold) cold=1 ;;
    --here) in_place=1 ;;
    -h|--help) sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed '$d' | sed 's/^# \{0,1\}//'; exit 0 ;;
    --only|--skip|--workflows|--rounds|--burners|--logs|--step-timeout) pass+=("$1" "${2:?$1 needs a value}"); shift ;;
    --slow|--list|-v|--verbose) pass+=("$1") ;;
    *) echo "ci-local.sh: unknown option $1 (see --help)" >&2; exit 2 ;;
  esac
  shift
done

export LC_ALL="${LC_ALL:-en_US.UTF-8}" LANG="${LANG:-en_US.UTF-8}"
# shellcheck disable=SC1091
source "$here/scripts/env.sh"
command -v ruby >/dev/null || { echo "ci-local.sh: ruby is needed to read the workflow files (macOS ships it)" >&2; exit 2; }

cd "$here"
branch="$(git branch --show-current)"
[ -n "$branch" ] || branch="detached-$(git rev-parse --short HEAD)"
sha="$(git rev-parse HEAD)"

if [ "$in_place" = 1 ]; then
  root="$here"
  echo "ci-local: running in $root (not a clone: uncommitted changes are included, so this is not what CI will see)" >&2
else
  if [ -n "$(git status --porcelain)" ]; then
    echo "ci-local: the tree has uncommitted changes. CI sees only what is committed: commit them, or use --here for a quick run." >&2
    exit 2
  fi
  base="${UNDRA_CI_LOCAL_DIR:-${TMPDIR:-/tmp}/undra-ci-local}"
  root="$base/$(printf '%s' "$branch" | tr '/ ' '--')"
  [ "$cold" = 1 ] && rm -rf "$root"
  mkdir -p "$base"
  if [ ! -d "$root/.git" ]; then
    git clone -q --no-checkout "$here" "$root"
  fi
  # `origin` of the clone is this checkout: the Bench job fetches `main` from it to find the base it compares with.
  git -C "$root" fetch -q --no-tags "$here" "+refs/heads/main:refs/remotes/origin/main" 2>/dev/null || true
  git -C "$root" fetch -q --no-tags "$here" "$sha"
  git -C "$root" checkout -qf --detach "$sha"
  git -C "$root" clean -fdq
  echo "ci-local: clone $root at ${sha:0:12} ($branch)" >&2
fi

exec ruby "$here/scripts/ci-local.rb" --root "$root" --branch "$branch" ${pass[@]+"${pass[@]}"}
