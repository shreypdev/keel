#!/usr/bin/env bash
# Reads a contract runner's output (stdin or a file) and fails unless all seventeen scenarios of
# scenarios.md reported PASS. A line looks like:  SCENARIO S07 PASS stream with backpressure
#
#   contract-tests/ts/run.sh 2>&1 | tee /tmp/ts.log | contract-tests/check.sh ts
#   contract-tests/check.sh kotlin < kotlin.log
#
# Prints the platform's column of the 17-scenario grid and exits 1 if any scenario is missing,
# failed or skipped (a skip is listed, so it can be told apart from a failure).
set -euo pipefail
platform="${1:-runner}"
IDS=(S01 S02 S03 S04 S05 S06 S07 S08 S09 S10 S11 S12 S13 S14 S15 S16 S17)
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
if [ "$bad" = 0 ]; then echo "$platform: all 17 scenarios pass"; else echo "$platform: NOT all scenarios pass" >&2; fi
exit "$bad"
