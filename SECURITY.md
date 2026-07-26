# Security Policy

## Reporting a vulnerability

**Do not open a public GitHub issue for security vulnerabilities.** Public
issues are indexed and searchable; a vulnerability report in a public issue
exposes users between the moment the issue is opened and the moment a fix
ships.

Use one of these private channels instead:

1. **Preferred — GitHub Security Advisory.** Open a private advisory on this
   repository: <https://github.com/mgraves/qana/security/advisories/new>.
   This gives a private channel for triage and a coordinated-disclosure
   timeline. Drafts are visible only to you and the maintainers.

2. **Fallback — email.** If GitHub Security Advisories are unavailable to
   you, email <groupmg@gmail.com> with `[qana Security]` in the subject
   line. Encrypt with PGP if you prefer; key on request.

In your report, please include:

- A description of the vulnerability and its impact.
- Steps to reproduce — ideally the smallest `.qana` grammar and input that
  demonstrates it.
- The qana commit hash or release tag you tested against.
- Your `rustc --version` and OS, if relevant.
- Any mitigation you've already identified.

## What to expect

- **Acknowledgement** within 72 hours of the report (often sooner).
- **Initial assessment** within 7 days — severity classification and a rough
  fix timeline.
- **Coordinated disclosure** by default. We'll work with you on the
  disclosure window; the goal is a fix landing before the issue becomes
  public knowledge. We'll credit you in the release notes and CVE record
  unless you ask us not to.
- **No bug bounty.** qana is a small open-source project and cannot offer
  paid bounties. We do credit reporters.

## Threat model — what qana actually processes

qana is a toolchain that reads **untrusted grammar files and untrusted
source text** and builds parsers from them. That makes the following
genuinely security-relevant rather than merely bugs:

- **Non-termination or unbounded memory** in grammar compilation (DFA
  construction, LR table generation) on an adversarial `.qana` file.
- **Non-termination or unbounded memory** in lexing, parsing, incremental
  reparse, or error recovery on an adversarial input document.
- **Unbounded macro expansion** escaping the fixpoint driver's limits.
- Anything that lets a grammar or an input escape the envelope's guarantees
  — for example, causing the incremental parser to diverge from a batch
  parse, since editors rely on that equivalence.

This matters because the LSP server (`qana-lsp`) processes whatever an
editor opens, and a hostile document in a victim's editor is a real
delivery path.

## Scope

Reports are welcome for any code in this repository — the toolchain crates
(`qana-grammar`, `qana-engine`, `qana-sem`, `qana-services`, `qana-lang`),
the umbrella (`qana`), the protocol crate (`linework`), the CLI
(`qana-cli`), and the language server (`qana-lsp`).

Reports about dependencies should usually go upstream; if the exploitable
surface is specifically how qana *uses* a dependency, report it here.

## What is NOT a security issue

These are quality concerns — please open a regular Issue or Discussion:

- A grammar being **refused** by the envelope lints. That is the design
  working, not a denial of service.
- Panics under conditions the API contract documents as invalid input.
- Performance regressions without an unbounded-growth path.
- Build / packaging issues.

If you're unsure whether something qualifies, err on the side of reporting
privately. We'd rather you over-report than have a real issue land in a
public tracker.

## Supported versions

During the pre-1.0 phase qana supports the latest `main` and the most recent
tagged release. Older releases will not receive security fixes; please
update to a current version.

Once qana reaches a stable release line, this section will be updated with
the formal support matrix.
