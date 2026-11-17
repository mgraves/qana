# Rust, for colouring

`rust.qana` is the grammar an editor uses to colour Rust. It is
[`../rust-lex/rust_lex.qana`](../rust-lex/rust_lex.qana), the lexical
grammar measured against rustc's own lexer, with three things added:

* **Keywords.** Rust 2021's strict keywords (with `async`, `await` and
  `dyn` from 2018) and its reserved words are tokens of their own,
  styled `keyword`. `self` and `Self` are two keywords.
* **Operators** (`= ! < > - & | + * / ^ %`) are styled apart from the
  rest of the punctuation.
* **Brackets** are tokens of their own, declared as pairs, so a block
  that spans lines folds.

## What it does not change

It lexes exactly as `rust_lex.qana` does: every line is cut into the
same tokens and leaves the lexer in the same mode. Only which token a
piece of text is differs (a keyword where the measured grammar has a
name, an operator or a bracket where it has punctuation). So the
measurement in [`../rust-lex/README.md`](../rust-lex/README.md) holds
for this grammar too. `cargo test -p qana-lang --test rust` checks that it
does, on the sample against rustc_lexer's record and on this
workspace's own Rust against the measured grammar.

## What is left as a name

A word Rust treats as a keyword only in some places is coloured as a
name everywhere, since a grammar with no syntax cannot tell where:

* the weak keywords `union`, `macro_rules`, `raw`, `safe` and `default`;
* `gen`, which only the 2024 edition reserves;
* `'static`, which is a lifetime like any other.

A raw identifier (`r#fn`) is a name.

## What it has no colour for

There is no syntax: the rule is flat, every token in any order, so a
broken program is never an error here and nothing is outlined or bound.
qana's style legend has no class for types, functions, macros,
attributes, lifetimes or doc comments, so those are coloured as names,
punctuation or comments.
