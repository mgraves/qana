//! P0 spike benchmark: the envelope's lexing contract, measured.
//!
//!   cargo run --release --bin bench
//!
//! Scenarios, in order of increasing hostility:
//!   1. cold lex + lossless roundtrip + block skeleton (100k-line file)
//!   2. typical batch: 1,000 scattered single-line edits that don't touch
//!      block-comment delimiters (the overwhelmingly common case)
//!   3. single-keystroke latency (same-shape in-place fast path)
//!   4. bounded construct: open + close a block comment 25 lines apart in
//!      one batch — the state wave must stop at the closer
//!   5. adversarial batch: unrestricted edits that may delete comment
//!      delimiters (state waves; nested comments make closers global)
//!   6. unbounded pathological: open an unclosed comment — relex to EOF,
//!      the inherent worst case (identical semantics to VS Code)

use rantlr_lex::*;
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

/// Deterministic, realistic-ish source generator: functions with bodies,
/// comments (line + multi-line block), strings, nesting.
fn generate(lines: usize, with_block_comments: bool) -> String {
    let mut out = String::with_capacity(lines * 40);
    let mut i = 0usize;
    while i < lines {
        match i % 10 {
            0 => out.push_str(&format!("fn compute_{i}(a: u32, b: u32) -> u32 {{\n")),
            1 => out.push_str(&format!("    let acc_{i} = a * {i} + b; // running total\n")),
            2 => out.push_str(&format!("    let name_{i} = \"item {i} label\";\n")),
            3 => out.push_str(&format!("    if acc_{i} > {i} {{ emit(name_{i}, [a, b]); }}\n")),
            4 => {
                if with_block_comments && i % 470 == 4 {
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

fn replacement_text(line: usize, round: usize) -> String {
    match (line + round) % 4 {
        0 => format!("    let edited_{line}_{round} = {line}; // touched"),
        1 => format!("    emit(\"edit {line} round {round}\", [1, 2.5]);"),
        2 => format!("    if edited {{ recompute_{line}(); }}"),
        _ => format!("    // rewritten line {line} in round {round}"),
    }
}

/// 1,000 scattered 1-for-1 line replacements. With `avoid_state_lines`,
/// lines containing block-comment delimiters are skipped, so no edit
/// changes a line-start state (the typical-case scenario).
fn scattered_edits(
    rng: &mut Rng,
    buf: &LexedBuffer,
    sites: usize,
    round: usize,
    avoid_state_lines: bool,
) -> Vec<LineEdit> {
    let n = buf.lines.len() - 1;
    let mut picks: Vec<usize> = Vec::with_capacity(sites);
    while picks.len() < sites {
        let line = rng.below(n);
        if avoid_state_lines {
            let t = &buf.lines[line].text;
            if t.contains("/*") || t.contains("*/") {
                continue;
            }
        }
        picks.push(line);
    }
    picks.sort_unstable();
    picks.dedup();
    picks
        .into_iter()
        .map(|line| LineEdit {
            start: line,
            end: line + 1,
            replacement: vec![Line::new(replacement_text(line, round), LineTerm::Lf)],
        })
        .collect()
}

fn run_batches(
    label: &str,
    buf: &mut LexedBuffer,
    rng: &mut Rng,
    rounds: usize,
    sites: usize,
    avoid_state_lines: bool,
) {
    let mut times = Vec::new();
    let mut total = DamageReport::default();
    for round in 0..rounds {
        let edits = scattered_edits(rng, buf, sites, round, avoid_state_lines);
        let (report, d) = time(|| buf.apply_edits(&edits));
        times.push(d);
        total.sites += report.sites;
        total.replaced_lines += report.replaced_lines;
        total.relexed_lines += report.relexed_lines;
        total.reconverged_extra += report.reconverged_extra;
        if round == 0 {
            let oracle = LexedBuffer::new(&buf.reproduce());
            assert!(buf.lexed == oracle.lexed, "DIFFERENTIAL GATE FAILED ({label})");
        }
    }
    times.sort();
    let median = times[times.len() / 2];
    println!(
        "{label}:{:>10}   (median of {rounds}; {:.2} µs/site)",
        fmt_dur(median),
        median.as_secs_f64() * 1e6 / sites as f64
    );
    println!(
        "  damage/batch: {:.0} replaced + {:.0} reconvergence = {:.2}% of file",
        total.replaced_lines as f64 / rounds as f64,
        total.reconverged_extra as f64 / rounds as f64,
        100.0 * (total.relexed_lines as f64 / rounds as f64) / buf.lines.len() as f64
    );
}

fn main() {
    const N_LINES: usize = 100_000;
    const SITES: usize = 1_000;
    const ROUNDS: usize = 10;

    println!("rantlr P0 spike — line-anchored incremental lexing benchmark");
    println!("============================================================");
    println!(
        "token: {} bytes; line-state: {} bytes\n",
        std::mem::size_of::<Token>(),
        std::mem::size_of::<LineState>()
    );

    // ---- corpus + cold numbers ----
    let src = generate(N_LINES, true);
    let mb = src.len() as f64 / 1e6;
    println!("corpus: {N_LINES} lines, {mb:.2} MB, block comments every ~470 lines");

    let mut best = Duration::MAX;
    let mut buf_opt = None;
    for _ in 0..5 {
        let (b, d) = time(|| LexedBuffer::new(&src));
        best = best.min(d);
        buf_opt = Some(b);
    }
    let buf = buf_opt.unwrap();
    let n_tokens: usize = buf.lexed.iter().map(|l| l.tokens.len()).sum();
    println!(
        "cold lex (from scratch):     {:>10}   ({:.1} MB/s, {n_tokens} tokens)",
        fmt_dur(best),
        mb / best.as_secs_f64()
    );

    let (repro, d) = time(|| buf.reproduce());
    assert_eq!(repro, src, "LOSSLESS ROUNDTRIP FAILED");
    println!(
        "lossless roundtrip:          {:>10}   (byte-identical: verified)",
        fmt_dur(d)
    );

    let (sk, d) = time(|| build_skeleton(&buf));
    println!(
        "block skeleton:              {:>10}   ({} blocks, max depth {}, {} folding ranges)",
        fmt_dur(d),
        sk.blocks.len(),
        sk.max_depth,
        sk.folding_ranges().count()
    );
    drop(buf);

    // ---- typical batch: state-neutral scattered edits ----
    let mut rng = Rng::new(0xBEEF);
    let mut buf = LexedBuffer::new(&src);
    run_batches(
        "typical batch (1000 sites)  ",
        &mut buf,
        &mut rng,
        ROUNDS,
        SITES,
        true,
    );

    // ---- single-keystroke latency (same buffer, in-place fast path) ----
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

    // ---- bounded construct: open + close in one batch ----
    let mut buf2 = LexedBuffer::new(&src);
    let open_at = 50_000;
    let close_at = 50_025;
    let batch = vec![
        LineEdit {
            start: open_at,
            end: open_at,
            replacement: vec![Line::new("/* begin refactor note", LineTerm::Lf)],
        },
        LineEdit {
            start: close_at,
            end: close_at,
            replacement: vec![Line::new("end refactor note */", LineTerm::Lf)],
        },
    ];
    let (report, d) = time(|| buf2.apply_edits(&batch));
    println!(
        "bounded construct (2 sites): {:>10}   (wave stopped after {} lines — at the in-batch closer)",
        fmt_dur(d),
        report.reconverged_extra
    );

    // ---- adversarial batch: may delete comment delimiters ----
    let mut buf3 = LexedBuffer::new(&src);
    let mut rng2 = Rng::new(0xF00D);
    run_batches(
        "adversarial batch (1000)    ",
        &mut buf3,
        &mut rng2,
        ROUNDS,
        SITES,
        false,
    );
    println!(
        "  note: this language NESTS block comments, so deleting a closer legitimately\n\
         \x20 re-lexes to EOF (nothing below can close it). C-style non-nested comments\n\
         \x20 reconverge at the next `*/` — a language-design knob the tool must surface."
    );

    // ---- unbounded pathological ----
    let src2 = generate(N_LINES, false);
    let mut buf4 = LexedBuffer::new(&src2);
    let edit = LineEdit {
        start: 1_000,
        end: 1_000,
        replacement: vec![Line::new("/* runaway comment, nothing closes it", LineTerm::Lf)],
    };
    let (report, d) = time(|| buf4.apply_edits(&[edit]));
    println!(
        "unbounded pathological:      {:>10}   ({} lines to EOF relexed — inherent worst case)",
        fmt_dur(d),
        report.reconverged_extra
    );

    println!("\nall assertions passed (lossless roundtrip + differential gates).");
}
