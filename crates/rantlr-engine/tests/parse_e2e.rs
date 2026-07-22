//! End-to-end: generated lexer → incremental buffer → LR parse →
//! s-expression goldens. The full generated pipeline, no hand-written
//! language code anywhere.

use rantlr_engine::*;
use rantlr_grammar::demo::{demo_grammar, demo_syn_grammar};
use rantlr_grammar::{build_lr, parse, sexpr, CompiledLexer, TermTok};

fn pipeline() -> (CompiledLexer, rantlr_grammar::SynGrammar, rantlr_grammar::LrTables) {
    let (g, ids) = demo_grammar();
    let lexer = CompiledLexer::build(&g).expect("demo lex grammar in envelope");
    let sg = demo_syn_grammar(&ids, &lexer.vocab);
    let tables = build_lr(&sg);
    assert!(
        tables.conflicts.is_empty(),
        "demo syntax grammar must be conflict-free: {:#?}",
        tables.conflicts
    );
    (lexer, sg, tables)
}

/// Harvest non-trivia terminals (with text) from a lexed buffer.
fn terminals(lexer: &CompiledLexer, buf: &LexedBuffer<'_, CompiledLexer>) -> Vec<TermTok> {
    let mut out = Vec::new();
    for (line, lt) in buf.lines.iter().zip(&buf.lexed) {
        let mut off = 0usize;
        for tok in &lt.tokens {
            let end = off + tok.len as usize;
            if !lexer.is_trivia(tok.id) {
                out.push(TermTok { id: tok.id, text: line.text[off..end].to_string() });
            }
            off = end;
        }
    }
    out
}

fn parse_sexpr(src: &str) -> String {
    let (lexer, sg, tables) = pipeline();
    let buf = LexedBuffer::new(&lexer, src);
    let toks = terminals(&lexer, &buf);
    let tree = parse(&sg, &tables, &toks).expect("parse must succeed");
    sexpr(&sg, &tree)
}

#[test]
fn precedence_groups_multiplication_tighter() {
    assert_eq!(
        parse_sexpr("let a = 1 + 2 * 3;"),
        "(file (stmts (stmts) (stmt KW_LET IDENT EQ \
         (expr (expr NUMBER) PLUS (expr (expr NUMBER) STAR (expr NUMBER))) SEMI)))"
    );
}

#[test]
fn subtraction_associates_left() {
    assert_eq!(
        parse_sexpr("let a = 1 - 2 - 3;"),
        "(file (stmts (stmts) (stmt KW_LET IDENT EQ \
         (expr (expr (expr NUMBER) MINUS (expr NUMBER)) MINUS (expr NUMBER)) SEMI)))"
    );
}

#[test]
fn parens_override_precedence() {
    assert_eq!(
        parse_sexpr("(1 + 2) * 3;"),
        "(file (stmts (stmts) (stmt (expr (expr LPAREN \
         (expr (expr NUMBER) PLUS (expr NUMBER)) RPAREN) STAR (expr NUMBER)) SEMI)))"
    );
}

#[test]
fn if_else_calls_and_args() {
    assert_eq!(
        parse_sexpr("if (x) { y(); } else { z(1, 2); }"),
        "(file (stmts (stmts) (stmt KW_IF LPAREN (expr IDENT) RPAREN \
         (block LBRACE (stmts (stmts) (stmt (expr IDENT LPAREN (args) RPAREN) SEMI)) RBRACE) \
         KW_ELSE \
         (block LBRACE (stmts (stmts) (stmt (expr IDENT LPAREN (args (args_ne (args_ne (expr NUMBER)) COMMA (expr NUMBER))) RPAREN) SEMI)) RBRACE))))"
    );
}

#[test]
fn comments_and_strings_are_invisible_to_the_parser() {
    // Trivia (comments) never reach the parser; multi-line comments work
    // through the line-state machinery.
    let with_comments = parse_sexpr("let a = /* one\n   two */ 1 + 2; // done");
    let without = parse_sexpr("let a = 1 + 2;");
    assert_eq!(with_comments, without);
}

#[test]
fn syntax_error_carries_expected_set() {
    let (lexer, sg, tables) = pipeline();
    let buf = LexedBuffer::new(&lexer, "let x = ;");
    let toks = terminals(&lexer, &buf);
    let err = parse(&sg, &tables, &toks).expect_err("must fail");
    assert_eq!(err.at, 3, "error at the `;`");
    assert_eq!(err.found, "`;`");
    // The expected set is the completion primitive: exactly what may
    // start an expression here.
    for want in ["NUMBER", "STRING", "IDENT", "LPAREN", "LBRACKET"] {
        assert!(
            err.expected.iter().any(|e| e == want),
            "expected set missing {want}: {:?}",
            err.expected
        );
    }
    // And nothing statement-y leaks in.
    assert!(!err.expected.iter().any(|e| e == "SEMI" || e == "KW_LET"));
}

#[test]
fn unexpected_eof_reports_expected_continuations() {
    let (lexer, sg, tables) = pipeline();
    let buf = LexedBuffer::new(&lexer, "let x = 1 + ");
    let toks = terminals(&lexer, &buf);
    let err = parse(&sg, &tables, &toks).expect_err("must fail");
    assert_eq!(err.found, "<eof>");
    assert!(err.expected.iter().any(|e| e == "NUMBER"));
}
