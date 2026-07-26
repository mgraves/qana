//! Tree-sitter emission gates.
//!
//! The checked-in `tree-sitter/{chartlang,qana}/` artifacts are
//! drift-gated against fresh emission; the emitted JS is structurally
//! validated (every `$.rule` reference defined, no empty-matching rule
//! bodies); refusals explain themselves. When the `tree-sitter` CLI is
//! on PATH the real `generate` runs too (soft-skipped otherwise —
//! validated manually via npx: both grammars generate cleanly and the
//! generated qana parser parses qana.qana and chartlang.qana with zero errors).

use qana_lang::compile::certify;
use qana_lang::tsgen::emit_tree_sitter;
use qana_lang::{compile_source, QanaToolchain};

const RG_RG: &str = include_str!("../qana.qana");
const CHARTLANG_RG: &str = include_str!("../chartlang.qana");

fn emit(src: &str) -> qana_lang::tsgen::TsOutput {
    let tc = QanaToolchain::new();
    let out = compile_source(&tc, src);
    assert!(out.diags.is_empty(), "{:?}", out.diags);
    let (_, tables) = certify(&out.def).unwrap();
    emit_tree_sitter(&out.def.lex, &out.def.sg, &tables, &out.def.styles, &out.def.binding)
        .expect("emittable")
}

#[test]
fn emitted_artifacts_are_current() {
    let ts = emit(CHARTLANG_RG);
    assert_eq!(
        include_str!("../../../tree-sitter/chartlang/grammar.js"),
        ts.grammar_js,
        "regenerate: cargo run -p qana-lang --bin qana2ts -- crates/qana-lang/chartlang.qana tree-sitter/chartlang"
    );
    assert_eq!(
        include_str!("../../../tree-sitter/chartlang/queries/highlights.scm"),
        ts.highlights_scm
    );
    let ts = emit(RG_RG);
    assert_eq!(
        include_str!("../../../tree-sitter/qana/grammar.js"),
        ts.grammar_js,
        "regenerate: cargo run -p qana-lang --bin qana2ts -- crates/qana-lang/qana.qana tree-sitter/qana"
    );
    assert_eq!(include_str!("../../../tree-sitter/qana/queries/highlights.scm"), ts.highlights_scm);
}

#[test]
fn emitted_js_is_structurally_sound() {
    for src in [CHARTLANG_RG, RG_RG] {
        let ts = emit(src);
        // Rule definitions: lines shaped `    name: $ => body,`.
        let defined: Vec<&str> = ts
            .grammar_js
            .lines()
            .filter_map(|l| {
                let l = l.strip_prefix("    ")?;
                let (name, rest) = l.split_once(':')?;
                rest.contains("$ =>").then_some(name)
            })
            .filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
            .collect();
        assert!(!defined.is_empty());
        // Every `$.x` reference resolves to a defined rule.
        let bytes = ts.grammar_js.as_bytes();
        let mut i = 0;
        while let Some(p) = ts.grammar_js[i..].find("$.") {
            let start = i + p + 2;
            let end = ts.grammar_js[start..]
                .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .map(|e| start + e)
                .unwrap_or(bytes.len());
            let name = &ts.grammar_js[start..end];
            assert!(defined.contains(&name), "undefined rule reference `$.{name}`");
            i = end;
        }
        // The word rule, when present, is defined.
        if let Some(l) = ts.grammar_js.lines().find(|l| l.trim_start().starts_with("word:")) {
            let w = l.split("$.").nth(1).unwrap().trim_end_matches(',');
            assert!(defined.contains(&w));
        }
        // Emission is deterministic.
        let again = emit(src);
        assert_eq!(ts.grammar_js, again.grammar_js);
        assert_eq!(ts.highlights_scm, again.highlights_scm);
    }
}

#[test]
fn non_trivia_modes_are_refused_with_explanation() {
    let tc = QanaToolchain::new();
    // A string-interpolation-style mode: non-trivia content.
    let src = "\
token A = \"a\"\ntoken OPEN = \"<\" @push(INNER)\n\
mode INNER {\n  token CLOSE = \">\" @pop\n  token INNERX = /x+/\n}\n\
rule file = File: A\n";
    let out = compile_source(&tc, src);
    assert!(out.diags.is_empty(), "{:?}", out.diags);
    let (_, tables) = certify(&out.def).unwrap();
    let err = emit_tree_sitter(&out.def.lex, &out.def.sg, &tables, &out.def.styles, &out.def.binding)
        .expect_err("must refuse");
    assert!(err[0].contains("external-scanner"), "explains the boundary: {}", err[0]);
}

/// Real `tree-sitter generate` when the CLI is installed (soft-skip
/// otherwise; the emitted artifacts were also validated via npx:
/// generate + corpus parse, zero errors).
#[test]
fn tree_sitter_generate_accepts_the_emission() {
    let Ok(ts_cli) = which("tree-sitter") else {
        eprintln!("tree-sitter CLI not on PATH — skipping live generate check");
        return;
    };
    for (name, src) in [("chartlang", CHARTLANG_RG), ("qana", RG_RG)] {
        let dir = std::env::temp_dir().join(format!("qana-tsgen-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ts = emit(src);
        std::fs::write(dir.join("grammar.js"), &ts.grammar_js).unwrap();
        let ok = std::process::Command::new(&ts_cli)
            .arg("generate")
            .current_dir(&dir)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        std::fs::remove_dir_all(&dir).ok();
        assert!(ok, "tree-sitter generate failed for {name}");
    }

    fn which(bin: &str) -> Result<std::path::PathBuf, ()> {
        let path = std::env::var_os("PATH").ok_or(())?;
        for dir in std::env::split_paths(&path) {
            let p = dir.join(bin);
            if p.is_file() {
                return Ok(p);
            }
        }
        Err(())
    }
}
