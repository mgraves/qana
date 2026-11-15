# Rust's lexical grammar, as a `.qana` file

`rust_lex.qana` is Rust's lexical grammar inside qana's envelope: 7 modes,
36 tokens, and a flat rule over the tokens so that the file certifies. It
is the lexical half only. There is no Rust syntax here, and whether Rust's
syntax fits deterministic LR(1) is a separate question this does not
answer.

It exists to measure one thing: **how much of real Rust does a `.qana`
grammar lex the way rustc's own lexer does?**

## The answer

Both lexers were run over the same files, and the class of every byte
compared (whitespace, comment, word, lifetime, number, string,
punctuation, unknown). A line agrees when every byte of it does.

| Corpus | Files | Lines | Lines that differ | Files with a difference |
|---|---|---|---|---|
| This repository (`crates`, `tools`, `examples`) | 83 | 32,991 | 0 | 0 |
| One machine's cargo registry (third-party crates) | 65,166 | 29,386,026 | 113 (0.0004%) | 37 (0.06%) |

The registry figures are from November 2026, against `ra-ap-rustc_lexer`
0.185. They are a measurement of that corpus, not a guarantee about
another.

The 37 files, by the first difference in each:

| What rustc_lexer saw | Files | Where they are |
|---|---|---|
| A malformed char or byte literal (`''`, a lone `'`, `'\`) | 12 | lexer error fixtures |
| `'🦀` and the like: a lifetime to rustc_lexer, a char here | 2 | lexer error fixtures |
| `r##` with no quote after it | 8 | lexer error fixtures |
| A raw string with 4 or more `#` | 9 | the test files of two lexers |
| The reserved prefix `k#name` | 3 | lexer fixtures |
| The reserved prefix `#"…"` | 3 | 2 fixtures, 1 macro call |

One of the 37 is ordinary code: a macro call that passes `#"…"`. Rust's
2024 edition reserves `#"` as a prefix; this grammar reads it as `#` and
then a string, which is how earlier editions read it.

## What the envelope costs

Two things about Rust do not fit, and both are bounded, not wrong:

* **Raw strings count their `#`.** A string that opens with `r##"` closes
  on `"##`. A mode cannot count, so each count is its own mode, and this
  grammar declares four (`RAW0` to `RAW3`). A raw string with more `#`
  than that is lexed wrong. In the corpus above, nothing outside a
  lexer's own tests used more than three.
* **Block comments nest.** Each level is a push on the mode stack, which
  holds 8. No file in the corpus went deeper.

A raw string, a string or a block comment that spans lines costs nothing
extra: the mode is what one line hands the next, so every line inside is
lexed knowing where it is.

## What trailing context is for

One token in the grammar asks what follows it:

```
\d[\d_]*\.(?![._\a])
```

`1.` is a float unless a `.`, a `_` or a letter comes next, so `0..10` is
a range and `1.max(2)` a method call. Without that one condition, 421 of
1,747 files of application code (24%) had a line read wrong, every one
at a range. The test below takes the condition out and shows the same on
the sample.

## The files

| File | What it is |
|---|---|
| `rust_lex.qana` | The grammar. |
| `sample.rs` | Rust chosen for its lexing: every token of the grammar, and the places one is easily taken for another. Nothing compiles it. |
| `sample.classes` | How rustc_lexer reads `sample.rs`, byte by byte. Written by `rust-fit record`. |

## Checking it

The workspace's toolchain crates have no external dependency, and
rustc_lexer is one. So the check is in two halves.

**In the workspace, with no dependency:** `cargo test -p qana-lang --test
rust_lex` holds qana to `sample.classes`. It also shows that the record
can tell a wrong reading from a right one (the grammar without its
trailing context fails on exactly the lines where it decides), that a raw
string keeps its mode across lines, and that this workspace's own Rust
lexes with no unknown byte and nothing left open.

**Outside it, against rustc_lexer:** `tools/rust-fit` is a crate of its
own, not a workspace member.

```
# Compare the two lexers over any source trees. Fails if a line differs.
cargo run --release --manifest-path tools/rust-fit/Cargo.toml -- compare crates tools examples

# After changing sample.rs, write the record again.
cargo run --manifest-path tools/rust-fit/Cargo.toml -- record

# Fail if the committed record is not what rustc_lexer says now.
cargo run --manifest-path tools/rust-fit/Cargo.toml -- record --check
```

CI runs the first and the last on every push.

## What the measure does not see

* It compares classes, not token kinds. An integer and a float are both
  "number"; every string kind, char and byte literal is "string".
* Keywords are words to both lexers. Punctuation is one character at a
  time in both, as rustc_lexer emits it; joining `>>` or `..=` is the
  parser's work.
* `\a` and `\w` in the grammar stand in for Unicode's identifier classes.
  The corpus has non-ASCII identifiers and they agreed, but the classes
  are not defined to be identical.
* A leading `#!` line is skipped, as rustc skips it.
