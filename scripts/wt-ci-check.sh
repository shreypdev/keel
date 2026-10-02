#!/usr/bin/env bash
# The check behind `scripts/wt.sh merge`: is CI green on a branch's exact head? (docs/AGENT_WORKFLOW.md)
#
#   wt-ci-check.sh verdict <runs.json> <workflow>...
#       <runs.json> is `gh run list --branch wt/<slug> --commit <head sha> --json workflowName,status,conclusion,createdAt,databaseId`.
#       For each required <workflow> (its `name:`), the newest run of it (a re-run supersedes the run it repeats) must be
#       completed with conclusion success. Prints one line per workflow (`ok`, `RED`, `RUNNING`, `MISSING`) and exits 0
#       only when every one is ok.
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
  *) die "usage: wt-ci-check.sh verdict|site-needed ..." ;;
esac
