# rantlr — P0 spike

Working name (crates.io naming TBD — `rantlr` was claimed by a third party on
2026-07-18; verify before publishing anything).

A language-building toolchain built around a **language-shape envelope**: any
language designed inside the envelope gets incremental parsing, editor
intelligence, error containment, lossless trees, and parallel cold parsing
**by construction**. The tool refuses grammars outside the envelope, with
counterexamples. Design rationale and research grounding: see the feasibility
report (artifact link in the project notes).

## What this spike proves (`crates/rantlr-lex`)

The two riskiest lexical-layer claims, in ~700 lines of dependency-free Rust:

1. **L1/L2 — line-anchored incremental lexing.** Lexing is a pure function of
   `(line text, entry state)` with a small finite cross-line state
   (`Normal | BlockComment(depth ≤ 8)`). Multi-line constructs live in the
   state, not in line-spanning tokens. After any edit batch, relex damaged
   lines plus a per-site reconvergence run that stops at the first line-start
   state agreement.
2. **Losslessness.** Every byte belongs to exactly one token (trivia included;
   unknown bytes are `Unknown` trivia), tokens store lengths not offsets, and
   `reproduce()` rebuilds the file byte-for-byte — including mixed LF/CRLF/CR,
   missing trailing newline, BOM, and unicode.

Plus a preview of the containment story: a **block skeleton** pass (bracket
matching over token runs, local handling of imbalance — a stray `)` never
unwinds an enclosing block).

## The gates

- **Roundtrip:** `reproduce(lex(text)) == text`, always.
- **Differential:** after every edit batch, the incremental result must be
  *identical* to lexing the current text from scratch. The batch lexer is the
  oracle; `tests/properties.rs` runs 1,600 randomized batches against it.

These gates caught two real bugs during the spike's own development (a
placeholder-state read on insertion-after-replacement, and CR+`\n` seam fusion
at the line-edit layer) — which is the methodology argument in miniature: the
full system keeps `incremental == batch` as a permanent CI gate.

## Measured results (M-series MacBook, release build)

100,000-line / 3.78 MB generated source file, 1.55 M tokens:

| Scenario | Result |
|---|---|
| Cold lex (from scratch) | **18.6 ms** (203 MB/s) |
| Lossless roundtrip | verified, 0.6 ms |
| Block skeleton (108,935 blocks, 10,000 folds) | 1.5 ms |
| **Typical batch: 1,000 scattered single-line edits** | **269 µs** (0.27 µs/site), damage 0.99% of file, **zero reconvergence** |
| **Single keystroke (in-place fast path)** | **0.4 µs median, 0.7 µs p99** |
| Bounded construct (open `/*` + close `*/` 25 lines apart, one batch) | wave stopped after exactly 25 lines |
| Adversarial batch (edits may delete comment delimiters) | 2.7 ms, 33% of file relexed |
| Unbounded pathological (unclosed `/*`, comment-free file) | 5.4 ms to relex 99k lines to EOF |

Interpretation:

- The typical path is effectively free: sub-microsecond per edit site, and a
  keystroke costs less than a microsecond end-to-end.
- The worst case (a state change that genuinely re-scopes the rest of the
  file, e.g. opening an unclosed block comment) is **semantically required**
  work — the tail *is* now a comment and must repaint — and even that is
  ~5 ms on 100k lines, under half a frame. VS Code behaves identically; it
  just can't prove it.
- The adversarial number carries a language-design lesson the tool should
  surface as a lint-adjacent report: **nested** block comments make a deleted
  closer global (nothing below can close it), while C-style non-nested
  comments reconverge at the next `*/`. Damage characteristics are a function
  of grammar choices — the envelope makes them statable.

## Spike caveats (deliberate scope cuts)

- The lexer is hand-written against a demo language; the real tool generates
  a DFA from token specs and verifies L1 (no accepting path through `\n`)
  statically on the automaton.
- Edits are line-granular; the real text layer is char-range edits over a
  rope/piece-tree (which also removes the general path's O(n) Vec rebuild —
  the fast path shows the true incremental cost today).
- The CR+`\n` seam rule and the canonical-form invariant are debug-asserted
  preconditions here; the rope layer owns them for real.
- Skeleton is rebuilt per call (1.5 ms); incremental skeleton maintenance is
  straightforward and deferred.

## Run it

```
cargo test                        # all gates: P0 + P1
cargo run --release --bin bench   # P0 hand-lexer numbers
cargo run --release --bin p1bench # P1 generated-lexer numbers
```

## P1 — increment 1 (shipped)

Two new crates supersede the hand-written approach while leaving the P0
crate untouched as the reference implementation:

- **`crates/rantlr-grammar`** — grammar-as-value core model (`LexGrammar`
  is a plain `Clone + Eq` value: modes, token patterns, trivia/bracket
  roles, keyword specialization, push/pop actions), compiled via Thompson
  NFA → subset-construction DFA over a 132-column classified alphabet,
  then **certified by the envelope lints**: L1 (no token can match across
  a line break — proved on the automaton, violations come with a witness
  string), L2 (mode-push graph acyclic or explicitly bounded — violations
  name the cycle), empty-match rejection. Out-of-envelope grammars don't
  produce a lexer; they produce a counterexample.
- **`crates/rantlr-engine`** — the P0 incremental machinery generalized
  over a `LineLexer` trait (any pure line lexer with a small `Eq` state),
  plus the vocab-driven block skeleton.

**The P1 gate:** the lexer *generated* from the demo-language grammar
value is observationally equivalent to the P0 *hand-written* lexer —
identical non-trivia token streams, identical per-byte trivia coverage,
isomorphic line states — across edge cases, a 5,000-line corpus, and 300
fuzzed documents; and the generic engine re-passes incremental ≡ batch
with the generated lexer. Ten tests, all green.

Generated-lexer numbers on the same 100k-line corpus (vs P0 hand lexer):

| Scenario | P1 generated | P0 hand |
|---|---|---|
| Grammar compile + lints + tables | **305 µs** | — (hand-written) |
| Cold lex | 24.1 ms (157 MB/s) | 18.6 ms (203 MB/s) |
| Typical 1,000-site batch | 265 µs, zero reconvergence | 269 µs |
| Single keystroke | 0.4 µs median | 0.4 µs |
| Certified line-state space | 9 states (bound 8) | by inspection |
| DFA sizes | 24 + 6 states | — |

The generated DFA is ~30% slower than the specialized hand loop on cold
lex (table indirection; row packing and alphabet-class compression are
known, deferred optimizations) and identical everywhere it matters
incrementally. The 305 µs compile time is the headline: **grammar
hot-reload is real** — edit the language, recompile tables + relint, and
open documents can relex incrementally within the same frame.

## P1 — increment 2 (shipped)

The syntax tier, same philosophy: `SynGrammar` is a first-class value
(nonterminals, productions, declarative precedence/associativity — the
envelope's only disambiguation mechanism), compiled to **canonical LR(1)
tables** whose surviving conflicts are reported as **counterexamples**:
the conflicting items with dots, plus an example input built from the
shortest path to the conflict state with nonterminals expanded to their
shortest terminal yields. The classic dangling-else grammar reports
exactly:

```
shift/reduce on ELSE:  IF IF X · ELSE
  [s → IF s · , ELSE]        (reduce the inner if)
  [s → IF s · ELSE s , …]    (shift the else)
```

and is resolved by declaring `ELSE` to bind tighter — zero conflicts,
resolution counted. A table-driven batch parser runs over the generated
lexer's non-trivia tokens (trees rendered as s-expressions for goldens),
and syntax errors carry **expected-token sets straight from the ACTION
row** — the completion primitive, exercised from day one:

```
let x = ;   →  error at `;`: expected IDENT, LBRACKET, LPAREN, NUMBER, STRING
```

Demo-language numbers: 23 productions → 135 canonical LR(1) states,
64 shift/reduce conflicts silently resolved by precedence declarations,
6 ms table build (lex + syntax certification together: <7 ms — the
hot-reload budget holds). End-to-end tests parse through the full
generated pipeline (lexer → incremental buffer → parser), including
multi-line comments dissolving into trivia invisibly to the parser.

Deliberate scope notes: canonical LR(1) (Pager/IELR state-merging is a
deferred optimization — state counts are small at DSL scale); traces are
shortest-path examples, not full unifying derivations (Isradisaikul &
Myers 2015 is the documented upgrade path).

## P1 — increment 3 (shipped)

**Lossless green trees.** Rowan-lineage immutable trees built from the LR
parse: kind = (nonterminal, production), byte widths, `Arc` children (the
seam P2's incremental reuse splices through), and tokens carrying text +
trivia flags — so trees are self-contained and `tree.text()` reproduces
the source **byte-for-byte**, comments, mixed CRLF/CR/LF line endings
(woven back as synthetic NEWLINE trivia), unicode and all. Trivia policy
(deterministic, documented): a trivia run attaches as siblings before the
next non-trivia token; richer ownership views are derived layers.
`ancestor_spans` / `token_at_offset` provide the LSP `selectionRange`
primitive — the nesting invariant is property-tested at every offset.

**Typed AST codegen.** `cargo run -p rantlr-grammar --bin astgen`
regenerates `demo_ast.rs` (checked in, 698 lines) from the grammar
*value*: an enum per multi-production nonterminal, a struct per named
production, an accessor per RHS symbol (trivia-transparent, token ids
inlined). A **drift test** asserts the checked-in file matches
regeneration — so a grammar change fails CI first, and regenerating
surfaces every downstream use that no longer typechecks: *ramification
as compile errors*, now enforced rather than promised.

Scale numbers (100k-line statement corpus, 3.1 MB): batch parse 145 ms
(1.05M terminals; HashMap action rows — dense-row packing is a known
deferred optimization), green tree build 76 ms (998k nodes / 1.68M
tokens incl. trivia), byte-identical `text()` in 14 ms.

**A finding worth keeping:** the corpus's `stmts → stmts stmt` left
recursion produces a 100k-deep tree spine, which overflows default
stacks in recursive walks — Wagner's balanced-sequence precondition
(envelope L4) demonstrating itself empirically. The bench runs that
section on a rustc-style big-stack thread until P2's auto-balanced lists
make trees log-depth and retire the workaround.

## P2 — increment 1 (shipped): the Wagner incremental parser

Sentential-form incremental LR parsing over the green trees
(`incremental.rs` + `IncSession` in the engine): after an edit batch, the
lexer's damage regions map to old-token intervals; a salvage walk emits
maximal clean subtrees interleaved with fresh tokens; the parser
**splices whole subtrees via GOTO** (Wagner's nonterminal shift), breaks
down dirty/unshiftable nodes, uses leftmost-terminal lookahead for
reduces, and — critically — **breaks down subtrees rooted at FRAGILE
productions** (those shaped by precedence resolution, Wagner §6). The
test suite constructs the exact wrong-splice scenario this prevents:
editing `1 + 2\n+ 9` to `1 + 2\n* 9` must NOT splice the old `Add(1,2)`
into `Mul(Add(1,2),9)` — the gate + a typed-AST shape assertion prove it
re-associates to `Add(1, Mul(2,9))`.

**The gate is FULL TREE EQUALITY** — incremental result equals a
from-scratch batch parse, structure *and* trivia placement (pending
trivia is injected down spliced subtrees' left spines to match the batch
builder byte-for-byte). Held across single edits, multi-site batches,
insertions/deletions, comment-only edits (100% terminal reuse,
trivia-local), block-comment state waves, error→invalidate→batch
recovery, 300 fuzz rounds, and a 700-edit session on the 100k-line
corpus. The gate caught two real bugs during development (an eager
reduce that destroyed splice opportunities; transposed trivia at splice
boundaries) — the methodology keeps paying.

Numbers (100k-line corpus): **near-EOF edit 88 µs median** — the true
spine-free incremental cost; mid-file edit 25 ms median with ~50k
splices — the **unbalanced left-recursive list tax** (Wagner §7's
predicted linear degeneration, measured precisely: each suffix statement
re-wraps through `StmtsMore`). Also fixed en route: two hidden O(n)s
(eager leftmost-terminal computation descending the whole spine;
full per-line count recomputation per edit).

## Next (P2 continued)

**L4 auto-balanced sequences** — the tool detects list-shaped rules and
represents repetition as balanced tree runs, turning the 25 ms mid-file
tax into O(damage + log n) and retiring the big-stack thread; then
top-down reuse + Wagner's optimality postpass, and error recovery.
