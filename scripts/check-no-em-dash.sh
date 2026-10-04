#!/usr/bin/env bash
# No tracked file contains an em-dash (U+2014): not the site, the README, the docs, program output, code comments
# or the working records under .10x/ (.10x/decisions/sde/no-em-dash.md; CLAUDE.md, "Engineering standards").
#
#   scripts/check-no-em-dash.sh          the tracked files of this repository (from any directory)
#   scripts/check-no-em-dash.sh DIR      the tracked files of the repository at DIR (scripts/check-no-em-dash.test.sh)
#
# Write what the dash stood for instead: a colon, a comma, parentheses, a semicolon or two sentences (a spaced hyphen is
# fine). En-dashes (U+2013) in ranges are not checked. The working-tree content of every tracked file is searched;
# binary files are skipped (git grep -I). The character is spelled here as its UTF-8 bytes, so this file passes its own
# check.
#
# Exit status: 0 when no tracked file has one; 1 when one does (each file:line is listed); 2 when it could not check.
set -euo pipefail

EM_DASH=$'\xe2\x80\x94'

case "${1:-}" in
  -h | --help) sed -n '2,13p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
esac
[ "$#" -le 1 ] || { echo "usage: scripts/check-no-em-dash.sh [DIR]" >&2; exit 2; }

if [ "$#" = 1 ]; then
  ROOT="$1"
else
  ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
fi
ROOT="$(git -C "$ROOT" rev-parse --show-toplevel 2>/dev/null)" || { echo "check-no-em-dash: $ROOT is not in a git repository" >&2; exit 2; }

rc=0
hits="$(git -C "$ROOT" grep -I -n --full-name -F -e "$EM_DASH")" || rc=$?
case "$rc" in
  0)
    echo "check-no-em-dash: $(printf '%s\n' "$hits" | wc -l | tr -d ' ') line(s) of tracked files hold an em-dash (U+2014):" >&2
    printf '%s\n' "$hits" | cut -d: -f1,2 | sed 's/^/  /' >&2
    echo "Write what it stands for: a colon, a comma, parentheses, a semicolon or two sentences (CLAUDE.md)." >&2
    exit 1
    ;;
  1) echo "check-no-em-dash: no em-dash in any tracked file" ;;
  *) echo "check-no-em-dash: git grep failed (exit $rc)" >&2; exit 2 ;;
esac
