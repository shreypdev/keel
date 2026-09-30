#!/usr/bin/env bash
# Runs the contract scenarios on every platform and prints the 17-scenario grid.
#
#   contract-tests/run-all.sh                 # ts, kotlin and swift (swift only on macOS)
#   contract-tests/run-all.sh ts kotlin       # a subset
#
# Each platform's run.sh builds what it needs with the keel CLI (the playground core as wasm, as a
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

status=0
for p in "${platforms[@]}"; do
  echo "==> contract-tests/$p"
  "$HERE/$p/run.sh" >"$LOGS/$p.log" 2>&1 || status=1
  "$HERE/check.sh" "$p" <"$LOGS/$p.log" >"$LOGS/$p.grade" 2>&1 || status=1
done

echo
printf '%-5s' ""; for p in "${platforms[@]}"; do printf '%-9s' "$p"; done; echo
for n in $(seq -w 1 17); do
  id="S$n"
  printf '%-5s' "$id"
  for p in "${platforms[@]}"; do
    cell="$(grep -E " $id " "$LOGS/$p.grade" | awk '{print $3}')"
    printf '%-9s' "${cell:-MISSING}"
  done
  echo
done
exit $status
