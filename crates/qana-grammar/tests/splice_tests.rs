//! The line-splice envelope feature (`@continues`): C's backslash-
//! newline expressed in mode-state terms. Tokens still never span
//! lines (L1 holds untouched); the SPLICE acts at a token boundary,
//! and the mode stack — which is already the line state — carries the
//! continuation, so incremental lexing inherits it for free. Strict C
//! semantics fall out of "final token of the line": trailing
//! whitespace after the backslash makes whitespace the final token,
//! and there is no splice.

use qana_grammar::lexer::{BuildError, CompiledLexer, MStack};
use qana_grammar::lints::LintError;
use qana_grammar::model::{LexGrammar, TokenDef};
use qana_grammar::pat::Pat;

/// BASE with `#` pushing an eol-bounded PP; PP has a word, spaces
/// (trivia), and a `\` splice (trivia + continues).
fn splice_grammar() -> LexGrammar {
    let mut g = LexGrammar::new("splice-test", &["BASE", "PP"]);
    g.eol_pop[1] = true;
    g.add(TokenDef::new("HASH", 0, Pat::Lit("#".into())).push(1));
    g.add(TokenDef::new("X", 0, Pat::Lit("x".into())));
    g.add(TokenDef::new("PP_WORD", 1, Pat::Lit("def".into())));
    g.add(TokenDef::new("PP_WS", 1, Pat::Lit(" ".into())).trivia());
    g.add(TokenDef::new("PP_CONT", 1, Pat::Lit("\\".into())).trivia().continues());
    g
}

const PP_WORD: u16 = 2;

#[test]
fn splice_carries_the_line_bounded_mode() {
    let lx = CompiledLexer::build(&splice_grammar()).unwrap();
    let (_, s1) = lx.lex_line("#def \\", MStack::default());
    assert_eq!(s1.depth(), 1, "the splice keeps PP alive past the eol");
    assert_eq!(s1.current_mode(), 1);

    // The continued line lexes IN PP: `def` is PP's word, and without
    // a further splice this line ends the mode normally.
    let (toks, s2) = lx.lex_line("def", s1);
    assert_eq!(toks[0].id, PP_WORD, "continued line tokenizes in directive space");
    assert_eq!(s2.depth(), 0, "no splice on the continued line: the mode ends");
}

#[test]
fn without_a_splice_the_mode_pops() {
    let lx = CompiledLexer::build(&splice_grammar()).unwrap();
    let (_, s) = lx.lex_line("#def", MStack::default());
    assert_eq!(s.depth(), 0, "eol-bounded means eol-bounded");
}

#[test]
fn trailing_space_defeats_the_splice() {
    // C's rule: the backslash must sit immediately before the newline.
    let lx = CompiledLexer::build(&splice_grammar()).unwrap();
    let (_, s) = lx.lex_line("#def \\ ", MStack::default());
    assert_eq!(s.depth(), 0, "whitespace after the backslash is no splice");
}

#[test]
fn mid_line_splice_is_inert() {
    let lx = CompiledLexer::build(&splice_grammar()).unwrap();
    let (_, s) = lx.lex_line("#\\def", MStack::default());
    assert_eq!(s.depth(), 0, "a splice anywhere but line-final is an ordinary token");
}

#[test]
fn continues_outside_a_line_bounded_mode_is_refused() {
    let mut g = splice_grammar();
    g.eol_pop[1] = false; // PP no longer line-bounded: the splice is dead config
    match CompiledLexer::build(&g) {
        Err(BuildError::Lint(LintError::ContinuationOutsideLineBoundedMode {
            token,
            mode,
        })) => {
            assert_eq!(token, "PP_CONT");
            assert_eq!(mode, "PP");
        }
        other => panic!("expected the continuation lint, got {other:?}"),
    }
}
