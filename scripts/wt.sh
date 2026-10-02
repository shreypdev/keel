#!/usr/bin/env bash
# Worktree helper for parallel work on Undra (humans and agents).
#
#   scripts/wt.sh new <slug>      create <repo-parent>/.work/<slug> on branch wt/<slug> from main
#   scripts/wt.sh merge <slug> [--no-ci]
#                                 from the main checkout: fast-forward main to wt/<slug>, keep the branch. Refuses unless
#                                 the branch contains main and CI (CI, Bench, Two cores, and Site when the branch touches
#                                 its paths) is green on the branch's exact head: push it first (`git push origin wt/<slug>`).
#                                 --no-ci skips the CI check, loudly: for commits that only change state files
#   scripts/wt.sh rm <slug>       remove the worktree AND delete wt/<slug> if fully merged
#   scripts/wt.sh clean           remove every .work worktree whose branch is fully merged, and prune
#   scripts/wt.sh list            show worktrees and how far each branch is from main
#
# The full workflow (briefs, review, who merges) is docs/AGENT_WORKFLOW.md.
set -euo pipefail

MAIN="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(cd "$MAIN/.." && pwd)/.work"
cmd="${1:-}"

die() { echo "wt.sh: $*" >&2; exit 2; }
need_slug() { [ -n "${2:-}" ] || die "usage: wt.sh $1 <slug>"; }

# CI is green on wt/<slug> at <head sha>: the newest run of every required workflow for exactly that commit is a success.
# The parsing is scripts/wt-ci-check.sh (tested against fixtures by scripts/wt-ci-check.test.sh).
ci_green() {
  local slug="$1" head="$2" runs site required changed
  command -v gh >/dev/null || die "gh is needed to read the CI runs of wt/$slug (or --no-ci, for state-only commits)"
  runs="$(mktemp)"; site="$(mktemp)"
  if ! gh run list --branch "wt/$slug" --commit "$head" --limit 100 \
       --json workflowName,status,conclusion,createdAt,databaseId > "$runs"; then
    rm -f "$runs" "$site"; die "gh could not list the runs of wt/$slug at $head"
  fi
  required=(CI Bench "Two cores")
  git show "wt/$slug:.github/workflows/site.yml" > "$site"
  changed="$(git diff --name-only "main...wt/$slug")"
  if [ "$(printf '%s\n' "$changed" | "$MAIN/scripts/wt-ci-check.sh" site-needed "$site")" = yes ]; then
    required+=(Site)
  fi
  echo "wt.sh: CI on wt/$slug at ${head:0:12}: required ${required[*]}" >&2
  if "$MAIN/scripts/wt-ci-check.sh" verdict "$runs" "${required[@]}" >&2; then
    rm -f "$runs" "$site"; return 0
  fi
  rm -f "$runs" "$site"
  {
    echo "wt.sh: not merging: CI is not green on the exact head of wt/$slug ($head)."
    echo "  Push the branch if it is not on origin yet:   git push origin wt/$slug"
    echo "  then wait for every line above to say ok:     gh run list --branch wt/$slug --commit $head"
    echo "  (a Site run is required, and absent, when its path filter did not see this push:  gh workflow run site.yml --ref wt/$slug)"
    echo "  --no-ci skips this check; it is for commits that only change state files."
  } >&2
  exit 2
}

case "$cmd" in
  new)
    need_slug new "${2:-}"; slug="$2"
    mkdir -p "$WORK"
    cd "$MAIN"
    git worktree add -q -b "wt/$slug" "$WORK/$slug" main 2>/dev/null \
      || git worktree add -q "$WORK/$slug" "wt/$slug"
    echo "$WORK/$slug"
    ;;
  merge)
    need_slug merge "${2:-}"; slug="$2"; no_ci=0
    case "${3:-}" in
      "") ;;
      --no-ci) no_ci=1 ;;
      *) die "usage: wt.sh merge <slug> [--no-ci]" ;;
    esac
    cd "$MAIN"
    [ "$(git branch --show-current)" = "main" ] || die "run merge from the main checkout on main"
    [ -z "$(git -C "$WORK/$slug" status --porcelain 2>/dev/null)" ] \
      || die "$WORK/$slug has uncommitted changes; commit or drop them first"
    head="$(git rev-parse "wt/$slug")"
    git merge-base --is-ancestor main "wt/$slug" \
      || die "wt/$slug does not contain main: run \`git merge main\` in $WORK/$slug, push the branch, and wait for CI on its new head"
    if [ "$no_ci" = 1 ]; then
      echo "wt.sh: !!! --no-ci: merging wt/$slug at ${head:0:12} WITHOUT a green CI run on that commit (state-only commits only) !!!" >&2
    else
      ci_green "$slug" "$head"
    fi
    git merge --ff-only "wt/$slug"
    echo "merged wt/$slug (fast-forward to ${head:0:12}) — now run the full test matrix, then: scripts/wt.sh rm $slug"
    ;;
  rm)
    need_slug rm "${2:-}"; slug="$2"
    cd "$MAIN"
    git worktree remove --force "$WORK/$slug"
    if [ "$(git rev-list --count main.."wt/$slug" 2>/dev/null || echo x)" = "0" ]; then
      git branch -d "wt/$slug"
      echo "removed worktree and merged branch wt/$slug"
    else
      echo "removed worktree; branch wt/$slug kept (NOT fully merged into main)"
    fi
    ;;
  clean)
    cd "$MAIN"
    git worktree prune
    removed=0
    for path in "$WORK"/*/; do
      [ -d "$path" ] || continue
      slug="$(basename "$path")"
      branch="wt/$slug"
      git show-ref --verify --quiet "refs/heads/$branch" || continue
      if [ "$(git rev-list --count main.."$branch")" = "0" ] \
         && [ -z "$(git -C "$path" status --porcelain 2>/dev/null)" ]; then
        git worktree remove --force "$path" && git branch -d "$branch" \
          && echo "cleaned: $slug" && removed=$((removed + 1))
      else
        echo "kept: $slug (unmerged commits or uncommitted changes)"
      fi
    done
    git worktree prune
    rmdir "$WORK" 2>/dev/null && echo "removed empty $WORK" || true
    echo "clean done ($removed removed)"
    ;;
  list)
    cd "$MAIN"
    git worktree list
    for b in $(git branch --format='%(refname:short)' | grep '^wt/' || true); do
      echo "$b: $(git rev-list --count main..$b) ahead of main"
    done
    ;;
  *)
    die "usage: wt.sh new|merge|rm|clean|list [<slug>] (merge takes --no-ci)"
    ;;
esac
