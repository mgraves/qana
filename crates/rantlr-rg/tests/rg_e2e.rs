//! P5 gates.
//!
//! 1. SELF-HOSTING FIXED POINT: `rg.rg` (the .rg grammar written in .rg),
//!    parsed by the bootstrap and compiled, reproduces the bootstrap
//!    exactly — and the compiled pipeline re-parses `rg.rg` into the
//!    same tree the bootstrap produced.
//! 2. TEXT ≡ CODE: `chartlang.rg` compiles to the demo grammar — values,
//!    tables, styles, outline, binding — and both pipelines parse the
//!    corpus into semantically identical trees.
//! 3. The envelope refuses BY SPAN: diagnostics point at the offending
//!    construct in the grammar file, carrying the tool's own witnesses
//!    and counterexamples.

use rantlr_engine::{IncSession, Line, LineEdit, LineTerm};
use rantlr_grammar::demo::{demo_grammar, demo_syn_grammar};
use rantlr_grammar::green::semantic_eq;
use rantlr_grammar::{build_lr, CompiledLexer};
use rantlr_rg::compile::{
    certify, dump_binding, dump_lex, dump_outline, dump_styles, dump_syn, dump_tables,
};
use rantlr_rg::{
    compile_source, rg_binding_config, rg_outline_config, rg_styles, RgToolchain,
};
use rantlr_sem::{demo_binding_config, SemDb};
use rantlr_services::demo_glue::{demo_outline_config, demo_styles};

const RG_RG: &str = include_str!("../rg.rg");
const CHARTLANG_RG: &str = include_str!("../chartlang.rg");

#[test]
fn self_host_fixed_point() {
    let tc = RgToolchain::new();
    let out = compile_source(&tc, RG_RG);
    assert!(out.repairs.is_empty(), "rg.rg parses cleanly: {:?}", out.repairs);
    assert!(out.diags.is_empty(), "rg.rg compiles cleanly: {:?}", out.diags);

    // The grammar rg.rg describes IS the bootstrap that parsed it.
    let (boot_lex, boot_ids) = rantlr_rg::bootstrap::rg_lex_grammar();
    assert_eq!(dump_lex(&out.def.lex), dump_lex(&boot_lex), "lexical fixed point");
    assert_eq!(dump_syn(&out.def.sg), dump_syn(&tc.sg), "syntactic fixed point");
    assert_eq!(out.def.lex, boot_lex, "structural equality, not just dump equality");

    let (lexer, tables) = certify(&out.def).expect("rg.rg is in envelope");
    assert_eq!(dump_tables(&tables), dump_tables(&tc.tables), "LR tables fixed point");

    // Editor-service glue is part of the fixed point too.
    assert_eq!(
        dump_styles(&out.def.styles, out.def.lex.tokens.len()),
        dump_styles(&rg_styles(&boot_ids), boot_lex.tokens.len()),
        "styles fixed point"
    );
    assert_eq!(
        dump_outline(&out.def.outline),
        dump_outline(&rg_outline_config(&tc.sg)),
        "outline fixed point"
    );
    assert_eq!(
        dump_binding(&out.def.binding),
        dump_binding(&rg_binding_config(&tc.sg)),
        "binding fixed point"
    );
    assert!(rg_binding_config(&tc.sg).unordered, "grammar namespaces are unordered");

    // Generation 1 parses its own source into the bootstrap's tree.
    let session = IncSession::new(&lexer, &out.def.sg, &tables, RG_RG).unwrap();
    let gen1_tree = session.tree().unwrap();
    assert!(semantic_eq(gen1_tree, &out.tree), "generation-1 parse equals bootstrap parse");
}

#[test]
fn chartlang_rg_matches_demo() {
    let tc = RgToolchain::new();
    let out = compile_source(&tc, CHARTLANG_RG);
    assert!(out.repairs.is_empty(), "chartlang.rg parses cleanly: {:?}", out.repairs);
    assert!(out.diags.is_empty(), "chartlang.rg compiles cleanly: {:?}", out.diags);

    let (demo_lex, demo_ids) = demo_grammar();
    let demo_lexer = CompiledLexer::build(&demo_lex).unwrap();
    let demo_sg = demo_syn_grammar(&demo_ids, &demo_lexer.vocab);
    let demo_tables = build_lr(&demo_sg);

    assert_eq!(dump_lex(&out.def.lex), dump_lex(&demo_lex), "lexical: text ≡ code");
    assert_eq!(out.def.lex, demo_lex, "lexical structural equality");
    assert_eq!(dump_syn(&out.def.sg), dump_syn(&demo_sg), "syntactic: text ≡ code");
    let (lexer, tables) = certify(&out.def).expect("chartlang.rg is in envelope");
    assert_eq!(dump_tables(&tables), dump_tables(&demo_tables), "tables: text ≡ code");
    assert_eq!(
        dump_styles(&out.def.styles, out.def.lex.tokens.len()),
        dump_styles(&demo_styles(&demo_ids), demo_lex.tokens.len()),
        "styles: text ≡ code"
    );
    assert_eq!(
        dump_outline(&out.def.outline),
        dump_outline(&demo_outline_config(&demo_sg)),
        "outline: text ≡ code"
    );
    assert_eq!(
        dump_binding(&out.def.binding),
        dump_binding(&demo_binding_config(&demo_sg)),
        "binding: text ≡ code"
    );

    // Corpus differential: both pipelines parse the same programs into
    // semantically identical trees.
    let corpus = [
        "let x = 1 + 2 * 3;\n",
        "if (x) { emit(x); } else { emit(0); }\n",
        "let s = \"hi \\\" there\"; /* c /* nested */ still */ s;\n",
        "let l = [1, 2, f(3, g())];\n{ let y = x; y + 1; }\n",
        "// comment only\n",
        "let broken = ;\nlet ok = 1;\n",
    ];
    for src in corpus {
        let a = IncSession::new(&lexer, &out.def.sg, &tables, src).unwrap();
        let b = IncSession::new(&demo_lexer, &demo_sg, &demo_tables, src).unwrap();
        assert!(
            semantic_eq(a.tree().unwrap(), b.tree().unwrap()),
            "differential parse mismatch on {src:?}"
        );
        assert_eq!(a.buf.reproduce(), src, "lossless");
    }
}

#[test]
fn rg_files_are_lossless_and_incremental() {
    let tc = RgToolchain::new();
    // Losslessness of the grammar files themselves.
    for src in [RG_RG, CHARTLANG_RG] {
        let session = IncSession::new(&tc.lexer, &tc.sg, &tc.tables, src).unwrap();
        assert_eq!(session.buf.reproduce(), src);
        let mut text = String::new();
        collect(session.tree().unwrap(), &mut text);
        assert_eq!(text, src, "tree text is byte-exact");
    }
    fn collect(n: &rantlr_grammar::GreenNode, out: &mut String) {
        out.push_str(&n.text());
    }

    // Incremental editing of a grammar file ≡ batch reparse.
    let mut session = IncSession::new(&tc.lexer, &tc.sg, &tc.tables, CHARTLANG_RG).unwrap();
    let edits: &[(usize, &str)] = &[
        // Add a token declaration mid-file.
        (22, "token CARET = \"^\" @style(operator)"),
        // Add a rule alternative line (inside expr's alternatives).
        (76, "  | PowExpr: expr \"^\" expr"),
    ];
    for &(line, text) in edits {
        let outcome = session
            .edit(&tc.sg, &tc.tables, &[LineEdit {
                start: line,
                end: line,
                replacement: vec![Line::new(text, LineTerm::Lf)],
            }])
            .unwrap();
        let now = session.buf.reproduce();
        let batch = IncSession::new(&tc.lexer, &tc.sg, &tc.tables, &now).unwrap();
        assert!(
            semantic_eq(session.tree().unwrap(), batch.tree().unwrap()),
            "incremental ≡ batch after editing line {line}"
        );
        assert!(outcome.stats.reused_terms > 0, "edit reuses the untouched grammar");
    }
    // The edited grammar still compiles (new token + production live).
    let out = compile_source(&tc, &session.buf.reproduce());
    assert!(out.diags.is_empty(), "edited grammar compiles: {:?}", out.diags);
    assert!(out.def.lex.tokens.iter().any(|t| t.name == "CARET"));
    assert!((0..out.def.sg.prods.len()).any(|i| out.def.sg.prod_name(i) == "PowExpr"));
}

#[test]
fn refusals_carry_spans() {
    let tc = RgToolchain::new();

    // Unknown symbol: the diagnostic span covers exactly that name.
    let src = "language X\nrule file = File: nope\n";
    let out = compile_source(&tc, src);
    let diag = out.diags.iter().find(|d| d.msg.contains("nope")).expect("unknown-name diag");
    assert_eq!(
        &src[diag.span.0 as usize..diag.span.1 as usize],
        "nope",
        "span points at the offending name"
    );

    // Empty-match refusal maps to the token's declaration.
    let src = "token BAD = /a*/\nrule file = File:\n";
    let out = compile_source(&tc, src);
    let (_, diags) = match certify(&out.def) {
        Err(d) => ((), d),
        Ok(_) => panic!("empty-match must be refused"),
    };
    assert_eq!(&src[diags[0].span.0 as usize..diags[0].span.1 as usize], "BAD");

    // L2 refusal: unbounded mode push cycle.
    let src = "mode M {\ntoken IN = \"x\" @push(M)\n}\ntoken A = \"a\"\nrule file = File:\n";
    let out = compile_source(&tc, src);
    assert!(out.diags.is_empty(), "compiles as values: {:?}", out.diags);
    let err = certify(&out.def).expect_err("L2 must refuse");
    assert!(err[0].msg.contains("L2"), "got: {}", err[0].msg);

    // LR conflict: counterexample text + span at an involved production.
    let src = "\
token IF = \"if\"\ntoken ELSE = \"else\"\ntoken X = \"x\"\n\
rule file = File: stmt\n\
rule stmt =\n  | S: \"if\" stmt\n  | SE: \"if\" stmt \"else\" stmt\n  | SX: \"x\"\n";
    let out = compile_source(&tc, src);
    assert!(out.diags.is_empty(), "compiles as values: {:?}", out.diags);
    let err = certify(&out.def).expect_err("dangling else must be refused");
    let d = &err[0];
    assert!(d.msg.contains("shift/reduce"), "got: {}", d.msg);
    assert!(d.msg.contains("example input"), "counterexample present: {}", d.msg);
    let span_text = &src[d.span.0 as usize..d.span.1 as usize];
    assert!(
        span_text == "S" || span_text == "SE",
        "span points at an involved production label, got {span_text:?}"
    );

    // Undeclared literal in a production.
    let src = "token A = \"a\"\nrule file = File: \"zap\"\n";
    let out = compile_source(&tc, src);
    let diag = out.diags.iter().find(|d| d.msg.contains("zap")).expect("literal diag");
    assert_eq!(&src[diag.span.0 as usize..diag.span.1 as usize], "\"zap\"");

    // Unknown style class lists the legend.
    let src = "token A = \"a\" @style(sparkly)\nrule file = File:\n";
    let out = compile_source(&tc, src);
    assert!(out.diags.iter().any(|d| d.msg.contains("sparkly") && d.msg.contains("keyword")));
}

#[test]
fn rg_navigation_is_unordered() {
    let tc = RgToolchain::new();
    let session = IncSession::new(&tc.lexer, &tc.sg, &tc.tables, RG_RG).unwrap();
    let mut db = SemDb::new(rg_binding_config(&tc.sg));
    db.set_tree("rg.rg", session.tree().unwrap().clone());

    // `rule file = File: decls` — the `decls` reference appears BEFORE
    // `rule decls` is declared. Go-to-definition must resolve forward.
    let use_at = RG_RG.find("File: decls").unwrap() + "File: ".len();
    let (uri, span) = db.definition("rg.rg", use_at as u32).expect("forward ref resolves");
    assert_eq!(uri, "rg.rg");
    let target = &RG_RG[span.0 as usize..span.1 as usize];
    assert_eq!(target, "decls");
    let decl_at = RG_RG.find("rule decls").unwrap() + "rule ".len();
    assert_eq!(span.0 as usize, decl_at, "definition is the rule declaration");

    // References to a token collect every RHS mention.
    let name_decl = RG_RG.find("token NAME").unwrap() + "token ".len();
    let (refs, _) = db.references("rg.rg", name_decl as u32).expect("token refs");
    assert!(refs.len() > 10, "NAME is referenced widely, got {}", refs.len());

    // Completion sees every rule/token name regardless of position.
    let names = db.names_in_scope("rg.rg", 0);
    for expect in ["file", "decls", "sym", "NAME", "STRING"] {
        assert!(names.iter().any(|n| n == expect), "missing {expect}");
    }

    // No unresolved references in rg.rg (every name is declared).
    assert!(db.unresolved("rg.rg").is_empty());
}
