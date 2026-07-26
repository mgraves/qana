<!--
Thanks for the PR! A few quick checks before submitting:

- Have you read CONTRIBUTING.md?
- Have you signed off every commit with `git commit -s`?
  (The DCO check will fail otherwise.)
-->

## Summary

<!-- One or two sentences on what this PR changes and why. -->

## Compliance checklist

- [ ] Every commit signed off with `git commit -s` (DCO requirement)
- [ ] `cargo test --workspace` passes
- [ ] `RUSTFLAGS="-D warnings" cargo build --workspace --all-targets` passes
- [ ] No `unsafe` code added (or approval obtained — flag explicitly below)
- [ ] No guarantee was weakened to admit a grammar (see CONTRIBUTING.md)
- [ ] Losslessness still holds — `tree.text() == source`

## The gate

<!--
Which gate covers this change? Name it. If none does, say so — that is a
design conversation, not a blocker.
-->

- [ ] Differential (incremental ≡ batch, or generated ≡ reference)
- [ ] Fixed-point (the system reproduces itself)
- [ ] Drift (generated artifacts match what is checked in)
- [ ] Plain unit test — sufficient for this change because: <!-- why -->
- [ ] None yet — needs discussion

## Type of change

<!-- Check one. -->

- [ ] Bug fix (non-breaking change that fixes an issue)
- [ ] New feature (non-breaking change that adds capability)
- [ ] Breaking change (fix or feature that changes existing API)
- [ ] Refactor / internal cleanup (no externally visible change)
- [ ] Documentation only

## Description

<!--
What does this PR do, and why? Focus on the *why* — the diff shows the
*what*. If this fixes an issue, link it (Closes #123).
-->

## How to test

<!--
Steps a reviewer can follow. If a grammar or example demonstrates the
change, name it:

    cargo run -p qana-cli -- check examples/<grammar>.qana

If a test covers it, name the test path.
-->

## Anything reviewers should look for?

<!--
Optional. Tradeoffs, places you'd like a second opinion, risks the change
introduces.
-->
