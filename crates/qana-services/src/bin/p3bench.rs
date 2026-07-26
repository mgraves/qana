//! P3 service benchmarks at 100k lines: semantic tokens full + delta,
//! folding, outline, completion.

use qana_engine::*;
use qana_grammar::demo::{demo_grammar, demo_syn_grammar};
use qana_grammar::{build_lr, CompiledLexer};
use qana_services::demo_glue::{demo_outline_config, demo_styles};
use qana_services::*;
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
    const N: usize = 100_000;
    println!("qana P3 — derived services at {N} lines");
    println!("=========================================");

    let (g, ids) = demo_grammar();
    let lexer = CompiledLexer::build(&g).expect("in envelope");
    let sg = demo_syn_grammar(&ids, &lexer.vocab);
    let tables = build_lr(&sg);
    let styles = demo_styles(&ids);

    let mut src = String::with_capacity(N * 24);
    for i in 0..N {
        match i % 5 {
            0 => src.push_str(&format!("let v{i} = {i} + {i} * 2;\n")),
            1 => src.push_str(&format!("if (v{i}) {{ emit(v{i}, 1); }} else {{ skip(); }}\n")),
            2 => src.push_str(&format!("emit(\"s{i}\", [1, 2.5]); // note {i}\n")),
            3 => src.push_str(&format!("let w{i} = (v{i} - 1) / 2;\n")),
            _ => src.push_str(&format!("done(v{i});\n")),
        }
    }
    let mut session = IncSession::new(&lexer, &sg, &tables, &src).expect("parses");

    let (mut cache, d) = time(|| semantic_tokens_full(&lexer, &session.buf, &styles));
    println!(
        "semantic tokens (full):      {:>10}   ({} styled tokens, {} u32s)",
        fmt_dur(d),
        cache.data.len() / 5,
        cache.data.len()
    );

    // Per-keystroke delta.
    let mut deltas = Vec::new();
    for k in 0..200usize {
        let line = 10 + (k * 487) % (N - 20);
        let edit = LineEdit {
            start: line,
            end: line + 1,
            replacement: vec![Line::new(format!("let e{k} = {k}; // touched"), LineTerm::Lf)],
        };
        let t0 = Instant::now();
        let out = session.edit(&sg, &tables, &[edit]).expect("valid");
        let delta = cache.update(&lexer, &session.buf, &styles, &out.damage);
        deltas.push((t0.elapsed(), delta.map(|d| d.insert.len()).unwrap_or(0)));
    }
    deltas.sort_by_key(|d| d.0);
    let (median, ins) = deltas[deltas.len() / 2];
    println!(
        "edit → reparse + sem delta:  {:>10}   (median incl. incremental parse; ~{} u32s per delta)",
        fmt_dur(median),
        ins
    );
    let fresh = semantic_tokens_full(&lexer, &session.buf, &styles);
    assert!(cache == fresh, "DELTA GATE FAILED AT SCALE");
    println!("delta gate at scale:             verified   (200 edits, cache == fresh encode)");

    let (folds, d) = time(|| folding_ranges(&lexer, &session.buf));
    println!("folding ranges:              {:>10}   ({} ranges)", fmt_dur(d), folds.len());

    let cfg = demo_outline_config(&sg);
    let (syms, d) = time(|| outline(session.tree().unwrap(), &cfg));
    println!("outline:                     {:>10}   ({} symbols)", fmt_dur(d), syms.len());

    let mid = (src.len() / 2) as u32;
    let (items, d) = time(|| completion_at(&lexer, &session.buf, &sg, &tables, mid));
    println!(
        "completion at mid-file:      {:>10}   ({} items; states-only prefix run — checkpointing is the refinement)",
        fmt_dur(d),
        items.len()
    );

    println!("\nall assertions passed.");
}
