# qana-services

Editor services derived from a grammar: semantic tokens with LSP deltas, folding, outline, completion from parser ACTION rows, and diagnostics from parse repairs.

Part of the [qana](https://crates.io/crates/qana) language-building toolchain.

```toml
qana-services = "0.0.1"
```

The services an editor asks for, none of them hand-written per language.

Semantic tokens come from the grammar's declared style classes plus binding facts, and are
emitted as LSP deltas so a keystroke sends a handful of bytes. Completion is derived from
the parser's own ACTION rows — the set of tokens that could legally appear at the cursor is
something an LR table already knows. Diagnostics come from the repairs the parser made, so
the error message and the recovery agree by construction.

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
