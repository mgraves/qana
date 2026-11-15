//! The identity-cached overlay against its oracle: after EVERY edit in
//! a hostile script, the incremental painter's frame must equal a
//! from-scratch painter's frame on the same document — run for run,
//! bit for bit. The C grammar is typeless, so `LiveDoc` takes the
//! identity-cached path; the oracle is a fresh open of the post-edit
//! text, whose overlay comes from the composed-view path.
//!
//! The script covers what the identity cache must survive: body edits
//! (one item's marks change), edits that ADD and REMOVE top-level
//! definitions (resolution cascades to other items), item insertion
//! and deletion (positional shifts), an edit inside a block comment
//! (mode state), a preprocessor line, and EOF edits.

use linework::{LineEdit, Limner, PaintDelta, Run};
use qana_lang::live::LiveDoc;
use qana_lang::EmbeddedLang;
use std::sync::Arc;

/// Consume a delta exactly the way an editor does: splice the damaged
/// window, then apply repaints at their (post-splice) line indices.
fn apply_delta(frame: &mut Vec<Vec<Run>>, delta: &PaintDelta) {
    if let Some(((lo, hi), repl)) = &delta.splice {
        frame.splice(*lo as usize..*hi as usize, repl.iter().cloned());
    }
    for (li, runs) in &delta.repaints {
        frame[*li as usize] = runs.clone();
    }
}

const C_QANA: &str = include_str!("../../../examples/c/c.qana");
const C_DEMO: &str = include_str!("../../../examples/c/demo.c");

fn apply(text: &str, edit: &LineEdit) -> String {
    let mut lines: Vec<&str> = text.lines().collect();
    let repl: Vec<&str> = edit.lines.iter().map(|s| s.as_str()).collect();
    let end = (edit.end as usize).min(lines.len());
    lines.splice(edit.start as usize..end, repl);
    let mut out = lines.join("\n");
    if text.ends_with('\n') {
        out.push('\n');
    }
    out
}

#[test]
fn incremental_overlay_equals_fresh_open_after_every_edit() {
    let lang = Arc::new(EmbeddedLang::from_qana_source(C_QANA).expect("c.qana certifies"));

    // Two copies of the demo so cross-item resolution has real targets.
    let mut text = format!("{C_DEMO}\n{C_DEMO}");
    let mut live: Box<dyn Limner> = Box::new(LiveDoc::open(lang.clone(), "diff.c", &text));
    let mut frame = live.open(&text).lines;

    let n = text.lines().count() as u32;
    let script: Vec<LineEdit> = vec![
        // Body edit inside a function.
        LineEdit { start: n / 3, end: n / 3 + 1, lines: vec!["  int zz = 1 + 2;".into()] },
        // Add a top-level definition (cascades env fingerprints).
        LineEdit { start: 0, end: 0, lines: vec!["int added_global;".into()] },
        // Reference it far away (marks in a different item).
        LineEdit {
            start: (n * 2) / 3,
            end: (n * 2) / 3 + 1,
            lines: vec!["  added_global = 7;".into()],
        },
        // Remove the definition again (unresolves the reference).
        LineEdit { start: 0, end: 1, lines: vec![] },
        // Insert a whole new item (positional shift for the suffix).
        LineEdit {
            start: 2,
            end: 2,
            lines: vec!["static int inserted(void) { return 3; }".into()],
        },
        // Preprocessor line (mode PP).
        LineEdit { start: 1, end: 1, lines: vec!["#define DIFF_GATE 1".into()] },
        // Open a block comment mid-file, then close it (mode BLK).
        LineEdit { start: 8, end: 8, lines: vec!["/* opened".into()] },
        LineEdit { start: 9, end: 9, lines: vec!["closed */".into()] },
        // Edit the final line.
        LineEdit {
            start: n.saturating_sub(1) + 4,
            end: n.saturating_sub(1) + 5,
            lines: vec!["/* eof tweak */".into()],
        },
        // Delete the inserted item again.
        LineEdit { start: 2, end: 3, lines: vec![] },
    ];

    for (step, edit) in script.iter().enumerate() {
        let delta = live.edit(edit);
        apply_delta(&mut frame, &delta);
        text = apply(&text, edit);
        assert_eq!(live.text(), text, "step {step}: mirror lost the document");

        let mut oracle: Box<dyn Limner> =
            Box::new(LiveDoc::open(lang.clone(), "oracle.c", &text));
        let fresh = oracle.open(&text);
        assert_eq!(frame.len(), fresh.lines.len(), "step {step}: line counts diverge");
        for (li, (a, b)) in frame.iter().zip(fresh.lines.iter()).enumerate() {
            assert_eq!(a, b, "step {step}: line {li} runs diverge");
        }
    }
}

/// Lines that make and use top-level names in every way the grammar
/// binds them: a function used before and after it is defined, one that
/// calls itself, a struct tag (a hoisted namespace) used before its
/// definition, an enum (one item, several names), two globals in one
/// declaration, a macro, a name nothing defines, and duplicates of all of
/// them once the script has inserted a line twice.
const POOL: &[&str] = &[
    "static int helper_a(void) { return 1; }",
    "static int helper_b(void) { return helper_a() + later_fn(); }",
    "static int later_fn(void) { return helper_b() + later_fn(); }",
    "struct box { int w; };",
    "struct box make_box(void);",
    "enum color { RED, GREEN };",
    "int g1, g2;",
    "int use_g(void) { return g1 + g2 + RED + undefined_name; }",
    "#define TWICE(x) ((x) * 2)",
    "int use_macro(void) { return TWICE(g1) + GREEN; }",
    "int shadow(int helper_a) { return helper_a + g2; }",
    "",
    "/* a comment */",
];

/// The same oracle, over a script nobody chose: lines from [`POOL`]
/// inserted, replaced and deleted at random places. What an item can see
/// depends on what is defined before it, so where a line lands decides
/// what resolves; a fresh open resolves the whole file from the set of
/// its names, and the incremental document resolves the one changed item
/// by asking the file's signatures. They must agree after every step.
///
/// Every line of the document is a whole top-level item, so every step
/// leaves a file that parses clean: this test is about names. (A step
/// that breaks the syntax is another matter. Replacing a line inside a
/// function body with a function definition can leave the incremental
/// parse and a fresh parse with different recoveries, and so different
/// paint; that divergence is older than this test and is not what it
/// holds.)
#[test]
fn a_random_script_of_definitions_and_uses_matches_a_fresh_open() {
    let lang = Arc::new(EmbeddedLang::from_qana_source(C_QANA).expect("c.qana certifies"));

    // Three rounds of the pool: enough items that a fresh open resolves
    // from the set, and every name defined more than once.
    let mut text = String::new();
    for _ in 0..3 {
        for line in POOL {
            text.push_str(line);
            text.push('\n');
        }
    }
    let mut live: Box<dyn Limner> = Box::new(LiveDoc::open(lang.clone(), "random.c", &text));
    let mut frame = live.open(&text).lines;

    let mut seed = 0x51CA_D0C5u64;
    let mut rand = move |n: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % n.max(1) as u64) as usize
    };

    for step in 0..120 {
        let n = text.lines().count();
        // Stay off the last line: the document keeps its final newline.
        let at = rand(n.saturating_sub(1)) as u32;
        let line = POOL[rand(POOL.len())].to_string();
        let edit = match rand(4) {
            0 => LineEdit { start: at, end: at, lines: vec![line] },
            1 | 2 => LineEdit { start: at, end: at + 1, lines: vec![line] },
            _ if n > 8 => LineEdit { start: at, end: at + 1, lines: vec![] },
            _ => LineEdit { start: at, end: at, lines: vec![line] },
        };
        let delta = live.edit(&edit);
        apply_delta(&mut frame, &delta);
        text = apply(&text, &edit);
        assert_eq!(live.text(), text, "step {step}: mirror lost the document");

        let mut oracle: Box<dyn Limner> = Box::new(LiveDoc::open(lang.clone(), "oracle.c", &text));
        let fresh = oracle.open(&text);
        assert_eq!(frame.len(), fresh.lines.len(), "step {step}: line counts diverge");
        for (li, (a, b)) in frame.iter().zip(fresh.lines.iter()).enumerate() {
            assert_eq!(
                a,
                b,
                "step {step} ({edit:?}): line {li} `{}` runs diverge",
                text.lines().nth(li).unwrap_or("")
            );
        }
    }
}
