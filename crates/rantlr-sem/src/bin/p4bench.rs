//! P4 benchmarks: semantic queries at 100k lines + the firewall at scale.

use rantlr_engine::*;
use rantlr_grammar::demo::{demo_grammar, demo_syn_grammar};
use rantlr_grammar::{build_lr, CompiledLexer};
use rantlr_sem::{demo_binding_config, SemDb};
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
    println!("rantlr P4 — semantic layer at {N} lines");
    println!("=======================================");

    let (g, ids) = demo_grammar();
    let lexer = CompiledLexer::build(&g).unwrap();
    let sg = demo_syn_grammar(&ids, &lexer.vocab);
    let tables = build_lr(&sg);

    // File A: 100k lines with defs and refs (some blocks with locals).
    let mut src = String::with_capacity(N * 28);
    for i in 0..N {
        match i % 10 {
            0 => src.push_str(&format!("let v{i} = {i};\n")),
            3 => src.push_str(&format!("{{ let inner{i} = v{};\n", i - 3)),
            4 => src.push_str(&format!("  let use{i} = inner{};\n}}\n", i - 1)),
            7 => src.push_str(&format!("let w{i} = v{} + v{};\n", i - 7, (i / 20) * 10)),
            _ => src.push_str(&format!("emit(v{}, {i});\n", (i / 10) * 10)),
        }
    }
    let mut a = IncSession::new(&lexer, &sg, &tables, &src).unwrap();
    let b = IncSession::new(&lexer, &sg, &tables, "let mirror = v0;\nlet m2 = v100;\n").unwrap();

    let mut db = SemDb::new(demo_binding_config(&sg));
    db.set_tree("a", a.tree().unwrap().clone());
    db.set_tree("b", b.tree().unwrap().clone());

    let (st, d) = time(|| db.symbols("a"));
    println!(
        "symbols (100k lines):        {:>10}   ({} defs, {} refs, {} scopes)",
        fmt_dur(d),
        st.defs.len(),
        st.refs.len(),
        st.scope_parents.len()
    );
    let (res, d) = time(|| db.resolve("a"));
    let unresolved = res
        .iter()
        .filter(|t| matches!(t, rantlr_sem::Target::Unresolved))
        .count();
    println!(
        "resolve (100k lines):        {:>10}   ({} refs resolved, {} unresolved)",
        fmt_dur(d),
        res.len(),
        unresolved
    );
    db.resolve("b");

    // Firewall at scale: a BODY edit inside a block in A must leave B's
    // resolution memoized (only A recomputes).
    let base = db.stats;
    a.edit(&sg, &tables, &[LineEdit {
        start: 4,
        end: 5,
        replacement: vec![Line::new("  let use4 = 12345;\n}", LineTerm::Lf)],
    }])
    .unwrap();
    db.set_tree("a", a.tree().unwrap().clone());
    let (_, d_a) = time(|| db.resolve("a"));
    let (_, d_b) = time(|| db.resolve("b"));
    let after = db.stats;
    println!(
        "body edit → resolve A:       {:>10}   (recomputed: full-file walk — per-item memoization is the refinement)",
        fmt_dur(d_a)
    );
    println!(
        "body edit → resolve B:       {:>10}   (memoized: {} recompute total — THE FIREWALL)",
        fmt_dur(d_b),
        after.resolve_computed - base.resolve_computed
    );
    assert_eq!(after.resolve_computed - base.resolve_computed, 1, "firewall must hold");

    println!("\nall assertions passed.");
}
