//! P1 benchmark: the GENERATED lexer on the same scenarios as P0's
//! hand-written lexer (see crates/qana-lex/src/bin/bench.rs), so the
//! two are directly comparable.

use qana_engine::*;
use qana_grammar::demo::{demo_grammar, demo_syn_grammar};
use qana_grammar::{build_lr, CompiledLexer, GreenChild, GreenNode};
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

    println!("qana P1 — GENERATED lexer on the P0 scenarios");
    println!("===============================================");

    let (g, ids) = demo_grammar();
    let (lexer, build_time) = time(|| CompiledLexer::build(&g).expect("demo grammar in envelope"));
    println!(
        "lex compile + lints:         {:>10}   (stack bound {}, line-state space {}, DFA states {:?})",
        fmt_dur(build_time),
        lexer.report.stack_bound,
        lexer.report.line_state_space,
        lexer.report.dfa_states
    );
    let sg = demo_syn_grammar(&ids, &lexer.vocab);
    let (tables, lr_time) = time(|| build_lr(&sg));
    assert!(tables.conflicts.is_empty());
    println!(
        "LR(1) tables (canonical):    {:>10}   ({} states, {} prods, {} S/R resolved by precedence)",
        fmt_dur(lr_time),
        tables.n_states,
        sg.prods.len(),
        tables.resolved_by_prec
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

    // ---- Corpus-scale parse + trees. L4 balanced sequences made trees
    // log-depth, so this runs on the ordinary stack — the big-stack
    // thread P1 increment 3 needed is retired.
    corpus_bench(&lexer, &sg, &tables);

    println!("\nall assertions passed.");
}

fn corpus_bench(
    lexer: &CompiledLexer,
    sg: &qana_grammar::SynGrammar,
    tables: &qana_grammar::LrTables,
) {
    const N_LINES: usize = 100_000;
    // Statement-only corpus (the demo SYNTAX grammar's language).
    let mut src2 = String::with_capacity(N_LINES * 32);
    let mut li = 0usize;
    while li < N_LINES {
        match li % 6 {
            0 => src2.push_str(&format!("let v{li} = {li} + {li} * 2;\n")),
            1 => src2.push_str(&format!("if (v{li}) {{ emit(v{li}, 1); }} else {{ skip(); }}\n")),
            2 => src2.push_str(&format!("emit(\"s{li}\", [1, 2.5]); // note {li}\n")),
            3 => {
                if li % 470 == 3 {
                    src2.push_str("/* corpus note:\n   spans lines\n   ends here */\n");
                    li += 3;
                    continue;
                }
                src2.push_str(&format!("let w{li} = (v{li} - 1) / 2;\n"));
            }
            4 => src2.push_str(&format!("{{ let t{li} = 3; }}\n")),
            _ => src2.push_str(&format!("done(v{li});\n")),
        }
        li += 1;
    }
    let mb2 = src2.len() as f64 / 1e6;
    let sbuf = LexedBuffer::new(lexer, &src2);
    let (all, d_harvest) = time(|| full_tokens(lexer, &sbuf));
    let n_terms = all.iter().filter(|t| !t.trivia).count();
    let (green, d_parse) =
        time(|| qana_grammar::batch_parse_green(sg, tables, &all).expect("corpus parses"));
    qana_grammar::green::check_balance(&green).expect("balanced");
    let d_tree = std::time::Duration::ZERO; // tree built during parse now
    let (txt, d_text) = time(|| green.text());
    assert_eq!(txt, src2, "GREEN TREE LOSSLESSNESS FAILED");
    fn count(n: &GreenNode) -> (usize, usize) {
        let mut nodes = 1;
        let mut toks = 0;
        for c in &n.children {
            match c {
                GreenChild::Node(m) => {
                    let (a, b) = count(m);
                    nodes += a;
                    toks += b;
                }
                GreenChild::Token(_) => toks += 1,
            }
        }
        (nodes, toks)
    }
    let (n_nodes, n_toks) = count(&green);
    let _ = d_tree;
    println!(
        "batch parse→tree (corpus):   {:>10}   ({:.2} MB, {} terminals, {:.1} MB/s, balanced tree built inline)",
        fmt_dur(d_parse),
        mb2,
        n_terms,
        mb2 / d_parse.as_secs_f64()
    );
    println!(
        "tree stats:                             {} nodes, {} tokens incl. trivia; harvest {}",
        n_nodes,
        n_toks,
        fmt_dur(d_harvest)
    );
    println!(
        "green text() roundtrip:      {:>10}   (byte-identical: verified)",
        fmt_dur(d_text)
    );

    // ---- P2: Wagner incremental parsing on the same corpus ----
    let mut session = IncSession::new(lexer, sg, tables, &src2).expect("corpus parses");
    let mut rng = Rng::new(0x1AC5E);
    let mut times = Vec::new();
    let mut worst_reuse: f64 = 1.0;
    let mut total_splices = 0u64;
    for k in 0..500usize {
        let line = 1 + rng.below(session.buf.lines.len().saturating_sub(3));
        // Keep edits valid: replace with a complete statement line.
        let edit = LineEdit {
            start: line,
            end: line + 1,
            replacement: vec![Line::new(format!("let inc{k} = {k} + 1;"), LineTerm::Lf)],
        };
        let t0 = Instant::now();
        match session.edit(sg, tables, &[edit]) {
            Ok(out) => {
                times.push(t0.elapsed());
                worst_reuse = worst_reuse.min(out.stats.reuse_fraction());
                total_splices += out.stats.splices as u64;
            }
            Err(_) => {
                // Replaced a line that was part of a multi-line construct
                // (block comment interior/opener): repair next round.
                let _ = session.edit(sg, tables, &[]);
            }
        }
    }
    times.sort();
    if !times.is_empty() {
        println!(
            "incremental reparse (P2):    {:>10}   (median of {}; p99 {}; worst reuse {:.2}%; avg {} splices/edit)",
            fmt_dur(times[times.len() / 2]),
            times.len(),
            fmt_dur(times[times.len() * 99 / 100]),
            worst_reuse * 100.0,
            total_splices / times.len().max(1) as u64
        );
    }

    // Near-EOF edit: tiny suffix ⇒ the spine tax vanishes — this is the
    // per-edit cost the L4 balanced-list increment generalizes to
    // arbitrary positions (O(damage + log n) instead of O(suffix)).
    let mut eof_times = Vec::new();
    let tail = session.buf.lines.len() - 3;
    for k in 0..200usize {
        let edit = LineEdit {
            start: tail,
            end: tail + 1,
            replacement: vec![Line::new(format!("let tail{k} = {k};"), LineTerm::Lf)],
        };
        let t0 = Instant::now();
        session.edit(sg, tables, &[edit]).expect("valid");
        eof_times.push(t0.elapsed());
    }
    eof_times.sort();
    println!(
        "incremental, near-EOF edit:  {:>10}   (median of 200 — the spine-free cost)",
        fmt_dur(eof_times[eof_times.len() / 2])
    );

    // Differential spot-check at scale, once.
    let all_now = full_tokens(lexer, &session.buf);
    let batch_now = qana_grammar::batch_parse_green(sg, tables, &all_now).expect("parses");
    assert!(
        **session.tree().expect("valid") == *batch_now,
        "P2 DIFFERENTIAL GATE FAILED AT SCALE"
    );
    println!("P2 gate at scale:                verified   (incremental tree == batch tree, 100k lines)");
}
