#!/usr/bin/env bash
# The check behind `scripts/wt.sh merge`: is CI green on a branch's exact head? (docs/AGENT_WORKFLOW.md)
#
#   wt-ci-check.sh verdict <runs.json> <workflow>...
#       <runs.json> is `gh run list --branch wt/<slug> --commit <head sha> --json workflowName,status,conclusion,createdAt,databaseId`.
#       For each required <workflow> (its `name:`), the newest run of it (a re-run supersedes the run it repeats) must be
#       completed with conclusion success. Prints one line per workflow (`ok`, `RED`, `RUNNING`, `MISSING`) and exits 0
#       only when every one is ok.
#   wt-ci-check.sh pr-verdict <checks.json> <workflow>...
#       <checks.json> is `gh pr checks <n> --json name,workflow,state,bucket`: the "All green" check of each required
#       workflow must have passed on the pull request's head. Same lines and exit code as verdict.
#   wt-ci-check.sh site-needed <site.yml>
#       Reads the changed files (one per line) from stdin and prints `yes` when one of them matches a `paths:` entry of
#       the Site workflow's `push` trigger, else `no`: Site runs only for those, so it is required only for them.
#
# The parsing is separate from `gh` so that a test can feed it a fixture (scripts/wt-ci-check.test.sh).
set -euo pipefail

die() { echo "wt-ci-check.sh: $*" >&2; exit 2; }
command -v jq >/dev/null || die "jq is needed (it ships with macOS and every GitHub runner)"

verdict() {
  local runs="$1"; shift
  [ -f "$runs" ] || die "no such file: $runs"
  local bad=0 name line
  for name in "$@"; do
    line="$(jq -r --arg name "$name" '
      [ .[] | select(.workflowName == $name) ] | sort_by(.createdAt, .databaseId) | last
      | if . == null then "MISSING no run of \($name) on this commit"
        elif .status != "completed" then "RUNNING \($name) is \(.status) (run \(.databaseId))"
        elif .conclusion == "success" then "ok \($name) (run \(.databaseId))"
        else "RED \($name) ended \(.conclusion) (run \(.databaseId))" end' "$runs")"
    printf '%s\n' "$line"
    case "$line" in ok\ *) ;; *) bad=1 ;; esac
  done
  return "$bad"
}

# pr-verdict <checks.json> <workflow>...: <checks.json> is `gh pr checks <n> --json name,workflow,state,bucket`. For each
# required workflow the check named "All green" must be in the pass bucket. One line per workflow (`ok`, `RED`,
# `RUNNING`, `MISSING`); exit 0 only when every one is ok.
pr_verdict() {
  local checks="$1"; shift
  [ -f "$checks" ] || die "no such file: $checks"
  local bad=0 name line
  for name in "$@"; do
    line="$(jq -r --arg wf "$name" '
      [ .[] | select(.workflow == $wf and .name == "All green") ] | last
      | if . == null then "MISSING no \"All green\" check of \($wf) on this head"
        elif .bucket == "pass" then "ok \($wf)"
        elif .bucket == "pending" then "RUNNING \($wf) is \(.state)"
        else "RED \($wf) is \(.state)" end' "$checks")"
    printf '%s\n' "$line"
    case "$line" in ok\ *) ;; *) bad=1 ;; esac
  done
  return "$bad"
}

site_needed() {
  local site="$1"
  [ -f "$site" ] || die "no such file: $site"
  # The entries of the first `paths:` list (the push trigger's): `      - "site/**"`.
  local globs
  globs="$(awk '/^  push:/{p=1} /^  pull_request:/{p=0} p && /^      - "/{gsub(/^      - "|"$/,""); print}' "$site")"
  [ -n "$globs" ] || { echo yes; return; }   # no filter: Site runs for every push
  local file glob
  while IFS= read -r file; do
    [ -n "$file" ] || continue
    while IFS= read -r glob; do
      # shellcheck disable=SC2053  # the pattern is the point
      if [[ "$file" == ${glob//\*\*/*} ]]; then echo yes; return; fi
    done <<< "$globs"
  done
  echo no
}

case "${1:-}" in
  verdict) shift; [ "$#" -ge 2 ] || die "usage: verdict <runs.json> <workflow>..."; verdict "$@" ;;
  site-needed) shift; [ "$#" -eq 1 ] || die "usage: site-needed <site.yml>"; site_needed "$1" ;;
  pr-verdict) shift; [ "$#" -ge 2 ] || die "usage: pr-verdict <checks.json> <workflow>..."; pr_verdict "$@" ;;
  *) die "usage: wt-ci-check.sh verdict|site-needed|pr-verdict ..." ;;
esac
