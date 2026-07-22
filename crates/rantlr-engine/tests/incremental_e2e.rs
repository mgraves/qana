//! P2 GATES: the Wagner incremental parser must produce trees FULLY
//! EQUAL (structure, trivia, text — everything) to a from-scratch batch
//! parse of the edited document, while actually reusing old subtrees.

use rantlr_engine::*;
use rantlr_grammar::demo::{demo_grammar, demo_syn_grammar};
use rantlr_grammar::demo_ast as ast;
use rantlr_grammar::{
    batch_parse_green, build_lr, AstNode, CompiledLexer, LrTables, NodeRef, SynGrammar,
};

struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed.max(1))
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn pipeline() -> (CompiledLexer, SynGrammar, LrTables) {
    let (g, ids) = demo_grammar();
    let lexer = CompiledLexer::build(&g).expect("in envelope");
    let sg = demo_syn_grammar(&ids, &lexer.vocab);
    let tables = build_lr(&sg);
    assert!(tables.conflicts.is_empty());
    (lexer, sg, tables)
}

/// The gate: session tree == batch parse of the current buffer, fully.
fn assert_gate(session: &IncSession<'_>, sg: &SynGrammar, tables: &LrTables) {
    let lexer = session.buf.lexer;
    let all = full_tokens(lexer, &session.buf);
    let batch = batch_parse_green(sg, tables, &all).expect("batch parses");
    let inc = session.tree().expect("tree valid");
    assert_eq!(inc.text(), session.buf.reproduce(), "tree text == buffer");
    assert!(**inc == *batch, "incremental tree must equal batch tree");
}

fn stmt_line(i: usize) -> Line {
    Line::new(format!("let v{i} = {i} + {i} * 2;"), LineTerm::Lf)
}

#[test]
fn single_edit_reuses_bulk_of_tree() {
    let (lexer, sg, tables) = pipeline();
    let src: String = (0..200).map(|i| format!("let v{i} = {i} + {i} * 2;\n")).collect();
    let mut s = IncSession::new(&lexer, &sg, &tables, &src).unwrap();
    let out = s
        .edit(&sg, &tables, &[LineEdit {
            start: 100,
            end: 101,
            replacement: vec![Line::new("let mid = f(1, 2.5); // edited", LineTerm::Lf)],
        }])
        .unwrap();
    assert_gate(&s, &sg, &tables);
    assert!(
        out.stats.reuse_fraction() > 0.9,
        "expected >90% terminal reuse, got {:.3} ({:?})",
        out.stats.reuse_fraction(),
        out.stats
    );
    assert!(out.stats.splices >= 1);
}

#[test]
fn comment_only_edit_is_trivia_local() {
    let (lexer, sg, tables) = pipeline();
    let src = "let a = 1;\n// just a comment\nlet b = 2;\n";
    let mut s = IncSession::new(&lexer, &sg, &tables, src).unwrap();
    let out = s
        .edit(&sg, &tables, &[LineEdit {
            start: 1,
            end: 2,
            replacement: vec![Line::new("// a REVISED comment", LineTerm::Lf)],
        }])
        .unwrap();
    assert_gate(&s, &sg, &tables);
    // Zero terminals were touched: 100% terminal reuse...
    assert_eq!(out.stats.reuse_fraction(), 1.0, "{:?}", out.stats);
    // ...and the new trivia text is in the tree.
    assert!(s.tree().unwrap().text().contains("REVISED"));
}

#[test]
fn multi_site_batch_holds_the_gate() {
    let (lexer, sg, tables) = pipeline();
    let src: String = (0..120).map(|i| format!("let v{i} = {i} + {i} * 2;\n")).collect();
    let mut s = IncSession::new(&lexer, &sg, &tables, &src).unwrap();
    let out = s
        .edit(&sg, &tables, &[
            LineEdit { start: 10, end: 11, replacement: vec![stmt_line(9010)] },
            LineEdit { start: 55, end: 57, replacement: vec![stmt_line(9055)] }, // 2→1 lines
            LineEdit { start: 90, end: 90, replacement: vec![stmt_line(9090), stmt_line(9091)] }, // insertion
        ])
        .unwrap();
    assert_gate(&s, &sg, &tables);
    assert!(out.stats.reuse_fraction() > 0.8, "{:?}", out.stats);
}

#[test]
fn fragile_breakdown_prevents_wrong_splice() {
    // Multi-line expression: `1 + 2` sits alone on a clean line as a
    // complete Add subtree. Changing the NEXT line's operator from + to *
    // changes how that subtree must associate: batch of `1 + 2 * 9` is
    // Add(1, Mul(2, 9)). A naive nonterminal splice of the old Add(1, 2)
    // would yield Mul(Add(1, 2), 9) — the Wagner §6 wrong-splice. The
    // fragile-production breakdown must prevent it; the full-equality
    // gate proves it.
    let (lexer, sg, tables) = pipeline();
    let src = "let a =\n1 + 2\n+ 9;\n";
    let mut s = IncSession::new(&lexer, &sg, &tables, src).unwrap();
    let out = s
        .edit(&sg, &tables, &[LineEdit {
            start: 2,
            end: 3,
            replacement: vec![Line::new("* 9;", LineTerm::Lf)],
        }])
        .unwrap();
    assert_gate(&s, &sg, &tables);
    assert!(out.stats.breakdowns > 0, "fragile breakdown must trigger: {:?}", out.stats);

    // And assert the SHAPE explicitly via the typed AST: Add(1, Mul(2,9)).
    let tree = s.tree().unwrap();
    let file = ast::File::cast(NodeRef(tree)).unwrap();
    let ast::Stmts::StmtsMore(more) = file.stmts().unwrap() else { panic!() };
    let ast::Stmt::LetStmt(ls) = more.stmt().unwrap() else { panic!() };
    let ast::Expr::AddExpr(add) = ls.expr().unwrap() else {
        panic!("top must be Add, not Mul — wrong splice detected")
    };
    assert!(matches!(add.expr_2(), Some(ast::Expr::MulExpr(_))), "rhs must be Mul(2, 9)");
}

#[test]
fn error_invalidates_then_batch_recovers() {
    let (lexer, sg, tables) = pipeline();
    let src = "let a = 1;\nlet b = 2;\n";
    let mut s = IncSession::new(&lexer, &sg, &tables, src).unwrap();
    // Break it.
    let err = s.edit(&sg, &tables, &[LineEdit {
        start: 0,
        end: 1,
        replacement: vec![Line::new("let a = ;", LineTerm::Lf)],
    }]);
    assert!(err.is_err());
    assert!(s.tree().is_none(), "tree invalid after error");
    // Fix it: full batch fallback, session healthy again.
    let out = s
        .edit(&sg, &tables, &[LineEdit {
            start: 0,
            end: 1,
            replacement: vec![Line::new("let a = 42;", LineTerm::Lf)],
        }])
        .unwrap();
    assert_eq!(out.stats.reused_terms, 0, "fallback is a full parse");
    assert_gate(&s, &sg, &tables);
}

#[test]
fn multiline_comment_edits_hold_the_gate() {
    let (lexer, sg, tables) = pipeline();
    let src = "let a = 1;\n/* note:\n   spans lines\n*/\nlet b = 2;\n";
    let mut s = IncSession::new(&lexer, &sg, &tables, src).unwrap();
    // Edit inside the block comment (state-neutral interior line).
    s.edit(&sg, &tables, &[LineEdit {
        start: 2,
        end: 3,
        replacement: vec![Line::new("   REWRITTEN interior", LineTerm::Lf)],
    }])
    .unwrap();
    assert_gate(&s, &sg, &tables);
    // Delete the closer: state wave; parse still fine (all comment → trivia).
    s.edit(&sg, &tables, &[LineEdit { start: 3, end: 4, replacement: vec![] }]).unwrap();
    // Everything below the opener is now comment — still a valid file.
    assert_gate(&s, &sg, &tables);
    // Restore a closer.
    s.edit(&sg, &tables, &[LineEdit {
        start: 3,
        end: 3,
        replacement: vec![Line::new("*/", LineTerm::Lf)],
    }])
    .unwrap();
    assert_gate(&s, &sg, &tables);
}

#[test]
fn fuzz_valid_edit_sequences_hold_the_gate() {
    let (lexer, sg, tables) = pipeline();
    const POOL: &[&str] = &[
        "let x = 1 + 2 * 3;",
        "emit(a, b);",
        "if (x) { y(); } else { z(1, 2); }",
        "// a comment line",
        "let s = \"text\";",
        "{ let inner = 5; }",
        "done([1, 2.5], f(g));",
        "",
    ];
    let mut rng = Rng::new(0xFACADE);
    for iter in 0..60 {
        let n = 20 + rng.below(60);
        let src: String =
            (0..n).map(|_| format!("{}\n", POOL[rng.below(POOL.len())])).collect();
        let mut s = IncSession::new(&lexer, &sg, &tables, &src).unwrap();
        for round in 0..5 {
            let lines = s.buf.lines.len() - 1; // never touch final empty line
            if lines == 0 {
                break;
            }
            let sites = 1 + rng.below(3);
            let mut cuts: Vec<usize> = (0..sites * 2).map(|_| rng.below(lines)).collect();
            cuts.sort_unstable();
            let mut edits = Vec::new();
            for pair in cuts.chunks(2) {
                let (start, end) = (pair[0], pair[1]);
                if let Some(prev) = edits.last() {
                    let prev: &LineEdit = prev;
                    if start < prev.end {
                        continue;
                    }
                }
                let rep = rng.below(3);
                let replacement =
                    (0..rep).map(|_| Line::new(POOL[rng.below(POOL.len())], LineTerm::Lf)).collect();
                edits.push(LineEdit { start, end, replacement });
            }
            if edits.is_empty() {
                continue;
            }
            let out = s.edit(&sg, &tables, &edits).unwrap_or_else(|e| {
                panic!("iter {iter} round {round}: pool edits must stay valid: {e}")
            });
            assert_gate(&s, &sg, &tables);
            let _ = out;
        }
    }
}
