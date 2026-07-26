//! The two gates the envelope promises, in executable form:
//!
//! 1. LOSSLESS: `reproduce(lex(text)) == text` for arbitrary bytes-as-UTF-8,
//!    including hostile line endings, no trailing newline, BOM, unicode.
//! 2. DIFFERENTIAL: incremental relexing after any edit batch produces a
//!    state byte-identical to lexing the edited text from scratch. The
//!    batch lexer is the oracle; the incremental engine must never diverge.

use qana_lex::*;

// -- tiny deterministic RNG (xorshift64*), no dependencies --
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

fn roundtrip(src: &str) {
    let buf = LexedBuffer::new(src);
    assert_eq!(buf.reproduce(), src, "lossless roundtrip failed");
    assert!(buf.verify_coverage(), "token coverage failed for {src:?}");
}

#[test]
fn roundtrip_edge_cases() {
    roundtrip("");
    roundtrip("no newline at end");
    roundtrip("\n");
    roundtrip("\r\n");
    roundtrip("\r");
    roundtrip("a\nb\r\nc\rd");
    roundtrip("mixed \t tabs\tand   spaces\r\n\r\n");
    roundtrip("\u{feff}fn main() {} // BOM survives as Unknown trivia");
    roundtrip("héllo \"wörld\" 🚀 émoji");
    roundtrip("let s = \"unterminated");
    roundtrip("/* open comment\nstill open\nnever closed");
    roundtrip("/* nested /* twice */ once */ done");
    roundtrip("x = 1.5 + 2. + .5 + 1..2");
    roundtrip("\"esc \\\" quote\" and \"esc \\\\ back\"");
    roundtrip("weird bytes: \u{0}\u{1}\u{7f} ok");
}

#[test]
fn line_locality_is_structural() {
    // L1: no token can span lines, because lexing is per-line by
    // construction. Verify token coverage equals line length everywhere,
    // even with multi-line constructs.
    let src = "a /* spans\nseveral\nlines */ b\n\"str\nnot-a-str\n";
    let buf = LexedBuffer::new(src);
    assert!(buf.verify_coverage());
    // The multi-line comment is represented as per-line BlockComment
    // trivia stitched by state, not one giant token.
    assert_eq!(buf.entry_state(1), LineState::BlockComment(1));
    assert_eq!(buf.entry_state(2), LineState::BlockComment(1));
    assert_eq!(buf.entry_state(3), LineState::Normal);
}

#[test]
fn unterminated_string_is_contained() {
    // Single-line strings: an unterminated string is an error token on its
    // own line; the NEXT line lexes as if nothing happened. No cascade.
    let clean = LexedBuffer::new("let a = 1;\nlet b = 2;\n");
    let broken = LexedBuffer::new("let a = \"oops;\nlet b = 2;\n");
    assert_eq!(clean.lexed[1], broken.lexed[1], "line 1 must be unaffected");
    let has_error_str = broken.lexed[0]
        .tokens
        .iter()
        .any(|t| t.kind == TokenKind::Str { terminated: false });
    assert!(has_error_str);
}

#[test]
fn keywords_specialize() {
    let buf = LexedBuffer::new("fn foo(let_, letx) { return true; }");
    let kinds: Vec<TokenKind> = buf.lexed[0]
        .tokens
        .iter()
        .filter(|t| !t.kind.is_trivia())
        .map(|t| t.kind)
        .collect();
    assert_eq!(kinds[0], TokenKind::Keyword); // fn
    assert_eq!(kinds[1], TokenKind::Ident); // foo
    // let_ and letx are idents, not keywords
    assert!(kinds.contains(&TokenKind::Ident));
    assert!(kinds.iter().filter(|k| **k == TokenKind::Keyword).count() >= 3); // fn, return, true
}

#[test]
fn skeleton_matches_and_contains_errors() {
    let src = "fn f() {\n  if x { y(); }\n}\n\nfn g() {\n  ) stray\n  [1, 2]\n}\n";
    let buf = LexedBuffer::new(src);
    let sk = build_skeleton(&buf);
    // Folding candidates: the two fn bodies (multi-line braces).
    let folds: Vec<(usize, usize)> = sk.folding_ranges().collect();
    assert!(folds.contains(&(0, 2)), "fn f body should fold: {folds:?}");
    assert!(folds.contains(&(4, 7)), "fn g body should fold: {folds:?}");
    // The stray `)` is recorded locally and does not unwind `fn g`'s brace.
    assert_eq!(sk.unmatched_closes, 1);
}

// ---------------------------------------------------------------------------
// Differential property: incremental == batch, always.
// ---------------------------------------------------------------------------

fn random_line(rng: &mut Rng) -> String {
    const PIECES: &[&str] = &[
        "fn f", "let x = 1", "if x { y() }", "\"str\"", "\"unterminated",
        "/* open", "*/", "/* whole */", "// line comment", "  ", "}",
        "{", "]", "[", "(a, b)", "1.25", "émoji 🚀", "return x",
        "/* nest /* deep */", "word", "",
    ];
    let n = 1 + rng.below(4);
    let mut s = String::new();
    for k in 0..n {
        if k > 0 {
            s.push(' ');
        }
        s.push_str(PIECES[rng.below(PIECES.len())]);
    }
    s
}

fn random_term(rng: &mut Rng, last: bool) -> LineTerm {
    if last {
        return if rng.below(2) == 0 { LineTerm::None } else { LineTerm::Lf };
    }
    // No lone Cr here: mixing Cr terminators with line-granular random
    // edits can create Cr+"\n" seams, which are ill-formed at this layer
    // (see the canonical-form invariant in `apply_edits`). Lone-CR
    // round-tripping itself is covered by `roundtrip_edge_cases`.
    match rng.below(8) {
        0 => LineTerm::CrLf,
        _ => LineTerm::Lf,
    }
}

fn random_doc(rng: &mut Rng, lines: usize) -> String {
    let mut ls = Vec::new();
    for i in 0..lines {
        ls.push(Line::new(random_line(rng), random_term(rng, i + 1 == lines)));
    }
    join_lines(&ls)
}

fn random_edit_batch(rng: &mut Rng, doc_lines: usize) -> Vec<LineEdit> {
    // Canonical-form rule: the final line is the only one allowed to lack a
    // terminator, so edits never touch or insert past it (char-range edits
    // at the LSP layer preserve this by construction; the line-edit model
    // must respect it explicitly).
    let sites = 1 + rng.below(8);
    let mut cuts: Vec<usize> = (0..sites * 2).map(|_| rng.below(doc_lines)).collect();
    cuts.sort_unstable();
    let mut edits = Vec::new();
    for pair in cuts.chunks(2) {
        let (start, end) = (pair[0], pair[1]);
        if let Some(prev) = edits.last() {
            let prev: &LineEdit = prev;
            if start < prev.end {
                continue; // keep non-overlapping & ascending
            }
        }
        let rep_n = rng.below(4); // 0..=3 replacement lines (0 = deletion)
        let mut replacement = Vec::new();
        for _ in 0..rep_n {
            replacement.push(Line::new(random_line(rng), LineTerm::Lf));
        }
        edits.push(LineEdit { start, end, replacement });
    }
    edits
}

#[test]
fn differential_incremental_equals_batch() {
    let mut rng = Rng::new(0xDEC0DE);
    for iter in 0..400 {
        let doc_len = 30 + rng.below(120);
        let mut doc = random_doc(&mut rng, doc_len);
        let mut buf = LexedBuffer::new(&doc);
        // Several successive edit batches against the same buffer.
        for round in 0..4 {
            let edits = random_edit_batch(&mut rng, buf.lines.len());
            if edits.is_empty() {
                continue;
            }
            let report = buf.apply_edits(&edits);
            doc = buf.reproduce();

            // Oracle: cold lex of the current text.
            let oracle = LexedBuffer::new(&doc);
            assert_eq!(buf.lines.len(), oracle.lines.len(), "iter {iter} round {round}");
            for li in 0..buf.lines.len() {
                assert_eq!(
                    buf.lexed[li], oracle.lexed[li],
                    "iter {iter} round {round}: divergence at line {li} \
                     (report: {report:?})\nline text: {:?}",
                    buf.lines[li].text
                );
            }
            assert!(buf.verify_coverage());
            // Damage sanity: we never relex fewer lines than were replaced.
            assert!(report.relexed_lines >= report.replaced_lines);
        }
    }
}

#[test]
fn reconvergence_is_tight_for_local_edits() {
    // Editing a line that doesn't change the exit state must relex exactly
    // the replaced lines: zero reconvergence overhead.
    let src: String = (0..1000).map(|i| format!("let x{i} = {i};\n")).collect();
    let mut buf = LexedBuffer::new(&src);
    let report = buf.apply_edits(&[LineEdit {
        start: 500,
        end: 501,
        replacement: vec![Line::new("let y = \"edited\"; // touch", LineTerm::Lf)],
    }]);
    assert_eq!(report.replaced_lines, 1);
    assert_eq!(report.reconverged_extra, 0, "no state change ⇒ no extra relex");

    // Opening a block comment mid-file must relex forward exactly until the
    // first pre-existing state agreement (here: EOF, since nothing closes
    // it) — the inherent, VS-Code-identical worst case, measured precisely.
    let report = buf.apply_edits(&[LineEdit {
        start: 900,
        end: 900,
        replacement: vec![Line::new("/* now everything below is comment", LineTerm::Lf)],
    }]);
    assert_eq!(report.replaced_lines, 1);
    assert_eq!(report.reconverged_extra, buf.lines.len() - 901);
}

#[test]
fn multi_site_batch_contains_state_wave() {
    // Two sites in ONE batch: open a block comment at line 100 and close it
    // at line 125. The state wave from site 1 must stop exactly at site 2's
    // closer — damage is the 24 lines in between, not the rest of the file.
    let src: String = (0..1000).map(|i| format!("let x{i} = {i};\n")).collect();
    let mut buf = LexedBuffer::new(&src);
    let report = buf.apply_edits(&[
        LineEdit {
            start: 100,
            end: 100,
            replacement: vec![Line::new("/* begin note", LineTerm::Lf)],
        },
        LineEdit {
            start: 125,
            end: 125,
            replacement: vec![Line::new("end note */", LineTerm::Lf)],
        },
    ]);
    assert_eq!(report.sites, 2);
    assert_eq!(report.replaced_lines, 2);
    // Reconvergence: the 25 carried lines between the two insertion points
    // (old lines 100..=124) — and nothing after the closer.
    assert_eq!(report.reconverged_extra, 25, "wave must stop at the in-batch closer");

    // And of course: identical to batch relex.
    let oracle = LexedBuffer::new(&buf.reproduce());
    assert!(buf.lexed == oracle.lexed);
}
