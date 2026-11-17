//! Random edit scripts against the incremental parser's contract: after
//! every edit, the tree reproduces the text, keeps its balance invariants
//! (every stored count is the sum of the children's), and is the tree a
//! batch parse of the text builds (`semantic_eq`: trivia placement, the
//! state each node began in, and how recovery built it, included), with
//! the same repairs. The edits are the ones typing makes and breaks code
//! with: brackets, quotes, comment openers and closers, line breaks,
//! joins and cuts. In the C scripts most steps leave the code broken, so
//! recovery runs nearly everywhere (`docs/incremental_recovery.md`).

use qana_engine::{IncSession, Line, LineEdit, LineTerm};
use qana_grammar::green::{check_balance, semantic_eq};
use qana_lang::EmbeddedLang;

const C: &str = include_str!("../../../examples/c/c.qana");
const C_DEMO: &str = include_str!("../../../examples/c/demo.c");
const RUST: &str = include_str!("../../../examples/rust/rust.qana");
const RUST_SAMPLE: &str = include_str!("../../../examples/rust-lex/sample.rs");

const PIECES: &[&str] = &[
    "{",
    "}",
    "(",
    ")",
    "[",
    "]",
    "\"",
    "/*",
    "*/",
    "//",
    "\n",
    "\n\n",
    "x",
    ";",
    "int y;\n",
    "f(1,\n",
    "\n}",
    "// c\n",
    "'a'",
    "\\",
    " ",
    "#define Z 1\n",
];

fn rng(mut seed: u64) -> impl FnMut(usize) -> usize {
    move |n: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % n.max(1) as u64) as usize
    }
}

/// Run `steps` random edits of `text` under `grammar`, holding the
/// session to the contract after each one.
fn script(grammar: &str, text: &str, seed: u64, steps: usize) {
    let lang = EmbeddedLang::from_qana_source(grammar).unwrap_or_else(|d| panic!("{d:?}"));
    let mut session: IncSession<'static> = lang.session(text);
    let mut pick = rng(seed);
    for step in 0..steps {
        let now = session.buf.reproduce();
        let lines: Vec<&str> = now.split('\n').collect();
        let line = pick(lines.len());
        let old = lines[line];
        let bounds: Vec<usize> =
            old.char_indices().map(|(i, _)| i).chain(std::iter::once(old.len())).collect();
        let at = bounds[pick(bounds.len())];
        let cut_to = bounds.iter().copied().filter(|&b| b >= at).nth(pick(3)).unwrap_or(at);
        let edited = if pick(4) == 0 {
            format!("{}{}", &old[..at], &old[cut_to..])
        } else {
            format!("{}{}{}", &old[..at], PIECES[pick(PIECES.len())], &old[at..])
        };
        let end = if pick(6) == 0 && line + 1 < lines.len() { line + 2 } else { line + 1 };
        let joined = if end == line + 2 { format!("{edited}{}", lines[line + 1]) } else { edited };
        let what = format!("step {step} (seed {seed}), line {line}");
        holds_contract(&lang, &mut session, line, end, &joined, &what);
    }
}

/// Replace lines `start..end` of the session's document by `joined`
/// (lines split at line feeds) and hold the session to the contract.
fn holds_contract(
    lang: &EmbeddedLang,
    session: &mut IncSession<'static>,
    start: usize,
    end: usize,
    joined: &str,
    what: &str,
) {
    // Interior lines end in a line feed; the document's last line ends in
    // nothing, so a replacement that reaches it does too.
    let reaches_end = end == session.buf.lines.len();
    let new_lines: Vec<&str> = joined.split('\n').collect();
    let replacement: Vec<Line> = new_lines
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let last = i + 1 == new_lines.len();
            Line::new(*l, if reaches_end && last { LineTerm::None } else { LineTerm::Lf })
        })
        .collect();
    session
        .edit(lang.sg, lang.tables, &[LineEdit { start, end, replacement }])
        .unwrap_or_else(|e| panic!("{what}: the edit parses: {e:?}"));
    let text = session.buf.reproduce();
    let tree = session.tree().expect("total");
    assert_eq!(tree.text(), text, "{what}: the tree reproduces the text");
    if let Err(e) = check_balance(tree) {
        panic!("{what}: balanced: {e}");
    }
    let batch = lang.session(&text);
    assert!(
        semantic_eq(tree, batch.tree().expect("total")),
        "{what}: the incremental tree is the batch tree of\n{text}"
    );
    assert_eq!(session.last_repairs, batch.last_repairs, "{what}: the repairs are batch's");
}

/// The C demo, broken and mended by typing.
#[test]
fn c_edit_scripts_keep_the_contract() {
    for seed in [7, 11, 23, 0xC0DE, 5, 13, 101, 977] {
        script(C, C_DEMO, seed, 160);
    }
}

/// The same over 512 scripts (81,920 edits): the sweep that found the
/// cases below. Release build: `cargo test --release -p qana-lang --test
/// edit_scripts -- --ignored`.
#[test]
#[ignore = "wide sweep: release build, a few minutes"]
fn c_edit_scripts_keep_the_contract_wide() {
    for k in 1..=512u64 {
        script(C, C_DEMO, k * 0x9E37_79B9 + 7, 160);
    }
}

#[test]
fn rust_edit_scripts_keep_the_contract() {
    for seed in [5, 13] {
        script(RUST, RUST_SAMPLE, seed, 200);
    }
}

/// One edit to `doc`, replacing lines `start..end` by `joined`, after
/// which the session holds the contract.
fn equals_batch_after_edit(doc: &str, start: usize, end: usize, joined: &str) {
    let lang = EmbeddedLang::from_qana_source(C).unwrap();
    let mut session = lang.session(doc);
    holds_contract(&lang, &mut session, start, end, joined, "the edit");
}

/// One edit to `doc`'s line `line`, after which the session holds the
/// contract.
fn equals_batch_after(doc: &str, line: usize, replacement: &str) {
    equals_batch_after_edit(doc, line, line + 1, replacement);
}

/// A reused node that holds a repair-inserted token, broken into its
/// children, used to hand the parser that token as input, and recovery
/// deleted it (`Deleted("")`, a repair batch never makes) and placed its
/// own repairs a terminal late. An inserted token is not input: the
/// parse re-derives it from the real tokens.
#[test]
fn a_repair_inserted_token_in_reused_structure_is_not_input() {
    equals_batch_after("n\ny\n{\n        n -= 2;\n\n\nl l <)", 3, "        n -= {2;");
}

/// The smallest document the C script's failure shrinks to. Before the
/// edit, `]` is skipped by recovery in front of `typedef`; after it, the
/// comment closes early and `r.*/` is new broken code. Reuse splices the
/// node holding `"\n" ERROR(]) "\n" typedef`, and the material recovery
/// drained in front of the new code goes before that whole run.
#[test]
fn an_error_node_in_front_of_a_reused_token_is_material_not_a_symbol() {
    equals_batch_after("/*\nr.*/\nr\nenum {D};\n]\ntypedef", 1, "*/r.*/");
}

/// An ERROR node, broken down, re-enters as its tokens. Recovery wraps
/// structure it built before giving up (here, pieces of `0?h:t * d`) in
/// an ERROR node; handed back as nodes, those pieces were spliced where a
/// batch parse, reading the tokens, recovers differently. Typing a line
/// comment after the broken declaration showed it.
#[test]
fn an_error_node_broken_down_re_enters_as_its_tokens() {
    let lang = EmbeddedLang::from_qana_source(C).unwrap();
    let mut session = lang.session("s 0?h:t * d;\n");
    session
        .edit(
            lang.sg,
            lang.tables,
            &[LineEdit {
                start: 1,
                end: 2,
                replacement: vec![Line::new("// c", LineTerm::Lf), Line::new("", LineTerm::None)],
            }],
        )
        .unwrap();
    let text = session.buf.reproduce();
    assert_eq!(text, "s 0?h:t * d;\n// c\n");
    let tree = session.tree().expect("total");
    assert_eq!(tree.text(), text);
    assert!(semantic_eq(tree, lang.session(&text).tree().expect("total")));
}

/// Recovery set aside a reused entry (popped it into an ERROR node) and
/// dropped the material that was to drain into it: the newline and indent
/// in front of `struct`. An entry set aside keeps everything it holds.
#[test]
fn an_entry_set_aside_keeps_the_material_drained_into_it() {
    equals_batch_after_edit(
        "    i*/nt total = 0;\n    unsigned long big =*/ 0xFFul;\n    int i;\n(\n    struct// point q = { . = 1, .y =// 2 }; [   p.x = 3;\n    p.y = ]4;",
        0,
        2,
        "    i*/nt tf(1,\notal = 0;    unsigned long big =*/ 0xFFul;",
    );
}

/// An empty `ptrs` list reduced on a token recovery inserted was reused
/// where a batch parse, at the same place, errors: a reduction recovery
/// makes is recovery's decision, not the input's, and the node it builds
/// is not reused on its own.
#[test]
fn a_reduction_on_an_inserted_token_is_recoverys() {
    equals_batch_after_edit(
        "static unsigned long checksum(const s{truct point *pt) {\n#define Z 1\n\"\ni//nt main(void) {\n    unsigned long big = 0xFFul;#define Z 1",
        2,
        3,
        "\"",
    );
}

/// The `ptrs` ε-node in front of `weight` was reduced on `:`, which
/// recovery then skipped; the tree cannot tell that from one reduced on
/// `weight` after the skip. Reused with the skipped material taken out,
/// it carried the old parse's account of itself where a batch parse,
/// mending `: 4;` differently, reduces it on `weight`.
#[test]
fn an_eps_node_in_front_of_skipped_material_is_not_reused() {
    equals_batch_after_edit(
        "struct point[ {\n    int x;\n    int y;\n    unsign[ed flags : 4;\n    word_t weight;\n}",
        2,
        3,
        "   */ int y;",
    );
}

/// The same for the ε seed of a list: `ptrs` opened on `word_t`, before
/// recovery set `word_t` aside in front of `*`. A whole list whose seed is
/// ε is not reused once the material in front of it is taken out.
#[test]
fn an_eps_seeded_list_in_front_of_skipped_material_is_not_reused() {
    equals_batch_after_edit(
        "point_thead_node;\nword_t twice(word_t w) {\n    word_t doubled = w + w;\n    return doubled;\n}\n    word_t *cursor = &global_words, lt = 3;",
        0,
        1,
        ";",
    );
}
