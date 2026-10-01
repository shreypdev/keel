#!/usr/bin/env bash
# Record a benchmark baseline from a BASE commit, with THIS tree's harness, on THIS machine.
#
#   scripts/bench-record-base.sh <base-rev> <out.toml> [stress-seconds]
#
# Why: the absolute budgets are 5x the reference host, so a commit path made 1.8x slower passes
# them, and a baseline recorded on one machine means nothing on another (the GitHub runner pool
# measured the same row 1.1x to 3.3x apart across six runs). A baseline recorded from the base
# commit minutes earlier, in the same job on the same VM, has the machine's speed in it by
# construction: gate the working tree against it with
#
#   UNDRA_BENCH_BASELINE=<out.toml> cargo test -p undra-bench --test budgets --release
#   UNDRA_BENCH_BASELINE=<out.toml> cargo test -p undra-bench --test stress  --release
#
# (that is what .github/workflows/bench.yml does for a pull request and for a push to main).
#
# How: a throwaway worktree of <base-rev> gets THIS tree's bench/ directory (so harness changes
# never read as regressions: only the core crates differ), builds into a target directory of its
# own, and runs the budgets test and the stress test with UNDRA_BENCH_RECORD=<out.toml>: best of
# three attempts per row. Both tests add their own tables to the one file.
#
# The target directory must NOT be shared with the tree being measured: cargo keys a workspace
# member's artifacts without its path, so a base build in the same directory overwrites the head's
# test binaries, and the head's `cargo test` then finds them fresh and measures the base against
# itself (found the hard way: the budgets file the binary read was the deleted worktree's).
# BENCH_BASE_TARGET picks the directory (default <repo>/target/bench-base, kept between runs so
# the third-party dependencies are not rebuilt every time).
#
# Exit status: 0 and the file written, or non-zero (the base does not build against this tree's
# harness, say, or it is not a commit): the caller then gates without a baseline and says so.
set -euo pipefail

die() { echo "bench-record-base.sh: $*" >&2; exit 2; }

[ $# -ge 2 ] || die "usage: bench-record-base.sh <base-rev> <out.toml> [stress-seconds]"
BASE_REV="$1"
OUT="$(cd "$(dirname "$2")" && pwd)/$(basename "$2")"
SECONDS_PER_SCENARIO="${3:-${UNDRA_STRESS_SECONDS:-2}}"

ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"

# A shallow clone (CI's default) may not have the base commit yet.
if ! git cat-file -e "${BASE_REV}^{commit}" 2>/dev/null; then
  git fetch --no-tags --depth=1 origin "$BASE_REV" 2>/dev/null \
    || die "$BASE_REV is not a commit here and could not be fetched"
fi
BASE_SHA="$(git rev-parse "${BASE_REV}^{commit}")"
HEAD_SHA="$(git rev-parse HEAD)"
if [ "$BASE_SHA" = "$HEAD_SHA" ] && [ -z "$(git status --porcelain --untracked-files=no)" ]; then
  die "the base ($BASE_SHA) is the tree being measured: nothing to compare"
fi

TARGET="${BENCH_BASE_TARGET:-$ROOT/target/bench-base}"
case "$(cd "$(dirname "$TARGET")" 2>/dev/null && pwd)/$(basename "$TARGET")" in
  "$(cd "${CARGO_TARGET_DIR:-$ROOT/target}" 2>/dev/null && pwd)") die "BENCH_BASE_TARGET must not be the target dir of the tree under test" ;;
esac
WORK="$(mktemp -d "${TMPDIR:-/tmp}/bench-base.XXXXXX")"
cleanup() {
  git worktree remove --force "$WORK/tree" >/dev/null 2>&1 || true
  rm -rf "$WORK"
}
trap cleanup EXIT

echo "bench-record-base: base ${BASE_SHA:0:12}, harness from $(git rev-parse --short=12 HEAD), ${SECONDS_PER_SCENARIO} s per scenario"
git worktree add --detach -q "$WORK/tree" "$BASE_SHA"
# This tree's harness against the base's crates.
rm -rf "$WORK/tree/bench"
cp -R "$ROOT/bench" "$WORK/tree/bench"
rm -rf "$WORK/tree/bench/baselines" # the base never measures against a committed baseline
rm -f "$OUT"

cd "$WORK/tree"
export CARGO_TARGET_DIR="$TARGET"
export UNDRA_BENCH_RECORD="$OUT"
export UNDRA_STRESS_SECONDS="$SECONDS_PER_SCENARIO"
unset UNDRA_BENCH_BASELINE UNDRA_STRESS_JSON UNDRA_BENCH_RESULTS_DIR

# Recording must not fail because the BASE misses an absolute budget: the file is written before
# the verdict, and the head's own run is the one that gates.
cargo test -p undra-bench --test budgets --release -- --nocapture --test-threads=1 || true
cargo test -p undra-bench --test stress --release -- --nocapture --test-threads=1 || true

[ -s "$OUT" ] || die "nothing was recorded (the base did not build against this tree's bench/?)"
grep -q '^\[bench\.' "$OUT" || die "the recording has no layer A rows"
echo "bench-record-base: baseline from ${BASE_SHA:0:12} written to $OUT"
