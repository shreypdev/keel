#!/usr/bin/env bash
# scripts/check-no-em-dash.sh against scratch repositories: it fails on a tracked file with an em-dash and names the
# file and line, and passes without one (an en-dash, a binary file holding the bytes and an untracked file do not count).
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CHECK="$HERE/check-no-em-dash.sh"
EM_DASH=$'\xe2\x80\x94'
EN_DASH=$'\xe2\x80\x93'
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

SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
REPO="$SCRATCH/repo"
mkdir -p "$REPO/docs"
git -C "$REPO" init -q

printf 'A label: its description.\nA range, 3.2%s3.5 us, keeps its en-dash.\n' "$EN_DASH" >"$REPO/README.md"
printf 'PNG\0 %s inside a binary file\n' "$EM_DASH" >"$REPO/docs/card.png"
git -C "$REPO" add README.md docs/card.png
expect "a clean repository passes (an en-dash and a binary file with the bytes do not count)" 0 "no em-dash in any tracked file" "$CHECK" "$REPO"

printf 'untracked %s here\n' "$EM_DASH" >"$REPO/scratch.md"
expect "an untracked file does not count" 0 "no em-dash in any tracked file" "$CHECK" "$REPO"

printf 'first line\nsecond %s line\n' "$EM_DASH" >"$REPO/docs/guide.md"
git -C "$REPO" add docs/guide.md
expect "a tracked file with an em-dash fails" 1 "1 line(s) of tracked files hold an em-dash" "$CHECK" "$REPO"
expect "the failure names the file and line" 1 "docs/guide.md:2" "$CHECK" "$REPO"
expect "the check is of the whole repository, from a subdirectory too" 1 "docs/guide.md:2" "$CHECK" "$REPO/docs"

printf 'first line\nsecond line: fixed\n' >"$REPO/docs/guide.md"
expect "the working-tree content is what is checked: fixed before it is committed, it passes" 0 "no em-dash in any tracked file" "$CHECK" "$REPO"

expect "a directory outside any repository is an error, not a pass" 2 "is not in a git repository" "$CHECK" "$SCRATCH"
expect "the script does not contain the character it looks for" 0 "" bash -c "! grep -qF -- '$EM_DASH' '$CHECK' '${BASH_SOURCE[0]}'"

if [ "$fails" -gt 0 ]; then
  echo "check-no-em-dash.test.sh: $fails failure(s)"
  exit 1
fi
echo "check-no-em-dash.test.sh: all passed"
