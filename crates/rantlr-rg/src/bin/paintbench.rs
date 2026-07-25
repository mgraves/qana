//! paintbench — the native editor protocol's numbers, on a 10k-line C
//! file: full two-wave paint, per-keystroke deltas (wave 0 alone and
//! both waves), and hover facts. Release mode is the honest run:
//!
//!     cargo run -p rantlr-rg --release --bin paintbench

use rantlr_engine::{IncSession, Line, LineEdit};
use rantlr_rg::compile::certify;
use rantlr_rg::{compile_source, RgToolchain};
use rantlr_sem::SemDb;
use rantlr_services::paint::{facts_at, Painter};
use std::time::Instant;

fn main() {
    let c_rg = std::fs::read_to_string("examples/c/c.rg").unwrap();
    let tc = RgToolchain::new();
    let out = compile_source(&tc, &c_rg);
    let (lexer, tables) = certify(&out.def).unwrap();

    // 10k lines: 2000 functions of five lines each.
    let mut doc = String::from("typedef unsigned long word_t;\n");
    for i in 0..2000 {
        doc.push_str(&format!(
            "word_t fn{i}(word_t v) {{\n    word_t r = v * {i};\n    if (r > 100)\n        r = r - {i};\n    return r;\n}}\n"
        ));
    }
    let n_lines = doc.lines().count();

    let t = Instant::now();
    let mut session = IncSession::new(&lexer, &out.def.sg, &tables, &doc).unwrap();
    let parse_ms = t.elapsed().as_secs_f64() * 1e3;

    let mut db = SemDb::new(out.def.binding.clone());
    db.set_tree("d", session.tree().unwrap().clone());
    let t = Instant::now();
    let (mut painter, full) = Painter::new(&session, &out.def.styles, Some((&mut db, "d")));
    let full_ms = t.elapsed().as_secs_f64() * 1e3;
    let n_runs: usize = full.lines.iter().map(|l| l.len()).sum();

    // A stream of single-line body edits in the middle of the file.
    let mut wave0_us = Vec::new();
    let mut both_us = Vec::new();
    let mut lex_painter = {
        let s2 = IncSession::new(&lexer, &out.def.sg, &tables, &doc).unwrap();
        let (p, _) = Painter::new(&s2, &out.def.styles, None);
        drop(s2);
        p
    };
    let mut lex_session = IncSession::new(&lexer, &out.def.sg, &tables, &doc).unwrap();
    for k in 0..50u32 {
        // Always the `word_t r = …` body line of some function
        // (line shapes repeat with period 6): a benign body edit.
        let li = 5000 + (k as usize % 100) * 6;
        let term = session.buf.lines[li].term;
        let text = format!("    word_t r = v * {k} + 7;");

        // Wave 0 alone (edit + lexical delta): the first-frame path.
        let t = Instant::now();
        let o = lex_session
            .edit(&out.def.sg, &tables, &[LineEdit {
                start: li,
                end: li + 1,
                replacement: vec![Line::new(text.clone(), term)],
            }])
            .unwrap();
        let _d = lex_painter.update(&lex_session, &o.damage, &out.def.styles, None);
        wave0_us.push(t.elapsed().as_secs_f64() * 1e6);

        // Both waves (edit + re-bind + overlay + delta).
        let t = Instant::now();
        let o = session
            .edit(&out.def.sg, &tables, &[LineEdit {
                start: li,
                end: li + 1,
                replacement: vec![Line::new(text, term)],
            }])
            .unwrap();
        db.set_tree("d", session.tree().unwrap().clone());
        let d = painter.update(&session, &o.damage, &out.def.styles, Some((&mut db, "d")));
        both_us.push(t.elapsed().as_secs_f64() * 1e6);
        assert!(d.splice.is_some());
    }

    // Hover facts, warm.
    let text = session.buf.reproduce();
    let at = text.find("fn1000(").unwrap() as u32;
    let t = Instant::now();
    let mut cards = 0;
    for _ in 0..100 {
        cards += facts_at(&mut db, "d", &text, at).is_some() as u32;
    }
    let facts_us = t.elapsed().as_secs_f64() * 1e6 / 100.0;
    assert_eq!(cards, 100);

    let stats = |v: &mut Vec<f64>| {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        (v[v.len() / 2], v[v.len() * 9 / 10])
    };
    let (w0_med, w0_p90) = stats(&mut wave0_us);
    let (bo_med, bo_p90) = stats(&mut both_us);

    println!("paintbench — {n_lines} lines of C, {n_runs} runs");
    println!("  initial parse                {parse_ms:8.2} ms");
    println!("  full two-wave paint          {full_ms:8.2} ms");
    println!("  keystroke → wave-0 delta     {w0_med:8.1} µs median   {w0_p90:8.1} µs p90");
    println!("  keystroke → two-wave delta   {bo_med:8.1} µs median   {bo_p90:8.1} µs p90");
    println!("  hover facts (warm)           {facts_us:8.1} µs");
}
