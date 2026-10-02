#!/usr/bin/env bash
# Reads a contract runner's output (stdin or a file) and fails unless every scenario of
# scenarios.md the platform runs reported PASS: S01 to S20 and S26 everywhere, plus S21 and S22 (worker
# mode and crash recovery, web-only, ADR-049) on ts. A line looks like:  SCENARIO S07 PASS stream with backpressure
#
#   contract-tests/ts/run.sh 2>&1 | tee /tmp/ts.log | contract-tests/check.sh ts
#   contract-tests/check.sh kotlin < kotlin.log
#
# Prints the platform's column of the scenario grid and exits 1 if any scenario is missing,
# failed or skipped (a skip is listed, so it can be told apart from a failure). The last line of an
# id counts: a second process (build B of S14/S15) prints only FAIL lines, which override a PASS.
set -euo pipefail
platform="${1:-runner}"
# S20 to S22 are ADR-049's (S21 and S22 web-only); S23 to S25 are held by other ADRs (the boundary-surface plan);
# S26 is ADR-044's two cores.
IDS=(S01 S02 S03 S04 S05 S06 S07 S08 S09 S10 S11 S12 S13 S14 S15 S16 S17 S18 S19 S20 S26)
# Web-only scenarios (ADR-049): the TypeScript runner runs them.
case "$platform" in
  ts) IDS+=(S21 S22) ;;
esac
log="$(cat "${2:-/dev/stdin}")"
bad=0
for id in "${IDS[@]}"; do
  line="$(printf '%s\n' "$log" | grep -E "^SCENARIO $id (PASS|FAIL|SKIP)( |$)" | tail -n 1 || true)"
  case "$line" in
    "SCENARIO $id PASS"*) printf '%s %s pass\n' "$platform" "$id" ;;
    "SCENARIO $id SKIP"*) printf '%s %s SKIP: %s\n' "$platform" "$id" "${line#SCENARIO $id SKIP }"; bad=1 ;;
    "SCENARIO $id FAIL"*) printf '%s %s FAIL: %s\n' "$platform" "$id" "${line#SCENARIO $id FAIL }"; bad=1 ;;
    *) printf '%s %s MISSING\n' "$platform" "$id"; bad=1 ;;
  esac
done
if [ "$bad" = 0 ]; then echo "$platform: all ${#IDS[@]} scenarios pass"; else echo "$platform: NOT all scenarios pass" >&2; fi
exit "$bad"
