//! P1 benchmark: the GENERATED lexer on the same scenarios as P0's
//! hand-written lexer (see crates/rantlr-lex/src/bin/bench.rs), so the
//! two are directly comparable.

use rantlr_engine::*;
use rantlr_grammar::demo::demo_grammar;
use rantlr_grammar::CompiledLexer;
use std::time::{Duration, Instant};

struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed.max(1))
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

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

fn generate(lines: usize) -> String {
    let mut out = String::with_capacity(lines * 40);
    let mut i = 0usize;
    while i < lines {
        match i % 10 {
            0 => out.push_str(&format!("fn compute_{i}(a: u32, b: u32) -> u32 {{\n")),
            1 => out.push_str(&format!("    let acc_{i} = a * {i} + b; // running total\n")),
            2 => out.push_str(&format!("    let name_{i} = \"item {i} label\";\n")),
            3 => out.push_str(&format!("    if acc_{i} > {i} {{ emit(name_{i}, [a, b]); }}\n")),
            4 => {
                if i % 470 == 4 {
                    out.push_str("    /* multi-line note:\n");
                    out.push_str("       spans three lines\n");
                    out.push_str("       and closes here */\n");
                    i += 3;
                    continue;
                }
                out.push_str(&format!("    while a < {i} {{ step(a); }}\n"));
            }
            5 => out.push_str(&format!("    let v_{i} = (a + b) * ({i} - 1);\n")),
            6 => out.push_str("    match a { 0 => b, _ => a }\n"),
            7 => out.push_str(&format!("    // checkpoint {i}: mid-function commentary\n")),
            8 => out.push_str(&format!("    return acc(v_{i}, {i}.5);\n")),
            _ => out.push_str("}\n"),
        }
        i += 1;
    }
    out
}

fn main() {
    const N_LINES: usize = 100_000;
    const SITES: usize = 1_000;
    const ROUNDS: usize = 10;

    println!("rantlr P1 — GENERATED lexer on the P0 scenarios");
    println!("===============================================");

    let (g, _ids) = demo_grammar();
    let (lexer, build_time) = time(|| CompiledLexer::build(&g).expect("demo grammar in envelope"));
    println!(
        "grammar compile + lints:     {:>10}   (stack bound {}, line-state space {}, DFA states {:?})",
        fmt_dur(build_time),
        lexer.report.stack_bound,
        lexer.report.line_state_space,
        lexer.report.dfa_states
    );

    let src = generate(N_LINES);
    let mb = src.len() as f64 / 1e6;

    let mut best = Duration::MAX;
    let mut buf_opt = None;
    for _ in 0..5 {
        let (b, d) = time(|| LexedBuffer::new(&lexer, &src));
        best = best.min(d);
        buf_opt = Some(b);
    }
    let mut buf = buf_opt.unwrap();
    let n_tokens: usize = buf.lexed.iter().map(|l| l.tokens.len()).sum();
    println!(
        "cold lex (generated DFA):    {:>10}   ({:.1} MB/s, {} tokens)",
        fmt_dur(best),
        mb / best.as_secs_f64(),
        n_tokens
    );

    let (repro, d) = time(|| buf.reproduce());
    assert_eq!(repro, src, "LOSSLESS ROUNDTRIP FAILED");
    println!("lossless roundtrip:          {:>10}   (byte-identical: verified)", fmt_dur(d));

    let (sk, d) = time(|| build_skeleton(&buf, &lexer.vocab));
    println!(
        "block skeleton:              {:>10}   ({} blocks, max depth {}, {} folds)",
        fmt_dur(d),
        sk.blocks.len(),
        sk.max_depth,
        sk.folding_ranges().count()
    );

    // Typical batch: 1,000 scattered state-neutral single-line edits.
    let mut rng = Rng::new(0xBEEF);
    let mut times = Vec::new();
    let mut total = DamageReport::default();
    for round in 0..ROUNDS {
        let n = buf.lines.len() - 1;
        let mut picks: Vec<usize> = Vec::new();
        while picks.len() < SITES {
            let line = rng.below(n);
            let t = &buf.lines[line].text;
            if t.contains("/*") || t.contains("*/") {
                continue;
            }
            picks.push(line);
        }
        picks.sort_unstable();
        picks.dedup();
        let edits: Vec<LineEdit> = picks
            .into_iter()
            .map(|line| LineEdit {
                start: line,
                end: line + 1,
                replacement: vec![Line::new(
                    format!("    let edited_{line}_{round} = {line}; // touched"),
                    LineTerm::Lf,
                )],
            })
            .collect();
        let (report, d) = time(|| buf.apply_edits(&edits));
        times.push(d);
        total.replaced_lines += report.replaced_lines;
        total.relexed_lines += report.relexed_lines;
        total.reconverged_extra += report.reconverged_extra;
    }
    times.sort();
    let median = times[times.len() / 2];
    println!(
        "typical batch (1000 sites):  {:>10}   ({:.2} µs/site; {:.0} replaced + {:.0} reconv/round)",
        fmt_dur(median),
        median.as_secs_f64() * 1e6 / SITES as f64,
        total.replaced_lines as f64 / ROUNDS as f64,
        total.reconverged_extra as f64 / ROUNDS as f64
    );

    // Single-keystroke latency.
    let mut singles = Vec::new();
    for k in 0..1000usize {
        let line = rng.below(buf.lines.len() - 1);
        if buf.lines[line].text.contains("/*") || buf.lines[line].text.contains("*/") {
            continue;
        }
        let edit = LineEdit {
            start: line,
            end: line + 1,
            replacement: vec![Line::new(format!("    let keystroke_{k} = {k};"), LineTerm::Lf)],
        };
        let (_, d) = time(|| buf.apply_edits(&[edit]));
        singles.push(d);
    }
    singles.sort();
    println!(
        "single edit (typical):       {:>10}   (median; p99 {})",
        fmt_dur(singles[singles.len() / 2]),
        fmt_dur(singles[singles.len() * 99 / 100])
    );

    println!("\nall assertions passed.");
}
