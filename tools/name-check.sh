#!/usr/bin/env bash
#
# name-check.sh — clear a candidate project name against crates.io.
#
# A project name is not one name. Publishing this workspace claims nine
# entries in a single global namespace, and a candidate that is free as a
# base name but taken as `<name>-grammar` is not actually available: the
# family has to be free together, or the crates end up named inconsistently
# forever.
#
#   ./tools/name-check.sh quarry lodestar red-oak
#
# Reports, per candidate, which of the family are free (.), taken (X), or
# reserved-but-empty (~ — an unpublished placeholder, which still blocks
# you). Exit status is 0 if at least one candidate is fully clear.
#
# crates.io is the binding constraint because it is global and
# first-come. GitHub is not checked: the repo lives under your own
# account, where you only collide with yourself.

set -euo pipefail

# The workspace as it stands. `limn` is deliberately absent — it is a
# standalone protocol crate with its own name, independent of whatever
# this project ends up called. Check it separately with --limn.
SUFFIXES=(lex grammar engine services lsp sem rg cli)

UA="name-check (local availability check)"
API="https://crates.io/api/v1/crates"

die() { printf '\033[31merror:\033[0m %s\n' "$*" >&2; exit 1; }

command -v curl >/dev/null || die "curl is required"

CHECK_LIMN=0
CANDIDATES=()
for a in "$@"; do
  case "$a" in
    --limn) CHECK_LIMN=1 ;;
    -h|--help) sed -n '2,/^set -euo/p' "$0" | sed 's/^# \{0,1\}//;$d'; exit 0 ;;
    *) CANDIDATES+=("$a") ;;
  esac
done

# `status` prints one of: free / taken / reserved
# crates.io answers 404 for a name nobody has claimed. A 200 with zero
# versions means the name is reserved by an empty publish — still taken
# for our purposes, but worth distinguishing because it usually means an
# abandoned placeholder rather than a real project.
status() {
  local name="$1" body code
  body=$(curl -sS -A "$UA" -w '\n%{http_code}' "$API/$name" 2>/dev/null || true)
  code=$(printf '%s' "$body" | tail -1)
  case "$code" in
    404) printf 'free' ;;
    200)
      if printf '%s' "$body" | grep -q '"versions":\[\]'; then printf 'reserved'
      else printf 'taken'; fi ;;
    *) printf 'unknown(%s)' "$code" ;;
  esac
  sleep 0.35   # crates.io asks for a modest request rate; be a good citizen
}

glyph() {
  case "$1" in
    free)     printf '\033[32m.\033[0m' ;;
    taken)    printf '\033[31mX\033[0m' ;;
    reserved) printf '\033[33m~\033[0m' ;;
    *)        printf '\033[35m?\033[0m' ;;
  esac
}

if [ "$CHECK_LIMN" = 1 ]; then
  s=$(status limn)
  printf 'limn  %s  %s\n\n' "$(glyph "$s")" "$s"
fi

[ ${#CANDIDATES[@]} -gt 0 ] || die "give me at least one candidate name (try --help)"

printf '%-16s %-6s' "candidate" "base"
for s in "${SUFFIXES[@]}"; do printf ' %-9s' "-$s"; done
echo

ANY_CLEAR=1
for c in "${CANDIDATES[@]}"; do
  printf '%s' "$c" | grep -qE '^[a-z][a-z0-9]*(-[a-z0-9]+)*$' \
    || { printf '%-16s  invalid crate name\n' "$c"; continue; }

  clear=1
  printf '%-16s' "$c"
  s=$(status "$c"); [ "$s" = free ] || clear=0
  printf ' %-6s' "$(glyph "$s")"
  for suf in "${SUFFIXES[@]}"; do
    s=$(status "$c-$suf"); [ "$s" = free ] || clear=0
    printf ' %-9s' "$(glyph "$s")"
  done
  if [ "$clear" = 1 ]; then
    printf '  \033[32mCLEAR\033[0m\n'
    ANY_CLEAR=0
  else
    printf '  \033[31mblocked\033[0m\n'
  fi
done

echo
echo "  . free    X taken    ~ reserved (published but empty — still blocks)    ? lookup failed"
exit $ANY_CLEAR
