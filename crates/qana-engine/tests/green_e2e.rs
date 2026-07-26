//! Green trees + typed AST, end to end — L4 edition: trees carry
//! balanced LIST/RUN sequence nodes; typed list access goes through
//! flattened `items()` accessors; batch trees are shallow (no more
//! left-recursive spines).

use qana_engine::*;
use qana_grammar::demo::{demo_grammar, demo_syn_grammar};
use qana_grammar::demo_ast as ast;
use qana_grammar::green::{check_balance, flat_children, is_seq_prod};
use qana_grammar::{
    batch_parse_green, build_green, build_lr, parse, AstNode, CompiledLexer, GreenChild,
    GreenNode, NodeRef, TermTok,
};
use std::sync::Arc;

struct Pipeline {
    lexer: CompiledLexer,
    sg: qana_grammar::SynGrammar,
    tables: qana_grammar::LrTables,
}

fn pipeline() -> Pipeline {
    let (g, ids) = demo_grammar();
    let lexer = CompiledLexer::build(&g).expect("in envelope");
    let sg = demo_syn_grammar(&ids, &lexer.vocab);
    let tables = build_lr(&sg);
    assert!(tables.conflicts.is_empty());
    assert!(tables.lists.len() >= 2, "stmts and args_ne must detect as lists");
    Pipeline { lexer, sg, tables }
}

fn tree_of(p: &Pipeline, src: &str) -> Arc<GreenNode> {
    let buf = LexedBuffer::new(&p.lexer, src);
    let all = full_tokens(&p.lexer, &buf);
    let tree = batch_parse_green(&p.sg, &p.tables, &all).expect("parse");
    check_balance(&tree).expect("balance invariants");
    tree
}

#[test]
fn tree_text_is_byte_identical() {
    let p = pipeline();
    for src in [
        "let a = 1;",
        "let a = /* c1 /* nested */ */ 1 + 2; // done\n",
        "if (x) { y(\"héllo 🚀\"); } else { z(1, 2.5); }\n",
        "let m = 1;\r\nlet n = 2;\rlet o = 3;\nemit(m, n);",
        "  \t let pad = [ 1 ,\t2 ] ;   // trailing ws  \n\n",
        "let long = /* a\n   b\n   c */ f(g(h(1)));\n// tail comment\n",
        "",
    ] {
        let tree = tree_of(&p, src);
        assert_eq!(tree.text(), src, "tree text must reproduce source exactly");
    }
}

#[test]
fn legacy_binary_builder_still_lossless() {
    // The PNode + build_green path (P1 increment 3) remains as the binary
    // reference implementation; it must stay byte-lossless.
    let p = pipeline();
    let src = "let a = /* note */ 1 + 2; // c\nlet b = f(1, 2);\n";
    let buf = LexedBuffer::new(&p.lexer, src);
    let all = full_tokens(&p.lexer, &buf);
    let terms: Vec<TermTok> = all
        .iter()
        .filter(|t| !t.trivia)
        .map(|t| TermTok { id: t.id, text: t.text.clone() })
        .collect();
    let pnode = parse(&p.sg, &p.tables, &terms).expect("parse");
    let tree = build_green(&pnode, &all).expect("build");
    assert_eq!(tree.text(), src);
}

/// Semantic shape: (nt, prod) preorder over the flattened view (runs
/// expanded), trivia skipped — stable under both trivia and chunking.
fn shape(n: &GreenNode, out: &mut Vec<(u16, u16)>) {
    out.push((n.nt, if is_seq_prod(n.prod) { u16::MAX - 1 } else { n.prod }));
    for c in flat_children(n) {
        if let GreenChild::Node(m) = c {
            shape(m, out);
        }
    }
}

#[test]
fn trivia_is_structurally_invisible() {
    let p = pipeline();
    let with = tree_of(&p, "let a = /* one\n   two */ 1 + 2; // done");
    let without = tree_of(&p, "let a = 1 + 2;");
    let (mut a, mut b) = (Vec::new(), Vec::new());
    shape(&with, &mut a);
    shape(&without, &mut b);
    assert_eq!(a, b);
}

#[test]
fn ancestor_spans_nest_at_every_offset() {
    let p = pipeline();
    let src = "let a = /* c */ 1 + 2 * f(3); // t\nif (a) { b(); }\n";
    let tree = tree_of(&p, src);
    assert_eq!(tree.text(), src);
    for off in 0..src.len() as u32 {
        let spans = qana_grammar::green::ancestor_spans(&tree, off);
        assert!(!spans.is_empty(), "offset {off}");
        for w in spans.windows(2) {
            let (outer, inner) = (w[0], w[1]);
            assert!(
                outer.0 <= inner.0 && inner.1 <= outer.1,
                "offset {off}: {outer:?} must contain {inner:?}"
            );
        }
        assert!(qana_grammar::green::token_at_offset(&tree, off).is_some());
    }
}

#[test]
fn typed_navigation_traverses_real_trees() {
    let p = pipeline();
    let tree = tree_of(&p, "let a = 1 + 2 * 3;");
    let file = ast::File::cast(NodeRef(&tree)).expect("root is File");

    // L4: stmts is a flattened list now.
    let stmts = file.stmts().expect("stmts");
    let items = stmts.items();
    assert_eq!(items.len(), 1);
    let ast::Stmt::LetStmt(let_stmt) = items[0] else { panic!("let statement expected") };

    assert_eq!(let_stmt.kw_let_token().expect("kw").text(), "let");
    assert_eq!(let_stmt.ident_token().expect("name").text(), "a");
    assert_eq!(let_stmt.semi_token().expect("semi").text(), ";");

    let ast::Expr::AddExpr(add) = let_stmt.expr().expect("expr") else { panic!("add at top") };
    let ast::Expr::NumLit(lhs) = add.expr().expect("lhs") else { panic!("number lhs") };
    assert_eq!(lhs.number_token().expect("1").text(), "1");
    let ast::Expr::MulExpr(mul) = add.expr_2().expect("rhs") else { panic!("mul rhs") };
    assert_eq!(mul.star_token().expect("*").text(), "*");
    let ast::Expr::NumLit(rhs) = mul.expr_2().expect("3") else { panic!("number rhs") };
    assert_eq!(rhs.number_token().expect("3").text(), "3");

    // Typed layer sees through trivia AND run chunking alike.
    let tree2 = tree_of(&p, "let /*x*/ a /*y*/ = 1 + /*z*/ 2 * 3; // c");
    let file2 = ast::File::cast(NodeRef(&tree2)).expect("root");
    let items2 = file2.stmts().unwrap().items();
    assert_eq!(items2.len(), 1);
    let ast::Stmt::LetStmt(ls2) = items2[0] else { panic!() };
    assert_eq!(ls2.ident_token().unwrap().text(), "a");
    assert!(matches!(ls2.expr(), Some(ast::Expr::AddExpr(_))));
}

#[test]
fn call_args_flatten_through_the_list() {
    let p = pipeline();
    let tree = tree_of(&p, "z(1, 2.5);");
    let file = ast::File::cast(NodeRef(&tree)).unwrap();
    let items = file.stmts().unwrap().items();
    let ast::Stmt::ExprStmt(es) = items[0] else { panic!() };
    let ast::Expr::CallExpr(call) = es.expr().unwrap() else { panic!() };
    assert_eq!(call.ident_token().unwrap().text(), "z");
    let ast::Args::ArgsSome(some) = call.args().unwrap() else { panic!() };
    let args: Vec<_> = some.args_ne().unwrap().items();
    assert_eq!(args.len(), 2, "flattened argument list");
    assert!(matches!(args[0], ast::Expr::NumLit(_)));
    let ast::Expr::NumLit(second) = args[1] else { panic!() };
    assert_eq!(second.number_token().unwrap().text(), "2.5");
}

#[test]
fn big_lists_are_balanced_and_shallow() {
    let p = pipeline();
    let src: String = (0..5000).map(|i| format!("let v{i} = {i};\n")).collect();
    let tree = tree_of(&p, &src);
    // Depth check: no path longer than ~4 sequence levels for 5000 items.
    fn depth(n: &GreenNode) -> usize {
        1 + n
            .children
            .iter()
            .filter_map(|c| match c {
                GreenChild::Node(m) => Some(depth(m)),
                _ => None,
            })
            .max()
            .unwrap_or(0)
    }
    let d = depth(&tree);
    assert!(d <= 12, "tree depth {d} must be logarithmic, not a spine");
    // And the typed view still sees all 5000 statements, in order.
    let file = ast::File::cast(NodeRef(&tree)).unwrap();
    assert_eq!(file.stmts().unwrap().items().len(), 5000);
}
