//! Emit a tree-sitter grammar from a `.qana` grammar file:
//!   cargo run -p qana-lang --bin qana2ts -- <grammar.qana> <outdir>
//!
//! Writes `<outdir>/grammar.js` and `<outdir>/queries/highlights.scm`.
//! The grammar is envelope-certified first — out-of-envelope grammars
//! emit nothing but their refusal diagnostics.

use qana_lang::compile::certify;
use qana_lang::tsgen::emit_tree_sitter;
use qana_lang::{compile_source, QanaToolchain};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, src_path, outdir] = args.as_slice() else {
        eprintln!("usage: qana2ts <grammar.qana> <outdir>");
        std::process::exit(2);
    };
    let src = std::fs::read_to_string(src_path).unwrap_or_else(|e| {
        eprintln!("qana2ts: cannot read {src_path}: {e}");
        std::process::exit(2);
    });
    let tc = QanaToolchain::new();
    let out = compile_source(&tc, &src);
    if !out.repairs.is_empty() || !out.diags.is_empty() {
        for d in &out.diags {
            eprintln!("qana2ts: {}..{}: {}", d.span.0, d.span.1, d.msg);
        }
        if !out.repairs.is_empty() {
            eprintln!("qana2ts: {} parse repair(s) — fix the grammar first", out.repairs.len());
        }
        std::process::exit(1);
    }
    let (_lexer, tables) = match certify(&out.def) {
        Ok(x) => x,
        Err(diags) => {
            for d in diags {
                eprintln!("qana2ts: {}..{}: {}", d.span.0, d.span.1, d.msg);
            }
            std::process::exit(1);
        }
    };
    let ts = match emit_tree_sitter(&out.def.lex, &out.def.sg, &tables, &out.def.styles, &out.def.binding)
    {
        Ok(ts) => ts,
        Err(errors) => {
            for e in errors {
                eprintln!("qana2ts: {e}");
            }
            std::process::exit(1);
        }
    };
    for w in &ts.warnings {
        eprintln!("qana2ts: note: {w}");
    }
    let out_path = std::path::Path::new(outdir);
    std::fs::create_dir_all(out_path.join("queries")).expect("create outdir");
    std::fs::write(out_path.join("grammar.js"), &ts.grammar_js).expect("write grammar.js");
    std::fs::write(out_path.join("queries/highlights.scm"), &ts.highlights_scm)
        .expect("write highlights.scm");
    println!(
        "qana2ts: wrote {}/grammar.js and {}/queries/highlights.scm",
        outdir, outdir
    );
}
