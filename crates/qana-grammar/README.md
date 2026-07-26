# qana-grammar

Grammar as a plain value: pattern-to-DFA compilation, canonical LR(1) tables with conflict counterexamples, and the envelope lints that make incremental parsing provable rather than hoped for.

Part of the [qana](https://crates.io/crates/qana) language-building toolchain.

```toml
qana-grammar = "0.0.2"
```

A grammar here is a `Clone + Eq` **value**, not generated code: token patterns, modes,
precedence, bracket pairs, and production shapes. From that value this crate compiles a
table-driven lexer and canonical LR(1) parse tables.

The distinctive part is what it **refuses**. Envelope lints reject grammars that would
break the guarantees everything downstream depends on — L1 (no token may match across a
newline), L2 (mode transitions stay line-resolvable), and the LR conflicts, each reported
with a concrete counterexample rather than a state number. A grammar that compiles here
is one whose incremental behaviour is already proven.

## The family

| crate | what it is |
|---|---|
| [`qana`](https://crates.io/crates/qana) | umbrella — re-exports the whole family under one namespace |
| [`qana-grammar`](https://crates.io/crates/qana-grammar) | grammar as a value; DFA and LR construction; the envelope lints |
| [`qana-engine`](https://crates.io/crates/qana-engine) | incremental lexing and parsing over compiled grammars |
| [`qana-sem`](https://crates.io/crates/qana-sem) | name binding, types, macros; revisioned queries |
| [`qana-services`](https://crates.io/crates/qana-services) | editor services derived from the grammar |
| [`qana-lang`](https://crates.io/crates/qana-lang) | the `.qana` grammar language, self-hosted |
| [`linework`](https://crates.io/crates/linework) | the editor protocol (no dependency on qana) |

Most users want `qana` rather than these directly.

## Licence

MIT OR Apache-2.0, at your option.
