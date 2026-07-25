# rantlr

Working name (crates.io naming TBD — `rantlr` was claimed by a third party on
2026-07-18; verify before publishing anything).

A language-building toolchain built around a **language-shape envelope**: any
language designed inside the envelope gets incremental parsing, editor
intelligence, error containment, lossless trees, and parallel cold parsing
**by construction**. The tool refuses grammars outside the envelope, with
counterexamples. Design rationale and research grounding: see the feasibility
report (artifact link in the project notes).

## Quick start

```bash
cargo build --release
cargo install --path crates/rantlr-cli    # puts `rantlr` on your PATH
rantlr new mylang --name Mylang --ext .my
rantlr check mylang/mylang.rg
rantlr parse mylang/mylang.rg mylang/example.my
```

One `.rg` file is the whole language: tokens, keywords, precedence,
syntax, highlighting, outline, and name binding. `check` certifies it
against the envelope and prints the certificate — or refuses it with a
counterexample. Everything else is derived.

* **[docs/GUIDE.md](docs/GUIDE.md)** — the user guide: your own language in
  ten minutes, the editor path, embedding, and an honest list of what is
  not built yet.
* **[docs/RG-REFERENCE.md](docs/RG-REFERENCE.md)** — the `.rg` language
  reference: every declaration, annotation, and envelope rule.

The rest of this file is the engineering log: what each increment
proves, how it is gated, and what it measures.

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

## P2 — increment 2 (shipped): L4 auto-balanced sequences

List-shaped rules (`L → L α | seed`, auto-detected from the grammar
value; fragile/expression shapes excluded) no longer build cons spines.
Trees carry **LIST nodes with balanced ≤16-ary RUN chunks**; one
`ListBuilder` serves batch and incremental parsing (batch appends per
cons-reduce; incremental additionally **concatenates whole salvaged runs
associatively** — the `B → B B` licence from Wagner §7). The typed AST
regenerated through the drift gate: list nonterminals became structs
with flattened `items()` accessors (cons-cell enums gone), and the
compiler enumerated every stale usage — the ramification workflow,
exercised for real on a semantic grammar change.

The gate refined honestly: **semantic tree equality** — full structural
equality everywhere, list nodes compared by flattened contents (run
chunking is representation, not meaning — that is what *associative*
declares) — plus explicit balance invariants (`check_balance`) and
byte-identical text. The fuzz gate caught one real bug during the build:
splice-time pending trivia landed at list level where batch places it
inside the first element; fixed by the same left-spine injection the
non-list path uses.

The collapse, measured (100k-line corpus, 500-edit session):

| | before L4 | after L4 |
|---|---|---|
| Mid-file edit (median) | 25 ms, ~50,238 splices | **315 µs, ~60 splices** |
| Near-EOF edit (median) | 88 µs | 331 µs |
| Position dependence | linear in suffix | **none** (Wagner: "location of changes does not affect running time") |
| Tree depth at 100k stmts | ~100k (big-stack thread) | logarithmic (**thread retired**) |
| Batch parse | 142 ms | 342 ms (ListBuilder constant factor — once-per-open cost; optimization headroom noted) |

Deliberate scope notes: upper run levels rebuild per finalize (O(#runs)
Arc clones ≈ the ~300 µs floor; B-tree-style level reuse is the next
refinement if it matters); leaf-run seams may drift from batch's exact
chunking (semantic equality is the contract; content-defined canonical
chunking is the documented upgrade path if full shape equality is ever
wanted).

## P2 — increment 3 (shipped): error recovery

Parsing is **total** now. On error: a bounded repair search (mini-CPCT+)
finds insert *sequences* up to length 3 (BFS over cloned state stacks,
validated against K=3 peeked real terminals) competing with single-token
deletion; EOF repair scores insertions by accept-reachability or strict
stack-depth progress (missing closers heal one per round); panic-skip
and stack-unwind guarantee termination under a repair budget. Skipped
tokens stay in the tree inside ERROR nodes (excluded from symbol
positions — typed accessors keep meaning); inserted tokens are
zero-width "missing" tokens excluded from all counts, so buffer
alignment survives and `has_err` poisoning lets salvage dissolve error
regions while clean regions keep splicing: **typing through broken
states holds >90% reuse**, and repaired documents still pass the
incremental ≡ batch semantic gate. Repairs surface as diagnostics.

## P3 — increment 1 (shipped): derived editor services

`rantlr-services`: everything computed from artifacts the toolchain
already generates, LSP-shaped but transport-free:

- **Semantic tokens** (tier 0): token-id → legend classes, LSP quintuple
  encoding, and **delta updates spliced from damage regions** — gated by
  a property test (delta-applied cache must byte-equal a fresh full
  encode, 160 random edit rounds + 200 at scale).
- **Folding** from the block skeleton + multi-line comment runs read
  straight off the line states.
- **Outline** from the tree with byte spans (name selection = exact
  ident span).
- **Completion from the ACTION rows** — the antlr4-c3 capability as a
  table lookup; keywords auto-lowercased for insertion.
- **Diagnostics from repairs** (`unexpected `)`` / `missing SEMI`),
  spans landing on the exact characters.
- **Selection ranges** (the ancestor-span substrate, session-wrapped).

At 100k lines: semantic tokens full 7.2 ms (1.14M styled tokens);
**edit → incremental reparse + semantic delta: 563 µs median, ~35 u32s
per delta** (vs 5.7M for a full resend); outline 12.5 ms; completion
30.6 ms mid-file (states-only prefix run — per-line state checkpoints
are the documented refinement).

## P3 — increment 2 (shipped): LSP server, VS Code extension, hot-reload

**`rantlr-lsp`** — a working language server (the workspace's first and
only external dependency: `serde_json`). Hand-rolled Content-Length
framing over stdio; a fully testable `Server::handle(msg) → messages`
core: initialize with utf-8 position-encoding negotiation, incremental
`didChange` (LSP ranges mapped onto line edits, terminators preserved),
semantic tokens full **and delta with resultId anchoring** (protocol
gate: client-side application of the returned edits must equal a fresh
full — held), foldingRange, documentSymbol, completion, selectionRange
(parent-chained), and publishDiagnostics from repairs (which clear when
the document heals).

**Hot-reload playground** — the "beautiful ways" demo, live: the server
watches `chartlang.toml` (keywords + operator precedence/associativity)
and rebuilds the ENTIRE certified pipeline on change — envelope lints,
LR tables, conflict analysis, style legend — in milliseconds, reparses
every open document, and asks the client to refresh tokens. Add
`async` to the keyword list and it re-colorizes everywhere instantly.
Remove `prec.left.2 = * /` and **the envelope refuses the reload**: the
shift/reduce conflict — with its example-input counterexample — appears
as an error diagnostic on `chartlang.toml`, and the last good language
stays live. The grammar-authoring feedback loop, running in an editor.

**`editor/vscode-chartlang`** — minimal extension (vanilla JS +
`vscode-languageclient`): language registration for `.cl`, semantic
token scope mapping, server discovery (config → workspace target/ →
PATH), and a watcher nudging reloads on config saves. **`examples/
playground/`** — `demo.cl` + `chartlang.toml` ready for F5.

Smoke-tested end-to-end over real stdio (initialize → didOpen →
diagnostics → tokens → shutdown/exit clean). Deliberate notes: reloads
`Box::leak` the previous pipeline (bounded, documented; Arc-ification
of session lifetimes is the refinement); non-utf-8 clients fall back to
byte-counted positions (fine for ASCII demos).

## P4 — increment 1 (shipped): the semantic layer

`rantlr-sem` — envelope commitments L8 (binding as data) and L9
(signature/body firewalls), running. The architecture is **salsa's** —
revisions, memoized queries, early cutoff by output comparison
("backdating": an unchanged signature keeps its old change-revision so
dependents stay valid) — as a minimal transparent engine (the query DAG
is tree → symbols → signature → resolution; swapping in the salsa crate
is the documented refinement as the workspace model grows).

Binding is declarative — a `BindingConfig` of (nonterminal, production)
entries: `LetStmt` defines, `NameRef`/callees reference, `Block` scopes
— the `@def/@ref/@scope` annotations as data. Sequential
definition-before-use scoping with block shadowing; a file's exported
**signature** = its sorted top-level definition names; cross-file
resolution goes only through signatures. Services derived: go-to-def,
find-references, rename (cross-file WorkspaceEdit, verified by
re-parsing the edited output), unresolved-variable warnings, and
binding-aware completion (innermost scope first, then other files'
exports, ranked ahead of grammar tokens via sortText). All wired into
the LSP server (definition/references/rename capabilities + enriched
completion + merged diagnostics), with cross-file protocol tests.

**The firewall gate, proven by counters and at scale:** editing a block
body in a 100k-line file recomputes only that file's resolution; the
second file's answer returns memoized in **1.6 µs** (recompute count: 1).
Editing an exported name recomputes both — and only then. Numbers:
symbols 11.2 ms (40k defs / 160k refs / 10k scopes), resolve 8.25 ms
(after an honest catch: the naive resolver measured 5.1 s at scale —
name-indexing fixed it, 620×). Deliberate scope notes: resolution
recomputes whole-file on any tree change (per-item memoization is the
next granularity); `names_in_scope` is demo-grade positional.

## P5 — increment 1 (shipped): the `.rg` textual grammar surface, self-hosted

`crates/rantlr-rg` is the textual surface: a `.rg` file declares the
ENTIRE language — tokens (`/regex/` or `"literal"` patterns), modes,
`keywords` (specialization), bracket `pair`s, `prec` lines, labeled
productions, and the editor annotations (`@style`, `@def/@ref/@scope`,
`@outline`) — and compiles to exactly the grammar VALUES the rest of the
toolchain runs on. `.rg` lives inside its own envelope: line terminators
are unwritable in patterns (negated classes and `.` exclude them by
construction), alternatives carry mandatory labels (which name the typed
AST — and keep the grammar LR(1)), and the whole surface is newline-
insensitive.

**Self-hosting, proven as a fixed point:** `rg.rg` — the `.rg` grammar
written in `.rg` — is parsed by the bootstrap grammar (the same
declarations as Rust values) and compiled; the result must reproduce the
bootstrap EXACTLY: lexical grammar (structural equality), syntax
grammar, LR tables, styles, outline, and binding config — and
generation 1 re-parses its own source into the bootstrap's tree.
Likewise `chartlang.rg` compiles to the programmatic demo grammar
value-for-value and table-for-table, with a corpus differential on top.
Envelope refusals are **span-mapped**: an unknown name, a bad pattern, an
L1/L2 witness, or an LR conflict counterexample each point at the
offending construct in the grammar file (`Conflict` now carries its
production indices for exactly this).

**Dogfooding found two real engine bugs** that seven increments of
demo-language gates never reached, both now fixed with engine-level
regressions:

* **Dangling-separator runs (L4):** balanced-list RUN chunks are
  arbitrary ≤16-child cuts, so a reused run can end mid repetition-unit
  (trailing `|`). Blind associative splicing left the LR state one shift
  behind the tree — spurious repairs on clean text. `ListShape` now
  knows its repetition anatomy (`alpha_first/alpha_last/seed_first`) and
  runs splice only at unit boundaries; dangles re-enter the input.
* **Right-edge lookahead dependence (Wagner right breakdown):** a reused
  subtree's right-spine reductions assumed the OLD following token.
  Appending `else` to an existing `if` (or a new alternative to an
  existing rule) must UN-SPLICE the reused subtree and re-derive it.
  On an error immediately after reused structure, the parser now unwinds
  the nearest reused piece (provenance-tracked; fresh reduces never
  unwind, so it terminates) and re-parses its children under the actual
  lookahead — repair is the fallback, not the first answer.

**The editor loop, closed:** the LSP now serves TWO languages — the
target language (pipeline hot-reloaded from `chartlang.rg`, with
`chartlang.toml` as legacy fallback) and `.rg` files themselves
(highlighting incl. a `regexp` class, outline of rules/tokens/modes with
LSP kinds, forward-reference go-to-definition via the new UNORDERED
binding mode in rantlr-sem, and live envelope diagnostics as you type —
conflicts pointing at productions). The playground's config is now the
full grammar file. Protocol tests cover both (.rg services + .rg
hot-reload with span-accurate refusal).

Numbers (M-series, release): bootstrap toolchain 1.78 ms; `rg.rg`
parse+compile 637 µs + certify 1.58 ms; `chartlang.rg` 457 µs + 3.24 ms
(the whole language, text → certified pipeline, under 4 ms); a
1302-line synthetic grammar (300 tokens, 800 prods) compiles in 4.67 ms
of which the compile pass is 474 µs — the text surface is noise next to
the table build. A keystroke edit in that grammar file reparses in
**21 µs** (99.9% reuse) vs 1.80 ms batch. Deliberate scope notes: BNF
only (EBNF sugar desugars later), literals must be declared (no silent
keyword auto-derivation), `pair` brackets fixed to `()[]{}`, and .rg's
reserved words are unusable as bare names in argument positions (quote
them) — the `bracket`-vs-`@style(bracket)` collision that forced the
`pair` keyword was itself caught by the fixed-point gate.

## P5 — increment 2 (shipped): EBNF sugar on the `.rg` surface

Rule-level repetition forms — `rule stmts = stmt*`, `rule kw_list =
kw_item+`, `rule args = expr* % ","` (raku-lineage `%` for separators),
`rule init = x?` — plus inline postfix `?`/`*`/`+` on RHS symbols (one
shared generated helper per element/op pair: `expr?` anywhere is the
same `expr_opt` rule). Everything desugars to the left-recursive shapes
L4 already balances, under a DOCUMENTED naming convention
(`Empty`/`More`, `First`/`More`, `None`/`Some`, `_ne` inners,
`{elem}_{op}` helpers) — so the sugar is sugar, not a second grammar
formalism: the typed AST, the balanced runs, the incremental splices,
and the conflict counterexamples are exactly those of the equivalent
hand-written grammar. Proven literally: the sugar gate compiles sugared
and hand-desugared sources and asserts identical grammar values AND
identical LR tables, with every generated repetition L4-detected.

The self-hosting story deepens: `rg.rg` now uses its own sugar (eight
list rules became one-liners; the bootstrap hand-writes the desugared
productions under the same names, and the fixed point still holds
exactly), and `chartlang.rg`'s `stmt*` / `expr* % ","` reproduce the
demo grammar value-for-value (demo production names adopted the
convention: `ArgsNone`/`ArgsNeFirst`/`ArgsNeMore` — the rename rippled
through the drift-gated typed AST, exercising ramification once more).
Envelope discipline for the sugar itself: repetition elements must be
RULES (a token element would evade L4 balancing — refused with a
wrap-it-in-a-rule hint), separators must be tokens, generated names
collide loudly, and `?` on tokens is fine. The refusal machinery even
caught a genuinely ambiguous grammar in this increment's own test
suite — a possibly-empty separated list adjacent to a bare repetition —
with a minimal counterexample (`B A · A`).

Numbers hold: the sugared `chartlang.rg` is 80 lines compiling in
235 µs (was 86 lines / 457 µs — sugar makes grammars smaller AND
cheaper); text → certified pipeline still under 2 ms.

## P5 — increment 3 (shipped): tree-sitter grammar emission

`rantlr_rg::tsgen` + the `rg2ts` binary emit a tree-sitter
`grammar.js` and `queries/highlights.scm` from any envelope-certified
grammar — the "derived artifact" of the original feasibility report:
one definition also reaches every tree-sitter-native surface (Neovim,
Helix, Zed, GitHub). The envelope makes the mapping principled:

* trivia-by-declaration → `extras`
* keyword specialization → `word:` + inline keyword strings
  (tree-sitter's keyword extraction mirrors our specialization)
* declarative precedence (L3) → `prec.left/right(level, …)`
* L4 lists → idiomatic `repeat1` / separated-`seq` forms; ε-productions
  become `optional(…)` at references (computed by nullability)
* binding name children (L8) → `field("name", …)`
* bounded self-push trivia modes (L2) → RECURSIVE extra rules: nested
  block comments with no external scanner (tree-sitter nests them
  unboundedly where we cap depth — a permissive-direction divergence,
  noted in the emitted header)
* `@style` classes → highlight captures (`@keyword`,
  `@punctuation.bracket`, `@string.regexp`, …)

Deliberate boundaries, refused with explanations: @error tokens are
skipped (tree-sitter has its own ERROR recovery) and non-trivia modes
(string interpolation) are named external-scanner territory.

Validated against the real thing: `tree-sitter generate` (v0.25)
accepts both emitted grammars — our canonical-LR(1) discipline held
under tree-sitter's weaker LALR(1) — and the generated parsers parse
the corpus with **zero errors**: the chartlang parser reads `demo.cl`,
and the rg parser reads `rg.rg` and `chartlang.rg` — a tree-sitter
parser, emitted from a self-hosted grammar, parsing that grammar's own
source. Checked-in artifacts under `tree-sitter/{chartlang,rg}/` are
drift-gated; emitted JS is structurally validated (all `$.rule`
references defined, deterministic output); the live `generate` gate
soft-skips when the CLI is absent.

## P6 — increment 1 (shipped): per-item semantic memoization

The granularity refinement named in P4: rantlr-sem now memoizes at the
level of a file's top-level ITEMS. The mechanism falls out of machinery
the toolchain already had — the incremental parser reuses untouched
subtrees by `Arc` identity, so an item's pointer is a free, exact memo
key. Two layers per item:

* **Fragments** (keyed by pointer alone): defs/refs with ITEM-RELATIVE
  spans, fragment-local scopes/order, and env-independent local
  resolution — a ref either binds inside its item or ESCAPES.
* **Item resolutions** (keyed by pointer + an environment fingerprint
  chained over preceding items' top-level NAME sequences + the foreign
  signature fingerprint): escaped refs are CLASSIFIED (top-level /
  foreign / unresolved). Positions are never cached — they're recovered
  through a lazily built per-revision name index — so caches survive
  item insertion and body edits without positional staleness.

`set_tree` diffs old/new item lists by pointer prefix/suffix: fragments
and resolutions carry over positionally (no map traffic), dropped
fragments are swept exactly, and the top-name count map updates from
the edit-sized delta — so an unchanged signature is PROVEN in O(edit)
and never re-sorted (backdating keeps other files' fingerprints still).

Measured at 100k lines (90k items, 40k defs / 160k refs): a body edit's
semantic cost fell from ~19.5 ms (P4 recomputed symbols + resolution
wholesale) to **~4.9 ms** — with counters proving **2 fragment walks +
2 item resolutions** (the edited item plus its trivia-adjacent
neighbor: the newline ending an item lives in the NEXT item's leading
spine, a bounded losslessness adjacency). The other file answers in
**0.5 µs**; go-to-definition is 1 µs after a 1.7 ms first-call index
build; a body edit in the small file leaves the 100k-line file fully
memoized (100 µs validation pass). New gate: inserting a NON-DEFINING
statement mid-file leaves every downstream item memoized even though
all of them shifted position. The differential gate (incremental DB ≡
fresh DB over random edit sequences) held throughout.

Honest scope notes: a SIGNATURE edit reclassifies downstream items
(~11 ms — the env fingerprint is deliberately coarse, since a changed
export could shadow anything below; per-NAME dependency tracking is the
salsa-grade refinement), and the per-keystroke floor is an O(items)
pointer walk + fingerprint-validation pass (~3 ms at 90k items) — the
damage-hint splice (letting the engine's damage report drive the item
diff directly) is the named next step.

## P7 — increment 1 (shipped): the composition tier

Part III's language nesting runs, and the design's central claim held:
**composition is a construction, not new machinery.** The generic
`compose()` operator (`rantlr_grammar::compose`) builds the PRODUCT
grammar of a host and a guest as ordinary grammar values — guest modes,
tokens, and nonterminals offset into one space (names prefixed,
astgen-clean), each island an OPEN token in a host mode pushing the
guest's base mode, a CLOSE token popping it, and a host production
`Island → OPEN guest_start CLOSE`. Finite host state × finite guest
state = finite product state on the SAME bounded mode stack (L2's
bound is the nesting depth), so **the product re-certifies through the
unchanged pipeline** — L1/L2 lints and conflict-checked LR(1) run on
every composition, which is the composition theorem as a checked
property with counterexamples. Everything downstream — losslessness,
Wagner splicing, recovery, L4 balancing, per-item memoization, product
highlighting — applies to island content verbatim, because the engine
never learns there were two languages.

The dogfood instance: **chartlang hosting `.rg` fenced islands**
(`` ```rg `` … `` ``` ``) — code files carrying grammar definitions,
the seed of the macro tier's declared argument grammars. Gates: the
product certifies; composed documents parse losslessly with full guest
structure inside islands (a guest `TokenDef` and EBNF sugar, as typed
nodes); island-interior and host edits hold incremental ≡ batch with
100% terminal reuse; fence deletion extends the island (an unclosed-
block-comment-class mode wave — total, lossless, recovered) and
healing restores the exact clean tree; host semantics flow AROUND
islands even broken ones (go-to-definition across a garbage island).

Composing honestly surfaced two real bugs, both fixed with gates:

* **Keyword specialization was global** — the guest's identifier token
  specialized `let` into the HOST's keyword id inside islands. Keywords
  now carry their OWNER (the @specialize token they belong to) through
  the model, the `.rg` compiler, and the lexer: per-owner
  specialization keeps composed keyword spaces separate — `rule let =`
  is valid `.rg` inside a chartlang island.
* **Right breakdown dead-ended at emptied list builders**: unwinding a
  reused list's pieces left the automaton in the after-list state with
  an empty builder, so recovery fabricated separators for valid text.
  Absorption-seeded list entries are now provenance-marked and unwind
  entirely (restoring the pre-list GOTO state) when emptied — fresh
  ε-reduce lists stay put.

Numbers (release): compose + certify the product in **3.5 ms** (76
tokens, 83 productions, 255 states); a 50k-line composed document with
500 islands cold-parses in 88.5 ms; an edit INSIDE a mid-file island
reparses in **416 µs at 100% reuse**, a host edit near islands in
**194 µs** — edit cost tracks the edit across language boundaries.
Deliberate scope notes: guest binding configs are not composed yet
(per-entry ordering is the missing piece — islands get syntax, trees,
and highlighting; guest-side IntelliSense inside islands is next),
island extent is lexical (unterminated guest modes extend it, like any
unclosed delimiter), and foreign guests (Markdown via pulldown-cmark)
plus the `.rg` `island` declaration surface are the tier's remaining
increments.

## P7 — increment 2 (shipped): guest binding composition

IntelliSense inside islands. The missing piece named in increment 1 —
per-entry ordering — landed as something better: **ordering is a
PER-SCOPE property**, and islands are **barrier scopes**. Binding
scopes now declare `(unordered, barrier)`: unordered scopes resolve
declaration-language style (forward references legal — it's the
guest's own semantics, carried into the island), and barrier scopes
seal their namespace in both directions — a guest reference never
silently resolves to a host binding (`rule file = File: tempting`
with a host `let tempting` is UNRESOLVED and diagnosed, not a
cross-language jump), and island names are invisible to the host and
to other islands. `rantlr_sem::compose_binding(host, guest, sg, map)`
composes the configs mechanically: host entries unchanged (ids
preserved by composition), guest entries offset, each island a barrier
scope carrying the guest's root ordering. The `.rg` surface gained
`@scope(unordered)` for declaration-language scopes.

Because islands attach as statements, island content lives entirely
inside one per-item fragment — so the whole feature lands in the
fragment-local resolution layer (visibility walks stop at barriers;
sealed refs classify as unresolved without consulting the environment)
and inherits per-item memoization for free: island edits recompute one
fragment, island navigation is fragment-local.

Gates: forward references between island rules resolve (`widget*`
jumps down to `rule widget`; a token used before its declaration
resolves — the unordered island scope at work); references and rename
stay island-local (two islands defining `widget` don't see each
other); the seal is diagnosed in both directions; and unresolved
diagnostics inside a garbage-filled island stay CONTAINED to the
island span while host navigation flows around it. 97 tests.

## The on-ramp (shipped): the `rantlr` CLI, the guide, and four surface fixes

Everything before this increment was reachable only from Rust or from a
running editor. This one makes the toolchain drivable: a `rantlr` binary
(`crates/rantlr-cli`, no third-party dependencies) whose subcommands each
expose one derived layer — `new` scaffolds a working grammar, `check`
prints the envelope certificate or the refusal, `tokens` shows the lex,
`parse` the lossless tree, `outline`/`defs` the derived services, `edit`
the incremental reparse, and `ts`/`ast` the exports. Diagnostics render
rustc-style with `file:line:col`, the source line, and a caret.

Two commands re-run a proof on every invocation rather than asserting
one: `parse` checks that the tree reproduces its input byte for byte,
and `edit` compares the incrementally-spliced tree against a full
reparse of the same final text and fails if they differ.

**Dogfooding the on-ramp found four defects**, each now gated:

* **Unknown pattern escapes compiled to literal letters.** `/[\s\S]*/`
  — a reflex for anyone who knows PCRE — silently became "whitespace or
  the letter S". Alphabetic escapes outside `\d \a \w \s \t` are now
  refused, naming what *is* supported. Punctuation escapes are untouched.
* **`@prec` was unreachable.** `prec` is a reserved word of the surface,
  so it lexed as a keyword and never arrived as an attribute name; the
  compiler arm behind it was dead code. Spelled `@precedence(tok)` it
  now works — gated on `- 2 * 3` grouping as `(-2) * 3`.
* **`@scope(unordered)` on the start rule did nothing.** Per-scope
  ordering only governs one item; whole-file ordering reads a global
  flag with no `.rg` surface, so declaring the root unordered silently
  failed and forward references at top level went unresolved *without*
  being reported unresolved. The root annotation now lifts to the file.
* **The tree printer crashed on synthetic node ids.** L4 LIST/RUN nodes
  and engine-synthesized NEWLINE trivia have out-of-vocabulary ids; the
  printer now names them (and shows the balanced-list shape, which makes
  L4 visible rather than merely claimed).

The grammar file also no longer has to be called `chartlang.rg`: the LSP
takes any single `.rg` in the workspace root as the language definition,
gated by `serves_a_grammar_under_any_name`.

Gates: six CLI integration tests drive the shipped binary end to end
(scaffold certifies/parses/resolves; ambiguity refused with a
counterexample; unknown escapes refused; broken documents stay lossless;
incremental equals batch; exports write files), plus the two `.rg`
regressions above. The self-hosting fixed point and the `chartlang.rg ≡
demo` differential both still hold across the pattern-parser change.
Docs: [docs/GUIDE.md](docs/GUIDE.md) and
[docs/RG-REFERENCE.md](docs/RG-REFERENCE.md), every command in them run
against a fresh scaffold before shipping.

## The declared type tier (shipped): types as grammar-author data

The client question behind this increment: language tools usually
either predefine their semantic vocabulary (every language gets the
same fixed "struct/function" ontology) or offer none. The design bet
here is a third way, the same one binding already took — **the tier is
not predefined, but the grammar definition has a means of building it.**
The toolchain ships zero types. A grammar declares a vocabulary and
per-production rules as annotations; the compiler lowers them to a
`TypeConfig` VALUE (`rantlr-sem::types`); one generic engine derives
type assignment and diagnostics for any language from that data. A
grammar that declares nothing gets a tier of exactly nothing.

The v0 declaration forms, riding the existing attribute syntax (no
grammar-surface change — the self-hosting fixed point is untouched):
`@type(Atom)` constants, `@type(of, label)` propagation, `@type(sig,
p…, R)` signatures over the alternative's rule symbols with local type
variables (unified per node: `sig, t, t, t` is polymorphic equality),
`@type(def, label)` giving a defined name its initializer's type, and
`@type(ref)` flowing types through the binding tier's own resolution.
Malformed declarations are refused at grammar compile time with spans
(arity vs the production, unknown labels, `def`/`ref` forms without
their binding counterparts) — the envelope pattern extended to types.

Checking is bottom-up synthesis with def→ref chains iterated to a fixed
point, so declaration order never matters; **unknown never cascades**
(unresolved names and repaired regions type as nothing rather than
erroring). Surfaced everywhere the other tiers are: `rantlr types`
(typed definitions, mismatches on the exact operand, exit 1), a type
line in `rantlr check`, and live LSP diagnostics that heal on fix —
including through grammar hot-reload. `compose_types` mirrors
`compose_binding` for island composition (host ids preserved, guest
offset, vocabularies merged by name).

Gates: six e2e tests (vocabulary/flow/convergence, sig-variable
unification with span-carrying mismatches, unknown-stays-silent,
no-declarations-no-tier, malformed-declarations-refused-with-spans,
and compose_types offset/merge), an LSP publish-and-heal test, and a
CLI test through the shipped binary. v0 limits recorded in the guide: atoms + signatures only (no
constructors, no subtyping), single-file flow, list children untyped,
file-granular recompute (per-item memoization is the named refinement).

## Type tier v1 (shipped): the document opens the vocabulary

v0's wall, found by the Structlang stress test: the type vocabulary was
fixed at grammar-compile time, so a document's `struct Point { … }`
could not BE a type. v1 removes exactly that wall with two forms.
`@type(deftype)` marks a def as INTRODUCING a type named by its def
child; `@type(named)` makes type annotations and constructor
expressions denote the type a reference resolves to (a resolved
non-type name is diagnosed: "this name does not denote a type";
unresolved stays silent — the binding tier owns it).

The design decision that does the heavy lifting: **a document type's
identity is its declaration site, not its name.** Which `T` an
annotation denotes is the binding tier's ordinary scoped resolution, so
type scoping, shadowing, and forward references are inherited rather
than implemented — two `struct T`s in different scopes are different
types, and cross-assigning them is a real mismatch (gated, including
the deliberately confusing ``expected `T`, found `T``` display, noted
for refinement). Grammar atoms and document types unify seamlessly in
signatures; the run vocabulary reports both
(`vocabulary Num, Str + document types: Point, Label`).

Engine: a static pre-pass collects introduction sites (document order,
site-keyed), the run vocabulary extends the grammar's atoms, and the
existing fixpoint machinery needs no change. `compose_types` passes the
new forms through untouched (they carry no atom ids). New surfaces:
`rantlr types` splits the vocabulary line; examples/structs/ is a
committed playground (structs + functions + `new`, served by the LSP
via the BYO-grammar path). Gates: 119 tests — open-vocabulary flow,
non-type-name diagnosis, nominal-identity-by-site under shadowing,
deftype/named binding cross-checks, and an example drift gate through
the shipped binary.

## Type tier v2 (shipped): applications

Three forms close the remaining Structlang silences around functions.
`@type(fn, params, rt)` assembles an ARROW type on the function node —
parameter types collected from the typed defs inside the params child
(arity taken from ALL def sites there, so an unknown param keeps the
arrow unknown rather than silently shortening it), return from the rt
child. Because the arrow is an ordinary carried type, `@type(def, …)`
hands it to the function's NAME, `@type(ref)` flows it like any value
(`let g = add;` makes `g` callable), and recursion converges through
the existing fixpoint — the call inside a function's own body checks
against its arrow on the next pass. `@type(apply, args)` checks calls:
arity ("expected 2 argument(s), found 1"), each argument on its exact
span, non-arrow callees ("not callable: this name has type `Num`"),
and produces the return type. `@type(returns, e)` is the tier's first
downward expectation: a walker stack of enclosing declared return
types, pushed once the rt child is walked.

Arrow types intern into the run vocabulary (displayed
`fn(Num, Num) -> Num`; the tables persist across fixpoint passes so
ids stay stable), and the vocabulary line now reports three segments:
grammar atoms, document types, arrows-where-used. One real bug found
and gated: transparent single-child wrappers share their child's byte
span, and the outer untyped wrapper's None clobbered the inner node's
type in the span map `apply` reads arguments through — Some now wins.

Gates: 123 tests — arrow assembly + zero-param arrows, all four
application failure modes with exact spans, first-class function flow,
recursion convergence, and static cross-checks for the new forms.

## Type tier v3 (shipped): members

The last Structlang silence. `@type(deftype, body)` extends v1's type
introduction with a member set: the typed defs inside the body child
ARE the members — fields are ordinary definitions, reusing @def and
@type(def, …) unchanged. `@type(member, base, name)` types field
access by looking the name token up in the base type's member set.
Member tables ride the same fixpoint as def types (stability now
checks both), so use-before-declaration works and struct-typed fields
chain — `l.a.x` resolves Line → Point → Num, one fixpoint level per
hop. Missing members diagnose on the name token ("no member `z` on
`Point`"), memberless base types likewise ("type `Num` has no
members"), and a member whose own type is unknown makes access silent
rather than falsely "missing" — member entries are Option-typed
precisely so existence and typedness stay separate facts.

Membership is span-based in v3: all def sites within the body child
count, nested ones included (fields-only structs are exact;
scope-precise membership arrives with methods). Gates: 126 tests —
chained access declared below its use, all failure modes with the
diagnostic on the name token, unknown-field-type silence, and static
cross-checks (the member name must label a token, deftype's body label
must exist).

## Type tier infrastructure (shipped): memoization + cross-file flow

The tier's capability roadmap being done, this increment makes it
CHEAP and makes it REACH. Per-item memoization: every item's type
outputs are cached with item-relative spans keyed by subtree identity
(the Arc pointers the incremental parser already preserves), and an
edit is classified by comparing the edited item's def-type sequence
and member contribution against its predecessor — equal means BODY
edit (replay everything else, re-walk one item, one pass), different
means SIGNATURE edit (honest full ripple, the P6 firewall philosophy
applied to types). `SemStats::{type_item_walks, type_passes}` are the
proof, and a differential gate holds the memoized report byte-equal to
a fresh SemDb's on every path.

That differential caught a real bug during development: persistent
arrow-intern tables accumulated stale entries across ripples, drifting
TypeIds from what a fresh run assigns. A ripple now rebuilds the arrow
vocabulary and purges arrow ids from its warm seeds, keeping the
trajectory identical to a cold run.

Cross-file: a reference resolving into another file (the binding
tier's foreign resolution) is typed from that file's own converged
report — recursively computed with a cycle guard, cheap because the
dependency is itself memoized. Grammar-atom types only, since atom ids
are file-independent; foreign document types and arrows stay unknown
until a global vocabulary exists (the honest boundary, gated). Editing
the dependency updates the dependent's diagnostics on its next query.

p8bench, 2001 items: cold 6.3 ms (4002 walks, 2 passes); body edit
1.2 ms (2 walks, 1 pass); signature edit 9.1 ms (full ripple); no-op
query 1.4 ms (0 walks — the assembly/resolution floor, the named next
refinement). Gates: 129 tests.

## Global type vocabulary (shipped): types are first-class across files

The infrastructure increment's honest boundary — foreign document
types and arrows stay unknown — is gone. TypeIds move from per-run to
PER-SEMDB: one vocabulary for every file, interned once, stable for
the session. Atoms keep their fixed prefix; document types intern by
(file, name, occurrence) — so within-file shadowing keeps distinct
types and ids survive edits that keep the declaration; arrows intern
structurally and are shared. Member tables go global too, keyed by the
owning type and replaced whenever the declaring file re-derives.

What that unlocks, all gated: a struct declared in file A is a type in
file B — values carry it, `let r: P = p` denotes it in annotations
(the binding tier already resolved the name; the report's deftype
table now answers "which type"), `p.x` reads A's member table, and
mismatches name it. Functions flow whole: direct cross-file calls and
first-class bindings both check arity and arguments against the
foreign arrow. Editing A flips B's diagnostics on its next query —
fast-path validity now snapshots the TRANSITIVE foreign closure (a
`p.i.y` chain depends on Inner's table even though only P appears in
the ref values).

The differential gate changed meaning deliberately: global ids are
history-dependent (a session that saw more types assigns different
numbers than a fresh one), so memoized ≡ fresh is now DISPLAY-
canonical — same spans, same type names, same diagnostics — which is
the semantics users can observe. The ripple-path arrow rebuild from
the previous increment is gone entirely; persistence is now the design
rather than the bug. Memoization counters kept their exact shape:
body edit 2 walks / 1 pass on the 2001-item bench.

Gates: 130 tests — foreign struct flow (annotation, member, mismatch,
staleness flip when the field retypes), foreign functions (direct +
first-class, arity + argument errors), cycles terminate, nominal
shadowing preserved, and the canonical differential on every path.

## The module tier (v0, shipped): exports, imports, visibility as data

The third declared tier, with Rust's module system as the semantic
reference. Two forms: `@export` on a def-carrying alternative
(visible to other files) and `@import(label)` (a token position that
resolves against other files' exports only — never local scopes, so
`use x;` can put @def and @import on the same token without binding to
itself). Declaring either activates strict semantics: file-private by
default, cross-file through imports only, no ambient names. A grammar
declaring neither keeps the open world — every prior test passes
unchanged.

"Not exported" is a first-class diagnostic distinct from "cannot
find": the engine distinguishes a name that exists privately somewhere
from one that exists nowhere. Navigation jumps THROUGH imports (the
import ref wins over the def-at-cursor — Rust's `use` behavior), types
flow through import chains via plain @type(ref), and aliasing
(`use x as y`) is just @def and @import on different labels.

The Rust bet that pays twice: `pub` is an incrementality contract.
With the tier on, a file's P6 signature is its EXPORT surface, so
editing a private definition cannot invalidate any other file's
resolutions — gated by counter (0 re-resolutions in the dependent
after a private body edit; removing a `pub` flips the dependent's
import to the access error).

examples/modules/ is the committed playground (lib.ml exports, app.ml
imports, one alias), drift-gated by the e2e suite include_str'ing it.
`rantlr check` reports the tier; `rantlr defs` marks pub/private,
shows cross-file arrows, loads sibling files, and exits 1 on access
errors. Named next steps: module scopes within a file, qualified
paths, re-exports, visibility levels.

Gates: 136 tests — the example world (resolution, types through
imports, navigation through `use`, exported flags as data), strict
semantics (private/typo/ambient distinctions), the signature firewall
counter, open-world compatibility, static cross-checks, and the LSP
wire.

## Status

Eight crates + two binaries (`rantlr`, `rantlr-lsp`); 136 tests; the full story runs:
**one grammar — now a text file — → certified lexer + LR tables +
incremental lexing/parsing (total under errors) + lossless trees + typed
AST + editor services + semantic binding + a declared type tier + LSP,
self-hosted: the grammar
language is defined in itself, parsed by its own engine, and its files
get the same editor intelligence as the languages it defines** — and it
is now drivable from a command line and documented for someone who has
never seen the codebase.
Remaining roadmap (per the feasibility report): the `.rg` `island`
declaration surface + foreign guests (Markdown) + composed pipelines
served over LSP, grouping on the `.rg` surface, per-name semantic
dependency tracking / real salsa swap-in, and the macro tier (declared
argument grammars over the island machinery). Nearer-term gaps the guide
records honestly: nothing is published anywhere, the VS Code extension is
an unpackaged development shell, and hover/formatting/code actions are
unimplemented.
