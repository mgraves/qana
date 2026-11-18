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

/// The same oracle inside one large item. A function of a couple of
/// thousand lines is put together from parts (`qana_sem`'s), and a miss
/// on it repaints the lines its marks stand on, old and new, instead of
/// every line it spans: typing in it has to leave every line exactly as a
/// fresh open paints it. Statements are retyped, inserted and deleted,
/// and some are retyped with only a digit changed, which leaves the item
/// declaring what it declared: its fragment is kept, and nothing is
/// repainted outside the line typed on.
#[test]
fn typing_in_one_large_function_matches_a_fresh_open() {
    let lang = Arc::new(EmbeddedLang::from_qana_source(C_QANA).expect("c.qana certifies"));

    let mut seed = 0x0B16_F00Du64;
    let mut rand = move |n: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % n.max(1) as u64) as usize
    };
    let statement = |rand: &mut dyn FnMut(usize) -> usize| -> String {
        let (n, m) = (rand(30), rand(30));
        match rand(6) {
            0 => format!("    int v{n} = g1 + {m};"),
            1 => format!("    v{n} = helper_a() + v{m};"),
            2 => format!("    if (v{n}) {{ g2 = v{m}; }}"),
            3 => "    undefined_name = 3;".to_string(),
            4 => format!("    {{ int inner = g1; inner = inner + {m}; }}"),
            _ => "    // a note".to_string(),
        }
    };

    let mut text = String::from("int g1, g2;\nstatic int helper_a(void) { return g1; }\n\nint big(void) {\n");
    for i in 0..30 {
        text.push_str(&format!("    int v{i} = {i};\n"));
    }
    for _ in 0..2000 {
        text.push_str(&statement(&mut rand));
        text.push('\n');
    }
    text.push_str("    return 0;\n}\n\nint after(void) { return g2; }\n");
    let first_body = 4 + 30;
    let mut body_end = first_body + 2000;

    let mut live: Box<dyn Limner> = Box::new(LiveDoc::open(lang.clone(), "big.c", &text));
    let mut frame = live.open(&text).lines;
    let mut quiet_steps = 0usize;

    for step in 0..80 {
        let at = (first_body + rand(body_end - first_body)) as u32;
        let current = text.lines().nth(at as usize).unwrap().to_string();
        let mut digit_only = false;
        let edit = match rand(5) {
            0 => {
                body_end += 1;
                LineEdit { start: at, end: at, lines: vec![statement(&mut rand)] }
            }
            1 if body_end - first_body > 100 => {
                body_end -= 1;
                LineEdit { start: at, end: at + 1, lines: vec![] }
            }
            2 if other_numbers(&current) != current => {
                digit_only = true;
                LineEdit { start: at, end: at + 1, lines: vec![other_numbers(&current)] }
            }
            _ => LineEdit { start: at, end: at + 1, lines: vec![statement(&mut rand)] },
        };
        let delta = live.edit(&edit);
        if digit_only {
            assert!(delta.repaints.is_empty(), "step {step}: a digit changed repaints nothing but its line");
            quiet_steps += 1;
        }
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
    assert!(quiet_steps > 0, "the script changed a digit at least once");
}

/// `line` with every digit of every number literal changed for another
/// (names keep theirs: `v12` stays `v12`). The line declares and refers to
/// what it did, at the same places.
fn other_numbers(line: &str) -> String {
    let mut in_name = false;
    line.chars()
        .map(|c| {
            let out = if c.is_ascii_digit() && !in_name {
                char::from(b'0' + ((c as u8 - b'0' + 1) % 10))
            } else {
                c
            };
            in_name = c.is_ascii_alphanumeric() && (in_name || !c.is_ascii_digit()) || c == '_';
            out
        })
        .collect()
}

/// The painter against a painter that never had a last frame, on the
/// SAME tree: braces typed and deleted inside one large function, so that
/// the parse breaks, recovers, and the code after the edit belongs to
/// other constructs than it did, while its text and its lines stay as
/// they were. A mark can vanish from a line nowhere near the one typed on;
/// the item it belonged to is replaced, and the line has to be repainted
/// from what that item had there. (On the same tree, so that how the
/// incremental parse recovers is not in question: only the paint is.)
#[test]
fn braces_typed_in_one_large_function_paint_as_a_fresh_painter_on_the_same_tree() {
    use qana_engine::{Line, LineEdit as EngineEdit, LineTerm};
    use qana_sem::SemDb;
    use qana_services::paint::Painter;

    let lang = EmbeddedLang::from_qana_source(C_QANA).expect("c.qana certifies");
    let mut seed = 0x00B2_ACE5u64;
    let mut rand = move |n: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % n.max(1) as u64) as usize
    };
    let mut text = String::from("int g1, g2;\nint big(void) {\n");
    for i in 0..600 {
        text.push_str(&match i % 4 {
            0 => format!("    int v{i} = g1 + {i};"),
            1 => format!("    if (v{}) {{ g2 = v{}; }}", i - 1, i - 1),
            2 => "    undefined_name = 3;".to_string(),
            _ => format!("    v{} = v{} + g2;", i - 3, i - 3),
        });
        text.push('\n');
    }
    text.push_str("    return 0;\n}\nint after(void) { return g2; }\n");

    let uri = "same.c";
    let mut session = lang.session(&text);
    let mut db = SemDb::new(lang.binding.clone());
    db.set_types(lang.types.clone());
    db.set_macro_bodies(&lang.macros);
    db.set_tree(uri, session.tree().expect("total").clone());
    let (mut painter, opened) = Painter::new(&session, &lang.styles, Some((&mut db, uri)));
    let mut frame = opened.lines;

    for step in 0..60 {
        let n = session.buf.lines.len();
        let at = 2 + rand(n - 4);
        let line = match rand(5) {
            0 => "    {".to_string(),
            1 => "    }".to_string(),
            2 => "}".to_string(),
            3 => "int g3;".to_string(),
            _ => format!("    int w{} = g1;", rand(9)),
        };
        let (end, replacement) = match rand(3) {
            0 => (at, vec![Line::new(line, LineTerm::Lf)]),
            1 => (at + 1, vec![]),
            _ => (at + 1, vec![Line::new(line, LineTerm::Lf)]),
        };
        let outcome = session
            .edit(lang.sg, lang.tables, &[EngineEdit { start: at, end, replacement }])
            .expect("total under errors");
        db.set_tree(uri, session.tree().expect("total").clone());
        let delta = painter.update(&session, &outcome.damage, &lang.styles, Some((&mut db, uri)));
        apply_delta(&mut frame, &delta);
        assert_eq!(frame, painter.frame().lines, "step {step}: the deltas build the painter's frame");

        let mut fresh_db = SemDb::new(lang.binding.clone());
        fresh_db.set_types(lang.types.clone());
        fresh_db.set_macro_bodies(&lang.macros);
        fresh_db.set_tree(uri, session.tree().expect("total").clone());
        let (_, fresh) = Painter::new(&session, &lang.styles, Some((&mut fresh_db, uri)));
        assert_eq!(frame.len(), fresh.lines.len(), "step {step}: line counts diverge");
        for (li, (a, b)) in frame.iter().zip(fresh.lines.iter()).enumerate() {
            assert_eq!(a, b, "step {step}: line {li} `{}` runs diverge", session.buf.lines[li].text);
        }
    }
}
