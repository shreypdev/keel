#!/usr/bin/env bash
# Worktree helper for parallel agents on the local VM.
#   scripts/wt.sh new <slug>      create $HOME/mnt/src/.work/<slug> on branch wt/<slug> from main
#   scripts/wt.sh merge <slug>    merge wt/<slug> into main (run from the main repo), then re-run tests
#   scripts/wt.sh rm <slug>       remove the worktree (branch is kept)
set -euo pipefail
MAIN="$HOME/mnt/src/keel"; WORK="$HOME/mnt/src/.work"; cmd="${1:-}"; slug="${2:-}"
[ -n "$cmd" ] && [ -n "$slug" ] || { echo "usage: wt.sh new|merge|rm <slug>"; exit 2; }
case "$cmd" in
  new)  mkdir -p "$WORK"; cd "$MAIN"; git worktree add -q -b "wt/$slug" "$WORK/$slug" main 2>/dev/null || git worktree add -q "$WORK/$slug" "wt/$slug"; echo "$WORK/$slug" ;;
  merge) cd "$MAIN"; git merge --no-ff -m "merge: wt/$slug" "wt/$slug" ;;
  rm)   cd "$MAIN"; git worktree remove --force "$WORK/$slug" ;;
esac
