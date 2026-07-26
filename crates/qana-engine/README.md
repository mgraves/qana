# qana-engine

Incremental lexing and parsing over compiled grammars: line-anchored damage tracking, optimal tree reuse on edit, and error recovery that always yields a complete tree.

Part of the [qana](https://crates.io/crates/qana) language-building toolchain.

```toml
qana-engine = "0.0.1"
```

Takes the compiled lexer and tables from `qana-grammar` and runs them incrementally.

An edit maps to a damage region, the lexer re-runs only over damaged lines, and the parser
reuses every subtree it can (Wagner & Graham's algorithm, with right-breakdown). Reparsing
one line of a 100,000-line file costs microseconds and is independent of where in the file
the edit lands.

Parsing is **total**: a broken document still produces a complete, lossless tree with error
nodes, because an editor cannot be handed "no tree" while someone is mid-keystroke.

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
