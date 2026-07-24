//! Semantic-layer benchmarks at 100k lines: per-item memoization (P6)
//! on top of the P4 firewall. The claim under test: after the first
//! analysis, a body edit's SEMANTIC cost tracks the edit — one fragment
//! walk, one item resolution — and everything else (same file and other
//! files) answers from cache.

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
    println!("rantlr P4/P6 — semantic layer at {N} lines (per-item memoization)");
    println!("=================================================================");

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
    let mut b = IncSession::new(&lexer, &sg, &tables, "let mirror = v0;\nlet m2 = v100;\n").unwrap();

    let mut db = SemDb::new(demo_binding_config(&sg));
    let (_, d) = time(|| db.set_tree("a", a.tree().unwrap().clone()));
    println!(
        "cold set_tree (fragments):   {:>10}   ({} fragments)",
        fmt_dur(d),
        db.stats.fragments_computed
    );
    db.set_tree("b", b.tree().unwrap().clone());
    let (u, d) = time(|| db.unresolved("a"));
    println!(
        "cold resolve (all items):    {:>10}   ({} item resolutions, {} unresolved)",
        fmt_dur(d),
        db.stats.item_resolves_computed,
        u.len()
    );
    let st = db.symbols("a");
    println!(
        "composed symbols view:                    ({} defs, {} refs, {} scopes)",
        st.defs.len(),
        st.refs.len(),
        st.scope_parents.len()
    );
    db.unresolved("b");

    // THE HEADLINE: a body edit's semantic cost tracks the edit.
    let base = db.stats;
    a.edit(&sg, &tables, &[LineEdit {
        start: 4,
        end: 5,
        replacement: vec![Line::new("  let use4 = 12345;\n}", LineTerm::Lf)],
    }])
    .unwrap();
    let (_, d_set) = time(|| db.set_tree("a", a.tree().unwrap().clone()));
    let (_, d_res) = time(|| db.unresolved("a"));
    let (_, d_b) = time(|| db.unresolved("b"));
    let after = db.stats;
    println!("\nbody edit in A:");
    println!(
        "  set_tree (fragment reuse):  {:>10}   ({} fragment recomputed)",
        fmt_dur(d_set),
        after.fragments_computed - base.fragments_computed
    );
    println!(
        "  unresolved(A):              {:>10}   ({} item resolutions recomputed)",
        fmt_dur(d_res),
        after.item_resolves_computed - base.item_resolves_computed
    );
    println!(
        "  unresolved(B):              {:>10}   (memoized — THE FIREWALL, per item)",
        fmt_dur(d_b)
    );
    // ≤2, not 1: the newline ending an item is trivia inside the NEXT
    // item's leading spine, so an edit may re-anchor its right neighbor
    // (bounded adjacency; the recomputed fragment is value-equal).
    assert!(
        (1..=2).contains(&(after.fragments_computed - base.fragments_computed)),
        "edit-sized fragment recompute"
    );
    assert!(
        (1..=2).contains(&(after.item_resolves_computed - base.item_resolves_computed)),
        "edit-sized item resolution recompute"
    );

    // Navigation stays cheap: binary-search + fragment-local scan (the
    // first call after an edit also builds the lazy position index).
    let now = a.buf.reproduce();
    let off = now.find("emit(v0").map(|p| (p + 5) as u32).unwrap();
    let (defn, d_first) = time(|| db.definition("a", off));
    assert!(defn.is_some(), "v0 resolves");
    let (_, d_again) = time(|| db.definition("a", off));
    println!(
        "\ngo-to-definition:            {:>10}   (first after edit, builds index; then {})",
        fmt_dur(d_first),
        fmt_dur(d_again)
    );

    // Signature edit: fragments stay edit-sized, but downstream items
    // RECLASSIFY (the env fingerprint is deliberately coarse — a
    // changed export could shadow anything below; per-name dependency
    // tracking is the salsa-grade refinement). B recomputes too: its
    // foreign fingerprint moved.
    let base = db.stats;
    a.edit(&sg, &tables, &[LineEdit {
        start: 0,
        end: 1,
        replacement: vec![Line::new("let v0renamed = 0;", LineTerm::Lf)],
    }])
    .unwrap();
    db.set_tree("a", a.tree().unwrap().clone());
    let (_, d_a) = time(|| db.unresolved("a"));
    let mid = db.stats;
    let (_, d_b) = time(|| db.unresolved("b"));
    let after = db.stats;
    println!("\nsignature edit in A (renames v0):");
    println!(
        "  unresolved(A):              {:>10}   ({} fragments, {} item resolutions)",
        fmt_dur(d_a),
        mid.fragments_computed - base.fragments_computed,
        mid.item_resolves_computed - base.item_resolves_computed
    );
    println!(
        "  unresolved(B):              {:>10}   ({} item resolutions — foreign fp moved)",
        fmt_dur(d_b),
        after.item_resolves_computed - mid.item_resolves_computed
    );
    assert!(
        mid.fragments_computed - base.fragments_computed <= 4,
        "fragment recompute stays edit-sized on a rename (edited item, its \
         trivia-adjacent neighbor, and at most a splice-seam re-anchor or two)"
    );

    // Body edit in B: A untouched entirely.
    let base = db.stats;
    b.edit(&sg, &tables, &[LineEdit {
        start: 1,
        end: 2,
        replacement: vec![Line::new("let m2 = v100 + 1;", LineTerm::Lf)],
    }])
    .unwrap();
    db.set_tree("b", b.tree().unwrap().clone());
    let (_, d_b) = time(|| db.unresolved("b"));
    let (_, d_a) = time(|| db.unresolved("a"));
    let after = db.stats;
    println!("\nbody edit in B:");
    println!(
        "  unresolved(B):              {:>10}   ({} fragment, {} item resolutions)",
        fmt_dur(d_b),
        after.fragments_computed - base.fragments_computed,
        after.item_resolves_computed - base.item_resolves_computed
    );
    println!(
        "  unresolved(A):              {:>10}   (fully memoized at 100k lines)",
        fmt_dur(d_a)
    );

    println!("\nall assertions passed.");
}
