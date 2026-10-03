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

# pr-verdict: the "All green" check of each required workflow on a pull request's head (gh pr checks --json).
expect "pr: every All green passed" 0 "ok Site" "$CHECK" pr-verdict "$DATA/pr-all-green.json" "${required[@]}" Site
expect "pr: a job's own check is not the roll-up" 1 "MISSING" "$CHECK" pr-verdict "$DATA/pr-one-red-one-running.json" "${required[@]}" Site
expect "pr: a failed roll-up is RED" 1 "RED CI is FAILURE" "$CHECK" pr-verdict "$DATA/pr-one-red-one-running.json" "${required[@]}"
expect "pr: a pending roll-up is RUNNING" 1 "RUNNING Bench is IN_PROGRESS" "$CHECK" pr-verdict "$DATA/pr-one-red-one-running.json" "${required[@]}"
expect "pr: no checks at all: MISSING" 1 "MISSING no \"All green\" check of CI" "$CHECK" pr-verdict "$DATA/pr-none.json" "${required[@]}"

# The single gate: one workflow whose last job, "All green", needs every called workflow. A called workflow's own
# roll-up ("Site / Complete") passing early is not the gate.
expect "gate: All green passed" 0 "ok Gate" "$CHECK" pr-verdict "$DATA/pr-gate-green.json" Gate
expect "gate: a called workflow finished first, the gate has not reported: MISSING" 1 "MISSING" "$CHECK" pr-verdict "$DATA/pr-gate-running.json" Gate
expect "gate: a failed called workflow fails it" 1 "RED Gate is FAILURE" "$CHECK" pr-verdict "$DATA/pr-gate-red.json" Gate

site_yml="$HERE/../.github/workflows/site.yml"
expect "a change under site/ needs the Site workflow" 0 "yes" bash -c "printf 'docs/x.md\nsite/index.html\n' | '$CHECK' site-needed '$site_yml'"
expect "a change to the scenarios needs the Site workflow (the cards read them)" 0 "yes" bash -c "printf 'contract-tests/scenarios.md\n' | '$CHECK' site-needed '$site_yml'"
expect "state files alone do not" 0 "no" bash -c "printf '.10x/status.md\n.10x/decisions/sde/x.md\nscripts/wt.sh\ndocs/AGENT_WORKFLOW.md\n' | '$CHECK' site-needed '$site_yml'"

[ "$fails" = 0 ] && echo "wt-ci-check: all passed" || { echo "wt-ci-check: $fails failed"; exit 1; }
