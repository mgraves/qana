# qana-sem

Semantic layer for qana grammars: declarative name binding, a declared type tier, macro expansion, and revisioned queries with signature firewalls.

Part of the [qana](https://crates.io/crates/qana) language-building toolchain.

```toml
qana-sem = "0.0.1"
```

Everything above the syntax tree: which names are definitions, which are references,
what resolves where, what type a name carries, and how macros expand.

Binding is **declarative** — the grammar annotates productions with `@def` / `@ref` /
`@scope` and this crate walks them, rather than each language hand-writing a resolver.
Queries are revisioned with signature firewalls, so an edit that changes a function body
does not invalidate anything that only depended on its signature.

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
