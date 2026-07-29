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

/// The world-swallow gate: an INCOMPLETE struct member before the
/// closer (`struct P { i }` — the state a keystroke passes through on
/// the way to `int x;`) must stay a LOCAL error region. Before the
/// wanted-below repair rule, deleting that `}` scored perfectly (the
/// next function parses as a member declaration), the outline
/// collapsed to 1-2 entries, and every keystroke inside the state
/// re-earned a document-sized error region at 3-4 SECONDS each at
/// 249k lines. Now the half-born declarator is set aside (one
/// `Unwound` repair) and the struct closes at its own brace.
#[test]
fn incomplete_member_stays_a_local_error_region() {
    use qana_engine::IncSession;
    use qana_lang::compile::certify;
    use qana_lang::{compile_source, QanaToolchain};

    let tc = QanaToolchain::new();
    let out = compile_source(&tc, C_QANA);
    let (lexer, tables) = certify(&out.def).expect("certifies");

    let tail: String = (1..6)
        .map(|i| {
            format!(
                "static int scale_{i}(int v, int factor) {{\n    int result = v * factor + {i};\n    return result;\n}}\n\n"
            )
        })
        .collect();

    let case = |name: &str, body: &str| -> (usize, bool, usize) {
        let text = format!("#define LIMIT 100\n\n{body}\n{tail}");
        let s = IncSession::new(&lexer, &out.def.sg, &tables, &text).unwrap();
        let repairs: Vec<String> = s
            .last_repairs
            .iter()
            .take(10)
            .map(|r| format!("{:?}@{}", r.kind, r.at_terminal))
            .collect();
        let symbols = qana_services::outline(s.tree().expect("total"), &out.def.outline);
        let names: Vec<&str> = symbols.iter().map(|m| m.name.as_str()).take(10).collect();
        println!(
            "CASE {name}: outline={} {:?} repairs={} {:?}",
            symbols.len(),
            names,
            s.last_repairs.len(),
            repairs
        );
        (
            symbols.len(),
            symbols.iter().any(|m| m.name == "Point"),
            s.last_repairs.len(),
        )
    };

    // Baselines: the empty struct and the well-formed struct parse
    // REPAIR-FREE, or the grammar itself lacks the empty case.
    for (name, body) in [
        ("empty_struct", "struct Point {}\n"),
        ("empty_struct_spaced", "struct Point {\n}\n"),
        ("wellformed_struct", "struct Point {\n  int x;\n}\n"),
    ] {
        let (outline, has_point, repairs) = case(name, body);
        assert_eq!(repairs, 0, "{name}: baseline must be repair-free");
        assert!(has_point && outline >= 7, "{name}: full outline");
    }

    // The incomplete member, in-function and at file scope, plus the
    // complete-declarator contrast (missing only `;`). Every case:
    // the struct closes, the world survives, repairs stay minimal.
    for (name, body, floor) in [
        (
            "in_fn_incomplete",
            "static int scale_0(int v, int factor) {\n    int result = v * factor + 0;\nstruct Point {\n  i}\n    result = result % LIMIT;\n    return result;\n}\n",
            8,
        ),
        ("file_scope_incomplete", "struct Point {\n  i}\n", 7),
        (
            "in_fn_int_x",
            "static int scale_0(int v, int factor) {\n    int result = v * factor + 0;\nstruct Point {\n  int x}\n    result = result % LIMIT;\n    return result;\n}\n",
            8,
        ),
        ("file_scope_int_x", "struct Point {\n  int x}\n", 7),
    ] {
        let (outline, has_point, repairs) = case(name, body);
        assert!(
            has_point,
            "{name}: the struct itself must survive its broken member"
        );
        assert!(
            outline >= floor,
            "{name}: containment — the world must not be swallowed (outline {outline} < {floor})"
        );
        assert!(repairs <= 2, "{name}: minimal repairs, not a delete-train (got {repairs})");
    }
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

/// Mike's Return keystroke: inserting a LINE (not replacing one) near
/// the top of the 249k clean fixture beach-balled and left the
/// structure view showing LOCAL VARIABLES (`result`) as top-level
/// symbols — a wrong tree, not a confused view. Every prior bench
/// used 1:1 line replacements; this gates line INSERTION: timing,
/// incremental == batch tree equality, and outline sanity.
#[test]
#[ignore = "timing evidence + divergence gate — release + --nocapture"]
fn return_insert_at_scale() {
    use qana_engine::IncSession;
    use qana_engine::{Line as ELine, LineEdit as ELineEdit, LineTerm};
    use qana_grammar::green::semantic_eq;
    use qana_lang::compile::certify;
    use qana_lang::{compile_source, QanaToolchain};

    let tc = QanaToolchain::new();
    let out = compile_source(&tc, C_QANA);
    let (lexer, tables) = certify(&out.def).expect("certifies");

    let mut s = String::new();
    s.push_str("#include <stdio.h>\n#define LIMIT 100\n\ntypedef unsigned long word_t;\nstruct Point {\n}\n\n");
    let mut i = 0usize;
    while s.lines().count() < 249_000 {
        if i % 50 == 0 {
            s.push_str(&format!(
                "#define SCALE_STEP_{i}(v) \\\n    ((v) * {i} + \\\n     (v) % LIMIT)\n\n"
            ));
        }
        s.push_str(&format!(
            "static int scale_{i}(int v, int factor) {{\n    int result = v * factor + {i};\n    result = result % LIMIT;\n    return result;\n}}\n\n"
        ));
        i += 1;
    }

    let mut sess = IncSession::new(&lexer, &out.def.sg, &tables, &s).unwrap();
    // The in-progress `struct Point {`/`}` (no semicolon) is an error
    // region — faithfully Mike's live document at the moment he
    // pressed Return.
    println!("setup repairs: {}", sess.last_repairs.len());

    // The Return: split line 5 ("struct Point {" region) by inserting
    // a blank line — 1 removed, 2 inserted.
    let line5: String = s.lines().nth(5).unwrap_or("").to_string();
    let t = Instant::now();
    sess.edit(&out.def.sg, &tables, &[ELineEdit {
        start: 5,
        end: 6,
        replacement: vec![
            ELine::new("", LineTerm::Lf),
            ELine::new(&line5, LineTerm::Lf),
        ],
    }])
    .unwrap();
    let insert_time = t.elapsed();

    let now = sess.buf.reproduce();
    let batch = IncSession::new(&lexer, &out.def.sg, &tables, &now).unwrap();
    assert_eq!(sess.tree().unwrap().text(), now, "lossless after Return");
    let equal = semantic_eq(sess.tree().unwrap(), batch.tree().unwrap());

    // Outline sanity: local variables must never appear.
    let symbols = qana_services::outline(sess.tree().expect("total"), &out.def.outline);
    let locals: Vec<&str> = symbols
        .iter()
        .filter(|m| m.name == "result")
        .map(|m| m.name.as_str())
        .take(3)
        .collect();

    println!(
        "return-insert={insert_time:?} inc==batch: {equal} outline_size={} leaked_locals={:?}",
        symbols.len(),
        locals
    );
    assert!(equal, "INCREMENTAL DIVERGED FROM BATCH after a line insertion");
    assert!(locals.is_empty(), "locals leaked into the outline");
}

/// The keystroke lab's engine finding, replayed at the Limner level.
///
/// The synkro lab (keystroke_lab.rs, run 2) measured typing a struct
/// member — `struct Point {` + Return + `int x;` near the top of a
/// 249k-line file, auto-close pairs keeping the brace balanced — and
/// found every keystroke of the INCOMPLETE member (`i` … `int x`)
/// costing 363–709ms inside limner.edit, with the `i` and the `x`
/// each triggering a ~full-document repaint wave (249,302 lines).
/// The `;` that completes the member dropped the next keystroke to
/// 14ms. This gate replays that exact edit-log script and prints
/// per-step evidence: edit time, splice span, repaint count, and the
/// outline size (a collapse = the struct swallowed the world; a
/// steady count = the cost is parse-side, not tree-shape). Run with
/// QANA_TRACE_EDIT=1 for the parse | sem | paint split inside each
/// edit.
#[test]
#[ignore = "timing evidence — release + --nocapture"]
fn in_struct_member_typing_at_scale() {
    let lang = c_lang();
    let mut s = String::new();
    s.push_str("#include <stdio.h>\n#define LIMIT 100\n\ntypedef unsigned long word_t;\n\n");
    let mut i = 0usize;
    while s.lines().count() < 249_000 {
        if i % 50 == 0 {
            s.push_str(&format!(
                "#define SCALE_STEP_{i}(v) \\\n    ((v) * {i} + \\\n     (v) % LIMIT)\n\n"
            ));
        }
        s.push_str(&format!(
            "static int scale_{i}(int v, int factor) {{\n    int result = v * factor + {i};\n    result = result % LIMIT;\n    return result;\n}}\n\n"
        ));
        i += 1;
    }

    let mut l: Box<dyn Limner> = Box::new(LiveDoc::open(lang.clone(), "member.c", &s));
    let _ = l.open(&s);
    let marks_open = l.marks().len();
    println!("ENG open marks={marks_open}");

    // The edit-log script the editor actually emitted (auto-indent
    // width 2, auto-close pairs on). Each step: one LineEdit — the
    // replaced line range and its replacement lines.
    let line10: String = s.lines().nth(10).unwrap_or("").to_string();
    let steps: Vec<(&str, u32, u32, Vec<String>)> = vec![
        // Return at the end of line 10: the line splits, a fresh empty
        // line 11 appears.
        ("return_open_line", 10, 11, vec![line10.clone(), String::new()]),
        // Typing `struct Point ` char by char on the fresh line.
        ("k_s", 11, 12, vec!["s".into()]),
        ("k_st", 11, 12, vec!["st".into()]),
        ("k_str", 11, 12, vec!["str".into()]),
        ("k_stru", 11, 12, vec!["stru".into()]),
        ("k_struc", 11, 12, vec!["struc".into()]),
        ("k_struct", 11, 12, vec!["struct".into()]),
        ("k_struct_", 11, 12, vec!["struct ".into()]),
        ("k_P", 11, 12, vec!["struct P".into()]),
        ("k_Po", 11, 12, vec!["struct Po".into()]),
        ("k_Poi", 11, 12, vec!["struct Poi".into()]),
        ("k_Poin", 11, 12, vec!["struct Poin".into()]),
        ("k_Point", 11, 12, vec!["struct Point".into()]),
        ("k_Point_", 11, 12, vec!["struct Point ".into()]),
        // `{` arrives closed: one keystroke, two chars.
        ("open_brace", 11, 12, vec!["struct Point {}".into()]),
        // Return between the braces: header keeps the `{`, the `}`
        // moves down behind the auto-indent.
        ("return_inside", 11, 12, vec!["struct Point {".into(), "  }".into()]),
        // The member, typed before the `}` — incomplete until the `;`.
        ("f_i", 12, 13, vec!["  i}".into()]),
        ("f_in", 12, 13, vec!["  in}".into()]),
        ("f_int", 12, 13, vec!["  int}".into()]),
        ("f_int_", 12, 13, vec!["  int }".into()]),
        ("f_int_x", 12, 13, vec!["  int x}".into()]),
        ("f_int_x_semi", 12, 13, vec!["  int x;}".into()]),
        // One keystroke AFTER completion: the recovery baseline.
        ("post_semi_space", 12, 13, vec!["  int x; }".into()]),
    ];

    let mut worst_incomplete = std::time::Duration::ZERO;
    let mut post_semi = std::time::Duration::ZERO;
    let mut min_marks = usize::MAX;
    for (name, start, end, lines) in steps {
        let edit = LineEdit { start, end, lines: lines.clone() };
        let t = Instant::now();
        let delta = l.edit(&edit);
        let took = t.elapsed();
        let splice = delta
            .splice
            .as_ref()
            .map(|((lo, hi), repl)| format!("{lo}..{hi}+{}", repl.len()))
            .unwrap_or_else(|| "-".into());
        let marks = l.marks().len();
        min_marks = min_marks.min(marks);
        println!(
            "ENG step={name} edit={:.1}ms splice={splice} repaints={} marks={marks}",
            took.as_secs_f64() * 1e3,
            delta.repaints.len()
        );
        if name.starts_with("f_") && name != "f_int_x_semi" {
            worst_incomplete = worst_incomplete.max(took);
        }
        if name == "post_semi_space" {
            post_semi = took;
        }
    }
    println!(
        "ENG worst_incomplete={:.1}ms post_semi={:.1}ms min_marks={min_marks}",
        worst_incomplete.as_secs_f64() * 1e3,
        post_semi.as_secs_f64() * 1e3
    );

    // Containment: at NO point in the journey may the outline collapse
    // — a marks cliff is the world-swallow, whatever the timings say.
    assert!(
        min_marks >= 40_000,
        "outline collapsed mid-journey (min {min_marks}): the struct swallowed the world"
    );

    // Divergence probe: the incremental journey must land on the same
    // marks a fresh open of the final text computes.
    let final_text = l.text();
    let mut fresh: Box<dyn Limner> =
        Box::new(LiveDoc::open(lang.clone(), "member_fresh.c", &final_text));
    let _ = fresh.open(&final_text);
    let (inc, batch) = (l.marks().len(), fresh.marks().len());
    println!("ENG final marks inc={inc} batch={batch}");
    assert_eq!(inc, batch, "incremental marks diverged from batch after the member journey");
}
