#!/usr/bin/env bash
# Worktree helper for parallel work on Undra (humans and agents).
#
#   scripts/wt.sh new <slug>      create <repo-parent>/.work/<slug> on branch wt/<slug> from main
#   scripts/wt.sh merge <slug> [--also <branch>]... [--ff [--no-ci] [--no-push]]
#                                 from the main checkout: land wt/<slug> on main. Refuses unless the branch contains main and
#                                 is pushed. By default it goes through a pull request: opens one if none is open, waits for
#                                 its checks, requires the "All green" check of CI, Bench, Two cores and Site on the exact head,
#                                 and merges with a merge commit (main is protected; GitHub refuses anything else). Then it
#                                 verifies that origin/main contains the head and leaves nothing behind: the remote branch, the
#                                 local branch, the worktree with its build output, the ci-local.sh clone, and the piece's helper
#                                 branches (proto/<slug>, wt/<slug>-*, and each --also <branch>) when they are merged; anything
#                                 not merged is kept and named, with why. It ends by listing worktrees and wt/* branches.
#                                 --ff is the fast-forward-and-push path for a repository without branch protection (and the
#                                 scratch repositories of the tests): it checks the runs on the head itself; --no-ci skips that
#                                 check, loudly, for commits that only change state files; --no-push merges locally only.
#   scripts/wt.sh pr <slug>       open a draft pull request for wt/<slug> early (CI runs on every push to it from then on)
#   scripts/wt.sh rm <slug>       remove the worktree AND delete wt/<slug> if fully merged
#   scripts/wt.sh clean           the same sweep for every wt/* branch (local and on origin) already merged into origin/main:
#                                 its worktree, build output, branches, ci-local clone. Branches with commits of their own
#                                 that main lacks, worktrees with uncommitted changes and new, empty pieces are kept, with why
#   scripts/wt.sh list           show worktrees and how far each branch is from main
#
# The full workflow (briefs, review, who merges) is docs/AGENT_WORKFLOW.md. Scratch repositories for the tests:
# UNDRA_WT_REMOTE names the remote (default origin).
set -euo pipefail

MAIN="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(cd "$MAIN/.." && pwd)/.work"
cmd="${1:-}"

die() { echo "wt.sh: $*" >&2; exit 2; }
need_slug() { [ -n "${2:-}" ] || die "usage: wt.sh $1 <slug>"; }

REMOTE="${UNDRA_WT_REMOTE:-origin}"
CI_LOCAL_BASE="${UNDRA_CI_LOCAL_DIR:-${TMPDIR:-/tmp}/undra-ci-local}"

# The path of the worktree that has branch $1 checked out, if any.
worktree_of() {
  git worktree list --porcelain | awk -v b="refs/heads/$1" '/^worktree /{p=substr($0,10)} /^branch /{if($2==b)print p}'
}

# Is the remote reachable? (a failed `git ls-remote` is not "no such branch")
remote_reachable() { git ls-remote --heads "$REMOTE" >/dev/null 2>&1; }

# Does the remote have branch $1? 0 yes, 1 no (2 unreachable is folded into no: callers check remote_reachable first).
remote_has() { git ls-remote --exit-code --heads "$REMOTE" "$1" >/dev/null 2>&1; }

# The local branches wt/* and the remote ones, each name once.
piece_branches() {
  { git for-each-ref --format='%(refname:short)' 'refs/heads/wt/*'
    git ls-remote --heads "$REMOTE" 'wt/*' 2>/dev/null | sed 's#.*refs/heads/##'
  } | sort -u
}

# A piece branch that was only just created (wt.sh new) has no commits of its own and is trivially "merged": keep it.
is_new_empty_piece() {
  local first count
  git show-ref --verify --quiet "refs/heads/$1" || return 1
  count="$(git reflog show --format=%gs "refs/heads/$1" 2>/dev/null | grep -c . || true)"
  first="$(git reflog show --format=%gs "refs/heads/$1" 2>/dev/null | tail -n 1)"
  [ "$count" = 1 ] && case "$first" in "branch: Created from"*) return 0 ;; esac
  return 1
}

# Delete branch $1 (its worktree with the build output, its remote branch, the local branch, the ci-local clone) only when
# every commit of it is in origin/main. Prints what it removed and what it kept, and why. Returns 0 when nothing is left.
sweep_branch() {
  local b="$1" tip wt left=0 ahead_note="" slug_dir
  if git show-ref --verify --quiet "refs/heads/$b"; then
    tip="$(git rev-parse "refs/heads/$b")"
  elif remote_has "$b"; then
    git fetch -q "$REMOTE" "refs/heads/$b" 2>/dev/null || { echo "  kept    $b: could not fetch it from $REMOTE to check it"; return 1; }
    tip="$(git rev-parse FETCH_HEAD)"
  else
    return 0
  fi
  if ! git merge-base --is-ancestor "$tip" "refs/remotes/$REMOTE/main" 2>/dev/null; then
    if git merge-base --is-ancestor "$tip" main 2>/dev/null; then
      echo "  kept    $b: merged into the local main only; $REMOTE/main does not contain it yet (push main first)"
    else
      ahead_note="$(git rev-list --count "refs/remotes/$REMOTE/main..$tip" 2>/dev/null || echo '?')"
      echo "  kept    $b: NOT merged ($ahead_note commit(s) that $REMOTE/main lacks)"
    fi
    return 1
  fi
  wt="$(worktree_of "$b")"
  if [ -n "$wt" ]; then
    if [ "$wt" = "$MAIN" ]; then
      echo "  kept    $b: it is checked out in the main checkout"; return 1
    fi
    if [ -n "$(git -C "$wt" status --porcelain 2>/dev/null)" ]; then
      echo "  kept    $b: its worktree $wt has uncommitted changes"; return 1
    fi
    if git worktree remove --force "$wt" 2>/dev/null || { rm -rf "$wt" && git worktree prune; }; then
      echo "  removed worktree $wt (with its build output)"
    else
      echo "  kept    $b: could not remove its worktree $wt"; return 1
    fi
  fi
  if remote_has "$b"; then
    if git push -q "$REMOTE" --delete "$b" 2>/dev/null; then
      echo "  removed $REMOTE/$b"
    else
      remote_has "$b" && { echo "  kept    $REMOTE/$b: the delete was refused"; left=1; }
    fi
  fi
  if git show-ref --verify --quiet "refs/heads/$b"; then
    # Ancestry in origin/main was verified above, so -d's own check against the local main (which may lag) is not the test.
    git branch -q -D "$b" && echo "  removed branch $b"
  fi
  git update-ref -d "refs/remotes/$REMOTE/$b" 2>/dev/null || true
  slug_dir="$CI_LOCAL_BASE/$(printf '%s' "$b" | tr '/ ' '--')"
  if [ -d "$slug_dir" ]; then rm -rf "$slug_dir" && echo "  removed ci-local clone $slug_dir"; fi
  return "$left"
}

# The helper branches of piece $1 (proto/<slug>, wt/<slug>-* sub-pieces, local or remote) and the extra names after it.
helper_branches() {
  local slug="$1" extra
  shift
  {
    echo "proto/$slug"
    git for-each-ref --format='%(refname:short)' "refs/heads/wt/$slug-*"
    git ls-remote --heads "$REMOTE" "wt/$slug-*" 2>/dev/null | sed 's#.*refs/heads/##'
    for extra in "$@"; do echo "$extra"; done
  } | sort -u
}

# What is left: worktrees and wt/* branches, local and remote.
show_state() {
  echo "worktrees:"; git worktree list | sed 's/^/  /'
  echo "local wt/* branches:"
  local local_b; local_b="$(git for-each-ref --format='%(refname:short)' 'refs/heads/wt/*')"
  if [ -n "$local_b" ]; then printf '%s\n' "$local_b" | sed 's/^/  /'; else echo "  (none)"; fi
  echo "remote wt/* branches ($REMOTE):"
  if remote_reachable; then
    local remote_b; remote_b="$(git ls-remote --heads "$REMOTE" 'wt/*' | sed 's#.*refs/heads/#  #')"
    if [ -n "$remote_b" ]; then printf '%s\n' "$remote_b"; else echo "  (none)"; fi
  else
    echo "  ($REMOTE is not reachable)"
  fi
}

# Brings origin/main up to date; dies when it cannot (nothing may be deleted on a guess).
fetch_remote_main() {
  git fetch -q "$REMOTE" "+refs/heads/main:refs/remotes/$REMOTE/main" 2>/dev/null \
    || die "cannot fetch $REMOTE/main: nothing was deleted (a branch is removed only once $REMOTE/main is known to contain it)"
}

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

# The pull request of wt/$1 (open, into main): its number, or nothing.
pr_number() { gh pr list --head "wt/$1" --base main --state open --json number --jq '.[0].number // empty' 2>/dev/null; }

# Lands wt/$1 at head $2 through a pull request: opens one if none is open (title and body from the piece's own
# commits), marks it ready, waits for its checks, requires the "All green" check of CI, Bench, Two cores and Site
# on that exact head, and merges with a merge commit so the tree that lands is the tree CI tested. Branch
# protection on main makes GitHub refuse anything else; this script only says what is missing and how to get it.
pr_land() {
  local slug="$1" head="$2" num remote_head title body checks
  command -v gh >/dev/null || die "gh is needed to land wt/$slug through a pull request (or --ff on a repository without branch protection)"
  remote_reachable || die "$REMOTE is not reachable"
  remote_head="$(git ls-remote "$REMOTE" "refs/heads/wt/$slug" | cut -f1)"
  [ -n "$remote_head" ] || die "wt/$slug is not on $REMOTE: git push $REMOTE wt/$slug, then run merge again"
  [ "$remote_head" = "$head" ] || die "$REMOTE/wt/$slug is at ${remote_head:0:12}, the local branch at ${head:0:12}: push (or pull) first"
  num="$(pr_number "$slug")"
  if [ -z "$num" ]; then
    title="$slug: $(git log --no-merges --format=%s -1 "main..wt/$slug")"
    body="$(printf 'Piece `%s` at %s.\n\n%s\n\nRecords: `.10x/decisions/sde/%s.md`, `.10x/reviews/`.\n' "$slug" "${head:0:12}" "$(git log --no-merges --format='- %s' "main..wt/$slug" | head -60)" "$slug")"
    num="$(gh pr create --base main --head "wt/$slug" --title "$title" --body "$body" | grep -oE '[0-9]+$')"
    [ -n "$num" ] || die "gh pr create did not return a pull request number"
    echo "wt.sh: opened pull request #$num for wt/$slug"
  else
    echo "wt.sh: pull request #$num is open for wt/$slug"
  fi
  gh pr ready "$num" >/dev/null 2>&1 || true   # a draft cannot merge; already-ready is not an error worth stopping for
  echo "wt.sh: waiting for the checks of #$num on ${head:0:12} (every workflow's \"All green\")"
  gh pr checks "$num" --watch >/dev/null 2>&1 || true   # the verdict below says what is red; --watch only waits
  checks="$(mktemp)"
  gh pr checks "$num" --json name,workflow,state,bucket > "$checks" \
    || { rm -f "$checks"; die "gh could not read the checks of #$num"; }
  if ! "$MAIN/scripts/wt-ci-check.sh" pr-verdict "$checks" CI Bench "Two cores" Site >&2; then
    rm -f "$checks"
    {
      echo "wt.sh: not merging: a required check is not green on ${head:0:12} (pull request #$num)."
      echo "  Fix the cause on the branch, push, and run merge again; a check that never reports is a workflow that did not run"
      echo "  (gh run list --branch wt/$slug --commit $head)."
    } >&2
    exit 2
  fi
  rm -f "$checks"
  [ "$(gh pr view "$num" --json headRefOid --jq .headRefOid)" = "$head" ] \
    || die "pull request #$num moved past ${head:0:12} while the checks ran: run merge again"
  gh pr merge "$num" --merge --delete-branch \
    || die "GitHub refused to merge #$num (branch protection: is main merged in and every required check green?); nothing was deleted"
  echo "wt.sh: merged pull request #$num (a merge commit of ${head:0:12})"
  git pull -q --ff-only "$REMOTE" main || die "main is merged on $REMOTE but the local main did not fast-forward: fix the local checkout, then run scripts/wt.sh clean"
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
    need_slug merge "${2:-}"; slug="$2"; shift 2
    mode=pr; no_ci=0; no_push=0; also=()
    while [ $# -gt 0 ]; do
      case "$1" in
        --ff) mode=ff ;;
        --no-ci) no_ci=1 ;;
        --no-push) no_push=1 ;;
        --also) [ -n "${2:-}" ] || die "--also needs a branch name"; also+=("$2"); shift ;;
        *) die "usage: wt.sh merge <slug> [--also <branch>]... [--ff [--no-ci] [--no-push]]" ;;
      esac
      shift
    done
    [ "$mode" = ff ] || [ "$no_ci$no_push" = 00 ] || die "--no-ci and --no-push go with --ff: a pull request's checks are GitHub's to skip"
    cd "$MAIN"
    [ "$(git branch --show-current)" = "main" ] || die "run merge from the main checkout on main"
    [ -z "$(git -C "$WORK/$slug" status --porcelain 2>/dev/null)" ] \
      || die "$WORK/$slug has uncommitted changes; commit or drop them first"
    head="$(git rev-parse "wt/$slug")"
    git merge-base --is-ancestor main "wt/$slug" \
      || die "wt/$slug does not contain main: run \`git merge main\` in $WORK/$slug, push the branch, and wait for CI on its new head"
    if [ "$mode" = pr ]; then
      # The default: a pull request, its "All green" checks, GitHub's merge (a merge commit of the exact head CI ran
      # on, so the tree that lands is the tree that was tested), then the same verification and clean-up as --ff.
      pr_land "$slug" "$head"
    else
      # Fast-forward and push: for a repository without branch protection, and for the scratch repositories of
      # scripts/wt-cleanup.test.sh.
      if [ "$no_ci" = 1 ]; then
        echo "wt.sh: !!! --no-ci: merging wt/$slug at ${head:0:12} WITHOUT a green CI run on that commit (state-only commits only) !!!" >&2
      else
        ci_green "$slug" "$head"
      fi
      git merge --ff-only "wt/$slug"
      echo "wt.sh: merged wt/$slug (fast-forward to ${head:0:12})"
      if [ "$no_push" = 1 ]; then
        echo "wt.sh: --no-push: main is merged locally and NOT pushed; nothing was deleted."
        echo "       After \`git push $REMOTE main\`, run: scripts/wt.sh clean   (it removes this piece and every other merged one)"
        exit 0
      fi
      echo "wt.sh: pushing main to $REMOTE"
      git push -q "$REMOTE" main \
        || die "main is merged locally but the push to $REMOTE failed; nothing was deleted. Fix the push, then run scripts/wt.sh clean"
    fi
    # (a) verify: the branch is in main, and the pushed main contains the head.
    git merge-base --is-ancestor "wt/$slug" main || die "wt/$slug is not an ancestor of main after the merge: nothing was deleted"
    fetch_remote_main
    git merge-base --is-ancestor "$head" "refs/remotes/$REMOTE/main" \
      || die "$REMOTE/main does not contain ${head:0:12} after the push: nothing was deleted"
    echo "wt.sh: verified: wt/$slug (${head:0:12}) is in main and in $REMOTE/main"
    # (b) the piece itself, (c) its helper branches, merged ones only.
    echo "wt.sh: cleaning up:"
    kept=0
    sweep_branch "wt/$slug" || kept=1
    while IFS= read -r h; do
      [ -n "$h" ] || continue
      sweep_branch "$h" || kept=1
    done < <(helper_branches "$slug" ${also[@]+"${also[@]}"})
    # (d) what is left.
    show_state
    if [ "$kept" = 1 ]; then echo "wt.sh: done, but something above was kept: read the 'kept' lines."; else echo "wt.sh: done: nothing of $slug is left."; fi
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
    fetch_remote_main
    removed=0; kept=0
    echo "wt.sh: sweeping the wt/* branches merged into $REMOTE/main:"
    while IFS= read -r b; do
      [ -n "$b" ] || continue
      if is_new_empty_piece "$b"; then
        echo "  kept    $b: a new piece with no commits of its own yet"; kept=$((kept + 1)); continue
      fi
      if sweep_branch "$b"; then removed=$((removed + 1)); else kept=$((kept + 1)); fi
    done < <(piece_branches)
    # Worktrees under .work whose branch is not a wt/* branch (detached, or a branch of another name) are not ours to judge.
    git worktree list --porcelain | awk -v w="$WORK/" '/^worktree /{p=substr($0,10)} /^branch /{b=$2} /^$/{if(index(p,w)==1 && b !~ /^refs\/heads\/wt\//) print p " (" (b==""?"detached":b) ")"; b=""}' \
      | sed 's/^/  left alone: worktree /; s/$/: not on a wt\/* branch/'
    rmdir "$WORK" 2>/dev/null && echo "removed empty $WORK" || true
    echo "wt.sh: clean done ($removed piece(s) removed, $kept kept)"
    show_state
    ;;
  pr)
    need_slug pr "${2:-}"; slug="$2"
    cd "$MAIN"
    command -v gh >/dev/null || die "gh is needed to open a pull request"
    git show-ref --verify --quiet "refs/heads/wt/$slug" || die "no branch wt/$slug"
    git push -q -u "$REMOTE" "wt/$slug" || die "could not push wt/$slug to $REMOTE"
    if num="$(pr_number "$slug")" && [ -n "$num" ]; then
      echo "wt.sh: pull request #$num is already open for wt/$slug"
    else
      gh pr create --draft --base main --head "wt/$slug" --title "$slug (draft)" \
        --body "$(printf 'Piece `%s`, in progress. CI runs on every push; `scripts/wt.sh merge %s` lands it when it is green and reviewed.' "$slug" "$slug")"
    fi
    ;;
  list)
    cd "$MAIN"
    git worktree list
    for b in $(git branch --format='%(refname:short)' | grep '^wt/' || true); do
      echo "$b: $(git rev-list --count main..$b) ahead of main"
    done
    ;;
  *)
    die "usage: wt.sh new|merge|pr|rm|clean|list [<slug>] (merge takes --also <branch>, and --ff with --no-ci/--no-push)"
    ;;
esac
