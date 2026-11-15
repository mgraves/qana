//! Trailing context, written in a `.qana` file: `(?=…)` and `(?!…)` at
//! the end of a token pattern.
//!
//! The lexer-level meaning is pinned down in
//! `qana-grammar/tests/context_tests.rs`. Here the same thing goes
//! through the language: a grammar that uses it compiles and certifies,
//! lexes Rust's numbers the way Rust does, stays incremental (an edit
//! gives the tree a batch parse gives), and a pattern that misuses it is
//! refused where the mistake is.

use qana_engine::{IncSession, Line, LineEdit, LineTerm};
use qana_grammar::green::semantic_eq;
use qana_grammar::lexer::MStack;
use qana_lang::compile::certify;
use qana_lang::tsgen::emit_tree_sitter;
use qana_lang::{compile_source, QanaToolchain};

/// Rust's numbers, as far as the lexer is concerned: `1.` is a float
/// unless a `.`, a `_` or a letter follows it.
const NUMBERS: &str = r#"language Numbers

token WS     = /\s+/ @trivia
token FLOAT  = /\d+\.\d+|\d+\.(?![._\a])/ @style(number)
token INT    = /\d+/ @style(number)
token IDENT  = /[\a_][\w_]*/ @style(variable)
token DOTDOT = ".." @style(operator)
token DOT    = "." @style(punctuation)
token LPAREN = "(" @style(bracket)
token RPAREN = ")" @style(bracket)

start file

rule file = File: items

rule items = item*

rule item =
  | Float:  FLOAT
  | Int:    INT
  | Name:   IDENT
  | Range:  ".."
  | Access: "."
  | Open:   "("
  | Close:  ")"
"#;

fn names(lexer: &qana_grammar::CompiledLexer, line: &str) -> Vec<String> {
    lexer
        .lex_line(line, MStack::default())
        .0
        .iter()
        .filter(|t| !lexer.is_trivia(t.id))
        .map(|t| lexer.vocab.names[t.id as usize].clone())
        .collect()
}

#[test]
fn a_qana_grammar_lexes_rusts_numbers() {
    let tc = QanaToolchain::new();
    let out = compile_source(&tc, NUMBERS);
    assert!(out.diags.is_empty(), "compiles: {:?}", out.diags);
    let (lexer, _tables) = certify(&out.def).expect("certifies: the envelope holds");

    assert_eq!(names(&lexer, "1..2"), ["INT", "DOTDOT", "INT"]);
    assert_eq!(names(&lexer, "1.foo()"), ["INT", "DOT", "IDENT", "LPAREN", "RPAREN"]);
    assert_eq!(names(&lexer, "1._x"), ["INT", "DOT", "IDENT"]);
    assert_eq!(names(&lexer, "1."), ["FLOAT"]);
    assert_eq!(names(&lexer, "(1.)"), ["LPAREN", "FLOAT", "RPAREN"]);
    assert_eq!(names(&lexer, "1.5"), ["FLOAT"]);
    assert_eq!(names(&lexer, "1.5.foo"), ["FLOAT", "DOT", "IDENT"]);
    assert_eq!(names(&lexer, "1.5 .. 2."), ["FLOAT", "DOTDOT", "FLOAT"]);
}

/// Editing a line whose tokens depend on a context gives the tree a batch
/// parse gives, and reuses the lines the edit did not touch. The context
/// reads only the edited line, so nothing else is relexed for it.
#[test]
fn a_grammar_with_a_context_stays_incremental() {
    let tc = QanaToolchain::new();
    let out = compile_source(&tc, NUMBERS);
    assert!(out.diags.is_empty(), "{:?}", out.diags);
    let (lexer, tables) = certify(&out.def).unwrap();
    let sg = &out.def.sg;

    let source = "1.\nfoo 2.5\n(3)\nbar 4 5\n";
    let mut session = IncSession::new(&lexer, sg, &tables, source).unwrap();
    assert_eq!(session.buf.reproduce(), source);

    // `1.` (a float) becomes `1..2` (a range), then `1.x` (an access),
    // then a float again; another line is edited in between.
    let edits: &[(usize, &str)] = &[
        (0, "1..2"),
        (0, "1.x"),
        (2, "(3.)"),
        (0, "1."),
        (2, "(3..4.)"),
    ];
    for &(line, text) in edits {
        let outcome = session
            .edit(sg, &tables, &[LineEdit {
                start: line,
                end: line + 1,
                replacement: vec![Line::new(text, LineTerm::Lf)],
            }])
            .unwrap();
        let now = session.buf.reproduce();
        let batch = IncSession::new(&lexer, sg, &tables, &now).unwrap();
        assert!(
            semantic_eq(session.tree().unwrap(), batch.tree().unwrap()),
            "incremental ≡ batch after line {line} became {text:?}"
        );
        assert!(outcome.stats.reused_terms > 0, "the untouched lines are reused");
    }
}

/// A context in the wrong place is refused at the place.
#[test]
fn a_misplaced_context_is_refused_where_it_stands() {
    let tc = QanaToolchain::new();

    // Something follows it: the diagnostic is on what follows.
    let src = "token BAD = /a(?=b)c/\nrule file = File:\n";
    let out = compile_source(&tc, src);
    let diag = out
        .diags
        .iter()
        .find(|d| d.msg.contains("must end the token"))
        .unwrap_or_else(|| panic!("a diagnostic about the context: {:?}", out.diags));
    assert_eq!(&src[diag.span.0 as usize..diag.span.1 as usize], "c");

    // Not a form the language has.
    let src = "token BAD = /a(?:b)/\nrule file = File:\n";
    let out = compile_source(&tc, src);
    assert!(
        out.diags.iter().any(|d| d.msg.contains("(?=…)")),
        "names the forms there are: {:?}",
        out.diags
    );

    // A context that can match nothing says nothing: refused when the
    // lexer is built, at the token.
    let src = "token BAD = /a(?=b*)/\nrule file = File:\n";
    let out = compile_source(&tc, src);
    assert!(out.diags.is_empty(), "compiles as a value: {:?}", out.diags);
    let diags = certify(&out.def).expect_err("an empty context is refused");
    assert_eq!(&src[diags[0].span.0 as usize..diags[0].span.1 as usize], "BAD");
    assert!(diags[0].msg.contains("empty string"), "{}", diags[0].msg);
}

/// Tree-sitter's lexer cannot make a token depend on what follows it.
/// Emitting the pattern without its context would describe a different
/// language, so the emission is refused and says why.
#[test]
fn tree_sitter_emission_refuses_a_trailing_context() {
    let tc = QanaToolchain::new();
    let out = compile_source(&tc, NUMBERS);
    assert!(out.diags.is_empty(), "{:?}", out.diags);
    let (_, tables) = certify(&out.def).unwrap();
    let errors =
        emit_tree_sitter(&out.def.lex, &out.def.sg, &tables, &out.def.styles, &out.def.binding)
            .expect_err("must refuse");
    assert!(
        errors.iter().any(|e| e.contains("FLOAT") && e.contains("trailing context")),
        "names the token and the reason: {errors:?}"
    );
}
