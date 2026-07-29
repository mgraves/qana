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

/// Mike's actual fixture shape: ~250k lines WITH multi-line macros
/// sprinkled through (552 of them). A plain body keystroke mid-file
/// must be editor-speed; the live app measured SECONDS.
#[test]
#[ignore = "timing evidence — release + --nocapture"]
fn big_demo_shaped_keystroke() {
    let lang = c_lang();
    let mut s = String::new();
    s.push_str("#include <stdio.h>\n#define LIMIT 100\n\n");
    s.push_str("#define CLAMP(x) \\\n    ((x) > LIMIT ? LIMIT : \\\n     (x) < 0 ? 0 : (x))\n\n");
    let mut i = 0usize;
    while s.lines().count() < 249_000 {
        if i % 50 == 0 {
            s.push_str(&format!(
                "#define SCALE_STEP_{i}(v) \\\n    ((v) * {i} + \\\n     (v) % LIMIT)\n\n"
            ));
        }
        s.push_str(&format!(
            "static int scale_{i}(int v, int factor) {{\n    int result = v * factor + {i};   /* block {i} */\n    if (result > LIMIT)\n        result = LIMIT;\n    else\n        result = result % LIMIT;\n    return result;\n}}\n\n"
        ));
        i += 1;
    }
    let lines = s.lines().count();
    let mut l: Box<dyn Limner> = Box::new(LiveDoc::open(lang.clone(), "big.c", &s));
    let t = Instant::now();
    let _ = l.open(&s);
    let open = t.elapsed();

    // A plain body keystroke mid-file, far from any macro.
    let mid = (lines / 2) as u32;
    let mid_text: String = s.lines().nth(mid as usize).unwrap_or("").to_string();
    let t = Instant::now();
    let _ = l.edit(&LineEdit { start: mid, end: mid + 1, lines: vec![format!("{mid_text} ")] });
    let ks1 = t.elapsed();
    let t = Instant::now();
    let _ = l.edit(&LineEdit { start: mid, end: mid + 1, lines: vec![mid_text.clone()] });
    let ks2 = t.elapsed();

    println!("lines={lines} open={open:?} keystroke1={ks1:?} keystroke2={ks2:?}");
    assert!(ks2.as_millis() < 5_000, "keystroke: {ks2:?}");
}

/// Mike's 12-second keystroke: typing `s` on a new line at the top of
/// a 249k-line document — a PARTIAL TOKEN at file scope. The clean
/// path costs 10ms; the transiently-invalid document hit the error
/// machinery, which the bridge trace timed at 11.96s in the app.
#[test]
#[ignore = "timing evidence — release + --nocapture"]
fn partial_token_keystroke_at_scale() {
    let lang = c_lang();
    let mut s = String::new();
    s.push_str("#include <stdio.h>\n#define LIMIT 100\n\n");
    let mut i = 0usize;
    while s.lines().count() < 249_000 {
        if i % 50 == 0 {
            s.push_str(&format!(
                "#define SCALE_STEP_{i}(v) \\\n    ((v) * {i} + \\\n     (v) % LIMIT)\n\n"
            ));
        }
        s.push_str(&format!(
            "static int scale_{i}(int v, int factor) {{\n    int result = v * factor + {i};\n    return result % LIMIT;\n}}\n\n"
        ));
        i += 1;
    }
    let mut l: Box<dyn Limner> = Box::new(LiveDoc::open(lang.clone(), "big.c", &s));
    let _ = l.open(&s);

    // Insert a blank line at line 8, then type `s` into it — the
    // exact transiently-invalid state a human typist creates.
    let line8: String = s.lines().nth(8).unwrap_or("").to_string();
    let _ = l.edit(&LineEdit { start: 8, end: 9, lines: vec![String::new(), line8.clone()] });
    let t = Instant::now();
    let _ = l.edit(&LineEdit { start: 8, end: 9, lines: vec!["s".to_string()] });
    let partial = t.elapsed();
    let t = Instant::now();
    let _ = l.edit(&LineEdit { start: 8, end: 9, lines: vec!["struct".to_string()] });
    let keyword = t.elapsed();
    let t = Instant::now();
    let _ = l.edit(&LineEdit { start: 8, end: 9, lines: vec![String::new()] });
    let healed = t.elapsed();
    println!("partial-token 's'={partial:?} 'struct'={keyword:?} healed={healed:?}");
}

/// THE REAL FILE: the fixture generator had a formatting bug that
/// put `%%` (a syntax error) in every function — 27,455 error
/// regions. The engine's clean-document keystroke is 10ms; this
/// measures the same keystroke against the error-dense document,
/// which the live app timed at 11.96s. Transient errors are what
/// TYPING IS — error density must not destroy incrementality.
#[test]
#[ignore = "timing evidence — release + --nocapture"]
fn error_dense_document_keystroke() {
    let lang = c_lang();
    for &functions in &[1_000usize, 5_000, 27_000] {
        let mut s = String::new();
        s.push_str("#include <stdio.h>\n#define LIMIT 100\n\n");
        for i in 0..functions {
            s.push_str(&format!(
                "static int scale_{i}(int v, int factor) {{\n    int result = v * factor + {i};\n    result = result %% LIMIT;\n    return result;\n}}\n\n"
            ));
        }
        let lines = s.lines().count();
        let mut l: Box<dyn Limner> = Box::new(LiveDoc::open(lang.clone(), "err.c", &s));
        let t = Instant::now();
        let _ = l.open(&s);
        let open = t.elapsed();

        let line8: String = s.lines().nth(8).unwrap_or("").to_string();
        let _ = l.edit(&LineEdit { start: 8, end: 9, lines: vec![String::new(), line8] });
        let t = Instant::now();
        let _ = l.edit(&LineEdit { start: 8, end: 9, lines: vec!["s".to_string()] });
        let partial = t.elapsed();
        println!(
            "functions={functions} lines={lines} errors={functions} open={open:?} partial-token={partial:?}"
        );
    }
}
