//! THE P1 GATE: the lexer GENERATED from the demo grammar value must be
//! observationally equivalent to the P0 hand-written lexer on the same
//! language, and the generic engine must preserve incremental ≡ batch.
//!
//! Observational equivalence (what downstream consumers can see):
//!   (a) byte coverage is total and lossless for both;
//!   (b) the NON-TRIVIA token stream is identical (kind + length);
//!   (c) each byte's trivia-ness is identical (trivia segmentation may
//!       differ: the hand lexer merges comment spans, the generated one
//!       emits open/content/close pieces — same bytes, same class);
//!   (d) line exit states are isomorphic (Normal ↔ empty stack,
//!       BlockComment(d) ↔ d nested BLOCK modes).

use qana_engine::*;
use qana_grammar::demo::{demo_grammar, DemoIds};
use qana_grammar::{CompiledLexer, MStack, TokenId};
use qana_lex as hand;

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

/// Comparison classes: the P0 hand lexer's granularity. The generated
/// lexer refines it (distinct keyword ids, distinct operator ids); both
/// sides map into these classes and must agree exactly.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Class {
    Ident,
    Keyword,
    Number,
    StrTerm,
    StrUnterm,
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Punct,
}

/// Hand-lexer token kind → class (`None` for trivia kinds).
fn class_of_hand(kind: hand::TokenKind) -> Option<Class> {
    use hand::TokenKind as K;
    Some(match kind {
        K::Whitespace | K::LineComment | K::BlockComment | K::Unknown => return None,
        K::Ident => Class::Ident,
        K::Keyword => Class::Keyword,
        K::Number => Class::Number,
        K::Str { terminated: true } => Class::StrTerm,
        K::Str { terminated: false } => Class::StrUnterm,
        K::LParen => Class::LParen,
        K::RParen => Class::RParen,
        K::LBracket => Class::LBracket,
        K::RBracket => Class::RBracket,
        K::LBrace => Class::LBrace,
        K::RBrace => Class::RBrace,
        K::Punct => Class::Punct,
    })
}

/// Generated token id → class (`None` for trivia ids).
fn class_of_gen(id: TokenId, ids: &DemoIds) -> Option<Class> {
    Some(if id == ids.ident {
        Class::Ident
    } else if ids.is_kw(id) {
        Class::Keyword
    } else if id == ids.number {
        Class::Number
    } else if id == ids.string {
        Class::StrTerm
    } else if id == ids.string_unterm {
        Class::StrUnterm
    } else if id == ids.lparen {
        Class::LParen
    } else if id == ids.rparen {
        Class::RParen
    } else if id == ids.lbracket {
        Class::LBracket
    } else if id == ids.rbracket {
        Class::RBracket
    } else if id == ids.lbrace {
        Class::LBrace
    } else if id == ids.rbrace {
        Class::RBrace
    } else if ids.is_punct_class(id) {
        Class::Punct
    } else {
        return None; // trivia (WS, comments, unknown, block pieces)
    })
}

fn state_iso(hand_state: hand::LineState, gen_state: MStack, ids: &DemoIds) -> bool {
    match hand_state {
        hand::LineState::Normal => gen_state.depth() == 0,
        hand::LineState::BlockComment(d) => {
            gen_state.depth() == d as usize && gen_state.current_mode() == ids.block_mode
        }
    }
}

/// Compare one line under both lexers. Returns the two exit states.
fn compare_line(
    text: &str,
    hand_entry: hand::LineState,
    gen_entry: MStack,
    lexer: &CompiledLexer,
    ids: &DemoIds,
) -> (hand::LineState, MStack) {
    let (h_toks, h_exit) = hand::lex_line(text, hand_entry);
    let (g_toks, g_exit) = lexer.lex_line(text, gen_entry);

    // (a) total coverage both
    assert_eq!(h_toks.iter().map(|t| t.len as usize).sum::<usize>(), text.len());
    assert_eq!(g_toks.iter().map(|t| t.len as usize).sum::<usize>(), text.len());

    // (b) non-trivia streams identical at class granularity
    let h_sig: Vec<(Class, u32)> =
        h_toks.iter().filter_map(|t| class_of_hand(t.kind).map(|c| (c, t.len))).collect();
    let g_sig: Vec<(Class, u32)> =
        g_toks.iter().filter_map(|t| class_of_gen(t.id, ids).map(|c| (c, t.len))).collect();
    assert_eq!(h_sig, g_sig, "non-trivia divergence on line {text:?}");
    // Consistency: the vocab's trivia view must agree with the class map.
    for t in &g_toks {
        assert_eq!(class_of_gen(t.id, ids).is_none(), lexer.is_trivia(t.id));
    }

    // (c) per-byte trivia flags identical
    let mut h_bytes = Vec::with_capacity(text.len());
    for t in &h_toks {
        for _ in 0..t.len {
            h_bytes.push(t.kind.is_trivia());
        }
    }
    let mut g_bytes = Vec::with_capacity(text.len());
    for t in &g_toks {
        for _ in 0..t.len {
            g_bytes.push(lexer.is_trivia(t.id));
        }
    }
    assert_eq!(h_bytes, g_bytes, "trivia-coverage divergence on line {text:?}");

    // (d) exit states isomorphic
    assert!(
        state_iso(h_exit, g_exit, ids),
        "state divergence on line {text:?}: hand {h_exit:?} vs gen {g_exit:?}"
    );
    (h_exit, g_exit)
}

fn compare_document(src: &str, lexer: &CompiledLexer, ids: &DemoIds) {
    let lines = split_lines(src);
    let mut hs = hand::LineState::Normal;
    let mut gs = MStack::default();
    for line in &lines {
        let (h, g) = compare_line(&line.text, hs, gs, lexer, ids);
        hs = h;
        gs = g;
    }
}

#[test]
fn generated_equals_hand_on_edge_cases() {
    let (g, ids) = demo_grammar();
    let lexer = CompiledLexer::build(&g).expect("demo grammar must be in envelope");
    for src in [
        "",
        "fn main() { let x = 1.5; }",
        "let s = \"héllo \\\" wörld 🚀\";",
        "let s = \"unterminated",
        "let s = \"trailing backslash\\",
        "/* open\nstill open /* nested */\nclosed here */ done",
        "/* a /* b /* c /* d /* e /* f /* g /* h /* i beyond cap */ */ */ */ */ */ */ */ */",
        "x = 1.5 + 2. + .5 + 1..2",
        "**/ stray close in DEFAULT",
        "émoji 🚀 §§ \u{00A0}nbsp \u{feff}bom",
        "let_ letx let leti32",
        "weird \u{0}\u{1}\u{7f} bytes",
        "a /* c1 */ b /* c2\nnext */ c",
    ] {
        compare_document(src, &lexer, &ids);
    }
}

#[test]
fn generated_equals_hand_on_generated_corpus() {
    let (g, ids) = demo_grammar();
    let lexer = CompiledLexer::build(&g).expect("in envelope");
    // Reuse the benchmark generator shape at smaller scale.
    let mut src = String::new();
    for i in 0..5000usize {
        match i % 10 {
            0 => src.push_str(&format!("fn compute_{i}(a: u32, b: u32) -> u32 {{\n")),
            1 => src.push_str(&format!("    let acc_{i} = a * {i} + b; // total\n")),
            2 => src.push_str(&format!("    let name_{i} = \"item {i} label\";\n")),
            3 => src.push_str(&format!("    if acc_{i} > {i} {{ emit(name_{i}, [a, b]); }}\n")),
            4 => src.push_str("    /* note: /* nested */ back */\n"),
            5 => src.push_str(&format!("    let v_{i} = (a + b) * ({i} - 1);\n")),
            6 => src.push_str("    match a { 0 => b, _ => a }\n"),
            7 => src.push_str(&format!("    // checkpoint {i}\n")),
            8 => src.push_str(&format!("    return acc(v_{i}, {i}.5);\n")),
            _ => src.push_str("}\n"),
        }
    }
    compare_document(&src, &lexer, &ids);
}

#[test]
fn generated_equals_hand_on_fuzzed_docs() {
    let (g, ids) = demo_grammar();
    let lexer = CompiledLexer::build(&g).expect("in envelope");
    const PIECES: &[&str] = &[
        "fn f", "let x = 1", "if x { y() }", "\"str\"", "\"unterm",
        "/* open", "*/", "/* whole */", "// line comment", "  ", "}",
        "{", "]", "[", "(a, b)", "1.25", "émoji 🚀", "return x",
        "/* nest /* deep */", "word", "", "\\", "\"esc \\\" q\"", "***", "///",
    ];
    let mut rng = Rng::new(0xC0FFEE);
    for _ in 0..300 {
        let n_lines = 5 + rng.below(60);
        let mut src = String::new();
        for i in 0..n_lines {
            let n = 1 + rng.below(4);
            for k in 0..n {
                if k > 0 {
                    src.push(' ');
                }
                src.push_str(PIECES[rng.below(PIECES.len())]);
            }
            if i + 1 < n_lines {
                src.push('\n');
            }
        }
        compare_document(&src, &lexer, &ids);
    }
}

#[test]
fn engine_incremental_equals_batch_with_generated_lexer() {
    let (g, _ids) = demo_grammar();
    let lexer = CompiledLexer::build(&g).expect("in envelope");
    const PIECES: &[&str] = &[
        "fn f", "let x = 1", "\"unterm", "/* open", "*/", "/* whole */",
        "// c", "}", "{", "1.25", "émoji 🚀", "", "word /* nest",
    ];
    let mut rng = Rng::new(0xDECAF);
    for iter in 0..200 {
        let n_lines = 20 + rng.below(80);
        let mut src = String::new();
        for i in 0..n_lines {
            src.push_str(PIECES[rng.below(PIECES.len())]);
            if i + 1 < n_lines {
                src.push('\n');
            }
        }
        let mut buf = LexedBuffer::new(&lexer, &src);
        for round in 0..3 {
            let doc_lines = buf.lines.len();
            let sites = 1 + rng.below(6);
            let mut cuts: Vec<usize> = (0..sites * 2).map(|_| rng.below(doc_lines)).collect();
            cuts.sort_unstable();
            let mut edits: Vec<LineEdit> = Vec::new();
            for pair in cuts.chunks(2) {
                let (start, end) = (pair[0], pair[1]);
                if let Some(prev) = edits.last() {
                    if start < prev.end {
                        continue;
                    }
                }
                let rep_n = rng.below(4);
                let replacement = (0..rep_n)
                    .map(|_| Line::new(PIECES[rng.below(PIECES.len())], LineTerm::Lf))
                    .collect();
                edits.push(LineEdit { start, end, replacement });
            }
            if edits.is_empty() {
                continue;
            }
            let report = buf.apply_edits(&edits);
            let oracle = LexedBuffer::new(&lexer, &buf.reproduce());
            assert_eq!(buf.lines.len(), oracle.lines.len());
            for li in 0..buf.lines.len() {
                assert_eq!(
                    buf.lexed[li].tokens, oracle.lexed[li].tokens,
                    "iter {iter} round {round} line {li} (report {report:?})"
                );
                assert_eq!(buf.lexed[li].exit, oracle.lexed[li].exit);
            }
            assert!(buf.verify_coverage());
        }
    }
}

#[test]
fn skeleton_works_through_generated_vocab() {
    let (g, _ids) = demo_grammar();
    let lexer = CompiledLexer::build(&g).expect("in envelope");
    let src = "fn f() {\n  if x { y(); }\n}\n\nfn g() {\n  ) stray\n  [1, 2]\n}\n";
    let buf = LexedBuffer::new(&lexer, src);
    let sk = build_skeleton(&buf, &lexer.vocab);
    let folds: Vec<(usize, usize)> = sk.folding_ranges().collect();
    assert!(folds.contains(&(0, 2)));
    assert!(folds.contains(&(4, 7)));
    assert_eq!(sk.unmatched_closes, 1);
}

// ---------------------------------------------------------------------------
// The envelope refuses out-of-envelope grammars WITH counterexamples.
// ---------------------------------------------------------------------------

mod lints {
    use qana_grammar::lints::LintError;
    use qana_grammar::lexer::BuildError;
    use qana_grammar::*;

    #[test]
    fn l1_rejects_line_spanning_token_with_witness() {
        // The classic mistake: a C-style block comment as ONE token that
        // can swallow newlines.
        let mut g = LexGrammar::new("Bad", &["DEFAULT"]);
        g.add(TokenDef::new(
            "BLOCK_COMMENT",
            0,
            Pat::seq([
                Pat::lit("/*"),
                Pat::star(Pat::Class(ClassSet::not_chars(&['*']))), // includes \n!
                Pat::lit("*/"),
            ]),
        ));
        let err = CompiledLexer::build(&g).expect_err("must be refused");
        match err {
            BuildError::Lint(LintError::TokenSpansLines { token, witness }) => {
                assert_eq!(token, "BLOCK_COMMENT");
                assert!(witness.contains('\n'), "witness must exhibit the newline: {witness:?}");
                assert!(witness.starts_with("/*") && witness.ends_with("*/"));
            }
            other => panic!("wrong error: {other:?}"),
        }
    }

    #[test]
    fn l2_rejects_unbounded_mode_cycle_naming_it() {
        let mut g = LexGrammar::new("Bad", &["DEFAULT", "INNER"]);
        g.add(TokenDef::new("OPEN", 0, Pat::lit("<<")).push(1));
        g.add(TokenDef::new("REOPEN", 1, Pat::lit("<<")).push(1)); // self-cycle
        g.add(TokenDef::new("CLOSE", 1, Pat::lit(">>")).pop());
        // no max_stack declared
        let err = CompiledLexer::build(&g).expect_err("must be refused");
        match err {
            BuildError::Lint(LintError::UnboundedModeStack { cycle }) => {
                assert!(cycle.contains(&"INNER".to_string()), "cycle: {cycle:?}");
            }
            other => panic!("wrong error: {other:?}"),
        }
    }

    #[test]
    fn l2_accepts_cycle_with_declared_bound_and_certifies_it() {
        let mut g = LexGrammar::new("Ok", &["DEFAULT", "INNER"]);
        g.add(TokenDef::new("OPEN", 0, Pat::lit("<<")).push(1));
        g.add(TokenDef::new("REOPEN", 1, Pat::lit("<<")).push(1));
        g.add(TokenDef::new("CLOSE", 1, Pat::lit(">>")).pop());
        g.add(TokenDef::new("TEXT", 1, Pat::plus(Pat::Class(ClassSet::not_chars(&['<', '>', '\r', '\n'])))));
        g.add(TokenDef::new("WS", 0, Pat::plus(Pat::Class(ClassSet::line_ws()))).trivia());
        g.max_stack = Some(4);
        let lexer = CompiledLexer::build(&g).expect("bounded cycle is fine");
        assert_eq!(lexer.report.stack_bound, 4);
    }

    #[test]
    fn empty_match_token_rejected() {
        let mut g = LexGrammar::new("Bad", &["DEFAULT"]);
        g.add(TokenDef::new("EMPTYABLE", 0, Pat::star(Pat::Class(ClassSet::digit()))));
        let err = CompiledLexer::build(&g).expect_err("must be refused");
        assert!(format!("{err}").contains("EMPTYABLE"));
    }

    #[test]
    fn acyclic_modes_get_natural_bound_without_declaration() {
        let mut g = LexGrammar::new("Ok", &["DEFAULT", "A", "B"]);
        g.add(TokenDef::new("TO_A", 0, Pat::lit("a{")).push(1));
        g.add(TokenDef::new("TO_B", 1, Pat::lit("b{")).push(2));
        g.add(TokenDef::new("END_A", 1, Pat::lit("}")).pop());
        g.add(TokenDef::new("END_B", 2, Pat::lit("}")).pop());
        g.add(TokenDef::new("X", 0, Pat::plus(Pat::Class(ClassSet::digit()))));
        g.add(TokenDef::new("XA", 1, Pat::plus(Pat::Class(ClassSet::digit()))));
        g.add(TokenDef::new("XB", 2, Pat::plus(Pat::Class(ClassSet::digit()))));
        let lexer = CompiledLexer::build(&g).expect("acyclic is always bounded");
        assert_eq!(lexer.report.stack_bound, 2, "DEFAULT→A→B is the longest chain");
    }
}
