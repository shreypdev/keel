#!/usr/bin/env bash
# rename-keel-to-undra.sh — the mechanical product rename (working name -> Undra).
#
#   scripts/rename-keel-to-undra.sh [--dry-run] [PATH ...]
#
# With no PATH the whole repository is renamed, except the exclusions below. With PATHs
# (files or directories, relative to the current directory) only tracked files under them are
# touched — that is how a branch is brought across the rename after it merged `main`, and how
# an area the default run leaves alone (`site/blog`) is renamed on request. Naming a PATH lifts
# the default-only exclusion of `site/`; the immutable-history exclusions always apply.
#
# What it does, in order:
#   (a) `git mv` every tracked path whose name carries the old name (keel / Keel / KEEL),
#       file by file, so untouched siblings and ignored build output stay where they were;
#   (b) applies the ordered content rules below with `perl -pi` to every tracked TEXT file;
#   (c) prints a summary.
#
# Idempotent: the rules never produce text they match, so a second run is a no-op. It works on
# tracked files only: `git add` new files first (untracked ones that need the rename are
# listed as a warning). Lockfiles are not rewritten — regenerate them (`cargo build`,
# `npm install`). Written for bash 3.2 (the macOS default) and perl 5.
#
# Ordered content rules (earlier rules are more specific; the last three are the general case):
#   @keel/ -> @undra/            npm scope
#   dev.keel -> dev.undra        Kotlin packages, Android application ids
#   dev/keel/ -> dev/undra/      Kotlin source paths
#   Java_dev_keel_ -> Java_dev_undra_   JNI symbols
#   KeelRuntime -> UndraRuntime  Swift module / package / targets
#   Keel -> Undra, KEEL -> UNDRA, keel -> undra   everything else
# plus pre-rules the general case would get wrong (keel.dev is someone else's site and undra.dev
# is not ours either, so a domain is never derived from the name; links go to the Pages site):
#   https://keel.dev/errors/ -> https://shreypdev.github.io/undra/docs/errors.html#
#       (so a diagnostics link reads .../docs/errors.html#E0007)
#   https://keel.dev/errors  -> https://shreypdev.github.io/undra/docs/errors.html
#   "[keel.dev docs & site"  -> "[Docs & site"        (the README label)
#   any other keel.dev       -> https://shreypdev.github.io/undra/
# and a grammar rule after the generic ones: "Undra" takes "an" ("a Keel project" -> "an Undra
# project"), also across a wrapped line and in front of code spans and identifiers.
# One protection: this script's own file name is never rewritten in prose.
#
# NOT touched (by design):
#   site/ and the launch-v2 planning records .10x/specs/** and .10x/decisions/*/launch-v2.md
#   (they describe the rename, so "Keel -> Undra" would become nonsense; default run only —
#   naming them lifts this) · .10x/reviews/** · .10x/adrs/ADR-018 … ADR-030 (immutable
#   history; ADR-030 states the mapping) · this script · .10x/decisions/sde/rename-undra.md
#   (the record of this rename names the old name on purpose) · Cargo.lock, package-lock.json
#   (paths of the lockfiles still move) · symlinks' targets · binary files · target/, node_modules/, .git/.
#
# Deliberately left as they are, although they spell the old name: the four envelope magic bytes
# 4B 45 45 4C ("K E E L") are part of the wire format and stay frozen (changing them is a wire
# change and needs an ADR), so they are written as hex/bytes, never as the word, and this
# script has nothing to rewrite there.

set -euo pipefail

DRY_RUN=0
ARGS=()
while [ $# -gt 0 ]; do
  case "$1" in
    -n|--dry-run) DRY_RUN=1 ;;
    -h|--help) sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    --) shift; while [ $# -gt 0 ]; do ARGS+=("$1"); shift; done; break ;;
    -*) echo "rename: unknown option: $1" >&2; exit 2 ;;
    *) ARGS+=("$1") ;;
  esac
  shift
done

START_DIR="$PWD"
ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"

die() { echo "rename: $*" >&2; exit 1; }

# --- path and content rules ----------------------------------------------------------------------

# The new spelling of a repository path (every occurrence, all three cases).
newpath() {
  local p="$1"
  p="${p//keel/undra}"
  p="${p//Keel/Undra}"
  p="${p//KEEL/UNDRA}"
  printf '%s' "$p"
}

# Paths that are never touched, in any mode.
always_excluded() {
  case "$1" in
    .git/*|*/target/*|target/*|*/node_modules/*|node_modules/*) return 0 ;;
    .10x/reviews/*) return 0 ;;
    .10x/adrs/ADR-01[89]-*|.10x/adrs/ADR-02[0-9]-*|.10x/adrs/ADR-030-*) return 0 ;;
    scripts/rename-keel-to-undra.sh) return 0 ;;
    .10x/decisions/sde/rename-undra.md) return 0 ;;
  esac
  return 1
}

# Files that move with their directory but whose content is regenerated, not rewritten.
content_excluded() {
  case "$1" in
    Cargo.lock|*/Cargo.lock|package-lock.json|*/package-lock.json) return 0 ;;
  esac
  return 1
}

# Paths skipped only when no PATH was named.
default_excluded() {
  case "$1" in
    site/*) return 0 ;;
    .10x/specs/*|.10x/decisions/*/launch-v2.md) return 0 ;;
  esac
  return 1
}

# PERL_RULES is the ordered content rule set, applied to each file as a whole (-0777, so the
# article rule can see a wrapped line). Byte semantics (no -C): the rules only ever see ASCII, so
# files in any encoding pass through unchanged.
read -r -d '' PERL_RULES <<'PERL' || true
s{https://keel\.dev/errors/}{https://shreypdev.github.io/undra/docs/errors.html#}g;
s{https://keel\.dev/errors}{https://shreypdev.github.io/undra/docs/errors.html}g;
s{\[keel\.dev docs & site}{[Docs & site}g;
s{\bkeel\.dev\b}{https://shreypdev.github.io/undra/}g;
s{rename-keel-to-undra}{\@\@RENAME_SCRIPT\@\@}g;
s{\@keel/}{\@undra/}g;
s{dev\.keel}{dev.undra}g;
s{dev/keel/}{dev/undra/}g;
s{Java_dev_keel_}{Java_dev_undra_}g;
s{KeelRuntime}{UndraRuntime}g;
s{Keel}{Undra}g;
s{KEEL}{UNDRA}g;
s{keel}{undra}g;
s{\b([Aa])(?=[ \t]+[`"'*_(\[]*(?:Undra|undra|UNDRA))}{$1n}g;
s{\b([Aa])(?=[ \t]*\n[ \t]*(?://[/!]?|\*|\#|-)?[ \t]*[`"'*_(\[]*(?:Undra|undra|UNDRA))}{$1n}g;
s{\@\@RENAME_SCRIPT\@\@}{rename-keel-to-undra}g;
PERL

# --- scope ---------------------------------------------------------------------------------------

# Each PATH as a repo-relative prefix, in its old spelling and in its renamed spelling (so the
# same command line works before and after the rename: idempotency).
SCOPE=()
if [ ${#ARGS[@]} -gt 0 ]; then
  ROOT_P="$(cd "$ROOT" && pwd -P)"
  for a in "${ARGS[@]}"; do
    rel=""
    # The path may be given in either spelling (a run after the rename names the old one).
    for cand in "$a" "$(newpath "$a")"; do
      case "$cand" in /*) abs="$cand" ;; *) abs="$START_DIR/$cand" ;; esac
      if dir="$(cd "$(dirname "$abs")" 2>/dev/null && pwd -P)"; then
        abs="$dir/$(basename "$abs")"
        rel="${abs#"$ROOT_P"/}"
        [ "$rel" != "$abs" ] || die "outside the repository: $a"
        rel="${rel%/}"
        break
      fi
    done
    [ -n "$rel" ] || die "no such path: $a"
    SCOPE+=("$rel")
    n="$(newpath "$rel")"
    [ "$n" = "$rel" ] || SCOPE+=("$n")
  done
fi

in_scope() {
  local p="$1" s
  always_excluded "$p" && return 1
  if [ ${#SCOPE[@]} -eq 0 ]; then
    default_excluded "$p" && return 1
    return 0
  fi
  for s in "${SCOPE[@]}"; do
    case "$p" in "$s"|"$s"/*) return 0 ;; esac
  done
  return 1
}

if [ ${#SCOPE[@]} -gt 0 ]; then
  hit=0
  while IFS= read -r -d '' f; do in_scope "$f" && { hit=1; break; }; done < <(git ls-files -z)
  [ "$hit" = 1 ] || die "no tracked, renameable file under: ${ARGS[*]}"
fi

# --- (a) path moves ------------------------------------------------------------------------------

MOVE_OLD=()
while IFS= read -r -d '' f; do
  case "$f" in *[kK][eE][eE][lL]*) ;; *) continue ;; esac
  in_scope "$f" || continue
  MOVE_OLD+=("$f")
done < <(git ls-files -z)

# Deepest paths first, then pre-flight every destination before the first move so a collision
# aborts with the tree untouched.
SORTED=()
if [ ${#MOVE_OLD[@]} -gt 0 ]; then
  while IFS= read -r line; do SORTED+=("${line#*$'\t'}"); done < <(
    for f in "${MOVE_OLD[@]}"; do
      d="${f//[^\/]/}"
      printf '%05d\t%s\n' "${#d}" "$f"
    done | sort -r -s -k1,1
  )
  for f in "${SORTED[@]}"; do
    n="$(newpath "$f")"
    if [ -e "$n" ] || [ -L "$n" ]; then die "destination exists, refusing to overwrite: $f -> $n"; fi
  done
fi

MOVED=0
OLD_DIRS=()
for f in ${SORTED[@]+"${SORTED[@]}"}; do
  n="$(newpath "$f")"
  if [ "$DRY_RUN" = 1 ]; then
    echo "mv  $f -> $n"
  else
    mkdir -p "$(dirname "$n")"
    git mv -- "$f" "$n"
  fi
  MOVED=$((MOVED + 1))
  OLD_DIRS+=("$(dirname "$f")")
done

# Tidy: drop the emptied old directories; report the ones that still hold ignored/untracked
# content (node_modules, build output) — those are stale and safe to delete by hand.
LEFTOVER=()
if [ "$DRY_RUN" = 0 ] && [ ${#OLD_DIRS[@]} -gt 0 ]; then
  while IFS= read -r d; do
    [ -d "$d" ] || continue
    # walk up while the directory name still carries the old name
    cur="$d"
    while [ "$cur" != "." ] && [ -d "$cur" ]; do
      case "$cur" in *[kK][eE][eE][lL]*) ;; *) break ;; esac
      if rmdir "$cur" 2>/dev/null; then cur="$(dirname "$cur")"; else LEFTOVER+=("$cur"); break; fi
    done
  done < <(printf '%s\n' "${OLD_DIRS[@]}" | sort -u -r)
  # a directory is only a leftover if it is still there once everything is moved
  KEPT=()
  for d in ${LEFTOVER[@]+"${LEFTOVER[@]}"}; do [ -d "$d" ] && KEPT+=("$d"); done
  LEFTOVER=()
  if [ ${#KEPT[@]} -gt 0 ]; then
    while IFS= read -r d; do LEFTOVER+=("$d"); done < <(printf '%s\n' "${KEPT[@]}" | sort -u)
  fi
fi

# --- (b) content ---------------------------------------------------------------------------------

EDIT_LIST="$(mktemp "${TMPDIR:-/tmp}/rename-edit.XXXXXX")"
BINARY_LIST="$(mktemp "${TMPDIR:-/tmp}/rename-bin.XXXXXX")"
trap 'rm -f "$EDIT_LIST" "$BINARY_LIST"' EXIT

EDITED=0
while IFS= read -r -d '' rec; do
  mode="${rec%% *}"
  f="${rec#*$'\t'}"
  [ "$mode" != "120000" ] || continue          # symlink: never rewrite through it
  [ -f "$f" ] || continue                      # (dry run: the moved path does not exist yet)
  in_scope "$f" || continue
  content_excluded "$f" && continue
  if grep -Iqi -- keel "$f" 2>/dev/null; then
    printf '%s\0' "$f" >>"$EDIT_LIST"
    EDITED=$((EDITED + 1))
  elif grep -aqi -- keel "$f" 2>/dev/null; then
    printf '%s\n' "$f" >>"$BINARY_LIST"
  fi
done < <(git ls-files -z -s)

if [ "$DRY_RUN" = 0 ] && [ "$EDITED" -gt 0 ]; then
  xargs -0 perl -0777 -pi -e "$PERL_RULES" <"$EDIT_LIST"
fi

# --- untracked files that would need the rename -------------------------------------------------

UNTRACKED=()
while IFS= read -r -d '' u; do
  in_scope "$u" || continue
  case "$u" in *[kK][eE][eE][lL]*) UNTRACKED+=("$u"); continue ;; esac
  if grep -Iqi -- keel "$u" 2>/dev/null; then UNTRACKED+=("$u"); fi
done < <(git ls-files -z --others --exclude-standard)

# --- (c) summary ---------------------------------------------------------------------------------

mode_note=""
[ "$DRY_RUN" = 1 ] && mode_note=" (dry run: nothing was changed)"
echo "rename: paths moved: $MOVED; files with content rewritten: $EDITED${mode_note}"
if [ ${#LEFTOVER[@]} -gt 0 ]; then
  echo "rename: old directories kept because they still hold untracked/ignored files (stale; delete them):"
  printf '  %s\n' "${LEFTOVER[@]}"
fi
if [ -s "$BINARY_LIST" ]; then
  echo "rename: binary files that mention the old name were left alone:"
  sed 's/^/  /' "$BINARY_LIST"
fi
if [ ${#UNTRACKED[@]} -gt 0 ]; then
  echo "rename: WARNING untracked files were NOT renamed — 'git add' them and run again:"
  printf '  %s\n' "${UNTRACKED[@]}"
fi
