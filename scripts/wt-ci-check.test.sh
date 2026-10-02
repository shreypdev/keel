#!/usr/bin/env bash
# The parsing of scripts/wt-ci-check.sh against fixtures of `gh run list --json` (scripts/testdata/wt-ci/).
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CHECK="$HERE/wt-ci-check.sh"
DATA="$HERE/testdata/wt-ci"
fails=0
expect() { # <name> <expected exit> <expected output substring> <command...>
  local name="$1" want="$2" needle="$3"; shift 3
  local out rc
  out="$("$@" 2>&1)"; rc=$?
  if [ "$rc" != "$want" ] || ! grep -qF -- "$needle" <<<"$out"; then
    echo "FAIL $name: exit $rc (wanted $want), output:"; echo "$out" | sed 's/^/    /'; fails=$((fails + 1))
  else
    echo "ok   $name"
  fi
}

required=(CI Bench "Two cores")
expect "all required green, a red workflow nobody requires is ignored" 0 "ok Two cores" "$CHECK" verdict "$DATA/all-green.json" "${required[@]}" Site
expect "a failure is RED and says which run" 1 "RED CI ended failure (run 201)" "$CHECK" verdict "$DATA/one-red-one-running.json" "${required[@]}"
expect "a run in progress is RUNNING, not green" 1 "RUNNING Bench is in_progress (run 202)" "$CHECK" verdict "$DATA/one-red-one-running.json" "${required[@]}"
expect "a workflow with no run is MISSING" 1 "MISSING no run of Site on this commit" "$CHECK" verdict "$DATA/one-red-one-running.json" "${required[@]}" Site
expect "nothing at all: every required workflow is MISSING" 1 "MISSING no run of CI" "$CHECK" verdict "$DATA/none.json" "${required[@]}"
expect "a re-run supersedes the failure it repeats, and an older cancelled run is not the verdict" 0 "ok CI (run 305)" "$CHECK" verdict "$DATA/rerun-supersedes.json" "${required[@]}"

site_yml="$HERE/../.github/workflows/site.yml"
expect "a change under site/ needs the Site workflow" 0 "yes" bash -c "printf 'docs/x.md\nsite/index.html\n' | '$CHECK' site-needed '$site_yml'"
expect "a change to the scenarios needs the Site workflow (the cards read them)" 0 "yes" bash -c "printf 'contract-tests/scenarios.md\n' | '$CHECK' site-needed '$site_yml'"
expect "state files alone do not" 0 "no" bash -c "printf '.10x/status.md\n.10x/decisions/sde/x.md\nscripts/wt.sh\ndocs/AGENT_WORKFLOW.md\n' | '$CHECK' site-needed '$site_yml'"

[ "$fails" = 0 ] && echo "wt-ci-check: all passed" || { echo "wt-ci-check: $fails failed"; exit 1; }
