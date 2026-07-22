//! LR(1) table construction: conflict traces (the syntax-tier
//! counterexample promise) and declarative resolution.

use rantlr_grammar::syn::{Assoc, Sym, SynGrammar};
use rantlr_grammar::{build_lr, TokenId};

const IF: TokenId = 0;
const ELSE: TokenId = 1;
const X: TokenId = 2;

fn dangling_else() -> SynGrammar {
    let mut sg = SynGrammar::new(
        "DanglingElse",
        vec!["IF".into(), "ELSE".into(), "X".into()],
    );
    let s = sg.nt("s");
    sg.start = s;
    sg.prod(s, vec![Sym::T(IF), Sym::N(s), Sym::T(ELSE), Sym::N(s)]);
    sg.prod(s, vec![Sym::T(IF), Sym::N(s)]);
    sg.prod(s, vec![Sym::T(X)]);
    sg
}

#[test]
fn dangling_else_conflict_reported_with_counterexample() {
    let sg = dangling_else();
    let t = build_lr(&sg);
    assert_eq!(t.conflicts.len(), 1, "exactly the classic conflict: {:#?}", t.conflicts);
    let c = &t.conflicts[0];
    assert_eq!(c.kind, "shift/reduce");
    assert_eq!(c.lookahead, ELSE);
    // The example must be the canonical nested-if prefix: after reading
    // `IF IF X`, seeing `ELSE`, both actions are possible.
    assert_eq!(c.example, "IF IF X · ELSE", "example: {}", c.example);
    // Items must show both sides with dots.
    let joined = c.items.join(" | ");
    assert!(joined.contains("s → IF s ·"), "items: {joined}");
    assert!(joined.contains("· ELSE"), "items: {joined}");
}

#[test]
fn dangling_else_resolved_by_declared_precedence() {
    let mut sg = dangling_else();
    // Classic fix: ELSE binds tighter than the short-if production
    // (whose default precedence comes from its last terminal, IF).
    sg.set_token_prec(IF, 1, Assoc::Right);
    sg.set_token_prec(ELSE, 2, Assoc::Right);
    let t = build_lr(&sg);
    assert!(t.conflicts.is_empty(), "conflicts: {:#?}", t.conflicts);
    assert!(t.resolved_by_prec >= 1);
}

#[test]
fn reduce_reduce_conflict_reported() {
    // A → a ; B → a ; S → A x | B y  — after `a`, lookahead decides, but
    // LR(1) sees reduce A vs reduce B... with distinct lookaheads x/y it's
    // actually fine; force the conflict by making both followed by x.
    let mut sg = SynGrammar::new("RR", vec!["a".into(), "x".into()]);
    let s = sg.nt("s");
    let a_nt = sg.nt("A");
    let b_nt = sg.nt("B");
    sg.start = s;
    sg.prod(s, vec![Sym::N(a_nt), Sym::T(1)]);
    sg.prod(s, vec![Sym::N(b_nt), Sym::T(1)]);
    sg.prod(a_nt, vec![Sym::T(0)]);
    sg.prod(b_nt, vec![Sym::T(0)]);
    let t = build_lr(&sg);
    assert!(
        t.conflicts.iter().any(|c| c.kind == "reduce/reduce" && c.lookahead == 1),
        "conflicts: {:#?}",
        t.conflicts
    );
    let c = t.conflicts.iter().find(|c| c.kind == "reduce/reduce").unwrap();
    assert_eq!(c.example, "a · x");
}

#[test]
fn expression_grammar_is_conflict_free_with_precedence() {
    // Standalone expr grammar: E → E+E | E*E | n with precedence.
    let mut sg = SynGrammar::new("Expr", vec!["PLUS".into(), "STAR".into(), "N".into()]);
    let e = sg.nt("e");
    sg.start = e;
    sg.prod(e, vec![Sym::N(e), Sym::T(0), Sym::N(e)]);
    sg.prod(e, vec![Sym::N(e), Sym::T(1), Sym::N(e)]);
    sg.prod(e, vec![Sym::T(2)]);
    sg.set_token_prec(0, 1, Assoc::Left);
    sg.set_token_prec(1, 2, Assoc::Left);
    let t = build_lr(&sg);
    assert!(t.conflicts.is_empty(), "conflicts: {:#?}", t.conflicts);
    assert!(t.resolved_by_prec >= 4, "resolved: {}", t.resolved_by_prec);
}

#[test]
fn without_precedence_the_same_grammar_reports_conflicts_with_traces() {
    let mut sg = SynGrammar::new("Expr", vec!["PLUS".into(), "N".into()]);
    let e = sg.nt("e");
    sg.start = e;
    sg.prod(e, vec![Sym::N(e), Sym::T(0), Sym::N(e)]);
    sg.prod(e, vec![Sym::T(1)]);
    let t = build_lr(&sg);
    assert!(!t.conflicts.is_empty());
    let c = &t.conflicts[0];
    assert_eq!(c.kind, "shift/reduce");
    // After `N PLUS N`, seeing another `PLUS`: associativity is undeclared.
    assert_eq!(c.example, "N PLUS N · PLUS");
}
