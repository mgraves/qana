# qana-lsp

Language server over the qana toolchain, with live grammar hot-reload: edit the .qana file and the language it defines updates in the editor immediately.

Part of the [qana](https://crates.io/crates/qana) language-building toolchain.

```toml
qana-lsp = "0.0.2"
```

A language server that serves whatever language a `.qana` file defines.

The distinctive feature is **hot-reload of the grammar itself**: edit the `.qana` file and
the language it describes updates live, without restarting the server or the editor. That
turns language design into an interactive loop rather than a build-and-relaunch cycle.

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
