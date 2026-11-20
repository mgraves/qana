# The `.qana` language reference

A `.qana` file is a complete language definition: lexical structure,
syntax, precedence, highlighting, outline, and name binding. It is
parsed and compiled by the same toolchain it describes — the surface is
[self-hosted](../crates/qana-lang/qana.qana), and `qana.qana` compiled by the
bootstrap must reproduce the bootstrap exactly.

For the practical path, start with the [guide](GUIDE.md).

---

## File shape

Declarations may appear in any order, but ids are assigned in file
order, so the conventional layout is:

```
language Name
extensions "ext"       // the file extensions that are this language's
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

## `extensions a "b" …`

The file extensions that are this language's, without the dot:
`extensions "c" "h"`. An editor that hosts several languages picks the
language for a file by them, so it needs no table of its own. They are
read when the grammar compiles (`compile_source(..).def.extensions`),
which does not certify it, and a certified `EmbeddedLang` carries them
with its `name`, as does the `GrammarArtifact` it is loaded from
(`extensions()`, `name()`). Words and quoted strings are both accepted, as in
`keywords`; quote one that starts with a digit or is a `.qana` keyword.
An extension written with its dot (`".rs"`), an empty one, one with a
dot, slash or space in it, one listed twice (case aside) and a second
`extensions` declaration are refused. Declaring none is allowed: the
language is then not chosen by extension.

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
| `(?=…)` | trailing context: accept the token only if `…` follows it |
| `(?!…)` | negated trailing context: accept the token only if `…` does not follow it |

There are no PCRE class shorthands beyond those four: `\S`, `\D`, `\b`,
and `\p{…}` are refused rather than silently read as literal letters.
There are no anchors, backreferences, or lookbehind, and no lookahead
inside a pattern — the patterns compile to a DFA. The one condition a
token can carry is on what follows it: a trailing context.

### Trailing context

A token is the **longest match**, and among tokens of the same length
the one declared first. The lexer reads ahead as far as any token can
reach and backs up to the last place one ended. That alone cannot turn
down a longer match because of what comes after it, and some languages
need exactly that. In Rust `1.` is a float, unless a `.`, a `_` or a
letter follows: `1..2` is a range of integers and `1.foo()` is a method
call.

```
token FLOAT = /\d+\.\d+|\d+\.(?![._\a])/ @style(number)
token INT   = /\d+/                        @style(number)
```

`(?=…)` and `(?!…)` end a pattern, or one branch of it. They match no
characters. The token before them is accepted only if the rest of the
line begins with a match of `…` (`(?=…)`), or does not (`(?!…)`).

| Input | Tokens | Why |
| --- | --- | --- |
| `1.5` | `FLOAT` | the first branch has no context |
| `1.` at the end of a line | `FLOAT` | nothing follows, so `(?!…)` holds |
| `1. ` | `FLOAT` `WS` | a space is not `.`, `_` or a letter |
| `1..2` | `INT` `..` `INT` | a `.` follows `1.`, so the lexer falls back to `1` |
| `1.foo()` | `INT` `.` `IDENT` … | a letter follows `1.` |

The rules:

- **The context is a pattern**, of any length: `/\a+(?=\s*:)/` is a word
  followed, after any spaces, by a colon.
- **Longest match still comes first.** A conditional token is tried at
  the longest place it could end, then at shorter ones; it gives way to
  another token only where its context fails.
- **Declaration order still breaks ties.** At the same length the
  earliest-declared token whose context holds wins, so an unconditional
  token declared before a conditional one of the same length always
  wins. Declare the conditional token first.
- **The end of the line is "nothing follows".** `(?=…)` fails there and
  `(?!…)` holds. `(?!.)` therefore means "at the end of the line".
- **A context reads its own line only.** It never sees the line
  terminator or the next line, so a line's tokens still depend on
  nothing but that line and its entry state (L1), and an edit relexes
  what it always relexed.
- **It only ends a token.** Nothing may follow it; it cannot be repeated
  (`*`, `+`), be optional by itself, or contain another context. A group
  that ends in one may be optional: `/a(b(?=c))?/`. A context that can
  match the empty string is refused, since it would hold everywhere.

A context costs nothing where it is not used: a grammar without one runs
the lexer it always ran. Where one is used, the lexer checks it once,
at the longest candidate, in the common case.

Tree-sitter's lexer has no equivalent, so `qana ts` refuses a
grammar that uses one rather than emit a grammar for a different
language.

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

The legend is one closed set, shared by every grammar, so that a theme
says once what each class looks like:

| Class | What it is |
| --- | --- |
| `keyword` | a keyword |
| `variable` | a name |
| `number` | a number |
| `string` | a string or character literal |
| `comment` | a comment |
| `doc_comment` | a comment that documents what follows it (`///`, `/** */`) |
| `operator` | an operator |
| `punctuation` | punctuation |
| `bracket` | a bracket |
| `regexp` | a pattern literal |
| `lifetime` | a lifetime (`'a`) |
| `type` | the name of a type |
| `function` | a function's name, where it is defined and where it is called |
| `method` | the name a method call calls (`x.name()`) |
| `macro` | a macro's name, where it is invoked or defined |
| `decorator` | an attribute or annotation (`#[derive(Debug)]`) |
| `label` | a label a loop or block is given, and a `break` names |

A token's `@style(class)` gives the class to every token of its kind.
What a name is depends on where it stands, so a class can also be given by
the syntax: an alternative's `@style(class, label…)` (see **Classes from
the syntax** below). Where the syntax gives a token a class, it is the
token's class; elsewhere the token's kind's is.

The classes are LSP's semantic token types where LSP has one. The LSP
server sends `doc_comment` as `comment` with the `documentation` modifier,
and the rest by their names; `punctuation`, `bracket` and `lifetime` are
qana's own, which the VS Code extension declares (falling back to
`punctuation` and `typeParameter`) and maps, with the others, to TextMate
scopes. `qana ts` exports the token kinds' classes to `highlights.scm`;
the syntax's are not exported, and it says so.

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

A keyword's token, as parse trees and conflict reports name it, is `KW_`
and its text in capitals: `while` is `KW_WHILE`. A keyword with a capital
of its own keeps its text as written, so Rust's `self` and `Self` are
`KW_SELF` and `KW_Self`. A keyword whose name a declared token already
has (`token KW_FN` beside the keyword `fn`) is refused. Completion
offers a keyword by its text, never by its token's name.

Bare words and quoted strings are both accepted; quote a word when it
collides with a `.qana` keyword (`language`, `max_stack`, `keywords`,
`token`, `mode`, `pair`, `prec`, `left`, `right`, `nonassoc`, `start`,
`rule`, `extensions`).

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

Declares a bracket pair by token name or literal. Pairs drive the
incremental skeleton that lets the engine find a damaged region's
enclosing block without reparsing, and error recovery's reading of
structure: a repair that inserts a bracket claims how every real bracket
after it pairs, and a bracket opened and not yet closed holds the tokens
up to its closer (see `docs/incremental_recovery.md`). Folding comes from
the syntax (`@fold`, below), not from pairs.

---

## `prec left|right|nonassoc tok …`

Precedence levels, lowest first — the same convention as yacc. Each
declaration binds tighter than the ones before it:

```
prec nonassoc "<" ">"
prec left "+" "-"
prec left "*" "/"
```

`left` groups `a - b - c` as `(a - b) - c`, `right` as `a - (b - c)`.
`nonassoc` does not group at all: a second operator of the level right
after the first one's operand is a syntax error, as Rust's comparisons
are (`a < b < c` must be parenthesized).

A production's precedence defaults to its last terminal; override it per
alternative with `@precedence(token)`.

Precedence is how you keep an expression grammar inside the envelope.
Without it, `expr "+" expr` is ambiguous and `qana check` refuses it.

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
| `label:name` | a named position (`@def`/`@ref`/`@outline`/`@style` target it) |
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
you see in `qana parse` — follow a fixed convention:

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
| `@def(label)` / `@def(label, flag…)` | The name at `label` is defined (see **Names through rules** for a label on a rule). Flags, each at most once: `hoisted` — visible throughout its scope, before it as well as after, even in an ordered scope (Rust's items in a block, or at the top of a file whose `let`s stay define-before-use); `open` — the namespace the name introduces has members the binding cannot see (a Rust type's `impl` blocks, a macro's expansion, a glob import), so a qualified name under it that is not among the members it does see escapes instead of being an error. Without `@module`, an open name has no members the binding sees: `Point::new` under `struct Point` escapes |
| `@ref(label)` / `@ref(label, kind)` | The name at `label` is referenced; kinds are `var` (default), `call`, and `escaping` — a name that may be defined where the file cannot see (a prelude, another crate, a glob import, a macro's expansion): it binds as `var` does when the file defines it, and otherwise is never diagnosed or painted unresolved |
| `@defer(pattern)` / `@defer(pattern, after)` | The names the `pattern` child defines take effect after the `after` child, or after this whole node: `let x = x;` reads the earlier `x`, and `for x in xs` reads `xs` before the loop's `x` exists |
| `@scope` | This node opens a lexical scope (names inside are invisible outside) |
| `@scope(kind…)` | A scope with any of these, each at most once: `unordered` — forward references are legal (declaration languages); `barrier` — names outside it are not seen inside (a Rust module, which sees its parent's items only through `use super::…`); `members` — what is defined directly in it is reached only by a qualified name, never a plain one (a type's fields, an enum's variants, an `impl`'s or a trait's items) |
| `@outline(label)` / `@outline(label, kind)` | Contribute a document symbol named by the symbol at `label`: a token's text, or for a rule, the first `@def` inside it, and failing that its text (`Point<T>`); kinds are `variable` (default), `constant`, `function`, `struct`, `module`, `class` |
| `@style(class, label…)` | The symbols at the labels are painted `class`: a token takes it, a rule is given it and passes it on where its alternatives say (see **Classes from the syntax** below) |
| `@style(inherit, label…)` | The symbols at the labels are painted the class this alternative's node is given, if it is given one |
| `@fold(kind)` / `@fold(kind, from)` / `@fold(kind, from, to)` | The construct folds when it runs over lines: from its first token, or the first token of the symbol labelled `from`, to its last token, or the last token of the symbol labelled `to` (see **Folding** below) |
| `@precedence(token)` | Override this alternative's precedence (yacc's `%prec`) |
| `@type(…)` | Declare this alternative's typing rule (see **The type tier** below) |
| `@export` | This alternative's `@def` is visible to other files (see **The module tier**) |
| `@import(label)` | The `label` token names an import: it resolves against other files' EXPORTS only (see **The module tier**) |
| `@module(body)` | The `@def` on this alternative introduces a NAMESPACE; its members are the body's own defs — the shallowest inside the `body` child, so a `let` in one of a module's functions is none of them |
| `@qualify(base, name)` | The `name` token resolves among the members of what `base` resolves to (`a::b` paths; nest via a recursive path rule). The base is the name the `base` child passes on (`Vec` of `Vec::<T>`), or, when it passes none, the last reference inside it |
| `@ns(name)` | This alternative's `@def`/`@ref` live in the NAMED namespace `name`: refs only bind same-namespace defs, and named namespaces resolve hoisted (order-free) in every scope — per-namespace ordering (C struct tags are forward-declarable while values stay define-before-use) |
| `@ns(name, label…)` | The subtrees at the labels stand in namespace `name`: every def and ref inside them without a namespace of its own takes it. One path rule serves values, types and macros, each use giving it its namespace (`MacUse: p:path "!" … @ns(macro, p)`). It places only the labelled children: an alternative whose own `@def` stands in the namespace too says `@ns(name)` as well, and one whose children stand in different namespaces gives several, each child in one |
| `@macro([params,] body)` | The meta tier: this alternative's `@def` introduces a MACRO whose parameters are the defs inside the `params` child and whose template is the `body` child — real syntax, parsed and bound at the definition. Substitution is binding-guided: body refs that resolve to a parameter get the corresponding argument's text. It is also SHAPE-PRESERVING: the expander compares the declared `prec` strengths of the argument, the hole, the body, and the use site, and where splicing would regroup them it wraps the text in the rule's own grouping production (`Paren: "(" expr ")"` — discovered by shape, whatever the delimiters are). A body with no declared shape (C's `pp_tokens`) splices textually, and a rule with no grouping production yields a diagnostic rather than a silent regrouping |
| `@splice(name[, args])` | Expansion may happen HERE: when `name`'s `@ref` resolves to a macro, this node is replaced by the substituted body (`args` is the argument-list child for parameterized macros). Refs without `@splice` never expand — C's `#ifdef X` stays closed while `return X;` opens. Run `qana expand` to materialize (deterministic `<stem>.exp.<ext>` + provenance sidecar, write-if-changed, `--check` = the read-only drift gate). Expansion is HYGIENIC: every reference that survives must resolve to the same definition afterwards as where it was written, and where it would not, the expander renames the binder that swallowed it (reported as a note) — falling back to a diagnostic when no name the grammar admits is available |
| `@reflect(ty[, "sep"[, facet…]])` | REFLECTION, on a `@splice` alternative: the macro's parameters bind PER MEMBER of the type `ty` resolves to — one body per member, joined by `sep`. The parameters take the declared FACETS in order (default `name, type`; available: `name`, `type`, `owner`, `index`, `count`), and a facet/parameter count mismatch is diagnosed. `name`/`type`/`owner` are copies of the declaration, so provenance points at the field or the struct; `index`/`count` are computed. The member map is the type tier's own `deftype`/`def` declarations, in whichever file declared them: a type resolving into a sibling reflects its members from there. Macro bodies are type-EXEMPT at the definition (templates type per instantiation, in the materialized output) |

Attribute names cannot be `.qana` reserved words, which is why the
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

**Names through rules.** The label of a `@def`, `@ref` or `@qualify`
may name a rule rather than a token. The name is then the token the rule
passes its class to: through `@style(inherit, label)` with a single
label, and through the rules EBNF sugar generates, however deep the
chain. With `rule name = Name: t:IDENT @style(inherit, t)`,
`Bind: n:name @def(n)` defines the identifier, and so does a pattern
rule that passes to `name`. A rule that passes to several symbols, or
to none, has no one token to bind, and naming it is an error.

**Lists that keep their nodes.** A rule of list shape (two
alternatives, `L → L α` and a seed) is stored as balanced runs, whose
nodes are not its alternatives'. When either alternative carries an
attribute (other than `@precedence`), the rule keeps its nodes
instead, so that what the attribute says of the node holds:
`More: b:path "::" n:name @qualify(b, n)` qualifies every segment. A
rule kept this way is a chain as deep as the list is long; for a long
list, annotate the element rule rather than the list.

`@scope(unordered)` on the **start rule** declares the whole file an
unordered scope — a declaration language, where a top-level name is
visible to references that appear before it. Without it, top-level names
follow definition-before-use.

### Classes from the syntax

A token's `@style` colours every token of its kind alike, but what a name
is depends on where it stands: `Vec` in `x: Vec<u8>` is a type, `push` in
`v.push(1)` a method, `println` before `!` a macro. An alternative says so:

```
rule fn_def = FnDef: "fn" n:name "(" params ")" block @style(function, n)
rule call   = Call:  c:expr "(" args ")"            @style(function, c)
rule ty     = PathType: p:path                      @style(type, p)
```

A label on a token paints that token. A label on a rule GIVES the rule the
class, and the rule passes it on where its own alternatives say, with
`@style(inherit, label…)`. That is how a class reaches the one token that
names a construct, however deep it is:

```
rule path =
  | One:  n:name                    @style(inherit, n)   // `Vec`
  | More: path "::" n:name          @style(inherit, n)   // `std::collections::HashMap`: only `HashMap`
rule name = Name: t:IDENT           @style(inherit, t)
rule expr =
  | PathExpr: p:path                @style(inherit, p)   // `f` in `f(x)`
  | Paren:    "(" expr ")"                               // `(f)(x)`: nothing passed on
```

A list or an option (`x*`, `x+ % ","`, `x?`) passes what it is given to
each element; a list's separators get nothing. What recovery set aside gets
nothing; a token recovery inserted holds its symbol's place. A token is
painted by its parent's alternative alone, so two classes never meet on
one token. A rule given a class must pass it on to a token, directly or
through rules that do: one that passes nothing on is a compile error, as
are an unknown class, a label styled twice, and `@style` on an alternative
with no label.

A live document keeps these classes across edits by subtree, as it keeps
its folds: a subtree the parser did not rebuild, given the class it was
given before, is not read again. An edit repaints the lines it changed,
and also the other lines whose classes it changed (typing `(` at the start
of the next line makes a name a call). `qana_services::syntax_classes`
lists what a tree's syntax paints.

### Folding

A document folds where its syntax says it does. `@fold(kind)` on an
alternative makes its construct foldable wherever one runs over lines;
`kind` is a lowercase name the grammar chooses (`block`, `list`,
`string`), and editors map the names onto their own fold rules, as themes
map style classes. A construct whose brackets sit among other symbols
names where its fold begins and ends:

```
rule block   = Block:  "{" stmts "}" @fold(block)                          // all of it
rule mod_def = ModDef: "mod" n:name open:"{" items "}" @fold(block, open)  // from `{` to the end
rule fn_def  = FnDef:  "fn" n:name open:"(" params close:")" ret body
                       @fold(list, open, close)                            // `(` to `)`
```

A fold's head is its first token; an editor keeps that line when it
folds. The start rule cannot fold (its construct is the whole document).
An outline entry folds too: the folds are the regions that are not
outline structure, in a channel of their own (`Limner::folds_shared`).

Comments are trivia, not syntax, so they fold by their tokens, without
being asked, as the `comment` kind: a comment that runs over lines (a
token styled `comment` followed by tokens of its mode, such as a block
comment's body), and two or more lines in a row holding nothing but
comments. `qana folds <grammar> <file>` shows what folds.

A live document keeps its folds and its outline across edits by subtree:
after an edit only the subtrees the parser rebuilt are read again, and a
large one (a module that is the whole file) child by child.

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
A qualified name ESCAPES — is neither an error nor painted unresolved —
when its base escapes (`@ref(base, escaping)`, unresolved), segment
after segment (`std::mem::swap` in a file that cannot see `std`); when
its base holds no reference (`self::x` before the grammar declares path
roots); and when its base names an open namespace (`@def(x, open)`)
that does not show the name. A base that resolves to a closed module
keeps the precise errors above.
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

L1 through L4 are what `qana check` reports on directly. L5, L6, L8,
and L9 are structural: they are properties of the design that the
surface has no way to express a violation of.
