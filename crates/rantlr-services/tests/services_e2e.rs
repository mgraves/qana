//! P3 gates: every derived service tested against ground truth — and the
//! semantic-token DELTA gate: applying incremental updates must always
//! yield exactly the same data as a fresh full encode.

use rantlr_engine::*;
use rantlr_grammar::demo::{demo_grammar, demo_syn_grammar, DemoIds};
use rantlr_grammar::{build_lr, CompiledLexer, LrTables, SynGrammar};
use rantlr_services::demo_glue::{demo_outline_config, demo_styles};
use rantlr_services::*;

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

fn pipeline() -> (CompiledLexer, SynGrammar, LrTables, DemoIds) {
    let (g, ids) = demo_grammar();
    let lexer = CompiledLexer::build(&g).expect("in envelope");
    let sg = demo_syn_grammar(&ids, &lexer.vocab);
    let tables = build_lr(&sg);
    assert!(tables.conflicts.is_empty());
    (lexer, sg, tables, ids)
}

#[test]
fn semantic_tokens_encode_expected_classes() {
    let (lexer, _sg, _tables, ids) = pipeline();
    let styles = demo_styles(&ids);
    let buf = LexedBuffer::new(&lexer, "let a = 1; // note");
    let st = semantic_tokens_full(&lexer, &buf, &styles);
    // let(kw) a(var) =(op) 1(num) ;(punct) //note(comment) = 6 styled tokens.
    assert_eq!(st.data.len(), 6 * 5);
    let classes: Vec<u32> = st.data.chunks(5).map(|q| q[3]).collect();
    let names: Vec<&str> = classes.iter().map(|&c| styles.legend[c as usize]).collect();
    assert_eq!(names, ["keyword", "variable", "operator", "number", "punctuation", "comment"]);
    // First token is absolute (0,0), length 3 ("let").
    assert_eq!(&st.data[..3], &[0, 0, 3]);
}

#[test]
fn semantic_token_delta_equals_fresh_full_under_random_edits() {
    let (lexer, sg, tables, ids) = pipeline();
    let styles = demo_styles(&ids);
    const POOL: &[&str] = &[
        "let x = 1 + 2 * 3;",
        "emit(a, b);",
        "if (x) { y(); } else { z(1, 2); }",
        "// a comment line",
        "let s = \"text\";",
        "{ let inner = 5; }",
        "",
    ];
    let mut rng = Rng::new(0x5EA7);
    for iter in 0..40 {
        let n = 15 + rng.below(40);
        let src: String = (0..n).map(|_| format!("{}\n", POOL[rng.below(POOL.len())])).collect();
        let mut s = IncSession::new(&lexer, &sg, &tables, &src).unwrap();
        let mut cache = semantic_tokens_full(&lexer, &s.buf, &styles);
        for round in 0..4 {
            let lines = s.buf.lines.len() - 1;
            if lines == 0 {
                break;
            }
            let start = rng.below(lines);
            let (end, replacement) = match rng.below(3) {
                0 => (start + 1, vec![Line::new(POOL[rng.below(POOL.len())], LineTerm::Lf)]),
                1 => (start, vec![Line::new(POOL[rng.below(POOL.len())], LineTerm::Lf)]), // insert
                _ => ((start + 1).min(lines), vec![]),                                   // delete
            };
            let out = s.edit(&sg, &tables, &[LineEdit { start, end, replacement }]).unwrap();
            cache.update(&lexer, &s.buf, &styles, &out.damage);
            let fresh = semantic_tokens_full(&lexer, &s.buf, &styles);
            assert_eq!(
                cache, fresh,
                "iter {iter} round {round}: delta-updated cache must equal fresh encode\ndoc:\n{}",
                s.buf.reproduce()
            );
        }
    }
}

#[test]
fn folding_covers_blocks_and_comments() {
    let (lexer, _sg, _tables, _ids) = pipeline();
    let src = "if (x) {\n  y();\n  z();\n}\n/* long\n   comment\n*/\nlet a = 1;\n";
    let buf = LexedBuffer::new(&lexer, src);
    let folds = folding_ranges(&lexer, &buf);
    assert!(
        folds.contains(&FoldRange { start_line: 0, end_line: 3, kind: FoldKind::Block }),
        "block fold: {folds:?}"
    );
    assert!(
        folds.iter().any(|f| f.kind == FoldKind::Comment && f.start_line == 4 && f.end_line >= 6),
        "comment fold: {folds:?}"
    );
}

#[test]
fn outline_lists_let_names_with_spans() {
    let (lexer, sg, tables, _ids) = pipeline();
    let src = "let alpha = 1;\nlet beta = 2 + 3;\n{ let inner = 4; }\n";
    let s = IncSession::new(&lexer, &sg, &tables, src).unwrap();
    let cfg = demo_outline_config(&sg);
    let syms = outline(s.tree().unwrap(), &cfg);
    let names: Vec<&str> = syms.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["alpha", "beta", "inner"]);
    // Selection span of alpha is exactly the ident.
    let alpha = &syms[0];
    assert_eq!(&src[alpha.selection.0 as usize..alpha.selection.1 as usize], "alpha");
    assert!(alpha.span.0 <= alpha.selection.0 && alpha.selection.1 <= alpha.span.1);
}

#[test]
fn completion_reads_the_action_row() {
    let (lexer, sg, tables, _ids) = pipeline();
    let src = "let x = ";
    let buf = LexedBuffer::new(&lexer, src);
    let items = completion_at(&lexer, &buf, &sg, &tables, src.len() as u32);
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    for want in ["NUMBER", "STRING", "IDENT", "LPAREN", "LBRACKET"] {
        assert!(labels.contains(&want), "missing {want}: {labels:?}");
    }
    assert!(!items.iter().any(|i| i.is_keyword), "no keywords start an expression here");

    // Statement position: keywords ARE offered, lowercased for insertion.
    let src2 = "let a = 1;\n";
    let buf2 = LexedBuffer::new(&lexer, src2);
    let items2 = completion_at(&lexer, &buf2, &sg, &tables, src2.len() as u32);
    let kw: Vec<&str> =
        items2.iter().filter(|i| i.is_keyword).map(|i| i.label.as_str()).collect();
    assert!(kw.contains(&"let") && kw.contains(&"if"), "keywords: {kw:?}");
}

#[test]
fn diagnostics_map_repairs_to_spans() {
    let (lexer, sg, tables, _ids) = pipeline();
    let src = "let a = ;\nlet b = ) 2;\n";
    let s = IncSession::new(&lexer, &sg, &tables, src).unwrap();
    let diags = diagnostics(&lexer, &s.buf, &sg, &s.last_repairs);
    assert!(!diags.is_empty());
    // The deleted `)` diagnostic covers exactly that character.
    let del = diags.iter().find(|d| d.message.contains("unexpected `)`")).expect("skip diag");
    assert_eq!(&src[del.span.0 as usize..del.span.1 as usize], ")");
    // The missing-expr diagnostic points at the `;`.
    let ins = diags.iter().find(|d| d.message.starts_with("missing")).expect("insert diag");
    assert_eq!(&src[ins.span.0 as usize..ins.span.1 as usize], ";");
}

#[test]
fn selection_ranges_nest_from_the_session() {
    let (lexer, sg, tables, _ids) = pipeline();
    let src = "let a = 1 + 2 * 3;\n";
    let s = IncSession::new(&lexer, &sg, &tables, src).unwrap();
    let off = src.find('2').unwrap() as u32;
    let spans = selection_ranges(&s, off);
    assert!(spans.len() >= 4, "root, stmt, exprs, token: {spans:?}");
    for w in spans.windows(2) {
        assert!(w[0].0 <= w[1].0 && w[1].1 <= w[0].1);
    }
}
