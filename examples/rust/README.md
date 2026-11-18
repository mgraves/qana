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
  name.
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

Measured over real code, 67,884 files: this workspace, Synkro, and
the 1,990 crates of a cargo registry (rust-analyzer's parser tests among
them). 67,308 parse with no repair. Each of the other 576 is not stable
Rust 2021: code of the 2015 and 2018 editions (`try!`, bare trait
objects), unstable syntax (`builtin #`, `box`, `gen`, `yield`, `try`
blocks, `if let` guards, const traits, macros 2.0, trait aliases and the
like), test fixtures rustc rejects on purpose, or one of the omissions
below.

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
  values, `macro` 2.0, trait aliases, type ascription, `builtin #`);
* the 2015 edition's anonymous trait method parameters and bare trait
  objects.

## What it derives

The outline: functions, types, traits, impls, modules, constants and
macro definitions (`@outline`). It declares no binding (`@def`/`@ref`)
yet.

qana's style legend has no class for types, functions, macros,
attributes, lifetimes or doc comments, so those are coloured as names,
punctuation or comments.
