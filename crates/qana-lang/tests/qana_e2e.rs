//! P5 gates.
//!
//! 1. SELF-HOSTING FIXED POINT: `qana.qana` (the .qana grammar written in .qana),
//!    parsed by the bootstrap and compiled, reproduces the bootstrap
//!    exactly — and the compiled pipeline re-parses `qana.qana` into the
//!    same tree the bootstrap produced.
//! 2. TEXT ≡ CODE: `chartlang.qana` compiles to the demo grammar — values,
//!    tables, styles, outline, binding — and both pipelines parse the
//!    corpus into semantically identical trees.
//! 3. The envelope refuses BY SPAN: diagnostics point at the offending
//!    construct in the grammar file, carrying the tool's own witnesses
//!    and counterexamples.

use qana_engine::{IncSession, Line, LineEdit, LineTerm};
use qana_grammar::demo::{demo_grammar, demo_syn_grammar};
use qana_grammar::green::semantic_eq;
use qana_grammar::{build_lr, CompiledLexer};
use qana_lang::compile::{
    certify, dump_binding, dump_lex, dump_outline, dump_styles, dump_syn, dump_tables,
};
use qana_lang::{
    compile_source, qana_binding_config, qana_outline_config, qana_styles, QanaToolchain,
};
use qana_sem::{demo_binding_config, SemDb};
use qana_services::demo_glue::{demo_outline_config, demo_styles};

const RG_RG: &str = include_str!("../qana.qana");
const CHARTLANG_RG: &str = include_str!("../chartlang.qana");

#[test]
fn self_host_fixed_point() {
    let tc = QanaToolchain::new();
    let out = compile_source(&tc, RG_RG);
    assert!(out.repairs.is_empty(), "qana.qana parses cleanly: {:?}", out.repairs);
    assert!(out.diags.is_empty(), "qana.qana compiles cleanly: {:?}", out.diags);

    // The grammar qana.qana describes IS the bootstrap that parsed it.
    let (boot_lex, boot_ids) = qana_lang::bootstrap::qana_lex_grammar();
    assert_eq!(dump_lex(&out.def.lex), dump_lex(&boot_lex), "lexical fixed point");
    assert_eq!(dump_syn(&out.def.sg), dump_syn(&tc.sg), "syntactic fixed point");
    assert_eq!(out.def.lex, boot_lex, "structural equality, not just dump equality");

    let (lexer, tables) = certify(&out.def).expect("qana.qana is in envelope");
    assert_eq!(dump_tables(&tables), dump_tables(&tc.tables), "LR tables fixed point");

    // Editor-service glue is part of the fixed point too.
    assert_eq!(
        dump_styles(&out.def.styles, out.def.lex.tokens.len()),
        dump_styles(&qana_styles(&boot_ids), boot_lex.tokens.len()),
        "styles fixed point"
    );
    assert_eq!(
        dump_outline(&out.def.outline),
        dump_outline(&qana_outline_config(&tc.sg)),
        "outline fixed point"
    );
    assert_eq!(
        dump_binding(&out.def.binding),
        dump_binding(&qana_binding_config(&tc.sg)),
        "binding fixed point"
    );
    assert!(qana_binding_config(&tc.sg).unordered, "grammar namespaces are unordered");

    // Generation 1 parses its own source into the bootstrap's tree.
    let session = IncSession::new(&lexer, &out.def.sg, &tables, RG_RG).unwrap();
    let gen1_tree = session.tree().unwrap();
    assert!(semantic_eq(gen1_tree, &out.tree), "generation-1 parse equals bootstrap parse");
}

#[test]
fn chartlang_qana_matches_demo() {
    let tc = QanaToolchain::new();
    let out = compile_source(&tc, CHARTLANG_RG);
    assert!(out.repairs.is_empty(), "chartlang.qana parses cleanly: {:?}", out.repairs);
    assert!(out.diags.is_empty(), "chartlang.qana compiles cleanly: {:?}", out.diags);

    let (demo_lex, demo_ids) = demo_grammar();
    let demo_lexer = CompiledLexer::build(&demo_lex).unwrap();
    let demo_sg = demo_syn_grammar(&demo_ids, &demo_lexer.vocab);
    let demo_tables = build_lr(&demo_sg);

    assert_eq!(dump_lex(&out.def.lex), dump_lex(&demo_lex), "lexical: text ≡ code");
    assert_eq!(out.def.lex, demo_lex, "lexical structural equality");
    assert_eq!(dump_syn(&out.def.sg), dump_syn(&demo_sg), "syntactic: text ≡ code");
    let (lexer, tables) = certify(&out.def).expect("chartlang.qana is in envelope");
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
fn qana_files_are_lossless_and_incremental() {
    let tc = QanaToolchain::new();
    // Losslessness of the grammar files themselves.
    for src in [RG_RG, CHARTLANG_RG] {
        let session = IncSession::new(&tc.lexer, &tc.sg, &tc.tables, src).unwrap();
        assert_eq!(session.buf.reproduce(), src);
        let mut text = String::new();
        collect(session.tree().unwrap(), &mut text);
        assert_eq!(text, src, "tree text is byte-exact");
    }
    fn collect(n: &qana_grammar::GreenNode, out: &mut String) {
        out.push_str(&n.text());
    }

    // Incremental editing of a grammar file ≡ batch reparse.
    let mut session = IncSession::new(&tc.lexer, &tc.sg, &tc.tables, CHARTLANG_RG).unwrap();
    let line_of = |needle: &str| {
        CHARTLANG_RG
            .lines()
            .position(|l| l.trim_start().starts_with(needle))
            .unwrap_or_else(|| panic!("line starting with {needle:?}"))
    };
    let edits: &[(usize, &str)] = &[
        // Add a token declaration mid-file.
        (line_of("token PUNCT"), "token CARET = \"^\" @style(operator)"),
        // Add a rule alternative line (inside expr's alternatives; +1
        // for the token line inserted above it by the first edit).
        (line_of("| CallExpr") + 1, "  | PowExpr: expr \"^\" expr"),
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
    let tc = QanaToolchain::new();

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

/// EBNF sugar desugars to EXACTLY the grammar a careful author writes
/// by hand with the documented names — values and tables identical.
#[test]
fn sugar_equals_handwritten_desugaring() {
    let tc = QanaToolchain::new();
    let check = |sugared: &str, explicit: &str, l4_nts: &[&str]| {
        let a = compile_source(&tc, sugared);
        let b = compile_source(&tc, explicit);
        assert!(a.diags.is_empty(), "{:?}", a.diags);
        assert!(b.diags.is_empty(), "{:?}", b.diags);
        assert_eq!(
            qana_lang::compile::dump_syn(&a.def.sg),
            qana_lang::compile::dump_syn(&b.def.sg),
            "sugar ≡ hand-written desugaring"
        );
        let (_, ta) = certify(&a.def).unwrap();
        let (_, tb) = certify(&b.def).unwrap();
        assert_eq!(dump_tables(&ta), dump_tables(&tb), "identical LR tables");
        // Every generated repetition is an L4 balanced list.
        for nt in l4_nts {
            let id = a.def.sg.nt_names.iter().position(|n| n == nt).unwrap() as u16;
            assert!(ta.lists.contains_key(&id), "`{nt}` must be L4-detected");
        }
    };

    // Inline postfix: `item*` (used TWICE — one shared helper), `SEMI?`.
    check(
        "token A = \"a\"\ntoken B = \"b\"\ntoken SEMI = \";\"\n\
         rule file = File: item* SEMI? B item*\n\
         rule item = Item: A\n",
        "token A = \"a\"\ntoken B = \"b\"\ntoken SEMI = \";\"\n\
         rule file = File: item_star semi_opt B item_star\n\
         rule item = Item: A\n\
         rule item_star = | ItemStarEmpty: | ItemStarMore: item_star item\n\
         rule semi_opt = | SemiOptNone: | SemiOptSome: SEMI\n",
        &["item_star"],
    );

    // Rule-level forms: `b+`, `item+ % ","`, `item* % ";"`.
    check(
        "token A = \"a\"\ntoken B = \"b\"\ntoken SEMI = \";\"\ntoken COMMA = \",\"\n\
         rule file = File: b_plus SEMI xs SEMI ys\n\
         rule b_plus = b+\n\
         rule b = BOne: B\n\
         rule item = Item: A\n\
         rule xs = item+ % \",\"\n\
         rule ys = item* % \";\"\n",
        "token A = \"a\"\ntoken B = \"b\"\ntoken SEMI = \";\"\ntoken COMMA = \",\"\n\
         rule file = File: b_plus SEMI xs SEMI ys\n\
         rule b_plus = | BPlusFirst: b | BPlusMore: b_plus b\n\
         rule b = BOne: B\n\
         rule item = Item: A\n\
         rule xs = | XsFirst: item | XsMore: xs \",\" item\n\
         rule ys = | YsNone: | YsSome: ys_ne\n\
         rule ys_ne = | YsNeFirst: item | YsNeMore: ys_ne \";\" item\n",
        &["b_plus", "xs", "ys_ne"],
    );
}

/// Sugar refusals: token repetition (with the wrap hint), rule
/// separators, and generated-name collisions — all span-carrying.
#[test]
fn sugar_refusals_explain() {
    let tc = QanaToolchain::new();

    // Token element under `*` (inline and rule-level).
    let out = compile_source(&tc, "token A = \"a\"\nrule file = File: A*\n");
    let diag = out.diags.iter().find(|d| d.msg.contains("wrap the token")).expect("wrap hint");
    assert_eq!(&"token A = \"a\"\nrule file = File: A*\n"[diag.span.0 as usize..diag.span.1 as usize], "A");
    let out = compile_source(&tc, "token A = \"a\"\nrule file = File: xs\nrule xs = A+\n");
    assert!(out.diags.iter().any(|d| d.msg.contains("wrap the token")));

    // `?` on a token is fine (optional terminal).
    let out = compile_source(&tc, "token A = \"a\"\nrule file = File: A?\n");
    assert!(out.diags.is_empty(), "{:?}", out.diags);
    assert!(certify(&out.def).is_ok());

    // Rule as separator.
    let out = compile_source(
        &tc,
        "token A = \"a\"\nrule file = File: xs\nrule item = Item: A\nrule xs = item+ % item\n",
    );
    assert!(out.diags.iter().any(|d| d.msg.contains("separators must be tokens")));

    // Generated name collides with an explicit declaration.
    let out = compile_source(
        &tc,
        "token A = \"a\"\nrule file = File: item*\nrule item = Item: A\nrule item_star = X: A\n",
    );
    assert!(out.diags.iter().any(|d| d.msg.contains("collides")), "{:?}", out.diags);
}

#[test]
fn qana_navigation_is_unordered() {
    let tc = QanaToolchain::new();
    let session = IncSession::new(&tc.lexer, &tc.sg, &tc.tables, RG_RG).unwrap();
    let mut db = SemDb::new(qana_binding_config(&tc.sg));
    db.set_tree("qana.qana", session.tree().unwrap().clone());

    // `rule file = File: decls` — the `decls` reference appears BEFORE
    // `rule decls` is declared. Go-to-definition must resolve forward.
    let use_at = RG_RG.find("File: decls").unwrap() + "File: ".len();
    let (uri, span) = db.definition("qana.qana", use_at as u32).expect("forward ref resolves");
    assert_eq!(uri, "qana.qana");
    let target = &RG_RG[span.0 as usize..span.1 as usize];
    assert_eq!(target, "decls");
    let decl_at = RG_RG.find("rule decls").unwrap() + "rule ".len();
    assert_eq!(span.0 as usize, decl_at, "definition is the rule declaration");

    // References to a token collect every RHS mention.
    let name_decl = RG_RG.find("token NAME").unwrap() + "token ".len();
    let (refs, _) = db.references("qana.qana", name_decl as u32).expect("token refs");
    assert!(refs.len() > 10, "NAME is referenced widely, got {}", refs.len());

    // Sugar elements are references too: the `decl` in `rule decls =
    // decl*` navigates to `rule decl`.
    let elem_at = RG_RG.find("= decl*").unwrap() + "= ".len();
    let (_, span) = db.definition("qana.qana", elem_at as u32).expect("elem resolves");
    let decl_rule = RG_RG.find("rule decl =").unwrap() + "rule ".len();
    assert_eq!(span.0 as usize, decl_rule, "sugar element resolves to the rule");

    // Completion sees every rule/token name regardless of position.
    let names = db.names_in_scope("qana.qana", 0);
    for expect in ["file", "decls", "sym", "NAME", "STRING"] {
        assert!(names.iter().any(|n| n == expect), "missing {expect}");
    }

    // No unresolved references in qana.qana (every name is declared).
    assert!(db.unresolved("qana.qana").is_empty());
}

/// `@scope(unordered)` on the START rule declares the FILE's own scope
/// unordered — a declaration language, where top-level names see each
/// other regardless of order. Per-scope entries only govern ordering
/// within one item, so without lifting this to the global flag the
/// annotation on the root would silently do nothing.
#[test]
fn unordered_root_scope_makes_the_file_a_declaration_language() {
    const DECL_LANG: &str = r#"
language Decls

token WS    = /\s+/ @trivia
token IDENT = /[\a_][\w_]*/ @specialize @style(variable)
token SEMI  = ";" @style(punctuation)

keywords IDENT = use def

start unit

rule unit = Unit: items @scope(unordered)

rule items = item*

rule item =
  | Def: "def" name:IDENT ";" @def(name)
  | Use: "use" name:IDENT ";" @ref(name)
"#;
    let tc = QanaToolchain::new();
    let out = compile_source(&tc, DECL_LANG);
    assert!(out.diags.is_empty(), "grammar compiles: {:?}", out.diags);
    assert!(out.def.binding.unordered, "@scope(unordered) on the start rule lifts to the file");
    let (lexer, tables) = certify(&out.def).expect("in envelope");

    // `use later;` precedes `def later;` — a forward reference.
    let doc = "use later;\ndef later;\n";
    let session = IncSession::new(&lexer, &out.def.sg, &tables, doc).unwrap();
    let mut db = SemDb::new(out.def.binding.clone());
    db.set_tree("d", session.tree().unwrap().clone());

    let use_at = doc.find("later").unwrap() as u32;
    let (_, span) = db.definition("d", use_at).expect("forward reference resolves");
    let def_at = doc.rfind("later").unwrap() as u32;
    assert_eq!(span.0, def_at, "it resolves to the LATER declaration");
    assert!(db.unresolved("d").is_empty(), "and nothing is left unresolved");
}

/// `@precedence(tok)` is yacc's `%prec`: it overrides an alternative's
/// precedence so unary minus can bind tighter than subtraction. The
/// attribute is spelled in full because `prec` is a reserved word of the
/// surface — spelled `@prec` it lexes as a keyword and never reaches the
/// attribute name, which made the feature unreachable.
#[test]
fn precedence_override_binds_an_alternative_tighter() {
    const PREC: &str = r#"
language Prec

token WS    = /\s+/ @trivia
token NUM   = /\d+/ @style(number)
token MINUS = "-" @style(operator)
token STAR  = "*" @style(operator)

prec left "-"
prec left "*"

start expr

rule expr =
  | Sub: expr "-" expr
  | Mul: expr "*" expr
  | Neg: "-" expr @precedence("*")
  | Lit: NUM
"#;
    let tc = QanaToolchain::new();
    let out = compile_source(&tc, PREC);
    assert!(out.diags.is_empty(), "grammar compiles: {:?}", out.diags);
    let (lexer, tables) = certify(&out.def).expect("precedence keeps it deterministic");

    // With Neg at `*` level, `- 2 * 3` groups as `(-2) * 3`.
    let session = IncSession::new(&lexer, &out.def.sg, &tables, "- 2 * 3").unwrap();
    let root = session.tree().unwrap();
    assert_eq!(out.def.sg.prod_name(root.prod as usize), "Mul", "the product is outermost");
    let first = root.symbol_children().next().expect("left operand");
    match first {
        qana_grammar::GreenChild::Node(n) => {
            assert_eq!(out.def.sg.prod_name(n.prod as usize), "Neg", "negation binds tighter");
        }
        _ => panic!("expected a node"),
    }
}
