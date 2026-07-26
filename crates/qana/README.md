# qana

**A language-building toolchain.** Design a language inside qana's
*language-shape envelope* and you get incremental parsing, editor
intelligence, error containment, lossless trees and parallel cold parsing
**by construction**. Grammars outside the envelope are refused, with
counterexamples.

```toml
qana = "0.0.2"
```

## The bet

Most parser generators accept whatever grammar you write and try to cope.
qana inverts that: it **refuses** grammars, and in exchange everything
downstream is derivable and provable rather than attempted.

The refusal is the feature. Because a grammar that compiles is guaranteed
to lex line-by-line, reparse incrementally with optimal reuse, and recover
from errors without losing a byte, the editor services above it are not
best-effort — they are consequences.

## One file is the language

```sh
cargo install qana-cli      # installs the `qana` command

qana new mylang --name Mylang --ext .my
qana check mylang/mylang.qana
```

A single `.qana` file declares tokens, keywords, precedence, syntax,
highlighting, outline structure and name binding. `check` certifies it
against the envelope and prints the certificate — or refuses it and shows
you the input that breaks it. Everything else is derived: the lexer, the
LR tables, the typed AST, the semantic tokens, the language server, and a
tree-sitter grammar for editors that don't speak this toolchain at all.

## This crate

`qana` is a facade with no logic of its own. It re-exports the family so
`qana::` is one namespace and a release moves one version number:

| module | crate |
|---|---|
| [`grammar`] | grammar as a value; DFA and LR construction; the envelope lints |
| [`engine`] | incremental lexing and parsing |
| [`sem`] | name binding, types, macros; revisioned queries |
| [`services`] | semantic tokens, folding, outline, completion, diagnostics |
| [`lang`] | the `.qana` grammar language, self-hosted |
| [`linework`] | the editor protocol — no dependency on qana |

The layers nest, so name only the highest you need:

```toml
qana = { version = "0.0.2", default-features = false, features = ["engine"] }
```

`grammar` ⊂ `engine` ⊂ `sem` ⊂ `services` ⊂ `lang` (default).

Every crate stays independently usable — depend on them directly for a
narrower tree than the features give you. In particular
[`linework`](https://crates.io/crates/linework), the editor protocol, has
**no dependency on qana at all**: an editor can adopt it alone and stay
ignorant of whatever engine sits behind the trait it holds.

## Status

Pre-1.0 and moving. The engine is gated by differential tests
(incremental ≡ batch), fixed-point tests (the `.qana` grammar compiles to
itself exactly) and a hard losslessness gate (`tree.text() == source`,
byte for byte). Expect breaking changes before 1.0; pin an exact version.

## Licence

MIT OR Apache-2.0, at your option.
