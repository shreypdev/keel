#!/usr/bin/env bash
# `scripts/wt.sh merge` and `clean` end to end, against scratch repositories (a bare origin, a main checkout, worktrees):
# after a merge nothing of the piece is left, and nothing that is not merged is ever deleted. docs/AGENT_WORKFLOW.md, section 5.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT
export GIT_AUTHOR_NAME=test GIT_AUTHOR_EMAIL=test@example.com GIT_COMMITTER_NAME=test GIT_COMMITTER_EMAIL=test@example.com
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null
export UNDRA_CI_LOCAL_DIR="$T/ci-local"
fails=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fails=$((fails + 1)); }
check() { # <name> <command...>: passes when the command succeeds
  local name="$1"; shift
  if "$@" >/dev/null 2>&1; then ok "$name"; else bad "$name"; fi
}
refute() { # <name> <command...>: passes when the command fails
  local name="$1"; shift
  if "$@" >/dev/null 2>&1; then bad "$name"; else ok "$name"; fi
}
has() { grep -qF -- "$2" <<<"$1"; }

# A world: $W/origin.git (bare), $W/main (the primary checkout with scripts/wt.sh), $W/.work (worktrees).
world() {
  W="$T/$1"; mkdir -p "$W"
  git init -q --bare -b main "$W/origin.git"
  git clone -q "$W/origin.git" "$W/main" 2>/dev/null
  mkdir -p "$W/main/scripts"
  cp "$HERE/wt.sh" "$HERE/wt-ci-check.sh" "$W/main/scripts/"
  git -C "$W/main" checkout -q -b main 2>/dev/null || true
  echo base > "$W/main/base.txt"
  echo "target/" > "$W/main/.gitignore"
  git -C "$W/main" add -A
  git -C "$W/main" commit -qm "base"
  git -C "$W/main" push -q origin main
  MAINDIR="$W/main"
}
wt() { (cd "$MAINDIR" && bash scripts/wt.sh "$@"); }
# A piece with one commit and some build output, pushed to origin.
piece() { # <slug>
  wt new "$1" >/dev/null
  echo "$1" > "$W/.work/$1/$1.txt"
  mkdir -p "$W/.work/$1/target"; echo blob > "$W/.work/$1/target/blob"
  git -C "$W/.work/$1" add "$1.txt"
  git -C "$W/.work/$1" commit -qm "feat($1): the piece"
  git -C "$W/.work/$1" push -q origin "wt/$1"
}
remote_branches() { git -C "$W/origin.git" for-each-ref --format='%(refname:short)' refs/heads; }
has_remote() { remote_branches | grep -qx -- "$1"; }
has_local() { git -C "$MAINDIR" show-ref --verify --quiet "refs/heads/$1"; }
no_remote() { ! has_remote "$1"; }
no_local() { ! has_local "$1"; }
same_main() { [ "$(git -C "$MAINDIR" rev-parse main)" = "$(git -C "$W/origin.git" rev-parse main)" ]; }

# ---- 1. merge: pushed, verified, and nothing of the piece is left; unmerged helpers are kept and named -------------------
world one
piece alpha
# Helpers: proto/alpha (merged: it is where the piece is), a sub-piece with a commit of its own, two --also branches.
git -C "$MAINDIR" branch proto/alpha wt/alpha
git -C "$MAINDIR" push -q origin proto/alpha
git -C "$MAINDIR" branch wt/alpha-sub wt/alpha
git -C "$MAINDIR" worktree add -q "$W/.work/alpha-sub" wt/alpha-sub
echo sub > "$W/.work/alpha-sub/sub.txt"; git -C "$W/.work/alpha-sub" add sub.txt; git -C "$W/.work/alpha-sub" commit -qm "sub: not merged"
git -C "$MAINDIR" branch spike/merged wt/alpha
git -C "$MAINDIR" branch spike/unmerged wt/alpha
git -C "$MAINDIR" worktree add -q "$W/.work/spike-u" spike/unmerged
echo u > "$W/.work/spike-u/u.txt"; git -C "$W/.work/spike-u" add u.txt; git -C "$W/.work/spike-u" commit -qm "spike: not merged"
mkdir -p "$UNDRA_CI_LOCAL_DIR/wt-alpha"; echo clone > "$UNDRA_CI_LOCAL_DIR/wt-alpha/marker"
out="$(wt merge alpha --no-ci --also spike/merged --also spike/unmerged 2>&1)"; rc=$?
[ "$rc" = 0 ] && ok "merge succeeds" || { bad "merge succeeds (exit $rc)"; echo "$out" | sed 's/^/    /'; }
check "main is fast-forwarded to the piece" test -f "$MAINDIR/alpha.txt"
check "the pushed main is the merged main" same_main
check "the remote branch of the piece is deleted" no_remote wt/alpha
check "the local branch of the piece is deleted" no_local wt/alpha
check "the worktree and its build output are gone" test ! -e "$W/.work/alpha"
check "the ci-local clone of the piece is gone" test ! -e "$UNDRA_CI_LOCAL_DIR/wt-alpha"
check "the merged proto/<slug> is deleted (local, remote)" bash -c "! git -C '$MAINDIR' show-ref --verify --quiet refs/heads/proto/alpha && ! git -C '$W/origin.git' show-ref --verify --quiet refs/heads/proto/alpha"
check "a merged --also branch is deleted" no_local spike/merged
check "the unmerged sub-piece branch is kept" has_local wt/alpha-sub
check "its worktree is kept" test -d "$W/.work/alpha-sub"
check "the unmerged --also branch is kept" has_local spike/unmerged
check "the kept sub-piece is named as not merged" has "$out" "kept    wt/alpha-sub: NOT merged (1 commit(s)"
check "the kept --also branch is named as not merged" has "$out" "kept    spike/unmerged: NOT merged"
check "the output verifies the pushed main" has "$out" "is in main and in origin/main"
check "the output ends with the worktree list and the wt/* branches" bash -c "grep -q '^worktrees:' <<<\"\$1\" && grep -q '^local wt/\\* branches:' <<<\"\$1\" && grep -q '^remote wt/\\* branches' <<<\"\$1\"" _ "$out"
check "the output says something was kept" has "$out" "something above was kept"

# ---- 2. --no-push deletes nothing; clean refuses a branch main's remote lacks, then removes it once main is pushed -----
world two
piece beta
out="$(wt merge beta --no-ci --no-push 2>&1)"; rc=$?
[ "$rc" = 0 ] && ok "--no-push merges" || { bad "--no-push merges (exit $rc)"; echo "$out" | sed 's/^/    /'; }
check "--no-push: main has the piece" test -f "$MAINDIR/beta.txt"
check "--no-push: nothing is deleted (branch, remote branch, worktree)" bash -c "git -C '$MAINDIR' show-ref --verify --quiet refs/heads/wt/beta && git -C '$W/origin.git' show-ref --verify --quiet refs/heads/wt/beta && test -d '$W/.work/beta'"
check "--no-push says to push main and run clean" has "$out" "scripts/wt.sh clean"
out="$(wt clean 2>&1)"
check "clean keeps a branch that origin/main does not contain yet" has "$out" "kept    wt/beta: merged into the local main only; origin/main does not contain it yet"
check "clean kept its worktree" test -d "$W/.work/beta"
git -C "$MAINDIR" push -q origin main
out="$(wt clean 2>&1)"
check "after main is pushed, clean removes the piece" bash -c "! git -C '$MAINDIR' show-ref --verify --quiet refs/heads/wt/beta && ! git -C '$W/origin.git' show-ref --verify --quiet refs/heads/wt/beta && test ! -e '$W/.work/beta'"

# ---- 3. a push that fails leaves everything in place ---------------------------------------------------------------------
world three
piece gamma
printf '#!/bin/sh\nwhile read old new ref; do [ "$ref" = refs/heads/main ] && exit 1; done\nexit 0\n' > "$W/origin.git/hooks/pre-receive"
chmod +x "$W/origin.git/hooks/pre-receive"
out="$(wt merge gamma --no-ci 2>&1)"; rc=$?
[ "$rc" != 0 ] && ok "a rejected push fails the merge" || bad "a rejected push fails the merge"
check "...and says nothing was deleted" has "$out" "nothing was deleted"
check "...and the branch, the remote branch and the worktree are still there" bash -c "git -C '$MAINDIR' show-ref --verify --quiet refs/heads/wt/gamma && git -C '$W/origin.git' show-ref --verify --quiet refs/heads/wt/gamma && test -d '$W/.work/gamma'"

# ---- 4. clean: merged goes, everything else stays and says why -----------------------------------------------------------
world four
piece done1
piece done2
piece mine      # merged, but its worktree has uncommitted changes
piece unmerged
git -C "$MAINDIR" merge -q --ff-only wt/done1
git -C "$MAINDIR" merge -q --no-edit wt/done2 >/dev/null 2>&1
git -C "$MAINDIR" merge -q --no-edit wt/mine >/dev/null 2>&1
git -C "$MAINDIR" push -q origin main
echo dirty > "$W/.work/mine/dirty.txt"
wt new fresh >/dev/null            # a new piece: no commits of its own, trivially an ancestor of main
git -C "$MAINDIR" push -q origin wt/fresh 2>/dev/null || true
out="$(wt clean 2>&1)"; rc=$?
[ "$rc" = 0 ] && ok "clean succeeds" || { bad "clean succeeds (exit $rc)"; echo "$out" | sed 's/^/    /'; }
check "clean removes a merged piece (branches, worktree)" bash -c "! git -C '$MAINDIR' show-ref --verify --quiet refs/heads/wt/done1 && ! git -C '$W/origin.git' show-ref --verify --quiet refs/heads/wt/done1 && test ! -e '$W/.work/done1'"
check "clean removes every merged piece" bash -c "! git -C '$MAINDIR' show-ref --verify --quiet refs/heads/wt/done2 && test ! -e '$W/.work/done2'"
check "clean keeps a merged piece whose worktree has uncommitted changes" bash -c "test -f '$W/.work/mine/dirty.txt' && git -C '$MAINDIR' show-ref --verify --quiet refs/heads/wt/mine"
check "...and says why" has "$out" "uncommitted changes"
check "clean keeps an unmerged piece, locally and on origin" bash -c "git -C '$MAINDIR' show-ref --verify --quiet refs/heads/wt/unmerged && git -C '$W/origin.git' show-ref --verify --quiet refs/heads/wt/unmerged && test -d '$W/.work/unmerged'"
check "...and says it is not merged" has "$out" "kept    wt/unmerged: NOT merged"
check "clean keeps a new piece with no commits of its own" bash -c "git -C '$MAINDIR' show-ref --verify --quiet refs/heads/wt/fresh && test -d '$W/.work/fresh'"
check "...and says so" has "$out" "kept    wt/fresh: a new piece with no commits of its own yet"
check "clean ends with the worktree list and the branches" has "$out" "remote wt/* branches (origin):"

[ "$fails" = 0 ] && echo "wt-cleanup: all passed" || { echo "wt-cleanup: $fails failed"; exit 1; }
