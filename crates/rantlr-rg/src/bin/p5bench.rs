//! P5 benchmarks: what the textual grammar surface costs.
//!
//! The claim under test: `.rg` is a SURFACE — parsing and compiling the
//! text is noise next to the table build both paths share, and grammar
//! files enjoy the same incremental editing budget as any hosted
//! language.

use rantlr_engine::{IncSession, Line, LineEdit, LineTerm};
use rantlr_rg::compile::{certify, compile};
use rantlr_rg::{compile_source, RgToolchain};
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

const RG_RG: &str = include_str!("../../rg.rg");
const CHARTLANG_RG: &str = include_str!("../../chartlang.rg");

fn main() {
    println!("rantlr P5 — the .rg textual grammar surface");
    println!("===========================================");

    let (tc, d) = time(RgToolchain::new);
    println!("bootstrap toolchain (lex DFA + LR tables): {:>10}", fmt_dur(d));

    // Self-hosting round trip.
    let (out, d_parse) = time(|| compile_source(&tc, RG_RG));
    let (cert, d_cert) = time(|| certify(&out.def).expect("in envelope"));
    println!("\nrg.rg ({} lines):", RG_RG.lines().count());
    println!("  parse + compile to values:               {:>10}", fmt_dur(d_parse));
    println!("  certify (lints + LR build):              {:>10}", fmt_dur(d_cert));
    drop(cert);

    let (out, d_parse) = time(|| compile_source(&tc, CHARTLANG_RG));
    let (_cert, d_cert) = time(|| certify(&out.def).expect("in envelope"));
    println!("chartlang.rg ({} lines):", CHARTLANG_RG.lines().count());
    println!("  parse + compile to values:               {:>10}", fmt_dur(d_parse));
    println!("  certify (lints + LR build):              {:>10}", fmt_dur(d_cert));

    // A large synthetic grammar: 300 tokens, 200 rules × 4 alternatives.
    let mut big = String::from("language Big\n");
    for i in 0..300 {
        big.push_str(&format!("token T{i} = \"t{i}\"\n"));
    }
    big.push_str("start r0\n");
    for r in 0..200 {
        big.push_str(&format!("rule r{r} =\n"));
        for a in 0..4 {
            let t = (r * 4 + a) % 300;
            if r + 1 < 200 && a == 3 {
                big.push_str(&format!("  | R{r}x{a}: T{t} r{}\n", r + 1));
            } else {
                big.push_str(&format!("  | R{r}x{a}: T{t}\n"));
            }
        }
    }
    let (out, d_parse) = time(|| compile_source(&tc, &big));
    assert!(out.diags.is_empty(), "{:?}", out.diags);
    let ((), d_compile_only) = time(|| {
        let (_, diags) = compile(&out.tree, &tc.prods);
        assert!(diags.is_empty());
    });
    let (_c, d_cert) = time(|| certify(&out.def).expect("in envelope"));
    println!(
        "synthetic ({} lines, {} tokens, {} prods):",
        big.lines().count(),
        out.def.lex.tokens.len(),
        out.def.sg.prods.len()
    );
    println!("  parse + compile to values:               {:>10}", fmt_dur(d_parse));
    println!("  compile pass alone (tree → values):      {:>10}", fmt_dur(d_compile_only));
    println!("  certify (lints + LR build):              {:>10}", fmt_dur(d_cert));

    // Incremental editing of the grammar FILE: a keystroke-sized edit
    // mid-file re-parses with the usual reuse budget.
    let mut session = IncSession::new(&tc.lexer, &tc.sg, &tc.tables, &big).unwrap();
    let line = 305; // inside the rule section
    let (outc, d_edit) = time(|| {
        session
            .edit(&tc.sg, &tc.tables, &[LineEdit {
                start: line,
                end: line + 1,
                replacement: vec![Line::new("  | Edited0x9: T7 // touched", LineTerm::Lf)],
            }])
            .unwrap()
    });
    println!("\nkeystroke edit in the synthetic .rg:");
    println!(
        "  incremental reparse:                     {:>10}   (reuse {:.1}%, {} splices)",
        fmt_dur(d_edit),
        outc.stats.reuse_fraction() * 100.0,
        outc.stats.splices
    );
    let (_, d_batch) = time(|| {
        IncSession::new(&tc.lexer, &tc.sg, &tc.tables, &session.buf.reproduce()).unwrap()
    });
    println!("  batch reparse (comparison):              {:>10}", fmt_dur(d_batch));

    println!("\nall assertions passed.");
}
