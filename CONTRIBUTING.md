# Contributing to qana

Thanks for your interest. qana is a language-building toolchain released
under MIT OR Apache-2.0; this document covers the practical mechanics of
getting a change merged.

## Quick reference

- Fork the repo, branch off `main`, open a pull request against `main`.
- Sign every commit with `git commit -s` (DCO — see below). No CLA.
- One coherent change per PR. Keep diffs reviewable.
- New behaviour ships with a **gate**, not just a test — see below.

## Developer Certificate of Origin (DCO)

qana uses the **Developer Certificate of Origin 1.1** in place of a
Contributor License Agreement. Every commit must be signed off:

```bash
git commit -s -m "Your commit message"
```

The `-s` flag adds a `Signed-off-by: Your Name <your@email>` trailer. By
signing off, you certify that you have the right to submit the contribution
under the project's licence (MIT OR Apache-2.0). The full text of the DCO is
at <https://developercertificate.org>.

CI enforces DCO on every PR. If you forget, amend your commits
(`git commit --amend -s` for the last one, `git rebase --signoff` for a
range) and force-push to your branch.

## Inbound = outbound

Your contribution is licensed under the same terms as the project (MIT OR
Apache-2.0). This is automatic under Apache-2.0 §5 — no separate paperwork.

## The quality bar

### The envelope is the product

qana's central bet is the inverse of most parser generators: **grammars are
refused, and in exchange everything downstream is derivable and provable.**
Incremental parsing, editor intelligence, error containment and losslessness
are guaranteed *by construction* because the envelope lints (L1–L5) reject
anything that would break them.

The one change that will always be refused is **weakening a guarantee so
that a grammar can be admitted.** If a grammar you want is outside the
envelope, the interesting question is whether the envelope is drawn in the
right place — open a Discussion. Do not widen it by removing a lint.

### Every change ships with a gate

A test asserts an example. A *gate* asserts a property, and this codebase is
built on them. Match the kind that fits your change:

- **Differential** — the new path agrees with a trusted one. Incremental
  parsing is gated against batch parsing on the same input; the generated
  lexer against the hand-written reference in `crates/qana-lex`.
- **Fixed-point** — the system reproduces itself. `rg.qana` compiled by the
  bootstrap grammar must equal the bootstrap exactly.
- **Drift** — generated artifacts match what is checked in, so regeneration
  is never silently skipped (typed ASTs, the tree-sitter emission).
- **Losslessness** — `tree.text() == source`, byte for byte, always. This is
  a hard gate; a change that breaks it is wrong, not merely untested.

If you cannot see which gate covers your change, say so in the PR. That is a
design conversation worth having, not a reason to skip it.

### Everything else

- **No `unsafe`** without prior discussion. There is almost always a safe
  pattern; this codebase currently has none.
- **Zero warnings.** CI builds with `-D warnings`.
- **No technical debt as shortcuts.** Placeholder bodies, "TODO: implement
  properly" stubs and simplified-for-now calculations are not accepted.
  Either land the real thing or don't land it.
- **Match existing patterns.** Search the codebase before inventing a shape.
  The README is an engineering log — it explains why each layer is built the
  way it is, and is the fastest way to understand a subsystem.

## What CI runs

Every PR runs:

1. `cargo build --workspace --all-targets` with `-D warnings`, on Linux and
   macOS, on **stable** Rust.
2. `cargo test --workspace` — the full suite.
3. `cargo deny check` — licence / advisory / source policy.
4. DCO sign-off check.
5. `cargo fmt --check` and `cargo clippy` — currently **advisory**. The tree
   is not yet rustfmt-clean and clippy has an open backlog; making either
   blocking today would turn every PR red for reasons unrelated to the
   change under review. Do not let this stop you formatting code you touch —
   just don't reformat files you otherwise didn't change.

External-contributor PRs are gated on a maintainer approving the workflow
run.

## Workflow

1. **Fork** the repo on GitHub.
2. **Clone** your fork; `cd` into it.
3. **Branch** from `main`: `git checkout -b your-feature-name`.
4. **Make** the change. Match existing patterns; respect the bar above.
5. **Test** locally:
   ```bash
   cargo test --workspace
   RUSTFLAGS="-D warnings" cargo build --workspace --all-targets
   ```
6. **Commit** with `-s` to sign off. Commit messages explain the *why* — the
   diff already shows the *what*.
7. **Push** to your fork and open a PR against `main`.
8. Address review by pushing more commits to the same branch. Don't
   force-push during review (it breaks comment anchors); a squash happens at
   merge.

## Reviewing & merging

- All PRs require ≥1 approving review from a code owner.
- All conversations must resolve before merge.
- Status checks must pass.
- Merge is **squash**; `main` is linear-history protected and never
  force-pushed.

## Graduated trust

New contributors start with fork-and-PR access. After several merged,
quality PRs a contributor may be invited to Triage (review without merge
rights), and later to Write. The path is open-ended; ask if you'd like to
take on more.

## Especially welcome

Documentation improvements, additional example grammars, clippy/rustfmt
backlog reduction, and bug fixes in stable code. Larger changes — anything
touching the envelope lints, the incremental engine, the LR construction, or
the composition operator — should start as a Discussion before significant
work goes into a PR.

## Reporting bugs & security issues

- **Bugs:** open a GitHub Issue with a reproduction — ideally the smallest
  `.qana` grammar and input that shows it.
- **Security vulnerabilities:** do NOT open a public issue. See
  [SECURITY.md](SECURITY.md).

## Questions

If you're stuck, open a Discussion (best for "how should I…?") or an Issue.

Thanks again for contributing.
