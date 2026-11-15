//! Trailing context: a token that is accepted only on condition of what
//! follows it on its line (`(?=…)` / `(?!…)` in a `.qana` pattern,
//! [`Pat::before`] / [`Pat::not_before`] here).
//!
//! The lexer was already a longest-match lexer that reads ahead and backs
//! up. What it could not do was refuse a longer match because of what
//! comes next, which is what `1..2` needs: `1.` is a float, unless a `.`
//! follows. These tests pin down what a context means (longest match
//! first, then declaration priority, the end of a line counting as
//! "nothing follows"), that it only ever ends a token, and that the
//! lexer agrees with a direct reading of the patterns on random input.
//!
//! The envelope is untouched: a context is read from the line being
//! lexed and nowhere else, so `lex_line` remains a function of the line's
//! text and its entry state.

use qana_grammar::dfa::CompileError;
use qana_grammar::lexer::{BuildError, CompiledLexer, MStack};
use qana_grammar::lints::LintError;
use qana_grammar::model::{LexGrammar, TokenDef};
use qana_grammar::pat::{classify, ClassSet, ContextError, Pat};
use std::collections::BTreeSet;

fn digits() -> Pat {
    Pat::plus(Pat::Class(ClassSet::digit()))
}
fn letters() -> Pat {
    Pat::plus(Pat::Class(ClassSet { alpha: true, ..Default::default() }))
}
fn spaces() -> Pat {
    Pat::plus(Pat::Class(ClassSet::line_ws()))
}
fn ident() -> Pat {
    Pat::seq([
        Pat::Class(ClassSet::ident_start()),
        Pat::star(Pat::Class(ClassSet::ident_cont())),
    ])
}

/// Lex `text` as one line from the base state: each token's name and text.
fn lex(lexer: &CompiledLexer, text: &str) -> Vec<(String, String)> {
    let (tokens, _) = lexer.lex_line(text, MStack::default());
    let mut at = 0usize;
    tokens
        .iter()
        .map(|token| {
            let piece = &text[at..at + token.len as usize];
            at += token.len as usize;
            let name = lexer
                .vocab
                .names
                .get(token.id as usize)
                .cloned()
                .unwrap_or_else(|| "?".to_string());
            (name, piece.to_string())
        })
        .collect()
}

fn pairs(expected: &[(&str, &str)]) -> Vec<(String, String)> {
    expected.iter().map(|(n, t)| (n.to_string(), t.to_string())).collect()
}

// ---------------------------------------------------------------------------
// Rust's numbers: the case that asked for this
// ---------------------------------------------------------------------------

/// `1.` is a float unless what follows is a `.`, a `_` or a letter (a
/// range, or a field or method of the integer). The first branch of
/// `FLOAT` needs no context: `1.5` is a float whatever follows it.
fn rust_numbers(float_dot_needs_context: bool) -> LexGrammar {
    let mut g = LexGrammar::new("numbers", &["BASE"]);
    let bare_dot = if float_dot_needs_context {
        Pat::seq([
            digits(),
            Pat::lit("."),
            Pat::not_before(Pat::Class(ClassSet {
                chars: vec!['.', '_'],
                alpha: true,
                ..Default::default()
            })),
        ])
    } else {
        Pat::seq([digits(), Pat::lit(".")])
    };
    g.add(TokenDef::new(
        "FLOAT",
        0,
        Pat::alt([Pat::seq([digits(), Pat::lit("."), digits()]), bare_dot]),
    ));
    g.add(TokenDef::new("INT", 0, digits()));
    g.add(TokenDef::new("DOTDOT", 0, Pat::lit("..")));
    g.add(TokenDef::new("DOT", 0, Pat::lit(".")));
    g.add(TokenDef::new("IDENT", 0, ident()));
    g.add(TokenDef::new("PLUS", 0, Pat::lit("+")));
    g.add(TokenDef::new("LPAREN", 0, Pat::lit("(")));
    g.add(TokenDef::new("RPAREN", 0, Pat::lit(")")));
    g.add(TokenDef::new("WS", 0, spaces()).trivia());
    g
}

#[test]
fn a_range_of_integers_is_not_a_float_and_a_dot() {
    let lexer = CompiledLexer::build(&rust_numbers(true)).unwrap();
    assert_eq!(
        lex(&lexer, "1..2"),
        pairs(&[("INT", "1"), ("DOTDOT", ".."), ("INT", "2")])
    );
    assert_eq!(
        lex(&lexer, "0..10+1"),
        pairs(&[("INT", "0"), ("DOTDOT", ".."), ("INT", "10"), ("PLUS", "+"), ("INT", "1")])
    );
}

#[test]
fn a_method_or_field_of_an_integer_is_not_a_float() {
    let lexer = CompiledLexer::build(&rust_numbers(true)).unwrap();
    assert_eq!(
        lex(&lexer, "1.foo()"),
        pairs(&[("INT", "1"), ("DOT", "."), ("IDENT", "foo"), ("LPAREN", "("), ("RPAREN", ")")])
    );
    assert_eq!(lex(&lexer, "1._x"), pairs(&[("INT", "1"), ("DOT", "."), ("IDENT", "_x")]));
    // A letter outside ASCII is a letter.
    assert_eq!(lex(&lexer, "1.é"), pairs(&[("INT", "1"), ("DOT", "."), ("IDENT", "é")]));
}

#[test]
fn a_float_is_still_a_float() {
    let lexer = CompiledLexer::build(&rust_numbers(true)).unwrap();
    // The end of the line: nothing follows, so the negated context holds.
    assert_eq!(lex(&lexer, "1."), pairs(&[("FLOAT", "1.")]));
    assert_eq!(lex(&lexer, "1. "), pairs(&[("FLOAT", "1."), ("WS", " ")]));
    assert_eq!(
        lex(&lexer, "1.+2."),
        pairs(&[("FLOAT", "1."), ("PLUS", "+"), ("FLOAT", "2.")])
    );
    assert_eq!(lex(&lexer, "(1.)"), pairs(&[("LPAREN", "("), ("FLOAT", "1."), ("RPAREN", ")")]));
    // `1.5` needs no context, so what follows it does not matter.
    assert_eq!(lex(&lexer, "1.5"), pairs(&[("FLOAT", "1.5")]));
    assert_eq!(
        lex(&lexer, "1.5.foo"),
        pairs(&[("FLOAT", "1.5"), ("DOT", "."), ("IDENT", "foo")])
    );
    assert_eq!(
        lex(&lexer, "1.5..2"),
        pairs(&[("FLOAT", "1.5"), ("DOTDOT", ".."), ("INT", "2")])
    );
}

/// The same grammar without the context is the lexer as it was: longest
/// match takes `1.` and breaks the range. This is what the context is for.
#[test]
fn without_the_context_the_longest_match_breaks_the_range() {
    let lexer = CompiledLexer::build(&rust_numbers(false)).unwrap();
    assert_eq!(
        lex(&lexer, "1..2"),
        pairs(&[("FLOAT", "1."), ("DOT", "."), ("INT", "2")])
    );
}

// ---------------------------------------------------------------------------
// What a context means
// ---------------------------------------------------------------------------

/// `KEY` is a word followed, after any spaces, by a colon: a context is a
/// whole pattern, of any length, read from the rest of the line.
fn keys_and_words(key_first: bool) -> LexGrammar {
    let mut g = LexGrammar::new("keys", &["BASE"]);
    let key = TokenDef::new(
        "KEY",
        0,
        Pat::seq([
            letters(),
            Pat::before(Pat::seq([
                Pat::star(Pat::Class(ClassSet::line_ws())),
                Pat::lit(":"),
            ])),
        ]),
    );
    let word = TokenDef::new("WORD", 0, letters());
    if key_first {
        g.add(key);
        g.add(word);
    } else {
        g.add(word);
        g.add(key);
    }
    g.add(TokenDef::new("COLON", 0, Pat::lit(":")));
    g.add(TokenDef::new("WS", 0, spaces()).trivia());
    g
}

#[test]
fn a_context_is_a_pattern_of_any_length() {
    let lexer = CompiledLexer::build(&keys_and_words(true)).unwrap();
    assert_eq!(
        lex(&lexer, "name: x"),
        pairs(&[("KEY", "name"), ("COLON", ":"), ("WS", " "), ("WORD", "x")])
    );
    assert_eq!(
        lex(&lexer, "name   :x"),
        pairs(&[("KEY", "name"), ("WS", "   "), ("COLON", ":"), ("WORD", "x")])
    );
    assert_eq!(
        lex(&lexer, "name x"),
        pairs(&[("WORD", "name"), ("WS", " "), ("WORD", "x")])
    );
}

/// At the end of a line nothing follows: a plain context fails there.
#[test]
fn a_plain_context_fails_at_the_end_of_the_line() {
    let lexer = CompiledLexer::build(&keys_and_words(true)).unwrap();
    assert_eq!(lex(&lexer, "name"), pairs(&[("WORD", "name")]));
    assert_eq!(lex(&lexer, "name  "), pairs(&[("WORD", "name"), ("WS", "  ")]));
}

/// Declaration priority is unchanged: at the same length, the earlier
/// token wins. An unconditional token declared first therefore always
/// beats a conditional one of the same length declared after it.
#[test]
fn an_earlier_unconditional_token_outranks_a_later_conditional_one() {
    let lexer = CompiledLexer::build(&keys_and_words(false)).unwrap();
    assert_eq!(lex(&lexer, "name: x")[0], ("WORD".to_string(), "name".to_string()));
}

/// Longest match comes first. A conditional token is tried at the longest
/// end where its context holds, and gives way to a shorter match only
/// when it holds at none.
#[test]
fn the_longest_end_whose_context_holds_wins() {
    let mut g = LexGrammar::new("runs", &["BASE"]);
    // A run of `a` that is followed by a `b`.
    g.add(TokenDef::new(
        "RUN",
        0,
        Pat::seq([Pat::plus(Pat::lit("a")), Pat::before(Pat::lit("b"))]),
    ));
    g.add(TokenDef::new("A", 0, Pat::lit("a")));
    g.add(TokenDef::new("B", 0, Pat::lit("b")));
    let lexer = CompiledLexer::build(&g).unwrap();
    assert_eq!(lex(&lexer, "aaab"), pairs(&[("RUN", "aaa"), ("B", "b")]));
    // No `b` after any of them: every `a` is by itself.
    assert_eq!(lex(&lexer, "aaa"), pairs(&[("A", "a"), ("A", "a"), ("A", "a")]));
}

/// A conditional candidate shorter than an unconditional match is never
/// consulted: the longer match has already won.
#[test]
fn a_longer_unconditional_match_beats_a_shorter_conditional_one() {
    let mut g = LexGrammar::new("short-long", &["BASE"]);
    g.add(TokenDef::new("SHORT", 0, Pat::seq([Pat::lit("ab"), Pat::before(Pat::lit("c"))])));
    g.add(TokenDef::new("LONG", 0, Pat::lit("abc")));
    g.add(TokenDef::new("C", 0, Pat::lit("c")));
    let lexer = CompiledLexer::build(&g).unwrap();
    assert_eq!(lex(&lexer, "abc"), pairs(&[("LONG", "abc")]));
}

/// When the only candidate is conditional and its context fails, there is
/// no token here: the character is Unknown, as for any other mismatch.
#[test]
fn a_failed_context_with_nothing_else_is_unknown() {
    let mut g = LexGrammar::new("only", &["BASE"]);
    g.add(TokenDef::new("A", 0, Pat::seq([Pat::lit("a"), Pat::before(Pat::lit("b"))])));
    g.add(TokenDef::new("B", 0, Pat::lit("b")));
    let lexer = CompiledLexer::build(&g).unwrap();
    assert_eq!(lex(&lexer, "ab"), pairs(&[("A", "a"), ("B", "b")]));
    let (tokens, _) = lexer.lex_line("a", MStack::default());
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens[0].id, g.unknown_id(), "nothing accepts a lone `a`");
}

/// A token's action follows the token: a conditional token that pushes a
/// mode pushes it exactly when its context held.
#[test]
fn a_conditional_token_keeps_its_mode_action() {
    let mut g = LexGrammar::new("tags", &["BASE", "TAG"]);
    g.max_stack = Some(2);
    // `<` opens a tag only when a letter follows.
    g.add(
        TokenDef::new(
            "TAG_OPEN",
            0,
            Pat::seq([
                Pat::lit("<"),
                Pat::before(Pat::Class(ClassSet { alpha: true, ..Default::default() })),
            ]),
        )
        .push(1),
    );
    g.add(TokenDef::new("LESS", 0, Pat::lit("<")));
    g.add(TokenDef::new("WS", 0, spaces()).trivia());
    g.add(TokenDef::new("WORD", 0, letters()));
    g.add(TokenDef::new("TAG_NAME", 1, letters()));
    g.add(TokenDef::new("TAG_CLOSE", 1, Pat::lit(">")).pop());
    let lexer = CompiledLexer::build(&g).unwrap();

    let (_, inside) = lexer.lex_line("<a", MStack::default());
    assert_eq!(inside.depth(), 1, "`<` before a letter opens the tag mode");
    assert_eq!(
        lex(&lexer, "<a>"),
        pairs(&[("TAG_OPEN", "<"), ("TAG_NAME", "a"), ("TAG_CLOSE", ">")])
    );

    let (_, outside) = lexer.lex_line("< a", MStack::default());
    assert_eq!(outside.depth(), 0, "`<` before a space is only a less-than");
    assert_eq!(lex(&lexer, "< a"), pairs(&[("LESS", "<"), ("WS", " "), ("WORD", "a")]));
}

// ---------------------------------------------------------------------------
// The envelope
// ---------------------------------------------------------------------------

/// The line is all a context can see. The same text gives the same tokens
/// whatever the lines around it hold, because `lex_line` is given no
/// other text: here the next line begins with the very character that
/// would have refused the float had it been on this line.
#[test]
fn a_context_does_not_look_past_the_end_of_its_line() {
    let lexer = CompiledLexer::build(&rust_numbers(true)).unwrap();
    let (first, state) = lexer.lex_line("1.", MStack::default());
    let (second, _) = lexer.lex_line(".foo", state);
    assert_eq!(first.len(), 1, "`1.` at the end of its line is one token");
    assert_eq!(lexer.vocab.names[first[0].id as usize], "FLOAT");
    assert_eq!(lexer.vocab.names[second[0].id as usize], "DOT");
    // And on one line, the same characters lex the other way.
    assert_eq!(lex(&lexer, "1..foo")[0], ("INT".to_string(), "1".to_string()));
}

/// L1 still certifies the grammar: a token that reaches its conditional
/// accept across a line terminator is refused like any other.
#[test]
fn a_conditional_token_may_not_span_a_line_either() {
    let mut g = LexGrammar::new("spans", &["BASE"]);
    g.add(TokenDef::new(
        "BAD",
        0,
        Pat::seq([Pat::lit("a\nb"), Pat::not_before(Pat::lit("c"))]),
    ));
    match CompiledLexer::build(&g) {
        Err(BuildError::Lint(LintError::TokenSpansLines { token, .. })) => {
            assert_eq!(token, "BAD")
        }
        other => panic!("expected the L1 refusal, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Where a context may stand
// ---------------------------------------------------------------------------

fn build_error(pat: Pat) -> CompileError {
    let mut g = LexGrammar::new("bad", &["BASE"]);
    g.add(TokenDef::new("T", 0, pat));
    match CompiledLexer::build(&g) {
        Err(BuildError::Compile(e)) => e,
        other => panic!("expected a compile error, got {other:?}"),
    }
}

#[test]
fn a_context_only_ends_a_token() {
    let b = || Pat::before(Pat::lit("b"));
    assert!(matches!(
        build_error(Pat::seq([Pat::lit("a"), b(), Pat::lit("c")])),
        CompileError::Context { error: ContextError::NotAtEnd, .. }
    ));
    // After a group that ends in one.
    assert!(matches!(
        build_error(Pat::seq([Pat::alt([Pat::seq([Pat::lit("a"), b()]), Pat::lit("x")]), Pat::lit("c")])),
        CompileError::Context { error: ContextError::NotAtEnd, .. }
    ));
    assert!(matches!(
        build_error(Pat::plus(Pat::seq([Pat::lit("a"), b()]))),
        CompileError::Context { error: ContextError::Repeated, .. }
    ));
    assert!(matches!(
        build_error(Pat::seq([Pat::lit("a"), Pat::opt(b())])),
        CompileError::Context { error: ContextError::Repeated, .. }
    ));
    assert!(matches!(
        build_error(Pat::seq([Pat::lit("a"), Pat::before(Pat::seq([Pat::lit("b"), b()]))])),
        CompileError::Context { error: ContextError::Nested, .. }
    ));
}

#[test]
fn a_context_must_say_something() {
    // One that can match nothing at all holds everywhere.
    assert!(matches!(
        build_error(Pat::seq([Pat::lit("a"), Pat::before(Pat::star(Pat::lit("b")))])),
        CompileError::EmptyContext { .. }
    ));
    // A token that is only a context consumes nothing.
    assert!(matches!(
        build_error(Pat::before(Pat::lit("a"))),
        CompileError::EmptyMatch { .. }
    ));
}

/// A group that ends in a context may be optional, and each branch of an
/// alternation is judged by itself.
#[test]
fn an_optional_tail_and_separate_branches_are_allowed() {
    let mut g = LexGrammar::new("tails", &["BASE"]);
    // `a`, or `ab` when a `c` follows.
    g.add(TokenDef::new(
        "T",
        0,
        Pat::seq([
            Pat::lit("a"),
            Pat::opt(Pat::seq([Pat::lit("b"), Pat::before(Pat::lit("c"))])),
        ]),
    ));
    g.add(TokenDef::new("B", 0, Pat::lit("b")));
    g.add(TokenDef::new("C", 0, Pat::lit("c")));
    let lexer = CompiledLexer::build(&g).unwrap();
    assert_eq!(lex(&lexer, "abc"), pairs(&[("T", "ab"), ("C", "c")]));
    assert_eq!(lex(&lexer, "abb"), pairs(&[("T", "a"), ("B", "b"), ("B", "b")]));
    assert_eq!(lex(&lexer, "a"), pairs(&[("T", "a")]));
}

// ---------------------------------------------------------------------------
// The lexer against a direct reading of the patterns
// ---------------------------------------------------------------------------

/// Every position at which `pat` can end when matched from `at`: the
/// patterns read directly, with no automaton. A context adds no
/// characters; it keeps `at` when the rest of the line does (or does
/// not) begin with a match of its pattern.
fn ends(pat: &Pat, text: &[char], at: usize) -> BTreeSet<usize> {
    match pat {
        Pat::Never => BTreeSet::new(),
        Pat::Lit(s) => {
            let lit: Vec<char> = s.chars().collect();
            if text[at..].starts_with(&lit) {
                BTreeSet::from([at + lit.len()])
            } else {
                BTreeSet::new()
            }
        }
        Pat::Class(class) => {
            if at < text.len() && class.matches_col(classify(text[at])) {
                BTreeSet::from([at + 1])
            } else {
                BTreeSet::new()
            }
        }
        Pat::Seq(parts) => parts.iter().fold(BTreeSet::from([at]), |from, part| {
            from.iter().flat_map(|&a| ends(part, text, a)).collect()
        }),
        Pat::Alt(parts) => parts.iter().flat_map(|part| ends(part, text, at)).collect(),
        Pat::Opt(inner) => {
            let mut set = ends(inner, text, at);
            set.insert(at);
            set
        }
        Pat::Star(inner) => {
            let mut set = BTreeSet::from([at]);
            let mut frontier = vec![at];
            while let Some(a) = frontier.pop() {
                for end in ends(inner, text, a) {
                    if end > a && set.insert(end) {
                        frontier.push(end);
                    }
                }
            }
            set
        }
        Pat::Plus(inner) => ends(inner, text, at)
            .into_iter()
            .flat_map(|a| ends(&Pat::Star(inner.clone()), text, a))
            .collect(),
        Pat::Ahead { context, negated } => {
            let matches = !ends(context, text, at).is_empty();
            if matches != *negated {
                BTreeSet::from([at])
            } else {
                BTreeSet::new()
            }
        }
    }
}

/// The reference lexer: at each position, the longest end any token
/// reaches, the earliest-declared token on a tie, and one Unknown
/// character when none matches. One mode, ASCII text (a char is a byte).
fn reference_lex(g: &LexGrammar, line: &str) -> Vec<(u16, u32)> {
    let text: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    let mut at = 0usize;
    while at < text.len() {
        let mut best: Option<(usize, u16)> = None;
        for (id, def) in g.tokens.iter().enumerate() {
            if let Some(&end) = ends(&def.pat, &text, at).iter().next_back() {
                if end > at && best.is_none_or(|(longest, _)| end > longest) {
                    best = Some((end, id as u16));
                }
            }
        }
        match best {
            Some((end, id)) => {
                out.push((id, (end - at) as u32));
                at = end;
            }
            None => {
                out.push((g.unknown_id(), 1));
                at += 1;
            }
        }
    }
    out
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn agrees_with_the_reference(g: &LexGrammar, alphabet: &str, seed: u64) {
    let lexer = CompiledLexer::build(g).unwrap();
    let alphabet: Vec<char> = alphabet.chars().collect();
    let mut rng = Rng(seed);
    for _ in 0..4_000 {
        let len = rng.below(14);
        let line: String = (0..len).map(|_| alphabet[rng.below(alphabet.len())]).collect();
        let (tokens, _) = lexer.lex_line(&line, MStack::default());
        let got: Vec<(u16, u32)> = tokens.iter().map(|t| (t.id, t.len)).collect();
        assert_eq!(
            got,
            reference_lex(g, &line),
            "grammar `{}`, line {line:?}",
            g.name
        );
    }
}

#[test]
fn the_lexer_agrees_with_a_direct_reading_of_the_patterns() {
    agrees_with_the_reference(&rust_numbers(true), "012.._ax+( ", 0x51);
    agrees_with_the_reference(&rust_numbers(false), "012.._ax+( ", 0x52);
    agrees_with_the_reference(&keys_and_words(true), "ab:  x", 0x53);
    agrees_with_the_reference(&keys_and_words(false), "ab:  x", 0x54);

    // Contexts that overlap the token's own repetition, plain and negated,
    // on several tokens at once.
    let mut g = LexGrammar::new("overlap", &["BASE"]);
    let a_run = || Pat::plus(Pat::lit("a"));
    g.add(TokenDef::new(
        "BEFORE_B",
        0,
        Pat::seq([a_run(), Pat::before(Pat::seq([Pat::star(Pat::lit("a")), Pat::lit("b")]))]),
    ));
    g.add(TokenDef::new("NOT_BEFORE_C", 0, Pat::seq([a_run(), Pat::not_before(Pat::lit("c"))])));
    g.add(TokenDef::new(
        "AB_OR_A",
        0,
        Pat::alt([
            Pat::seq([Pat::lit("ab"), Pat::not_before(Pat::lit("b"))]),
            Pat::seq([Pat::lit("a"), Pat::before(Pat::lit("cc"))]),
        ]),
    ));
    g.add(TokenDef::new("A", 0, Pat::lit("a")));
    g.add(TokenDef::new("B", 0, Pat::lit("b")));
    g.add(TokenDef::new("CC", 0, Pat::lit("cc")));
    agrees_with_the_reference(&g, "abc", 0x55);
    agrees_with_the_reference(&g, "aabc ", 0x56);
}
