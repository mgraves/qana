---
name: Bug report
about: Report a defect, regression, or incorrect behaviour
title: ""
labels: bug
---

<!--
Before filing, please:

- Search existing issues for duplicates.
- If your grammar was REFUSED by the envelope lints, that is usually the
  design working — please start a Discussion instead, with the grammar and
  the counterexample qana printed.
- If this is a security vulnerability, DO NOT file it here.
  See SECURITY.md for the private reporting channel.
-->

## What happened?

<!-- A clear description of the actual behaviour. -->

## What did you expect?

<!-- A clear description of the expected behaviour. -->

## Reproduction

<!--
The smallest grammar and input that show the problem. Minimal beats
complete — a 10-line grammar that reproduces it is far more useful than a
real one that also does.
-->

```
// minimal .qana grammar
```

```
// input document
```

```bash
cargo run -p qana-cli -- check repro.qana
# or the API call that misbehaves
```

## Which layer?

<!-- Check any that apply, or leave blank if unsure. -->

- [ ] Grammar compilation / envelope lints (`qana-grammar`)
- [ ] Lexing or parsing, batch (`qana-engine`)
- [ ] Incremental reparse — diverges from a batch parse (`qana-engine`)
- [ ] Losslessness — round-trip is not byte-exact
- [ ] Binding / types / macros (`qana-sem`)
- [ ] Editor services (`qana-services`) or the protocol (`linework`)
- [ ] The `.qana` surface language (`qana-lang`)
- [ ] CLI (`qana-cli`) or language server (`qana-lsp`)

## Environment

- qana commit / version:
- `rustc --version`:
- OS + version:
- Editor + extension version (if the report involves the LSP):

## Additional context

<!-- Logs, the certificate qana printed, related issues. -->
