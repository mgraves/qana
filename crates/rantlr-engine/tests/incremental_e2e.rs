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

/// The gate, L4 edition: byte-identical text, SEMANTIC tree equality
/// (full structural equality except list nodes compare by flattened
/// contents — association is meaningless by declaration), and balance
/// invariants on both trees.
fn assert_gate(session: &IncSession<'_>, sg: &SynGrammar, tables: &LrTables) {
    use rantlr_grammar::green::{check_balance, semantic_eq};
    let lexer = session.buf.lexer;
    let all = full_tokens(lexer, &session.buf);
    let batch = batch_parse_green(sg, tables, &all).expect("batch parses");
    let inc = session.tree().expect("tree valid");
    assert_eq!(inc.text(), session.buf.reproduce(), "tree text == buffer");
    check_balance(inc).expect("incremental tree balance");
    check_balance(&batch).expect("batch tree balance");
    if !semantic_eq(inc, &batch) {
        panic!(
            "incremental tree must semantically equal batch tree\nfirst divergence: {}\ndoc:\n{}",
            first_diff(inc, &batch, String::new()),
            session.buf.reproduce()
        );
    }
}

/// Locate the first semantic divergence, for debugging.
fn first_diff(
    a: &rantlr_grammar::GreenNode,
    b: &rantlr_grammar::GreenNode,
    path: String,
) -> String {
    use rantlr_grammar::green::flat_children;
    use rantlr_grammar::GreenChild;
    if a.nt != b.nt || (a.prod != b.prod && !(a.prod >= u16::MAX - 2 && b.prod >= u16::MAX - 2)) {
        return format!("{path}: node kinds differ: ({},{}) vs ({},{})", a.nt, a.prod, b.nt, b.prod);
    }
    let fa = flat_children(a);
    let fb = flat_children(b);
    if fa.len() != fb.len() {
        return format!(
            "{path}: flattened child counts differ: {} vs {} (nt {})\n  a: {:?}\n  b: {:?}",
            fa.len(),
            fb.len(),
            a.nt,
            fa.iter().map(|c| kind_of(c)).collect::<Vec<_>>(),
            fb.iter().map(|c| kind_of(c)).collect::<Vec<_>>()
        );
    }
    for (i, (x, y)) in fa.iter().zip(&fb).enumerate() {
        match (x, y) {
            (GreenChild::Token(t), GreenChild::Token(u)) => {
                if t.id != u.id || t.trivia != u.trivia || t.text != u.text {
                    return format!(
                        "{path}[{i}]: tokens differ: {:?}/{}/{:?} vs {:?}/{}/{:?}",
                        t.id, t.trivia, t.text, u.id, u.trivia, u.text
                    );
                }
            }
            (GreenChild::Node(m), GreenChild::Node(n)) => {
                if !rantlr_grammar::green::semantic_eq(m, n) {
                    return first_diff(m, n, format!("{path}/{}#{i}", m.nt));
                }
            }
            _ => return format!("{path}[{i}]: token/node mismatch"),
        }
    }
    format!("{path}: (no diff found?)")
}

fn kind_of(c: &rantlr_grammar::GreenChild) -> String {
    match c {
        rantlr_grammar::GreenChild::Token(t) => format!("T{}:{:?}", t.id, t.text),
        rantlr_grammar::GreenChild::Node(n) => format!("N{}p{}", n.nt, n.prod),
    }
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

/// REGRESSION (found by the P5 `.rg` dogfood): balanced-list RUN chunks
/// are arbitrary ≤MAX_RUN cuts, so an intact reused run can end with a
/// dangling separator. Blind associative absorption then leaves the LR
/// state one shift behind the tree, and the next item triggers a
/// spurious repair. A multi-line comma list with enough entries forms
/// several runs; a late insertion keeps early runs intact across the
/// splice, exercising every boundary parity.
#[test]
fn multiline_separator_list_run_boundaries_hold_the_gate() {
    let (lexer, sg, tables) = pipeline();
    let mut src = String::from("let x = f(\n");
    for i in 0..24 {
        src.push_str(&format!("  a{i},\n"));
    }
    src.push_str("  last\n);\n");
    // Insert (and delete) at every interior line: run boundaries land at
    // different phases relative to the [item COMMA] repetition.
    for line in 2..24 {
        let mut s = IncSession::new(&lexer, &sg, &tables, &src).unwrap();
        s.edit(&sg, &tables, &[LineEdit {
            start: line,
            end: line,
            replacement: vec![Line::new("  zz9,", LineTerm::Lf)],
        }])
        .unwrap();
        assert_gate(&s, &sg, &tables);
        assert!(s.last_repairs.is_empty(), "no spurious repairs at line {line}");

        let mut s = IncSession::new(&lexer, &sg, &tables, &src).unwrap();
        s.edit(&sg, &tables, &[LineEdit { start: line, end: line + 1, replacement: vec![] }])
            .unwrap();
        assert_gate(&s, &sg, &tables);
        assert!(s.last_repairs.is_empty(), "no spurious repairs deleting line {line}");
    }
}

/// REGRESSION (found by the P5 `.rg` dogfood): Wagner right breakdown.
/// A reused subtree's right-spine reductions assumed the OLD following
/// lookahead. Appending an `else` branch after an existing `if` must
/// UN-SPLICE the reused IfStmt and re-derive it as IfElseStmt — without
/// the breakdown, recovery fabricates repairs for perfectly valid text.
#[test]
fn appending_else_unsplices_the_if() {
    let (lexer, sg, tables) = pipeline();
    let mut src = String::from("let a = 1;\n");
    for i in 0..40 {
        src.push_str(&format!("let v{i} = {i};\n"));
    }
    src.push_str("if (a) { emit(a); }\n");
    let n_lines = 42;
    let mut s = IncSession::new(&lexer, &sg, &tables, &src).unwrap();
    let out = s
        .edit(&sg, &tables, &[LineEdit {
            start: n_lines,
            end: n_lines,
            replacement: vec![Line::new("else { emit(0); }", LineTerm::Lf)],
        }])
        .unwrap();
    assert_gate(&s, &sg, &tables);
    assert!(s.last_repairs.is_empty(), "valid text needs no repairs: {:?}", s.last_repairs);
    assert!(out.stats.reused_terms > 0, "prefix still reuses");
    // And the reverse edit: deleting the else re-derives plain IfStmt.
    s.edit(&sg, &tables, &[LineEdit {
        start: n_lines,
        end: n_lines + 1,
        replacement: vec![],
    }])
    .unwrap();
    assert_gate(&s, &sg, &tables);
    assert!(s.last_repairs.is_empty());
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
    let items = file.stmts().unwrap().items();
    let ast::Stmt::LetStmt(ls) = items[0] else { panic!() };
    let ast::Expr::AddExpr(add) = ls.expr().unwrap() else {
        panic!("top must be Add, not Mul — wrong splice detected")
    };
    assert!(matches!(add.expr_2(), Some(ast::Expr::MulExpr(_))), "rhs must be Mul(2, 9)");
}

#[test]
fn recovery_repairs_missing_expr_and_session_stays_valid() {
    use rantlr_grammar::RepairKind;
    let (lexer, sg, tables) = pipeline();
    let src = "let a = 1;\nlet b = 2;\n";
    let mut s = IncSession::new(&lexer, &sg, &tables, src).unwrap();
    // Break it: parsing is TOTAL now — the session keeps a valid tree.
    let out = s
        .edit(&sg, &tables, &[LineEdit {
            start: 0,
            end: 1,
            replacement: vec![Line::new("let a = ;", LineTerm::Lf)],
        }])
        .unwrap();
    assert!(s.tree().is_some(), "tree survives syntax errors");
    assert!(
        out.repairs.iter().any(|r| matches!(r.kind, RepairKind::Inserted(_))),
        "repairs: {:?}",
        out.repairs
    );
    assert_gate(&s, &sg, &tables);
    // Text is still byte-exact through the error.
    assert!(s.tree().unwrap().text().contains("let a = ;"));
    // Typed view: the statement exists; its expression is a placeholder
    // whose literal token is missing (accessor returns None).
    let tree = s.tree().unwrap();
    let file = ast::File::cast(NodeRef(tree)).unwrap();
    let items = file.stmts().unwrap().items();
    assert_eq!(items.len(), 2, "both statements survive");
    // Fix it: repairs drain away.
    let out = s
        .edit(&sg, &tables, &[LineEdit {
            start: 0,
            end: 1,
            replacement: vec![Line::new("let a = 42;", LineTerm::Lf)],
        }])
        .unwrap();
    assert!(out.repairs.is_empty(), "healed: {:?}", out.repairs);
    assert_gate(&s, &sg, &tables);
}

#[test]
fn recovery_skips_garbage_with_error_nodes() {
    use rantlr_grammar::RepairKind;
    let (lexer, sg, tables) = pipeline();
    let src = "let a = ) 1;\nlet b = 2;\n";
    let s = IncSession::new(&lexer, &sg, &tables, src).unwrap();
    assert!(
        s.last_repairs.iter().any(|r| matches!(&r.kind, RepairKind::Deleted(t) if t == ")")),
        "repairs: {:?}",
        s.last_repairs
    );
    assert_gate(&s, &sg, &tables);
    // The skipped token is still IN the tree (lossless), inside an ERROR
    // node that symbol accessors skip.
    let tree = s.tree().unwrap();
    assert!(tree.has_err);
    assert!(tree.text().contains(") 1;"));
    let file = ast::File::cast(NodeRef(tree)).unwrap();
    let items = file.stmts().unwrap().items();
    let ast::Stmt::LetStmt(ls) = items[0] else { panic!() };
    assert!(matches!(ls.expr(), Some(ast::Expr::NumLit(_))), "expr healed to the 1");
}

#[test]
fn recovery_inserts_missing_closer_at_eof() {
    use rantlr_grammar::RepairKind;
    let (lexer, sg, tables) = pipeline();
    let src = "if (x) {\ny();\n";
    let s = IncSession::new(&lexer, &sg, &tables, src).unwrap();
    assert!(
        s.last_repairs.iter().any(|r| matches!(r.kind, RepairKind::Inserted(_))),
        "repairs: {:?}",
        s.last_repairs
    );
    let tree = s.tree().unwrap();
    assert_eq!(tree.text(), src, "zero-width insertions keep text exact");
    let file = ast::File::cast(NodeRef(tree)).unwrap();
    let items = file.stmts().unwrap().items();
    assert!(
        matches!(items[0], ast::Stmt::IfStmt(_)),
        "if-statement healed with a virtual closer"
    );
}

#[test]
fn typing_through_errors_stays_incremental() {
    let (lexer, sg, tables) = pipeline();
    let src: String = (0..200).map(|i| format!("let v{i} = {i} + {i} * 2;\n")).collect();
    let mut s = IncSession::new(&lexer, &sg, &tables, &src).unwrap();
    // Simulate typing a new statement mid-file: several broken keystrokes.
    for text in ["let mid", "let mid =", "let mid = f(", "let mid = f(1,", "let mid = f(1, 2);"] {
        let out = s
            .edit(&sg, &tables, &[LineEdit {
                start: 100,
                end: 101,
                replacement: vec![Line::new(text, LineTerm::Lf)],
            }])
            .unwrap();
        assert!(s.tree().is_some());
        assert!(
            out.stats.reuse_fraction() > 0.9,
            "clean regions must keep splicing while broken: {:?} at {text:?}",
            out.stats
        );
        assert_gate(&s, &sg, &tables);
    }
    assert!(s.last_repairs.is_empty(), "final state healed");
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
