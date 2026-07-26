# qana-lang

The .qana grammar language: one file declares tokens, syntax, precedence, highlighting, outline and name binding — self-hosted on the qana engine.

Part of the [qana](https://crates.io/crates/qana) language-building toolchain.

```toml
qana-lang = "0.0.1"
```

One `.qana` file is a whole language: token patterns, keywords, precedence, productions
with labelled alternatives, style classes, outline structure, and binding annotations.

It is **self-hosted** — the `.qana` grammar is itself written in `.qana`, and the fixed-point
gate requires that compiling it with the bootstrap produces the bootstrap exactly. It also
emits a tree-sitter grammar, so a language defined once works in editors that do not speak
this toolchain at all.

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
