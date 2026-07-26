#!/usr/bin/env bash
#
# rename.sh — rename this project, everywhere, in one command.
#
# The project name is woven through two repositories: this one (crate
# directories, package names, Rust module paths, prose, the CLI binary,
# the LSP server identity, the VS Code language id) and Synkro, which
# reaches in here by RELATIVE PATH. A rename that touches only one of
# them leaves a broken build, so this script drives both together and
# refuses to report success until zero traces of the old name survive.
#
#   ./tools/rename.sh --to NAME [--ext EXT] [options]
#
# Options:
#   --to NAME       New project name. Lowercase kebab, e.g. `foo`, `red-oak`.
#                   A single word is strongly preferred (see NOTE below).
#   --ext EXT       Also rename the grammar-file extension and its
#                   identifier family: `.rg` -> `.EXT`, `RgToolchain` ->
#                   `ExtToolchain`, `rg_ast` -> `ext_ast`, crate
#                   `<name>-rg` -> `<name>-EXT`. Independent decision;
#                   omit to keep `.rg` exactly as it is.
#   --dir-name D    Directory the repo will live in (default: NAME).
#                   Synkro's path deps are pointed here.
#   --keep-dir      Leave the repo directory named `rantlr`. Implies
#                   --dir-name rantlr.
#   --synkro PATH   Path to the Synkro repo (default: ../synkro).
#   --dry-run       Report what would change; modify nothing.
#   --allow-dirty   Skip the clean-worktree check. Not recommended: a
#                   clean tree is what makes this reversible with a
#                   single `git checkout .`.
#
# NOTE on multi-word names: a hyphenated name works, but it produces
# mixed separators across the surfaces that need them (crate `red-oak-sem`,
# Rust path `red_oak_sem`, type `RedOakLang`, env `CARGO_BIN_EXE_red-oak`).
# All of that is handled correctly below, but a single word is cleaner to
# live with and to say out loud.
#
# The script is idempotent: running it twice with the same --to is a no-op
# the second time, because the audit's definition of done is "no `rantlr`
# anywhere", and that is already true.

set -euo pipefail

OLD=rantlr

# ---------------------------------------------------------------------------
# Arguments
# ---------------------------------------------------------------------------

NEW=""
EXT=""
DIRNAME=""
KEEP_DIR=0
SYNKRO=""
DRY=0
ALLOW_DIRTY=0
LIMN_TO=""
LIMN_TRAIT=""

die() { printf '\033[31merror:\033[0m %s\n' "$*" >&2; exit 1; }
note() { printf '\033[36m%s\033[0m\n' "$*"; }
ok()   { printf '\033[32m%s\033[0m\n' "$*"; }
warn() { printf '\033[33mwarning:\033[0m %s\n' "$*" >&2; }

while [ $# -gt 0 ]; do
  case "$1" in
    --to)          NEW="${2:-}"; shift 2 ;;
    --ext)         EXT="${2:-}"; shift 2 ;;
    --dir-name)    DIRNAME="${2:-}"; shift 2 ;;
    --keep-dir)    KEEP_DIR=1; shift ;;
    --synkro)      SYNKRO="${2:-}"; shift 2 ;;
    --limn-to)     LIMN_TO="${2:-}"; shift 2 ;;
    --limn-trait)  LIMN_TRAIT="${2:-}"; shift 2 ;;
    --dry-run)     DRY=1; shift ;;
    --allow-dirty) ALLOW_DIRTY=1; shift ;;
    -h|--help)     sed -n '2,/^set -euo/p' "$0" | sed 's/^# \{0,1\}//;$d'; exit 0 ;;
    *)             die "unknown argument: $1" ;;
  esac
done

[ -n "$NEW" ] || die "--to NAME is required (try --help)"

printf '%s' "$NEW" | grep -qE '^[a-z][a-z0-9]*(-[a-z0-9]+)*$' \
  || die "'$NEW' is not a valid crate name: lowercase letters/digits, hyphen-separated, must start with a letter"

[ "$NEW" != "$OLD" ] || die "--to is the current name; nothing to do"

if [ -n "$EXT" ]; then
  printf '%s' "$EXT" | grep -qE '^[a-z][a-z0-9]*$' \
    || die "'$EXT' is not a valid file extension: lowercase letters/digits, must start with a letter"
fi

if [ "$KEEP_DIR" = 1 ]; then
  DIRNAME="$OLD"
elif [ -z "$DIRNAME" ]; then
  DIRNAME="$NEW"
fi

# ---------------------------------------------------------------------------
# Derived name forms
#
# The name appears in four shapes, and each surface wants a specific one.
# Deriving all four from a single input is what keeps them from drifting.
# ---------------------------------------------------------------------------

KEBAB="$NEW"                                          # crates, CLI, prose
SNAKE="${NEW//-/_}"                                   # Rust module paths
UPPER="$(printf '%s' "$SNAKE" | tr '[:lower:]' '[:upper:]')"   # consts, env
CAMEL="$(printf '%s' "$NEW" | awk -F- '{for(i=1;i<=NF;i++) printf toupper(substr($i,1,1)) substr($i,2)}')"

EXT_LOWER="$EXT"
EXT_CAMEL=""
if [ -n "$EXT" ]; then
  EXT_CAMEL="$(printf '%s' "$EXT" | awk '{print toupper(substr($0,1,1)) substr($0,2)}')"
fi

# The protocol crate is named independently of the project — it is a
# standalone artifact that Synkro depends on WITHOUT depending on this
# project, which was the whole point of splitting it out. So it gets its
# own optional rename. The trait defaults to the agent noun (`limn` ->
# `Limner`, `tint` -> `Tinter`); override when that reads badly.
LIMN_CAMEL=""
LIMN_UPPER=""
if [ -n "$LIMN_TO" ]; then
  printf '%s' "$LIMN_TO" | grep -qE '^[a-z][a-z0-9]*(-[a-z0-9]+)*$' \
    || die "'$LIMN_TO' is not a valid crate name"
  LIMN_CAMEL="$(printf '%s' "$LIMN_TO" | awk -F- '{for(i=1;i<=NF;i++) printf toupper(substr($i,1,1)) substr($i,2)}')"
  LIMN_UPPER="$(printf '%s' "${LIMN_TO//-/_}" | tr '[:lower:]' '[:upper:]')"
  [ -n "$LIMN_TRAIT" ] || LIMN_TRAIT="${LIMN_CAMEL}er"
fi
LIMN_TRAIT_LOWER="$(printf '%s' "$LIMN_TRAIT" | tr '[:upper:]' '[:lower:]')"

# ---------------------------------------------------------------------------
# Locate the repos
# ---------------------------------------------------------------------------

HERE="$(cd "$(dirname "$0")/.." && pwd)"
[ -f "$HERE/Cargo.toml" ] || die "cannot find the project root from $0"

if [ -z "$SYNKRO" ]; then SYNKRO="$(dirname "$HERE")/synkro"; fi

HAVE_SYNKRO=0
if [ -d "$SYNKRO/.git" ]; then
  HAVE_SYNKRO=1
else
  warn "Synkro not found at $SYNKRO — skipping it. Its path deps into this repo will break."
  warn "Re-run with --synkro PATH, or fix Synkro by hand afterwards."
fi

check_clean() { # $1 = repo
  [ "$ALLOW_DIRTY" = 1 ] && return 0
  [ -z "$(git -C "$1" status --porcelain)" ] \
    || die "$1 has uncommitted changes. Commit or stash first so this rename is revertable with 'git checkout .' (or pass --allow-dirty)."
}

# ---------------------------------------------------------------------------
# The substitution program
#
# Order matters. The suffixed forms (`rantlr_x`, `rantlr-x`) must be
# rewritten before the bare form, or the bare rule would consume their
# prefix and leave the wrong separator behind.
#
# Synkro's path dependencies are stashed behind a sentinel first, because
# the directory the repo LIVES in is a separate choice from the project
# name (--keep-dir), and the general rules cannot tell the two apart.
# ---------------------------------------------------------------------------

DIRSENT=$'\001RENAME_DIR_SENTINEL\001'

build_perl_program() {
  cat <<PERL
    # -- 0. stash cross-repo path deps behind a sentinel -------------------
    s{\Q$OLD\E/crates/}{${DIRSENT}/crates/}g;

    # -- 1. compound forms, longest-separator-first ------------------------
    s{\bsynkro_\Q$OLD\E\b}{synkro_${SNAKE}}g;
    s{\Q$OLD\E_}{${SNAKE}_}g;
    s{\Q$OLD\E-}{${KEBAB}-}g;

    # -- 2. case variants (case-sensitive, so these cannot collide) --------
    #    Unanchored for the same reason as the limn rules: an identifier
    #    like FOO_RANTLR has no word boundary before the name.
    s{Rantlr}{${CAMEL}}g;
    s{RANTLR}{${UPPER}}g;

    # -- 3. the bare form, last --------------------------------------------
    s{\Q$OLD\E}{${KEBAB}}g;

    # -- 4. restore the path-dep directory ---------------------------------
    s{\Q${DIRSENT}\E}{${DIRNAME}}g;
PERL
}

build_ext_program() {
  [ -n "$EXT" ] || return 0
  # These stay anchored, unlike the name rules: `rg` is two characters, and
  # an unanchored `s{rg}{zg}` would turn "large" into "lazge" and "merge"
  # into "mezge". The cost of anchoring is that `\b` treats `_` as a word
  # character, so the Cargo path form `<name>_rg` needs its own rule — the
  # package name `<name>-rg` matches `\brg\b` (hyphen is a boundary) but
  # the Rust module path `<name>_rg` does not, and renaming one without
  # the other unlinks the crate.
  cat <<PERL
    s{\\.rg\b}{.${EXT_LOWER}}g;
    s{_rg\b}{_${EXT_LOWER}}g;
    s{\bRg}{${EXT_CAMEL}}g;
    s{\brg_}{${EXT_LOWER}_}g;
    s{\brg2}{${EXT_LOWER}2}g;
    s{\brg\b}{${EXT_LOWER}}g;
PERL
}

build_limn_program() {
  [ -n "$LIMN_TO" ] || return 0
  # `Limner` before `Limn`, or an overridden trait name would be clobbered
  # by the generic prefix rule.
  # No \b anchors here. `_` is a word character, so `\bLIMN` does not
  # match inside `CHECK_LIMN` — and identifiers that embed the name after
  # an underscore are exactly the ones a rename must not miss. `limn` is
  # distinctive enough that an unanchored match is safe; the only English
  # word containing it is "limnology", which this codebase has no reason
  # to mention. Contrast the `rg` rules below, which MUST stay anchored:
  # unanchored, they would turn "large" into "lazge".
  cat <<PERL
    s{Limner}{${LIMN_TRAIT}}g;
    s{LIMN}{${LIMN_UPPER}}g;
    s{Limn}{${LIMN_CAMEL}}g;
    s{limner}{${LIMN_TRAIT_LOWER}}g;
    s{limn}{${LIMN_TO}}g;
PERL
}

PROGRAM="$(build_perl_program)$(build_ext_program)$(build_limn_program)"

# ---------------------------------------------------------------------------
# Dry run: report, change nothing
# ---------------------------------------------------------------------------

if [ "$DRY" = 1 ]; then
  note "DRY RUN — nothing will be modified."
  echo
  echo "  name forms"
  printf '    kebab (crates, CLI)   %s\n' "$KEBAB"
  printf '    snake (Rust paths)    %s\n' "$SNAKE"
  printf '    camel (types)         %s\n' "$CAMEL"
  printf '    upper (consts)        %s\n' "$UPPER"
  printf '    repo directory        %s\n' "$DIRNAME"
  if [ -n "$EXT" ]; then
    printf '    grammar extension     .rg -> .%s  (%s* identifiers)\n' "$EXT_LOWER" "$EXT_CAMEL"
  else
    printf '    grammar extension     .rg (unchanged)\n'
  fi
  if [ -n "$LIMN_TO" ]; then
    printf '    protocol crate        limn -> %s  (trait Limner -> %s)\n' "$LIMN_TO" "$LIMN_TRAIT"
  else
    printf '    protocol crate        limn (unchanged)\n'
  fi
  echo
  for repo in "$HERE" $([ "$HAVE_SYNKRO" = 1 ] && echo "$SYNKRO"); do
    files=$(git -C "$repo" grep -Iil "$OLD" -- . | wc -l | tr -d ' ')
    hits=$(git -C "$repo" grep -Iio "$OLD" -- . | wc -l | tr -d ' ')
    printf '  %-52s %4s hits in %3s files\n' "$(basename "$repo")" "$hits" "$files"
  done
  echo
  echo "  directories that would move"
  for d in $(git -C "$HERE" ls-files | grep -o "^crates/$OLD-[a-z]*" | sort -u); do
    printf '    %s -> %s\n' "$d" "$(printf '%s' "$d" | sed "s/$OLD/$KEBAB/")"
  done
  [ "$HAVE_SYNKRO" = 1 ] && printf '    (synkro) synkro_%s -> synkro_%s\n' "$OLD" "$SNAKE"
  if [ -n "$EXT" ]; then
    echo
    echo "  files that would be renamed"
    # Same basename transform the real run uses, so the preview cannot
    # promise something different from what happens.
    _p="$(build_ext_program)"
    while IFS= read -r f; do
      _b="$(basename "$f")"
      _n="$(printf '%s' "$_b" | perl -pe "$_p")"
      [ "$_n" = "$_b" ] || printf '    %s -> %s\n' "$f" "$(dirname "$f")/$_n"
    done < <(git -C "$HERE" ls-files)
  fi
  echo
  [ "$KEEP_DIR" = 1 ] || printf '  repo directory would move: %s -> %s\n' "$HERE" "$(dirname "$HERE")/$DIRNAME"
  exit 0
fi

# ---------------------------------------------------------------------------
# Execute
# ---------------------------------------------------------------------------

check_clean "$HERE"
[ "$HAVE_SYNKRO" = 1 ] && check_clean "$SYNKRO"

# This script names the old project on purpose — it is the one file that
# must keep saying `rantlr` for its own patterns to work. It is also the
# file bash is reading as it runs, and bash reads scripts incrementally:
# rewriting it mid-execution would make the interpreter resume inside
# shifted bytes. Excluded from both the rewrite and the audit.
SELF_REL="tools/$(basename "$0")"

rewrite_repo() { # $1 = repo
  local repo="$1" n=0
  # `git grep -Il ''` lists every tracked file git considers text —
  # binaries are excluded automatically, so no extension allowlist to
  # keep in sync.
  while IFS= read -r f; do
    [ "$f" = "$SELF_REL" ] && continue
    perl -i -pe "$PROGRAM" "$repo/$f"
    n=$((n + 1))
  done < <(git -C "$repo" grep -Il '' -- .)
  printf '    rewrote %s tracked text files\n' "$n"
}

move_dirs() { # $1 = repo
  local repo="$1" d target
  for d in $(git -C "$repo" ls-files | grep -oE "^crates/$OLD-[a-z]+" | sort -u); do
    target="$(printf '%s' "$d" | sed "s/$OLD/$KEBAB/")"
    git -C "$repo" mv "$d" "$target"
    printf '    %s -> %s\n' "$d" "$target"
  done
  # Plain directory tests, NOT `ls-files | grep -q`. Under `pipefail`,
  # `grep -q` exits on first match and closes the pipe; `git ls-files`
  # then dies of SIGPIPE and the pipeline reports failure even though the
  # match succeeded. That only bites once ls-files output exceeds the
  # 64K pipe buffer, so it silently skips large repos and works on small
  # ones — which is exactly how it got past a first reading.
  if [ -d "$repo/synkro_$OLD" ]; then
    git -C "$repo" mv "synkro_$OLD" "synkro_$SNAKE"
    printf '    synkro_%s -> synkro_%s\n' "$OLD" "$SNAKE"
  fi
  if [ -n "$LIMN_TO" ] && [ -d "$repo/crates/limn" ]; then
    git -C "$repo" mv crates/limn "crates/$LIMN_TO"
    printf '    crates/limn -> crates/%s\n' "$LIMN_TO"
  fi
}

rename_ext_dirs() { # $1 = repo
  [ -n "$EXT" ] || return 0
  local repo="$1" d base newbase prog
  prog="$(build_ext_program)"
  # Directories carry the name too — `tree-sitter/rg/`, and the crate dir
  # `<name>-rg` itself. Renaming files without them leaves `include_str!`
  # pointing into a directory that no longer exists under that name.
  # Deepest-first, so renaming a parent never invalidates a path still
  # queued for its children.
  while IFS= read -r d; do
    base="$(basename "$d")"
    newbase="$(printf '%s' "$base" | perl -pe "$prog")"
    [ "$newbase" = "$base" ] && continue
    git -C "$repo" mv "$d" "$(dirname "$d")/$newbase"
    printf '    %s/ -> %s/%s/\n' "$d" "$(dirname "$d")" "$newbase"
  done < <(git -C "$repo" ls-files | grep '/' | sed 's|/[^/]*$||' | sort -u \
             | awk -F/ '{print NF, $0}' | sort -k1,1rn | cut -d' ' -f2-)
}

rename_grammar_files() { # $1 = repo
  [ -n "$EXT" ] || return 0
  local repo="$1" f dir base newbase prog
  prog="$(build_ext_program)"
  # Rename by applying the extension rules to each file's BASENAME, not
  # just its suffix. The identifier rules rewrite `rg_ast` to `zg_ast`
  # inside `pub mod rg_ast;` — if the file `rg_ast.rs` does not move with
  # it, the module simply ceases to exist and the crate stops compiling.
  # Same for `rg2ts.rs`, `rg_astgen.rs`, and `rg.rg`, whose stem names the
  # language and so becomes `zg.zg`, not `rg.zg`.
  while IFS= read -r f; do
    dir="$(dirname "$f")"
    base="$(basename "$f")"
    newbase="$(printf '%s' "$base" | perl -pe "$prog")"
    [ "$newbase" = "$base" ] && continue
    git -C "$repo" mv "$f" "$dir/$newbase"
    printf '    %s -> %s/%s\n' "$f" "$dir" "$newbase"
  done < <(git -C "$repo" ls-files)
}

note "1/5  rewriting file contents"
rewrite_repo "$HERE"
[ "$HAVE_SYNKRO" = 1 ] && rewrite_repo "$SYNKRO"

note "2/5  moving directories"
move_dirs "$HERE"
[ "$HAVE_SYNKRO" = 1 ] && move_dirs "$SYNKRO"

if [ -n "$EXT" ]; then
  note "3/5  renaming grammar files"
  # Directories before files: the file pass reads paths from git, and
  # those paths must already reflect any directory that moved.
  rename_ext_dirs "$HERE"
  rename_grammar_files "$HERE"
else
  note "3/5  grammar extension unchanged (.rg)"
fi

note "4/5  regenerating lockfiles"
( cd "$HERE" && cargo metadata --format-version 1 >/dev/null 2>&1 ) \
  && printf '    %s ok\n' "$(basename "$HERE")" \
  || warn "cargo metadata failed in $HERE — inspect before committing"
if [ "$HAVE_SYNKRO" = 1 ]; then
  # Synkro's path deps point at the NEW directory name, which does not
  # exist yet. Its lockfile is regenerated after the directory moves.
  printf '    synkro deferred until the directory move\n'
fi

# ---------------------------------------------------------------------------
# 5. Audit — the definition of done
#
# Not "the tests pass" (they can pass with the old name still in prose),
# and not "it built" (a stale doc comment builds fine). Done is: the old
# name does not appear in any tracked file, in any tracked path, in either
# repo. Anything less and the rename is half-applied.
# ---------------------------------------------------------------------------

note "5/5  auditing"
FAIL=0
audit_repo() { # $1 = repo, $2 = label
  local repo="$1" label="$2" content paths pat="$OLD"
  [ -n "$LIMN_TO" ] && pat="$OLD\|limn"
  content=$(git -C "$repo" grep -Iin "$pat" -- . ":(exclude)$SELF_REL" || true)
  paths=$(git -C "$repo" ls-files | grep -i "$pat" || true)
  # Under --keep-dir the repo intentionally stays at its old directory
  # name, so Synkro's cross-repo path deps must keep pointing there. Those
  # are the one legitimate survivor; everything else still has to go.
  if [ "$KEEP_DIR" = 1 ]; then
    content=$(printf '%s' "$content" | grep -v "$OLD/crates/" || true)
  fi
  if [ -n "$content" ]; then
    printf '\033[31m  %s: old name survives in file contents:\033[0m\n' "$label"
    printf '%s\n' "$content" | head -20 | sed 's/^/    /'
    FAIL=1
  fi
  if [ -n "$paths" ]; then
    printf '\033[31m  %s: old name survives in tracked paths:\033[0m\n' "$label"
    printf '%s\n' "$paths" | head -20 | sed 's/^/    /'
    FAIL=1
  fi
  [ -z "$content$paths" ] && printf '    %s clean\n' "$label"
}
audit_repo "$HERE" "$(basename "$HERE")"
[ "$HAVE_SYNKRO" = 1 ] && audit_repo "$SYNKRO" synkro

if [ -n "$EXT" ]; then
  leftover=$(git -C "$HERE" ls-files '*.rg' || true)
  if [ -n "$leftover" ]; then
    printf '\033[31m  .rg files survive:\033[0m\n%s\n' "$leftover" | sed 's/^/    /'
    FAIL=1
  fi
fi

[ "$FAIL" = 0 ] || die "audit failed — the rename is incomplete. 'git checkout .' in both repos to revert."
ok "  audit clean: no trace of '$OLD' in either repo"
echo "    ($SELF_REL still names the old project — it is spent now; 'git rm' it.)"

# ---------------------------------------------------------------------------
# The directory move, last, because it invalidates every relative path
# (including this script's own) the moment it happens.
# ---------------------------------------------------------------------------

echo
if [ "$KEEP_DIR" = 1 ]; then
  ok "Done. Repo directory left as '$OLD' (--keep-dir)."
  echo "Next: cargo test --workspace, in both repos."
else
  NEWPATH="$(dirname "$HERE")/$DIRNAME"
  [ -e "$NEWPATH" ] && die "cannot move repo: $NEWPATH already exists"
  mv "$HERE" "$NEWPATH"
  ok "Done. Repo moved to $NEWPATH"
  echo
  warn "Your shell and any editor session are still pointed at the old path."
  echo "  Next:"
  echo "    cd $NEWPATH && cargo test --workspace"
  echo "    cd $SYNKRO && cargo test --workspace   # regenerates its lockfile"
  echo
  # Claude Code keys per-project memory by the working directory, with
  # slashes flattened to dashes. Moving the directory orphans it: the
  # notes are still on disk, but no future session will look there.
  OLDKEY="$(printf '%s' "$HERE"    | tr / -)"
  NEWKEY="$(printf '%s' "$NEWPATH" | tr / -)"
  if [ -d "$HOME/.claude/projects/$OLDKEY" ]; then
    warn "Project memory is keyed by path and will not follow the move."
    echo "    mv ~/.claude/projects/$OLDKEY \\"
    echo "       ~/.claude/projects/$NEWKEY"
  fi
fi
