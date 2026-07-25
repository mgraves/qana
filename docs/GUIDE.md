# rantlr — user guide

You describe a language in one `.rg` file. rantlr checks that description
against a fixed set of rules (the *envelope*), and if it passes, derives
everything else: an incremental lexer, an LR(1) parser that never fails,
syntax highlighting, folding, an outline, go-to-definition, rename, and
completion. No code generation step, no runtime you have to integrate —
the grammar file *is* the language.

The trade is deliberate. The envelope refuses grammars that most parser
generators accept and then quietly mis-parse. In exchange, every language
that passes gets incremental reparsing and editor intelligence for free,
with the guarantees checked rather than assumed.

New to the design? [`RG-REFERENCE.md`](RG-REFERENCE.md) is the language
reference; the [`README`](../README.md) is the engineering log with
benchmarks and proofs. This guide is the practical path.

---

## Install

There is nothing on crates.io — build from the repo:

```bash
cargo build --release
```

That gives you two binaries in `target/release/`: `rantlr` (this guide)
and `rantlr-lsp` (the editor server). To put `rantlr` on your `PATH`:

```bash
cargo install --path crates/rantlr-cli
```

Everything below assumes `rantlr` is on your `PATH`. If it isn't,
substitute `cargo run -q -p rantlr-cli --` for `rantlr`.

---

## Quick start: your own language in ten minutes

### 1. Scaffold

```bash
rantlr new mylang --name Mylang --ext .my
```

You get `mylang/mylang.rg` — a complete, commented, working grammar with
comments, strings, numbers, identifiers, keywords, bracket pairs,
operator precedence, statements, blocks, and expressions — plus
`mylang/example.my` to parse.

### 2. Certify it

```bash
rantlr check mylang/mylang.rg
```

```text
✓ Mylang — certified

  lexical envelope
    modes                 1 (DEFAULT)
    tokens                18 + 2 keywords
    mode-stack bound      0  (L2)
    line entry states     1  (L2: bounded)
    DFA states            DEFAULT 25

  syntax envelope
    nonterminals          6
    productions           17
    LR(1) states          74
    conflicts             0  (L3: deterministic)
    settled by prec       48  (4 fragile prods)
    auto-balanced lists   2  (L4): args, stmts

  derived services
    highlight classes     bracket, comment, keyword, number, operator, …
    outline entries       1
    binding sites         1 defs, 1 refs, 1 scopes
```

This is the certificate, and it is worth reading closely. **Conflicts: 0**
means the grammar is unambiguous — proven, not hoped. **Line entry
states: 1** means relexing can restart at any line. **Auto-balanced
lists** are the sequence rules that get logarithmic-depth trees instead
of thousand-deep recursion. The bottom block is everything an editor
will do with this language, derived from the same file.

### 3. Parse something

```bash
rantlr parse mylang/mylang.rg mylang/example.my
```

```text
Program @0..1059
└─ stmts (balanced list) @0..400
   ├─ LetStmt @0..147
   │  ├─ KW_LET "let" @132
   │  ├─ IDENT "width" @136
   │  ├─ EQ "=" @143
   │  ├─ NumLit @144..146
   │  │  └─ NUMBER "3" @145
   │  └─ SEMI ";" @146
   …
✓ lossless: the tree reproduces the source byte for byte
✓ no errors
```

Node names (`LetStmt`, `NumLit`) are exactly the alternative labels you
wrote in the grammar. Add `--trivia` to see whitespace and comments —
they are in the tree too, which is what "lossless" means and why the
tree can back an editor. `--depth N` truncates deep output.

### 4. Break it on purpose

Delete a `;` from `example.my` and parse again. You still get a complete
tree; the damaged region is marked `[error]` and the problem is reported
with a caret:

```text
error: missing SEMI
 --> mylang/example.my:7:1
  |
7 | {
  | ^
Parsing is total: the tree above is complete and usable despite these errors.
```

This is the property that makes editor integration work. There is no
input for which parsing fails, so highlighting and folding never go
blank while you are mid-keystroke.

### 5. See the semantics

```bash
rantlr defs mylang/mylang.rg mylang/example.my
```

```text
definitions
  width                    top level    4:5
  height                   top level    5:5
  area                     nested scope 10:7

references
  width                    5:14         → 4:5
  area                     11:9         → 10:7
  …
✓ every reference resolves
```

Those arrows are go-to-definition. Nothing in the grammar declared a
symbol table — `@def(name)`, `@ref(name)`, and `@scope` did all of it.
Delete `let width = 3;` and rerun: the references come back
`unresolved`, which is a semantic answer, not a parse failure.

### 6. Watch an edit stay cheap

```bash
rantlr edit mylang/mylang.rg mylang/example.my --line 4 --text "let width  = 30;"
```

```text
  reuse
    terminals reused      26 of 31  (83.9%)
    subtree splices       4
    breakdowns            1

  time
    incremental           36.166µs
    full reparse          105.209µs  (same final text)

✓ incremental result is identical to a full reparse
```

That last line is a differential check run on every invocation: the
incrementally-updated tree is compared against parsing the same final
text from scratch, and they must be identical. Reuse percentages are
meaningless if the fast path can drift from the slow one.

---

## The loop: changing your language

The point of a one-file language definition is that changing the
language is an edit, not a rebuild. A few things to try in `mylang.rg`:

**Add a keyword.** Put `while` in the `keywords IDENT = …` line, then
add an alternative to `rule stmt`:

```
  | WhileStmt: "while" "(" expr ")" block
```

`rantlr check` re-certifies in milliseconds and `WhileStmt` is now a node
kind, an outline candidate, and a highlight class.

**Add an operator.** Declare `token CARET = "^" @style(operator)`, add
`prec left "^"`, and add `| PowExpr: expr "^" expr` to `rule expr`.

**Break it on purpose.** Delete the two `prec` lines. The expression
rule is now genuinely ambiguous and the toolchain refuses it:

```text
error: grammar conflict (shift/reduce on PLUS) — example input: KW_PRINT NUMBER PLUS NUMBER · PLUS
  [expr → expr PLUS expr · , PLUS]
  [expr → expr · PLUS expr , PLUS]
  --> mylang/ambiguous.rg:77:5
   |
77 |   | AddExpr:   expr "+" expr
   |     ^^^^^^^
refused: mylang/ambiguous.rg is outside the envelope
  note: L3 — the grammar must be deterministic LR(1). Add a `prec` line, or refactor the rule.
```

You get the conflicting LR items, an input that exhibits the ambiguity,
and a caret on the production responsible. Undo, and the language comes
back. This is the envelope doing its job: the ambiguity is caught in
your grammar, not in someone's document a year later.

---

## The type tier: declared, not predefined

rantlr ships **no types**. The starter grammar declares its own:

```
rule expr =
  | AddExpr:   expr "+" expr @type(sig, Num, Num, Num)
  | ParenExpr: "(" e:expr ")" @type(of, e)
  | NumLit:    NUMBER @type(Num)
  | StrLit:    STRING @type(Str)
  | NameRef:   name:IDENT @ref(name) @type(ref)
```

`Num` and `Str` are this grammar's invented vocabulary — the toolchain
has never heard of them. `@type(sig, …)` reads as a signature over the
alternative's rule symbols; `@type(def, e)` (on the `let` rule) gives
the defined name its initializer's type; `@type(ref)` flows it back
through every use, riding the same resolution that powers
go-to-definition. One generic engine derives the rest:

```bash
rantlr types mylang/mylang.rg mylang/example.my
```

```text
vocabulary Num, Str

typed definitions
  width                    Num        4:5
  height                   Num        5:5
  area                     Num        10:7

✓ no type errors (12 nodes typed)
```

Change `3` to `3 + "three"` and the mismatch is reported on the exact
operand (`expected `Num`, found `Str``), in the terminal and as a live
red squiggle in the editor. Unknown never cascades: an unresolved name
or a parse-repaired region types as *nothing*, not as an error.

Malformed declarations are refused at grammar compile time, spans and
all — a signature whose arity doesn't match the alternative, a label
that doesn't exist, `@type(ref)` without `@ref`. Same envelope
philosophy, extended to the tier: bad declarations are caught in your
grammar, not discovered in someone's document.

Delete every `@type` annotation and the tier is exactly gone — empty
vocabulary, zero rules, no type queries. Its power is precisely what
the grammar declared.

And the vocabulary is not limited to what the grammar declares:
`@type(deftype)` lets **documents** introduce types. In
[examples/structs](../examples/structs/structlang.rg), each `struct`
declaration creates a type, annotations and `new` expressions denote
it, and mismatches name the document's own types:

```text
vocabulary Num, Str  + document types: Point, Label

error: type mismatch: expected `Point`, found `Label`
```

Type identity is the declaration site, so two `struct T`s in different
scopes are genuinely different types — scoping, shadowing, and forward
references all come from the same name resolution that powers
go-to-definition.

Functions get real signatures: `@type(fn, p, rt)` assembles an arrow
type from the parameter defs and return annotation, `@type(apply, a)`
checks every call (arity, each argument, produces the return), and
`@type(returns, e)` checks return statements against the declaration:

```text
scale                    fn(Num) -> Num

error: expected 2 argument(s), found 1
error: return type mismatch: expected `Num`, found `Str`
error: not callable: this name has type `Num`
```

Arrows are ordinary carried types, so `let g = add;` makes `g`
callable, and recursive calls check against the function's own
signature.

Current limits, honestly: atoms, document types, signatures, and
arrows — no constructors like `List<T>`, no subtyping, no member/field
lookup yet (`p.x` is invisible; the next planned form); types flow
within one file; list-shaped children stay untyped; checking is
file-granular per query rather than per-item memoized.

---

## In an editor

The VS Code extension is a thin client over `rantlr-lsp`. It serves two
languages: your target language, and `.rg` grammar files themselves.

```bash
cargo build --release -p rantlr-lsp
cd editor/vscode-chartlang && npm install
```

Open `editor/vscode-chartlang` in VS Code and press **F5**. In the
Extension Development Host window that opens, open your language's
folder (`mylang/`, or the bundled `examples/mini/` and
`examples/playground/`).

The server treats **any single `.rg` file in the workspace root** as the
language definition, so `mylang.rg` works with no configuration. Editing
and saving it hot-reloads the pipeline: open documents re-colorize, and
a grammar that leaves the envelope publishes its refusal as a diagnostic
on the offending line while the last good pipeline stays live.

For a custom file extension, `rantlr new` already wrote
`.vscode/settings.json` for you:

```json
{ "files.associations": { "*.my": "chartlang" } }
```

`chartlang` is the extension's internal id for "the target language" —
a naming leftover from the demo, not a constraint on your language.

What you get in the editor today: semantic highlighting (with deltas on
every keystroke), diagnostics from recovery, folding, document symbols,
completion, go-to-definition, find-references, rename, and expand-
selection. What is not wired: hover, formatting, code actions, inlay
hints, and workspace-wide symbol search.

---

## In your own application

Editors are not the only host. `EmbeddedLang` compiles a grammar and
hands back a ready pipeline, in process, with no LSP hop:

```rust
use rantlr_rg::EmbeddedLang;

let lang = EmbeddedLang::from_rg_source(include_str!("mylang.rg"))?;
let mut session = lang.session("let x = 1;\n");
let tree = session.tree().expect("parsing is total");
```

`session` is an incremental session: feed it `LineEdit`s and it splices
the tree. `lang.styles`, `lang.outline`, and `lang.binding` drive
highlighting, outline, and navigation exactly as they do over LSP.

A worked integration lives outside this repo: `synkro_rantlr` in the
Synkro workspace implements that GUI framework's `TokenProvider` and
`CreaseProvider` over `EmbeddedLang`, giving its `CodeArea` widget
grammar-driven highlighting and folding.

---

## Exports

```bash
rantlr ts mylang/mylang.rg tree-sitter/mylang   # tree-sitter grammar + highlight queries
rantlr ast mylang/mylang.rg > src/ast.rs        # typed Rust AST over the green tree
```

The tree-sitter export gives you an escape hatch into the tree-sitter
ecosystem (Neovim, GitHub, Zed). It is a one-way emit: edit the `.rg`
and re-emit, not the reverse.

The typed AST is zero-copy accessors over the same green tree, with
production labels as variant names, so `LetStmt::name()` returns the
identifier token you labelled `name:` in the grammar.

---

## What is not here yet

An honest inventory, so nothing surprises you halfway in.

**Packaging.** Nothing is published. Build from source; there is no
`cargo install rantlr` from crates.io, and the `rantlr` name there
belongs to an unrelated project. The VS Code extension is a development
shell — it is not packaged as a `.vsix` or published to the marketplace.

**Language features.** The `.rg` surface has no parenthesized groups
(`(a b)?`); repetition sugar is `*`, `+`, `?`, and `% separator`, and
the repeated element must be a rule rather than a bare token. Composition
(one language hosting another, like fenced code blocks) exists and is
tested, but only through the Rust `compose()` API; there is no `island`
declaration on the `.rg` surface yet.

**Semantics.** The semantic layer does binding, scoping, and the
declared type tier described above — atoms, signatures, and name-flow,
but no type constructors, no subtyping, and no cross-file type flow.
There is no module or import system — top-level names form one
namespace across files. Custom diagnostics and quick fixes are not
authorable; error messages are terminal-level ("missing SEMI").

**Editor surface.** Hover, formatting, code actions, inlay hints, and
workspace symbol search are unimplemented. The Synkro adapter re-lexes
on each content change, because that framework's provider seam carries
no edit deltas yet — the incremental session is available but not yet
driven from its edits.

None of these are blocked by the design; they are unbuilt. The parts
that carry risk — the envelope, incremental parsing, error recovery,
memoized semantics, composition — are built and gated.

---

## Command reference

| Command | What it shows |
| --- | --- |
| `rantlr new <dir> [--name L] [--ext .x]` | Scaffold a working grammar and sample document |
| `rantlr check <g.rg>` | Certify; print the envelope report, or the refusal with a counterexample |
| `rantlr tokens <g.rg> <file>` | The lex: every token, its style class, and each line's entry state |
| `rantlr parse <g.rg> <file> [--trivia] [--depth N]` | The lossless tree, losslessness check, and any repairs |
| `rantlr outline <g.rg> <file>` | Derived document symbols |
| `rantlr defs <g.rg> <file>` | Derived binding: definitions, references, resolution |
| `rantlr types <g.rg> <file> [--all]` | The declared type tier: typed definitions and mismatches |
| `rantlr edit <g.rg> <file> --line N --text "…"` | Incremental reparse: reuse, timing, and the batch differential |
| `rantlr ts <g.rg> <outdir>` | Emit a tree-sitter grammar and highlight queries |
| `rantlr ast <g.rg>` | Emit a typed Rust AST to stdout |

Exit codes: `0` success, `1` refusal or error in the input, `2` usage.
