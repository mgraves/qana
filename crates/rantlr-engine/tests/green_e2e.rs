//! Green trees + typed AST, end to end: generated lexer → buffer →
//! LR parse → lossless green tree → generated typed wrappers.
//!
//! Gates:
//!   1. TREE LOSSLESSNESS: `green.text()` is byte-identical to the
//!      source — comments, whitespace, unicode, mixed line endings, all
//!      of it — for every parseable document.
//!   2. TRIVIA INVISIBILITY: symbol-level structure is identical with
//!      and without interleaved comments.
//!   3. SELECTION EXPANSION: ancestor spans at every offset nest.
//!   4. TYPED NAVIGATION: the generated wrappers traverse real trees
//!      (compile-time proof that grammar → types → accessors line up).

use rantlr_engine::*;
use rantlr_grammar::demo::{demo_grammar, demo_syn_grammar};
use rantlr_grammar::demo_ast as ast;
use rantlr_grammar::{
    build_green, build_lr, parse, AstNode, CompiledLexer, GreenNode, NodeRef, TermTok,
};
use std::sync::Arc;

struct Pipeline {
    lexer: CompiledLexer,
    sg: rantlr_grammar::SynGrammar,
    tables: rantlr_grammar::LrTables,
}

fn pipeline() -> Pipeline {
    let (g, ids) = demo_grammar();
    let lexer = CompiledLexer::build(&g).expect("in envelope");
    let sg = demo_syn_grammar(&ids, &lexer.vocab);
    let tables = build_lr(&sg);
    assert!(tables.conflicts.is_empty());
    Pipeline { lexer, sg, tables }
}

fn tree_of(p: &Pipeline, src: &str) -> Arc<GreenNode> {
    let buf = LexedBuffer::new(&p.lexer, src);
    let all = full_tokens(&p.lexer, &buf);
    let terms: Vec<TermTok> = all
        .iter()
        .filter(|t| !t.trivia)
        .map(|t| TermTok { id: t.id, text: t.text.clone() })
        .collect();
    let pnode = parse(&p.sg, &p.tables, &terms).expect("parse");
    build_green(&pnode, &all).expect("tree build")
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
        // "" parses as an empty file (stmts → ε).
        let tree = tree_of(&p, src);
        assert_eq!(tree.text(), src, "tree text must reproduce source exactly");
    }
}

#[test]
fn trivia_is_structurally_invisible() {
    let p = pipeline();
    let with = tree_of(&p, "let a = /* one\n   two */ 1 + 2; // done");
    let without = tree_of(&p, "let a = 1 + 2;");
    // Compare symbol-level shape (kinds down the tree, trivia skipped).
    fn shape(n: &GreenNode, out: &mut Vec<(u16, u16)>) {
        out.push((n.nt, n.prod));
        for c in n.symbol_children() {
            match c {
                rantlr_grammar::GreenChild::Node(m) => shape(m, out),
                rantlr_grammar::GreenChild::Token(_) => {}
            }
        }
    }
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
        let spans = rantlr_grammar::green::ancestor_spans(&tree, off);
        assert!(!spans.is_empty(), "offset {off}");
        for w in spans.windows(2) {
            let (outer, inner) = (w[0], w[1]);
            assert!(
                outer.0 <= inner.0 && inner.1 <= outer.1,
                "offset {off}: {outer:?} must contain {inner:?}"
            );
        }
        let (tok, s, e) = rantlr_grammar::green::token_at_offset(&tree, off).expect("token");
        assert!(s <= off && off < e);
        let _ = tok;
    }
}

#[test]
fn typed_navigation_traverses_real_trees() {
    let p = pipeline();
    let tree = tree_of(&p, "let a = 1 + 2 * 3;");
    let file = ast::File::cast(NodeRef(&tree)).expect("root is File");

    // file → stmts → (more (empty) stmt)
    let stmts = file.stmts().expect("stmts");
    let ast::Stmts::StmtsMore(more) = stmts else {
        panic!("one statement expected")
    };
    let ast::Stmt::LetStmt(let_stmt) = more.stmt().expect("stmt") else {
        panic!("let statement expected")
    };

    assert_eq!(let_stmt.kw_let_token().expect("kw").text(), "let");
    assert_eq!(let_stmt.ident_token().expect("name").text(), "a");
    assert_eq!(let_stmt.semi_token().expect("semi").text(), ";");

    // 1 + 2 * 3 groups as Add(1, Mul(2, 3)).
    let ast::Expr::AddExpr(add) = let_stmt.expr().expect("expr") else {
        panic!("add at the top")
    };
    let ast::Expr::NumLit(lhs) = add.expr().expect("lhs") else {
        panic!("number lhs")
    };
    assert_eq!(lhs.number_token().expect("1").text(), "1");
    let ast::Expr::MulExpr(mul) = add.expr_2().expect("rhs") else {
        panic!("mul rhs")
    };
    assert_eq!(mul.star_token().expect("*").text(), "*");
    let ast::Expr::NumLit(rhs) = mul.expr_2().expect("3") else {
        panic!("number rhs")
    };
    assert_eq!(rhs.number_token().expect("3").text(), "3");

    // Typed layer sees through trivia: same navigation with comments.
    let tree2 = tree_of(&p, "let /*x*/ a /*y*/ = 1 + /*z*/ 2 * 3; // c");
    let file2 = ast::File::cast(NodeRef(&tree2)).expect("root");
    let ast::Stmts::StmtsMore(m2) = file2.stmts().unwrap() else { panic!() };
    let ast::Stmt::LetStmt(ls2) = m2.stmt().unwrap() else { panic!() };
    assert_eq!(ls2.ident_token().unwrap().text(), "a");
    assert!(matches!(ls2.expr(), Some(ast::Expr::AddExpr(_))));
}

#[test]
fn call_args_navigate_through_left_recursion() {
    let p = pipeline();
    let tree = tree_of(&p, "z(1, 2.5);");
    let file = ast::File::cast(NodeRef(&tree)).unwrap();
    let ast::Stmts::StmtsMore(more) = file.stmts().unwrap() else { panic!() };
    let ast::Stmt::ExprStmt(es) = more.stmt().unwrap() else { panic!() };
    let ast::Expr::CallExpr(call) = es.expr().unwrap() else { panic!() };
    assert_eq!(call.ident_token().unwrap().text(), "z");
    let ast::Args::ArgsSome(some) = call.args().unwrap() else { panic!() };
    let ast::ArgsNe::ArgMore(last) = some.args_ne().unwrap() else { panic!() };
    // Left recursion: (ArgMore (ArgFirst 1) COMMA 2.5)
    let ast::ArgsNe::ArgFirst(first) = last.args_ne().unwrap() else { panic!() };
    assert!(matches!(first.expr(), Some(ast::Expr::NumLit(_))));
    let ast::Expr::NumLit(second) = last.expr().unwrap() else { panic!() };
    assert_eq!(second.number_token().unwrap().text(), "2.5");
}
