#!/usr/bin/env bash
# Runs the contract scenarios on every platform and prints the scenario grid (scenarios.md: the
# ids check.sh expects of each platform; a cell no platform's check expects reads n/a).
#
#   contract-tests/run-all.sh                 # ts, kotlin and swift (swift only on macOS)
#   contract-tests/run-all.sh ts kotlin       # a subset
#
# Each platform's run.sh builds what it needs with the undra CLI (the playground core as wasm, as a
# host library, ...) and prints `SCENARIO Sxx PASS|FAIL|SKIP <title>` lines; check.sh grades them.
# The logs are kept in contract-tests/.logs/ (not committed).
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LOGS="$HERE/.logs"
mkdir -p "$LOGS"

platforms=("$@")
if [ "${#platforms[@]}" = 0 ]; then
  platforms=(ts kotlin)
  [ "$(uname -s)" = "Darwin" ] && platforms+=(swift)
fi

# Indexed arrays, not associative: macOS ships bash 3.2.
status=0
run_rc=(); grade_rc=()
for p in "${platforms[@]}"; do
  echo "==> contract-tests/$p"
  rc=0; "$HERE/$p/run.sh" >"$LOGS/$p.log" 2>&1 || rc=$?; run_rc+=("$rc")
  rc=0; "$HERE/check.sh" "$p" <"$LOGS/$p.log" >"$LOGS/$p.grade" 2>&1 || rc=$?; grade_rc+=("$rc")
  [ "${run_rc[${#run_rc[@]}-1]}" = 0 ] && [ "${grade_rc[${#grade_rc[@]}-1]}" = 0 ] || status=1
done

echo
printf '%-5s' ""; for p in "${platforms[@]}"; do printf '%-9s' "$p"; done; echo
for id in $(seq -f 'S%02g' 1 25) S26; do
  printf '%-5s' "$id"
  for p in "${platforms[@]}"; do
    cell="$(grep -E " $id " "$LOGS/$p.grade" | awk '{print $3}')"
    # check.sh prints a line (pass, FAIL, SKIP or MISSING) for every id it expects of the platform.
    if [ -z "$cell" ]; then cell="n/a"; fi
    printf '%-9s' "$cell"
  done
  echo
done
# On failure, say which runner or grade failed and show the reasons and the log tail, so a CI log
# is enough to diagnose it (a runner can exit non-zero with every scenario passing: a failing
# non-scenario test, a build step, a cleanup).
if [ "$status" != 0 ]; then
  i=0
  for p in "${platforms[@]}"; do
    r="${run_rc[$i]}"; g="${grade_rc[$i]}"; i=$((i + 1))
    if [ "$r" != 0 ] || [ "$g" != 0 ]; then
      echo; echo "==> $p: run.sh exited $r, check.sh exited $g"
      grep -E " (FAIL|SKIP|MISSING)" "$LOGS/$p.grade" || true
      echo "==> $p: last 60 lines of $LOGS/$p.log"; tail -n 60 "$LOGS/$p.log"
    fi
  done
fi
exit $status
