//! P7 benchmarks: the composition tier at scale. A 50k-line composed
//! document (chartlang hosting `.qana` islands every ~100 lines) — the
//! claims under test: composing + certifying the product costs
//! milliseconds, and edits INSIDE an island or in the host around one
//! keep the usual incremental budget, because the engine parses the
//! product like any other envelope language.

use qana_engine::{IncSession, Line, LineEdit, LineTerm};
use qana_lang::chartlang_with_qana_islands;
use std::time::{Duration, Instant};

fn time<R>(f: impl FnOnce() -> R) -> (R, Duration) {
    let t0 = Instant::now();
    let r = f();
    (r, t0.elapsed())
}

fn fmt_dur(d: Duration) -> String {
    let us = d.as_secs_f64() * 1e6;
    if us < 1000.0 {
        format!("{us:.1} µs")
    } else if us < 1_000_000.0 {
        format!("{:.2} ms", us / 1000.0)
    } else {
        format!("{:.3} s", us / 1e6)
    }
}

fn main() {
    println!("qana P7 — composition tier (chartlang ⊃ qana islands)");
    println!("=====================================================");

    let (tc, d) = time(chartlang_with_qana_islands);
    println!(
        "compose + certify product:   {:>10}   ({} tokens, {} prods, {} states)",
        fmt_dur(d),
        tc.lexer.vocab.names.len(),
        tc.sg.prods.len(),
        tc.tables.n_states
    );

    // 50k lines: blocks of 90 host statements + a 10-line qana island.
    let mut src = String::new();
    let mut island_lines: Vec<usize> = Vec::new();
    let mut line_no = 0usize;
    for block in 0..500 {
        for i in 0..90 {
            src.push_str(&format!("let b{block}_v{i} = {i};\n"));
            line_no += 1;
        }
        src.push_str("```qana\n");
        line_no += 1;
        island_lines.push(line_no); // first interior line
        for t in 0..7 {
            src.push_str(&format!("token B{block}T{t} = \"x{block}_{t}\"\n"));
            line_no += 1;
        }
        src.push_str(&format!("rule r{block} = B{block}T0?\n"));
        line_no += 1;
        src.push_str("```\n");
        line_no += 1;
    }
    let n_lines = src.lines().count();

    let (mut s, d) = time(|| IncSession::new(&tc.lexer, &tc.sg, &tc.tables, &src).unwrap());
    assert!(s.last_repairs.is_empty());
    println!(
        "cold parse ({} lines, {} islands): {:>10}",
        n_lines,
        island_lines.len(),
        fmt_dur(d)
    );

    // Edit INSIDE a mid-file island (guest territory).
    let inside = island_lines[250] + 3;
    let (out, d) = time(|| {
        s.edit(&tc.sg, &tc.tables, &[LineEdit {
            start: inside,
            end: inside + 1,
            replacement: vec![Line::new("token EDITED = \"zz\" @style(string)", LineTerm::Lf)],
        }])
        .unwrap()
    });
    println!(
        "edit inside island #250:     {:>10}   (reuse {:.2}%, {} splices)",
        fmt_dur(d),
        out.stats.reuse_fraction() * 100.0,
        out.stats.splices
    );
    assert!(out.stats.reuse_fraction() > 0.99);

    // Edit the HOST right between two islands.
    let host_line = island_lines[250] - 30;
    let (out, d) = time(|| {
        s.edit(&tc.sg, &tc.tables, &[LineEdit {
            start: host_line,
            end: host_line + 1,
            replacement: vec![Line::new("let edited_host = 42;", LineTerm::Lf)],
        }])
        .unwrap()
    });
    println!(
        "edit host near islands:      {:>10}   (reuse {:.2}%, {} splices)",
        fmt_dur(d),
        out.stats.reuse_fraction() * 100.0,
        out.stats.splices
    );
    assert!(out.stats.reuse_fraction() > 0.99);

    // Fence catastrophe: delete a mid-file island's close fence (the
    // island swallows text until the next fence), then heal it.
    let close_line = island_lines[250] + 8;
    let (_, d) = time(|| {
        s.edit(&tc.sg, &tc.tables, &[LineEdit {
            start: close_line,
            end: close_line + 1,
            replacement: vec![],
        }])
        .unwrap()
    });
    println!("delete island close fence:   {:>10}   (island extends; parse stays total)", fmt_dur(d));
    let (_, d) = time(|| {
        s.edit(&tc.sg, &tc.tables, &[LineEdit {
            start: close_line,
            end: close_line,
            replacement: vec![Line::new("```", LineTerm::Lf)],
        }])
        .unwrap()
    });
    assert!(s.last_repairs.is_empty(), "healed");
    println!("restore the fence:           {:>10}   (repairs clear)", fmt_dur(d));

    println!("\nall assertions passed.");
}
