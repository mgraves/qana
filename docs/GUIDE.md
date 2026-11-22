# qana — user guide

You describe a language in one `.qana` file. qana checks that description
against a fixed set of rules (the *envelope*), and if it passes, derives
everything else: an incremental lexer, an LR(1) parser that never fails,
syntax highlighting, folding, an outline, go-to-definition, rename, and
completion. No code generation step, no runtime you have to integrate —
the grammar file *is* the language.

The trade is deliberate. The envelope refuses grammars that most parser
generators accept and then quietly mis-parse. In exchange, every language
that passes gets incremental reparsing and editor intelligence for free,
with the guarantees checked rather than assumed.

New to the design? [`QANA-REFERENCE.md`](QANA-REFERENCE.md) is the language
reference; the [`README`](../README.md) is the engineering log with
benchmarks and proofs. This guide is the practical path.

---

## Install

There is nothing on crates.io — build from the repo:

```bash
cargo build --release
```

That gives you two binaries in `target/release/`: `qana` (this guide)
and `qana-lsp` (the editor server). To put `qana` on your `PATH`:

```bash
cargo install --path crates/qana-cli
```

Everything below assumes `qana` is on your `PATH`. If it isn't,
substitute `cargo run -q -p qana-cli --` for `qana`.

---

## Quick start: your own language in ten minutes

### 1. Scaffold

```bash
qana new mylang --name Mylang --ext .my
```

You get `mylang/mylang.qana` — a complete, commented, working grammar with
comments, strings, numbers, identifiers, keywords, bracket pairs,
operator precedence, statements, blocks, and expressions — plus
`mylang/example.my` to parse.

### 2. Certify it

```bash
qana check mylang/mylang.qana
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
qana parse mylang/mylang.qana mylang/example.my
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
qana defs mylang/mylang.qana mylang/example.my
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
qana edit mylang/mylang.qana mylang/example.my --line 4 --text "let width  = 30;"
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
language is an edit, not a rebuild. A few things to try in `mylang.qana`:

**Add a keyword.** Put `while` in the `keywords IDENT = …` line, then
add an alternative to `rule stmt`:

```
  | WhileStmt: "while" "(" expr ")" block
```

`qana check` re-certifies in milliseconds and `WhileStmt` is now a node
kind, an outline candidate, and a highlight class.

**Add an operator.** Declare `token CARET = "^" @style(operator)`, add
`prec left "^"`, and add `| PowExpr: expr "^" expr` to `rule expr`.

**Break it on purpose.** Delete the two `prec` lines. The expression
rule is now genuinely ambiguous and the toolchain refuses it:

```text
error: grammar conflict (shift/reduce on PLUS) — example input: KW_PRINT NUMBER PLUS NUMBER · PLUS
  [expr → expr PLUS expr · , PLUS]
  [expr → expr · PLUS expr , PLUS]
  --> mylang/ambiguous.qana:77:5
   |
77 |   | AddExpr:   expr "+" expr
   |     ^^^^^^^
refused: mylang/ambiguous.qana is outside the envelope
  note: L3 — the grammar must be deterministic LR(1). Add a `prec` line, or refactor the rule.
```

You get the conflicting LR items, an input that exhibits the ambiguity,
and a caret on the production responsible. Undo, and the language comes
back. This is the envelope doing its job: the ambiguity is caught in
your grammar, not in someone's document a year later.

---

## The type tier: declared, not predefined

qana ships **no types**. The starter grammar declares its own:

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
qana types mylang/mylang.qana mylang/example.my
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
[examples/structs](../examples/structs/structlang.qana), each `struct`
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

And structs have members: `@type(deftype, b)` makes the typed defs
inside the body the type's member set, and `@type(member, b, m)` gives
field access its type — chained through struct-typed fields:

```text
let deep: Num = l.a.x;          ✓ (Line → Point → Num)

error: no member `z` on `Point`
error: type `Num` has no members
```

Type checking is memoized per item: an edit that keeps a definition's
type (a body edit — the keystroke case) re-walks only the edited item;
an edit that changes one ripples through the file. And the vocabulary
is GLOBAL: a struct or function declared in one file is a first-class
type in every other. Values flow with their types, annotations denote
foreign structs, member access reads the declaring file's tables, and
calls into another file check arity and arguments:

```text
a.lang:  struct P { x: Num }     fn add(a: Num, b: Num) -> Num { … }
b.lang:  let r: P = p;           ✓ foreign type in an annotation
         let m: Num = p.x;       ✓ foreign member access
         let bad = add(1);       error: expected 2 argument(s), found 1
```

Editing the declaring file updates every dependent's diagnostics on
its next query.

Current limits, honestly: atoms, document types, signatures, arrows,
and members — no constructors like `List<T>`, no subtyping; membership
is span-based (defs inside the body child, including nested ones);
list-shaped children stay untyped.

---

## The module tier: imports and exports, declared

Like binding and types, the module system is declared, not predefined.
[examples/modules](../examples/modules/modlang.qana) makes a
Rust-flavored one from two annotations — `@export` (the `pub`
productions) and `@import` (the `use` productions):

```
| UseDecl:    "use" name:IDENT ";" @def(name) @import(name) @type(ref)
| PubFnDecl:  "pub" "fn" name:IDENT t:fn_tail @def(name) @export @type(def, t)
```

Declaring either activates strict semantics: private by default,
cross-file only through imports, and a dedicated diagnostic when an
import names something real but private:

```text
error: `secret` exists but is not exported by its file
```

Types flow through the import chain, go-to-definition jumps through a
`use` into the exporting file, and `pub` doubles as an incrementality
contract — editing a private definition re-resolves nothing anywhere
else (gated by counter). A grammar that declares neither form keeps
the open world.

Namespaces and paths are two more annotations: `@module(b)` makes a
def introduce a namespace (its members = the defs in the body child),
and `@qualify(base, name)` resolves `a::b` paths — nested segments,
import chasing (`use math;` then `math::pi`), and re-export chains
(`pub use`) included:

```text
mod math { pub let pi = 3; }        # lib.ml
let area = math::pi + width;        # app.ml — resolves, types, navigates

error: no member `nothere` in this module
error: `tau_hidden` exists in the module but is not exported
```

### Units, paths and visibility, for a project index

The module tier above treats each file as one flat module, resolved
among the files a session holds. A language whose modules nest, whose
paths begin at roots, and whose files a build system arranges into a
crate (Rust's) declares that structure with eight more annotations, which
a project index (`qana-index`) reads into each file's summary, and whose
`Resolver` resolves paths across files over a project model's unit graph
(Rust's: `cargo metadata` and its rules for module files). For finding a
declaration's usages, `qana_index::references` reads a file's references
to names: each one's path, and what it binds to within the file. Within a file,
binding is unchanged: what a path names in another file is the index's to
find.
[examples/units](../examples/units/unitlang.qana) is their language:

```
| ModDecl: "mod" n:name ";" @def(n, hoisted, open) @unit(n)
| ModDef:  "mod" n:name "{" b:items "}" @def(n, hoisted, open) @unit(n, b) @scope(unordered, barrier)
| PubItem: v:vis k:item_kind @visibility(v, k)
| PubCrate: "pub" "(" "crate" ")" @vis(crate)
| CrateRoot:  "crate" @root(crate)
| SuperAgain: root "::" "super" @root(super)
| UseRename: p:use_path "as" n:name @def(n, hoisted, open) @import(p) @ns(import, p)
| UseGroup:  p:path "::" "{" t:use_trees "}" @import_prefix(p, t)
| UseGlob:   p:path "::" "*" @import_glob(p)
| ImplFor: "impl" tr:path "for" t:path "{" impl_body "}" @impl(t, tr) @scope
```

- `@unit(name[, body])`: the definition `name` makes is a unit, a scope
  for imports and visibility. With `body`, its body is that child (and
  the definition is a module of it, as `@module(body)` says); without,
  its body is in another file, which a project model finds.
- `@root(kind[, first])`: the node begins a path at a root: `self` (the
  unit it stands in), `super` (its parent; `super` on `self` or `super`
  goes one further), `crate` (the crate's root unit), `global` (the
  crates, `::name`), `self_type` (Rust's `Self`) or `macro_crate` (the
  root of the crate that defines the macro whose expansion wrote the
  path: Rust's `$crate`), then the name `first` holds.
- `@vis(level)`: a visibility: `public`, `crate`, `super`, `private`, or
  `in` with the label of a path. `@visibility(v, item)`: what `item`
  defines and imports where the node stands has the visibility at `v`;
  without one there, the default, within its own unit.
- `@import(path)`, `@import`: the alternative imports the declaration
  the path names, under the name its `@def` gives it (or, when that
  `@def` reaches no name, as `self` in a group does, the path's last
  name; without a `@def`, none, as `as _`). Without a label, the path
  is the one the node itself spells (`extern crate a` with
  `@root(global, a)`). A token is a path of one segment: the module
  tier's `@import(name)` is this one.
- `@import_glob(path)`, `@import_glob`: every name the unit the path
  names offers. `@import_prefix([path,] trees)`: the imports in `trees`
  begin with the path.
- `@impl(type[, trait])`: an implementation block of the type the path
  at `type` names, of the trait at `trait`; its members are what is
  defined directly in the members scope it opens.
- `@attribute(path[, args])`: the node is an attribute, the one the path
  names, with the paths `args` spells as its arguments (Rust's
  `#[macro_use(a, b)]`). `@attributes(attrs[, item])`: the attributes in
  `attrs` are said of what `item` declares (a unit, an import), or of the
  unit the node stands in; what they mean is the project model's.
  `@loads`: the import loads the library its path names (Rust's `extern
  crate`).
- `@condition(pred)`: the attribute is a condition, its predicate at
  `pred`. With no attributes of its own, the construct the attributes it
  stands among are said of (`@attributes`) is under it (Rust's
  `#[cfg(…)]`); holding attributes, those hold under it (Rust's
  `cfg_attr(pred, …)`). A summary keeps each construct's, bodies' too
  (`Locations::condition_regions`), and what each offered item is under
  (its `condition`); a project model says each crate's configuration,
  which decides them (`qana_index::Condition::holds`).
- How a predicate reads: `@predicate(name)`, an option (`test`);
  `@predicate(name, value)`, an option with a string value (`feature =
  "std"`); `@predicates(name, list)`, the predicate `name` names over
  those in `list` (`all`, `any`, `not`; Rust's `target(os = "…")`, its
  `target_`-prefixed options); `@predicate_literal(literal)`, `true` or
  `false`. Any other is one the index does not read, which may hold.
- `@def(name, textual)`: a definition seen in textual order, as Rust's
  `macro_rules!` is: after it, into the modules declared after it, out
  of a module whose attributes the model says carry it out (Rust's
  `#[macro_use]`), not past its block. Within a file it binds as
  `hoisted`; `qana_index::textual_at` and `Resolver::resolve_macro` find
  a name in that order.

What path a subtree spells is read from the annotations: a token is one
segment; a `@qualify(base, name)` node is its base's path then its name;
a `@root` node is its root; a node that passes its name to one child
(`@style(inherit, …)`) is that child's path; any other node is its only
symbol child's. So `Vec::<T>::new` spells `Vec::new`, and `&T` spells
none.

---

## In an editor

The VS Code extension is a thin client over `qana-lsp`. It serves two
languages: your target language, and `.qana` grammar files themselves.

```bash
cargo build --release -p qana-lsp
cd editor/vscode-chartlang && npm install
```

Open `editor/vscode-chartlang` in VS Code and press **F5**. In the
Extension Development Host window that opens, open your language's
folder (`mylang/`, or the bundled `examples/mini/` and
`examples/playground/`).

The server treats **any single `.qana` file in the workspace root** as the
language definition, so `mylang.qana` works with no configuration. Editing
and saving it hot-reloads the pipeline: open documents re-colorize, and
a grammar that leaves the envelope publishes its refusal as a diagnostic
on the offending line while the last good pipeline stays live.

For a custom file extension, `qana new` already wrote
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
use qana_lang::EmbeddedLang;

let lang = EmbeddedLang::from_qana_source(include_str!("mylang.qana"))?;
let mut session = lang.session("let x = 1;\n");
let tree = session.tree().expect("parsing is total");
```

`session` is an incremental session: feed it `LineEdit`s and it splices
the tree. `lang.styles`, `lang.outline`, `lang.folds` and `lang.binding`
drive highlighting, outline, folding and navigation exactly as they do
over LSP.

Certifying a large grammar takes time: about two seconds for Rust's in a
release build, many more in a debug one. `from_qana_source` is two steps,
and a host can take them apart. `GrammarArtifact::certify` compiles and
certifies the grammar into an owned value that serializes with serde, so
it can run in a worker thread, or in an earlier run, and cross as bytes;
`EmbeddedLang::from_artifact` loads it (Rust's, 400 KB with `postcard`,
decodes and loads in about a millisecond), and the language it serves is
the one certifying made:

```rust
use qana_lang::{EmbeddedLang, GrammarArtifact};

// Where certifying is cheap to wait for (a worker, a build step):
let artifact = GrammarArtifact::certify(include_str!("mylang.qana"))?;
let bytes = postcard::to_allocvec(&artifact)?;

// Where the language is used:
let artifact: GrammarArtifact = postcard::from_bytes(&bytes)?;
let lang = EmbeddedLang::from_artifact(artifact);
```

The bytes are the encoding of this build of qana's types, and say
nothing of which build that was. A cache keeps each artifact under
`GrammarArtifact::key(source)`, which changes with the grammar's text and
with the code that certifies it: built in qana's workspace, with any
change to the sources of `qana-lang` and the crates it certifies with,
whether or not a version moved; built from a registry, with a new
version. An artifact found under the key this build computes is the one
this build would certify. `artifact.name()` and `artifact.extensions()`
say which language it is without loading it.

A worked integration lives outside this repo: `synkro_qana` in the
Synkro workspace implements that GUI framework's `TokenProvider` and
`CreaseProvider` over `EmbeddedLang`, giving its `CodeArea` widget
grammar-driven highlighting and folding.

---

## Exports

```bash
qana ts mylang/mylang.qana tree-sitter/mylang   # tree-sitter grammar + highlight queries
qana ast mylang/mylang.qana > src/ast.rs        # typed Rust AST over the green tree
```

The tree-sitter export gives you an escape hatch into the tree-sitter
ecosystem (Neovim, GitHub, Zed). It is a one-way emit: edit the `.qana`
and re-emit, not the reverse. A grammar whose tokens use a trailing
context (`(?=…)`, `(?!…)`) is refused: tree-sitter's lexer cannot
express one.

The typed AST is zero-copy accessors over the same green tree, with
production labels as variant names, so `LetStmt::name()` returns the
identifier token you labelled `name:` in the grammar.

---

## What is not here yet

An honest inventory, so nothing surprises you halfway in.

**Packaging.** Nothing is published. Build from source; there is no
`cargo install qana` from crates.io yet. The name family is free there as
of 2026-07-25 but unclaimed, so it stays free only until someone claims
it. The VS Code extension is a development shell — it is not packaged as
a `.vsix` or published to the marketplace.

**Language features.** The `.qana` surface has no parenthesized groups
(`(a b)?`); repetition sugar is `*`, `+`, `?`, and `% separator`, and
the repeated element must be a rule rather than a bare token. Composition
(one language hosting another, like fenced code blocks) exists and is
tested, but only through the Rust `compose()` API; there is no `island`
declaration on the `.qana` surface yet.

**Semantics.** The semantic layer does binding, scoping, and the
declared type tier described above — atoms, signatures, and name-flow,
but no type constructors, no subtyping, and no cross-file type flow.
Across files, the module tier resolves flat modules among the files a
session holds; units, paths and visibility are read into summaries for a
project index (`qana-index`), which resolves paths across a project's
files on demand; the index does not yet keep every name resolved as files
change, so it reports nothing unresolved. Custom diagnostics and quick fixes are not
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
| `qana new <dir> [--name L] [--ext .x]` | Scaffold a working grammar and sample document |
| `qana check <g.qana>` | Certify; print the envelope report, or the refusal with a counterexample |
| `qana tokens <g.qana> <file>` | The lex: every token, its style class, and each line's entry state |
| `qana parse <g.qana> <file> [--trivia] [--depth N]` | The lossless tree, losslessness check, and any repairs |
| `qana outline <g.qana> <file>` | Derived document symbols |
| `qana defs <g.qana> <file>` | Derived binding: definitions, references, resolution |
| `qana types <g.qana> <file> [--all]` | The declared type tier: typed definitions and mismatches |
| `qana edit <g.qana> <file> --line N --text "…"` | Incremental reparse: reuse, timing, and the batch differential |
| `qana ts <g.qana> <outdir>` | Emit a tree-sitter grammar and highlight queries |
| `qana ast <g.qana>` | Emit a typed Rust AST to stdout |

Exit codes: `0` success, `1` refusal or error in the input, `2` usage.
