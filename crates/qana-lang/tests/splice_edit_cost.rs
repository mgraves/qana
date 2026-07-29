//! Mike's beach-ball repro: deleting the backslash of a multi-line
//! #define near the top of a 250k-line C file froze qana_edit for
//! seconds, emptied the structure view of everything but `LIMIT`,
//! and left stale colors on the macro body lines. This file asks the
//! ENGINE which of those it owns: is the damage contained, do the
//! deltas carry the new truth, and what does the edit cost at scale?

use linework::{LineEdit, Limner};
use qana_lang::live::LiveDoc;
use qana_lang::EmbeddedLang;
use std::sync::Arc;
use std::time::Instant;

const C_QANA: &str = include_str!("../../../examples/c/c.qana");

fn c_lang() -> Arc<EmbeddedLang> {
    Arc::new(EmbeddedLang::from_qana_source(C_QANA).expect("c.qana certifies"))
}

fn clamp_doc(functions: usize) -> String {
    let mut s = String::new();
    s.push_str("#define LIMIT 100\n\n");
    s.push_str("#define CLAMP(x) \\\n");
    s.push_str("    ((x) > LIMIT ? LIMIT : \\\n");
    s.push_str("     (x) < 0 ? 0 : (x))\n\n");
    for i in 0..functions {
        s.push_str(&format!(
            "static int scale_{i}(int v) {{\n    return (v + {i}) % LIMIT;\n}}\n\n"
        ));
    }
    s
}

/// Small doc: the layers' TRUTH after the splice is deleted.
#[test]
fn deleting_the_splice_is_contained_and_the_deltas_tell_the_truth() {
    let lang = c_lang();
    let text = clamp_doc(20);
    let mut l: Box<dyn Limner> = Box::new(LiveDoc::open(lang.clone(), "clamp.c", &text));
    let _ = l.open(&text);

    let marks_before = l.marks().len();
    assert!(marks_before >= 21, "functions + defines outline before the edit");

    // Delete the backslash at the end of the #define line (line 2).
    let delta = l.edit(&LineEdit {
        start: 2,
        end: 3,
        lines: vec!["#define CLAMP(x)".to_string()],
    });

    // (a) The relex wave must cover the macro body lines — their mode
    // flipped from PP to BASE, and the delta must SAY so.
    let (lo, hi) = delta.splice.as_ref().map(|((lo, hi), _)| (*lo, *hi)).expect("a splice");
    assert!(lo <= 2 && hi >= 5, "damage covers the body lines: {lo}..{hi}");

    // (b) CONTAINMENT: the document survives the broken macro. The
    // construct IMMEDIATELY adjacent to the damage may be absorbed
    // into the recovery (a garbage token can legitimately start a
    // typedef-style declaration — recovery cannot know it will not
    // become one), but everything beyond the neighbor must outline.
    // Before the list-boundary + default-reduction-closure repair
    // discipline, ZERO of these survived — the whole file was one
    // error region.
    let marks_after = l.marks();
    let fn_marks = marks_after.iter().filter(|m| m.name.contains("scale_")).count();
    assert!(fn_marks >= 19, "containment: at most the adjacent neighbor lost (got {fn_marks}/20)");
    for far in ["scale_5", "scale_12", "scale_19"] {
        assert!(
            marks_after.iter().any(|m| m.name == far),
            "{far} must survive a broken macro at the top of the file"
        );
    }
}

/// Debug scope: what does the repaired tree DO with the unclosed
/// paren? Run with --ignored --nocapture.
#[test]
#[ignore]
fn debug_broken_tree_shape() {
    use qana_engine::IncSession;
    use qana_lang::compile::certify;
    use qana_lang::{compile_source, QanaToolchain};

    let tc = QanaToolchain::new();
    let out = compile_source(&tc, C_QANA);
    let (lexer, tables) = certify(&out.def).expect("certifies");

    let mut text = String::new();
    text.push_str("#define LIMIT 100\n\n");
    text.push_str("#define CLAMP(x)\n"); // splice already deleted
    text.push_str("    ((x) > LIMIT ? LIMIT : \\\n");
    text.push_str("     (x) < 0 ? 0 : (x))\n\n");
    for i in 0..3 {
        text.push_str(&format!(
            "static int scale_{i}(int v) {{\n    return (v + {i}) % LIMIT;\n}}\n\n"
        ));
    }

    let s = IncSession::new(&lexer, &out.def.sg, &tables, &text).unwrap();
    println!("FILE-SCOPE repairs: {}", s.last_repairs.len());
    let symbols = qana_services::outline(s.tree().expect("total"), &out.def.outline);
    println!(
        "FILE-SCOPE outline: {:?}",
        symbols.iter().map(|m| m.name.clone()).collect::<Vec<_>>()
    );

    // CONTRAST: the same naked expression INSIDE a function body,
    // where `;` and `}` give the repair natural sync points.
    let mut inside = String::new();
    inside.push_str("#define LIMIT 100\n\n");
    inside.push_str("static int probe(int x) {\n");
    inside.push_str("    ((x) > LIMIT ? LIMIT : \\\n");
    inside.push_str("     (x) < 0 ? 0 : (x))\n");
    inside.push_str("    return x;\n}\n\n");
    for i in 0..3 {
        inside.push_str(&format!(
            "static int scale_{i}(int v) {{\n    return (v + {i}) % LIMIT;\n}}\n\n"
        ));
    }
    let s2 = IncSession::new(&lexer, &out.def.sg, &tables, &inside).unwrap();
    println!("IN-BODY repairs: {}", s2.last_repairs.len());
    let symbols2 = qana_services::outline(s2.tree().expect("total"), &out.def.outline);
    println!(
        "IN-BODY outline: {:?}",
        symbols2.iter().map(|m| m.name.clone()).collect::<Vec<_>>()
    );
}

/// Scale: the same edit at 250k lines, timed per layer. Generous
/// ceiling so slow machines stay green; run --nocapture --release
/// for honest numbers.
#[test]
fn deleting_the_splice_at_scale_is_not_a_beach_ball() {
    let lang = c_lang();
    let text = clamp_doc(27_000); // ~110k lines
    let lines = text.lines().count();
    let mut l: Box<dyn Limner> = Box::new(LiveDoc::open(lang.clone(), "big.c", &text));
    let t = Instant::now();
    let _ = l.open(&text);
    let open = t.elapsed();

    let t = Instant::now();
    let _ = l.edit(&LineEdit {
        start: 2,
        end: 3,
        lines: vec!["#define CLAMP(x)".to_string()],
    });
    let break_edit = t.elapsed();

    let t = Instant::now();
    let _ = l.edit(&LineEdit {
        start: 2,
        end: 3,
        lines: vec!["#define CLAMP(x) \\".to_string()],
    });
    let heal_edit = t.elapsed();

    let t = Instant::now();
    let marks = l.marks();
    let marks_time = t.elapsed();

    println!(
        "lines={lines} open={open:?} break={break_edit:?} heal={heal_edit:?} marks={marks_time:?} ({} marks)",
        marks.len()
    );
    assert!(
        break_edit.as_secs() < 10 && heal_edit.as_secs() < 10,
        "splice-flip edits must not be beach balls: break={break_edit:?} heal={heal_edit:?}"
    );
}
