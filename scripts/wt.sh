#!/usr/bin/env bash
# Worktree helper for parallel work on Undra (humans and agents).
#
#   scripts/wt.sh new <slug>      create <repo-parent>/.work/<slug> on branch wt/<slug> from main
#   scripts/wt.sh merge <slug>    from the main checkout: merge wt/<slug> (--no-ff), keep the branch
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
    need_slug merge "${2:-}"; slug="$2"
    cd "$MAIN"
    [ "$(git branch --show-current)" = "main" ] || die "run merge from the main checkout on main"
    [ -z "$(git -C "$WORK/$slug" status --porcelain 2>/dev/null)" ] \
      || die "$WORK/$slug has uncommitted changes; commit or drop them first"
    git merge --no-ff -m "merge: wt/$slug" "wt/$slug"
    echo "merged wt/$slug — now run the full test matrix, then: scripts/wt.sh rm $slug"
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
    die "usage: wt.sh new|merge|rm|clean|list [<slug>]"
    ;;
esac
