# The `.rg` language reference

A `.rg` file is a complete language definition: lexical structure,
syntax, precedence, highlighting, outline, and name binding. It is
parsed and compiled by the same toolchain it describes — the surface is
[self-hosted](../crates/rantlr-rg/rg.rg), and `rg.rg` compiled by the
bootstrap must reproduce the bootstrap exactly.

For the practical path, start with the [guide](GUIDE.md).

---

## File shape

Declarations may appear in any order, but ids are assigned in file
order, so the conventional layout is:

```
language Name
max_stack 8            // only when using modes that push

token …                // lexical layer
keywords OWNER = …
mode NAME { … }
pair OPEN CLOSE
prec left|right …

start rule_name        // syntax layer
rule … = …
```

Comments are `//` to end of line.

---

## `language Name`

Names the language. Used in generated artifacts and reports.

## `max_stack N`

The static bound on mode-stack depth (envelope rule L2). Required when
any token pushes a mode; `N` must fit in a `u8`. Without pushes, omit
it — the bound is 0.

---

## Tokens

```
token NAME = pattern @attribute …
```

The pattern is either a **literal string** (`"{"`, `"else"`) or a
**regex between slashes** (`/[\a_][\w_]*/`).

### Pattern syntax

| Form | Meaning |
| --- | --- |
| `abc` | literal characters |
| `.` | any character except a line terminator |
| `[abc]`, `[a-z]` | character class |
| `[^abc]` | negated class (line terminators are always excluded) |
| `(…)` | grouping |
| `a\|b` | alternation |
| `x*` `x+` `x?` | repetition |
| `\d` | digit |
| `\a` | alphabetic |
| `\w` | alphanumeric |
| `\s` | whitespace (never a line terminator) |
| `\t` | tab |
| `\.` `\/` `\\` `\"` … | an escaped punctuation character, literally |

There are no PCRE class shorthands beyond those four: `\S`, `\D`, `\b`,
and `\p{…}` are refused rather than silently read as literal letters.
There are no anchors, backreferences, or lookaround — the patterns
compile to a DFA.

`\n` and `\r` are refused outright (envelope rule L1). Multi-line
constructs are expressed with modes, not multi-line tokens.

### Token attributes

| Attribute | Effect |
| --- | --- |
| `@trivia` | Whitespace/comments: kept in the tree, skipped by the parser |
| `@error` | A real, lossless token that is diagnostics-worthy (e.g. an unterminated string) |
| `@specialize` | Matched text is looked up in the keyword table and re-tagged |
| `@style(class)` | Highlight class (see below) |
| `@push(MODE)` | Enter `MODE` after this token |
| `@push(MODE, eol)` | Enter a LINE-BOUNDED mode: it pops automatically at end of line, never reaches another line's entry state, and edits inside it stay line-local (preprocessor directives are the canonical use). Every push of a mode must agree on `eol` |
| `@pop` | Leave the current mode after this token |

### Style classes

`keyword`, `variable`, `number`, `string`, `comment`, `operator`,
`punctuation`, `bracket`, `regexp`. These are the LSP semantic-token
legend; the VS Code extension maps them to TextMate scopes.

---

## `keywords OWNER = a b "c" …`

Declares keywords for an `@specialize` token. Each keyword gets its own
terminal, so rules can reference `"else"` directly instead of matching a
generic identifier.

Keywords are **owned** by the token they specialize. That is what keeps
composed languages separate: a host language's keyword remains an
ordinary identifier inside a guest island.

Literal spellings in RULES resolve to DEFAULT-mode, non-trivia tokens
only — a mode-local `"if"` (in a preprocessor mode, say) never shadows
the base language's keyword. Reference mode-local tokens BY NAME in
productions.

Bare words and quoted strings are both accepted; quote a word when it
collides with a `.rg` keyword (`language`, `max_stack`, `keywords`,
`token`, `mode`, `pair`, `prec`, `left`, `right`, `start`, `rule`).

---

## `mode NAME { token … }`

A lexer mode: a distinct token set entered with `@push(NAME)` and left
with `@pop`. Nested block comments are the canonical use:

```
token BLOCK_OPEN = "/*" @trivia @push(BLOCK) @style(comment)

mode BLOCK {
  token B_OPEN    = "/*"      @trivia @push(BLOCK) @style(comment)
  token B_CLOSE   = "*/"      @trivia @pop         @style(comment)
  token B_CONTENT = /[^*\/]+/ @trivia              @style(comment)
  token B_MISC    = /[*\/]/   @trivia              @style(comment)
}
```

Modes may push themselves. `max_stack` bounds the nesting, which is what
keeps a line's entry state a small value the engine can cache.

---

## `pair OPEN CLOSE`

Declares a bracket pair by token name or literal. Pairs drive folding
and the incremental skeleton that lets the engine find a damaged
region's enclosing block without reparsing.

---

## `prec left|right tok …`

Precedence levels, lowest first — the same convention as yacc. Each
declaration binds tighter than the ones before it:

```
prec left "+" "-"
prec left "*" "/"
```

A production's precedence defaults to its last terminal; override it per
alternative with `@precedence(token)`.

Precedence is how you keep an expression grammar inside the envelope.
Without it, `expr "+" expr` is ambiguous and `rantlr check` refuses it.

---

## `start rule_name`

The start symbol.

---

## Rules

```
rule name = Label: sym sym … @attribute …
```

Multiple alternatives are separated by `|`, and a leading `|` is
allowed:

```
rule stmt =
  | LetStmt: "let" name:IDENT "=" expr ";" @def(name)
  | ExprStmt: expr ";"
```

Every alternative carries a **label** (`LetStmt`). Labels become node
kinds in the tree, variant names in the generated typed AST, and the
handles `@outline`/`@def`/`@ref` refer to.

### Symbol forms

| Form | Meaning |
| --- | --- |
| `name` | another rule, or a token by name |
| `"lit"` | a token by its literal text |
| `label:name` | a named position (`@def`/`@ref`/`@outline` target it) |
| `label:"lit"` | a named literal position |
| `name?` `name*` `name+` | optional / zero-or-more / one-or-more |

Repetition inside an alternative generates a shared helper rule per
(symbol, operator) pair — every `expr?` in the grammar is the same
`expr_opt` rule, and `stmt*` becomes `stmt_star`. Symbol-level
repetition takes no separator; use a rule-level list for that.

**The repeated element must be a rule, not a token.** `IDENT*` is
refused; wrap it first, which is also what gives the list a typed node
to hold:

```
rule params = param*
rule param  = Param: name:IDENT @def(name)
```

### Rule-level EBNF sugar

```
rule stmts = stmt*            // zero or more
rule items = item+            // one or more
rule args  = expr* % ","      // zero or more, comma-separated
rule ops   = op+   % "|"      // one or more, pipe-separated
rule tail  = suffix?          // optional
```

These desugar to ordinary left-recursive productions, which the engine
then recognises as **auto-balanced lists** (envelope rule L4): the tree
holds shallow balanced runs instead of a cons spine, so editing one item
in a 10,000-item file stays cheap. The generated labels — which is what
you see in `rantlr parse` — follow a fixed convention:

| Sugar | Generated alternatives |
| --- | --- |
| `x*` | `…Empty`, `…More` |
| `x+` | `…First`, `…More` |
| `x?` | `…None`, `…Some` |
| `x+ % s` | `…First`, `…More` (with separator) |
| `x* % s` | `…None`, `…Some` over an inner `name_ne` rule |

A sugar rule is a whole rule, not an alternative, so it carries **no
attributes** — `rule stmts = stmt* @scope` is a syntax error. Put the
attribute on a labelled alternative that contains the list instead
(`rule block = Block: "{" stmts "}" @scope`).

### Rule attributes

Attributes attach to a **labelled alternative**, after its symbols.

| Attribute | Effect |
| --- | --- |
| `@def(label)` | The token at `label` defines a name |
| `@ref(label)` / `@ref(label, call)` | The token at `label` references a name; kinds are `var` (default) and `call` |
| `@scope` | This node opens a lexical scope (names inside are invisible outside) |
| `@scope(unordered)` | A scope where forward references are legal (declaration languages) |
| `@outline(label)` / `@outline(label, kind)` | Contribute a document symbol; kinds are `variable` (default), `constant`, `function`, `struct`, `module`, `class` |
| `@precedence(token)` | Override this alternative's precedence (yacc's `%prec`) |
| `@type(…)` | Declare this alternative's typing rule (see **The type tier** below) |
| `@export` | This alternative's `@def` is visible to other files (see **The module tier**) |
| `@import(label)` | The `label` token names an import: it resolves against other files' EXPORTS only (see **The module tier**) |
| `@module(body)` | The `@def` on this alternative introduces a NAMESPACE; the defs inside the `body` child are its members |
| `@qualify(base, name)` | The `name` token resolves among the members of what `base` resolves to (`a::b` paths; nest via a recursive path rule) |
| `@ns(name)` | This alternative's `@def`/`@ref` live in the NAMED namespace `name`: refs only bind same-namespace defs, and named namespaces resolve hoisted (order-free) in every scope — per-namespace ordering (C struct tags are forward-declarable while values stay define-before-use) |
| `@macro([params,] body)` | The meta tier: this alternative's `@def` introduces a MACRO whose parameters are the defs inside the `params` child and whose template is the `body` child — real syntax, parsed and bound at the definition. Substitution is binding-guided: body refs that resolve to a parameter get the corresponding argument's text. It is also SHAPE-PRESERVING: the expander compares the declared `prec` strengths of the argument, the hole, the body, and the use site, and where splicing would regroup them it wraps the text in the rule's own grouping production (`Paren: "(" expr ")"` — discovered by shape, whatever the delimiters are). A body with no declared shape (C's `pp_tokens`) splices textually, and a rule with no grouping production yields a diagnostic rather than a silent regrouping |
| `@splice(name[, args])` | Expansion may happen HERE: when `name`'s `@ref` resolves to a macro, this node is replaced by the substituted body (`args` is the argument-list child for parameterized macros). Refs without `@splice` never expand — C's `#ifdef X` stays closed while `return X;` opens. Run `rantlr expand` to materialize (deterministic `<stem>.exp.<ext>` + provenance sidecar, write-if-changed, `--check` = the read-only drift gate) |
| `@reflect(ty[, "sep"])` | REFLECTION, on a `@splice` alternative: the macro's two parameters bind PER MEMBER of the type `ty` resolves to — parameter 1 the member's name (substituted at binding refs and at the type tier's member-name positions), parameter 2 its declared type — one body per member, joined by `sep`. The member map is the type tier's own `deftype`/`def` declarations, in whichever file declared them: a type resolving into a sibling reflects its members from there, and provenance names that file. Macro bodies are type-EXEMPT at the definition (templates type per instantiation, in the materialized output) |

Attribute names cannot be `.rg` reserved words, which is why the
precedence override is spelled `@precedence` rather than `@prec`. Use it
for the classic unary-minus case:

```
prec left "+" "-"
prec left "*"

rule expr =
  | Sub: expr "-" expr
  | Mul: expr "*" expr
  | Neg: "-" expr @precedence("*")     // binds tighter than subtraction
  | Lit: NUM
```

`@def`, `@ref`, and `@scope` are the entire configuration for
go-to-definition, find-references, rename, and scope-aware completion.
There is no separate symbol-table pass to write.

`@scope(unordered)` on the **start rule** declares the whole file an
unordered scope — a declaration language, where a top-level name is
visible to references that appear before it. Without it, top-level names
follow definition-before-use.

---

## The type tier

The toolchain predefines no types. A grammar declares its own vocabulary
and per-production typing rules with `@type`, and one generic engine
derives type assignment and mismatch diagnostics from that data. A
grammar with no `@type` annotations has no type tier.

At most one `@type` per alternative. Forms:

| Form | Meaning |
| --- | --- |
| `@type(Atom)` | Nodes of this alternative have the atomic type `Atom` |
| `@type(of, label)` | The type of the child at `label` |
| `@type(sig, p…, R)` | The alternative's **rule symbols** (in order) must have types `p…`; the node has type `R` |
| `@type(def, label)` | The name defined here (requires `@def`) carries the type of the child at `label`; the node itself stays untyped |
| `@type(ref)` | The type the resolved definition CARRIES (a value's type) |
| `@type(deftype)` / `@type(deftype, body)` | The name defined here (requires `@def`) INTRODUCES a document-level type; with `body`, the typed defs inside that child are the type's MEMBERS (without it the type is opaque) |
| `@type(named)` | The type the resolved definition IS (requires `@ref`; the target must be a `deftype` — a resolved non-type name is diagnosed) |
| `@type(fn, params, rt)` | This node has an ARROW type: the typed defs inside `params` (in order) → the type of `rt`. Hand it to the declaration's name with `@type(def, …)` |
| `@type(apply, args)` | Call checking (requires `@ref`): the callee's arrow is checked against `args`'s items — arity and each argument — and the node has the return type. A non-arrow callee is diagnosed as not callable |
| `@type(returns, e)` | `e` is checked against the nearest enclosing `fn` node's declared return type |
| `@type(member, base, name)` | The `name` TOKEN is looked up in `base`'s type's member set; the node has the member's type. Missing members ("no member `z` on `Point`") and memberless base types ("type `Num` has no members") are diagnosed |

Type atoms are **Capitalized** names, invented freely — `Num`, `Str`,
`Temperature`. Lowercase names inside a `sig` are **type variables**,
unified per node: `@type(sig, t, t, t)` accepts `Num + Num` and
`Str + Str` but reports the exact operand of a mixed pair.

Checked at grammar compile time, with spans: a `sig` whose parameter
count differs from the alternative's rule-symbol count, an unknown
label, a label naming a token where a rule is required, `@type(def, …)`
without `@def`, `@type(ref)` without `@ref`, and lowercase bare atoms
are all refused before any document is ever parsed.

Semantics at check time: synthesis is bottom-up; types cross names via
the binding tier's resolution (def→ref chains converge regardless of
declaration order); **unknown never cascades** — unresolved names and
parse-repaired regions type as nothing, and a diagnostic is emitted
only where two known types disagree.

**Document-level types.** `@type(deftype)` opens the vocabulary at the
document level: each declaration (a `struct`, say) introduces a type
named by its `@def` child, and `@type(named)` in type positions and
constructor expressions denotes it. Identity is the **declaration
site**, not the name — two `struct T`s in different scopes are
different types, and which `T` an annotation denotes is ordinary
scoped name resolution. Shadowing therefore produces real mismatches,
shown as ``expected `T`, found `T``` (a display refinement is planned).
The grammar's atoms and the document's types unify seamlessly in
signatures and mismatch reports.

**Functions.** `@type(fn, …)` assembles arrow types (displayed
`fn(Num, Num) -> Num`) from the parameter definitions and the return
annotation. Because the arrow is an ordinary type a def carries,
functions flow like values: a plain `let g = add;` makes `g` callable,
and recursion converges through the same fixpoint that handles forward
references. The `rt` child must precede the body in the production
(it supplies the expectation `returns` checks against).

**Members.** With `@type(deftype, body)`, fields are nothing special:
any typed def inside the body child is a member, so struct-typed
fields chain (`l.a.x`) and member types feed signatures and
annotations like any other type. A member whose own type is unknown
makes access silent (the member exists) rather than a false
"no member". Membership is span-based in this version: all defs
within the body child count, including nested ones — scope-precise
membership arrives with methods.

**The vocabulary is global.** Document types and arrows live in one
TypeId space per language instance, so types declared in one file flow
to every other — values, annotations, members, and calls alike. A
document type's cross-file identity is (file, name, occurrence);
within-file shadowing keeps distinct types.

Current limits: no type constructors, no subtyping, list-shaped
children untyped, span-based membership.

---

## The module tier

The toolchain predefines no module system; a grammar declares one.
`@export` on a def-carrying alternative makes that definition visible
to other files; `@import(label)` makes a token position an import,
resolving against other files' exports. Combine `@def` and `@import`
on the same token for `use x;` (the local binding and the foreign
reference are one name), or on different labels for `use x as y;`.

Declaring either form activates STRICT semantics, Rust-flavored:

* definitions are file-private unless `@export`ed;
* cross-file resolution goes through imports only — no ambient names;
* importing a private name is its own diagnostic ("exists but is not
  exported"), distinct from a typo's "cannot find";
* go-to-definition on an import jumps THROUGH to the foreign export;
* types flow through imports (`@type(ref)` on the import production);
* and `@export` is an incrementality contract: a file's signature is
  its export surface, so editing a private definition can never
  re-resolve another file.

A grammar declaring neither form keeps the open world: every top-level
name is ambiently visible to every file.

**Namespaces and paths.** `@module(body)` is the binding-level twin of
`@type(deftype, body)`: the def introduces a namespace whose members
are the defs inside the body child (give the body its own `@scope` so
members stay out of the enclosing namespace). `@qualify(base, name)`
resolves `name` among the members of whatever `base` resolves to;
nested paths (`a::b::c`) come from a left-recursive path rule with
`@qualify` on the segment. Path bases chase through imports —
`use math;` then `math::pi` lands in the exporting file — and through
re-export chains (`pub use` is just `@export` on an import; it needs
no new form). Same-file paths see all members; crossing a file
requires the member exported. Path errors are precise: "no member",
"is not a module", and "exists in the module but is not exported".
`@type(ref, label)` selects which reference feeds a type on
multi-reference productions (a path's base and name).

Not yet built: visibility LEVELS (`pub(crate)`-style) — deferred until
a unit hierarchy (directories/packages) exists for them to scope over;
today's units are flat files. Completion after `::` and rename through
paths are named refinements.

---

## The envelope

The rules a grammar must satisfy. Each is checked, and each refusal
carries a witness — a concrete input, not a category.

| Rule | Requirement | Why it buys something |
| --- | --- | --- |
| **L1** | No token may match a line terminator | A line's lexical state is self-contained, so relexing restarts at any line |
| **L2** | The mode stack is statically bounded | A line's entry state is a small cacheable value |
| **L3** | The grammar is deterministic LR(1); conflicts are errors | Parses are unambiguous, and reuse is sound |
| **L4** | Sequence rules are detected and auto-balanced | Edits in long lists cost O(log n), not O(n) |
| **L5** | Parsing is pure — no side effects, no semantic feedback | The same input always yields the same tree, so subtrees can be reused |
| **L6** | Modes are containment-checked | A damaged region has a bounded blast radius |
| **L8** | Binding is data (`@def`/`@ref`/`@scope`), not code | Name resolution is derivable and memoizable |
| **L9** | Signature and body are firewalled | Editing a function body cannot invalidate other files |

L1 through L4 are what `rantlr check` reports on directly. L5, L6, L8,
and L9 are structural: they are properties of the design that the
surface has no way to express a violation of.
