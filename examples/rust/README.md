# Rust

`rust.qana` is Rust's syntax, edition 2021 with what later releases made
stable (let chains, `&raw`, `unsafe extern`, `use<..>` bounds, async
closures), as one deterministic LR(1) grammar. Its header says how each
choice that needed one was made; every one is rustc's own reading.

## Lexing

It lexes as [`../rust-lex/rust_lex.qana`](../rust-lex/rust_lex.qana), the
lexical grammar measured against rustc's own lexer, does: every line is
cut into the same tokens and leaves the lexer in the same mode, so the
measurement in [`../rust-lex/README.md`](../rust-lex/README.md) holds for
this grammar too. Which token a piece of text is differs:

* **Keywords.** Rust 2021's strict keywords (with `async`, `await` and
  `dyn` from 2018) and its reserved words are tokens of their own.
  `self` and `Self` are two keywords. A raw identifier (`r#fn`) is a
  name. `_` alone is a token of its own too, as the Reference has it (an
  identifier is not `_`): the wildcard pattern, the inferred type, the
  underscore expression, `const _`, `as _`.
* **Weak keywords** (`union`, `auto`, `default`, `raw`, `safe`) are
  keywords only before what makes them so, read from the rest of the
  line, and names everywhere else: `a.union(b)` and
  `Default::default()` are names. `macro_rules` is a name, and
  `macro_rules! name { … }` the macro invocation that takes a name.
* **Punctuation** is one character per token, as rustc_lexer cuts it,
  each lexed as the operator it begins: `>` before `>=` begins `>>=`,
  before `>` a shift, before `=` a comparison. The parser knows at the
  first character which operator it reads, and in a type takes any `>`
  as a closer, so `Vec<Vec<u8>>=` closes two generic lists and leaves
  `=`.

`cargo test -p qana-lang --test rust` checks the lexing: on the sample
against rustc_lexer's record, and on this workspace's own Rust against
the measured grammar.

## Syntax

`cargo test -p qana-lang --test rust_syntax` pins the tree rustc's
reading gives each construct the grammar had to decide (operator
grouping, the condition tier's missing struct literal, block-like
statements, let chains and `let … else`, closures, casts, paths,
patterns) and the errors rustc reports (chained comparisons, `&dyn A +
B`, `x as u8 < y`).

Measured over real code, 68,103 files: this workspace, Synkro, and
the 2,002 crates of a cargo registry (rust-analyzer's parser tests among
them). 67,524 parse with no repair. Each of the other 579 is not stable
Rust 2021: code of the 2015 and 2018 editions (`try!`, bare trait
objects), unstable syntax (`builtin #`, `box`, `gen`, `yield`, `try`
blocks, `if let` guards, const traits, macros 2.0, trait aliases and the
like), test fixtures rustc rejects on purpose (`static _`, a variant
named `_`), or one of the omissions below.

What it leaves out:

* a `#!` shebang line;
* a raw string with more than three `#`, and block comments nested
  deeper than 8 (the lexical grammar's limits);
* `impl <T as A>::B {}`, an impl on a qualified path with no generics:
  rustc reads past `impl <` to tell generics from a qualified type,
  which one token of lookahead cannot. `impl<> <T as A>::B {}` parses.
* a weak keyword whose next word is on the next line (a trailing
  context reads its own line only);
* a path that goes on after `Fn(A) -> B` sugar
  (`F::(A) -> (B)::C`): the sugar ends its path, which is what reads
  `-> B::C` as `B::C`;
* unstable syntax (`box`, `yield`, `try` blocks, `do yeet`, `gen`
  blocks, coroutine closures, `impl const`, `~const`, default field
  values, trait aliases, type ascription, `builtin #`). Macros 2.0's
  `macro` items, which core and std use, are read (since that
  measurement);
* the 2015 edition's anonymous trait method parameters and bare trait
  objects.

## What it derives

The outline: functions, types, traits, impls, modules, constants and
macro definitions (`@outline`).

Names within a file (`@def`, `@ref`, `@scope`), bound as rustc resolves
them where one file can tell:

* items (functions, types, traits, modules, constants, statics, `use`
  leaves, `extern crate`, `macro_rules!`) are visible throughout their
  module or block, before their declaration too; generic parameters
  throughout their item;
* a pattern's names take effect after their `let` (`let line =
  line.trim()` reads the earlier `line`), after a `for` loop's iterator,
  after an `if let` or `while let` scrutinee; what a condition binds
  reaches its block, the conditions after it, and not its `else`; a
  match arm scopes its pattern over its guard and body; closures scope
  their parameters;
* fields, variants and the items of an `impl` or a trait are members:
  reached by a path (`E::A`) or through a value, never by a plain name,
  even inside the impl;
* an inline module sees its own items and not its parent's (a module
  is a barrier, as in Rust); its items are its members (`m::f`);
* labels, lifetimes and macros stand in namespaces of their own;
  an attribute's path and a `use` path stand in namespaces where nothing
  is defined (`attribute`, `import`), and escape;
* a name the file cannot place escapes: never a problem, never painted
  unresolved. That is every name from another file, a dependency, a
  prelude (`Some`, `Vec`, `'static`) or a glob import, every name under
  `crate::`, `super::`, `self::`, `Self::` or a qualified self type, and
  a member of a type the file defines that its `impl` blocks declare
  (`Cell::new`): types, units and imports come later.

Not yet told apart: a lone name in a pattern binds it, though rustc reads
it as a path when it names a unit struct, a unit variant or a constant
(`None =>`); telling which needs the names the preludes and other files
declare. `use super::*` does not yet open a module to its parent's items,
and `macro_rules!` defined outside an inline module is not seen inside
it. `cargo test -p qana-lang --test rust_binding` pins the binding; over
this workspace's largest files, no name is a problem.

A `macro_rules!` is a textual definition (`@def(n, textual)`): the
file's binding reads it as an item, seen throughout its scope, and its
order (after it, into the modules declared after it, out of a
`#[macro_use]` module, not past its block) is the project index's, which
finds a macro's name as rustc does (`qana_index::textual_at` and
`Resolver::resolve_macro`). A `macro` item is an item of its module, as
rustc reads one.

Folding (`@fold`), wherever one runs over lines: blocks and the bodies of
modules, structs, enums, unions, traits, impls, `extern` blocks, `match`
and macro calls and definitions (`block`); parameter, argument, generic
parameter and pattern lists, `use` groups, arrays, tuples, struct
literals and the other token trees (`list`); string literals (`string`).
Comments fold on their own (`comment`): one over lines, and lines in a
row holding nothing but comments, such as doc comments.
`cargo test -p qana-lang --test rust_folds` pins them.

## Colouring

Doc comments (`///`, `//!`, `/** */`, `/*! */`, as rustc_lexer tells them
from plain comments) and lifetimes are coloured by their tokens; a block
doc comment reads its body in a mode of its own, which cuts and nests as a
block comment's does. The rest is coloured by where a name stands
(`@style` on alternatives): the name a path in a type ends in, and a
type's, a type parameter's and an associated type's name where they are
defined, are a `type`; a function's name where it is defined and the name
a call calls are a `function` (a tuple struct or variant called as a
function is one too: the syntax cannot tell them apart); the name a
method call calls is a `method`; a macro invocation's name and `!`, and
`macro_rules!` and the name it defines, are a `macro`; an attribute's
`#[`, `]` and path are a `decorator`; a loop's or block's label, where it
is given and where `break` or `continue` names it, is a `label`. A path's
qualifiers (`std::collections::` in `std::collections::HashMap`) are
names: whether one is a module or a type is not the syntax's to say.
`cargo test -p qana-lang --test rust_classes` pins them, and that a live
document keeps them through edits.
