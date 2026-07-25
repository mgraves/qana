//! Gates for the module tier (v0): exports, imports, and visibility as
//! GRAMMAR-AUTHOR DATA (`@export`/`@import`), enforced by one generic
//! engine. Declaring either form switches cross-file resolution to
//! Rust-flavored strict semantics: private by default, importable only
//! when exported, cross-file only through imports. A grammar declaring
//! neither keeps the open world — gated below.
//!
//! The grammar under test IS the committed example
//! (examples/modules/modlang.rg), so the example drift-gates itself.

use rantlr_engine::{IncSession, Line, LineEdit};
use rantlr_rg::compile::certify;
use rantlr_rg::{compile_source, RgToolchain};
use rantlr_sem::SemDb;

const MODLANG: &str = include_str!("../../../examples/modules/modlang.rg");
const LIB: &str = include_str!("../../../examples/modules/lib.ml");
const APP: &str = include_str!("../../../examples/modules/app.ml");

struct World {
    lexer: rantlr_grammar::CompiledLexer,
    sg: rantlr_grammar::SynGrammar,
    tables: rantlr_grammar::LrTables,
    def: rantlr_rg::compile::LangDef,
}

fn world() -> World {
    let tc = RgToolchain::new();
    let out = compile_source(&tc, MODLANG);
    assert!(out.diags.is_empty(), "modlang compiles: {:?}", out.diags);
    assert!(out.def.binding.module_tier(), "modlang declares the module tier");
    let (lexer, tables) = certify(&out.def).expect("in envelope");
    World { lexer, sg: out.def.sg.clone(), tables, def: out.def }
}

fn open<'w>(w: &'w World, db: &mut SemDb, uri: &str, text: &str) -> IncSession<'w> {
    let s = IncSession::new(&w.lexer, &w.sg, &w.tables, text).unwrap();
    db.set_tree(uri, s.tree().unwrap().clone());
    s
}

/// The committed example: lib exports `scale` and `base`, keeps
/// `secret` private; app imports (one aliased) and everything resolves,
/// type-checks, and navigates through the import chain.
#[test]
fn the_example_world_resolves_types_and_navigates() {
    let w = world();
    let mut db = SemDb::new(w.def.binding.clone());
    db.set_types(w.def.types.clone());
    open(&w, &mut db, "lib.ml", LIB);
    open(&w, &mut db, "app.ml", APP);

    assert!(db.unresolved("app.ml").is_empty(), "{:?}", db.unresolved("app.ml"));
    assert!(db.not_exported("app.ml").is_empty());
    let r = db.types("app.ml");
    assert!(r.diags.is_empty(), "{:?}", r.diags);
    let width = r
        .def_types
        .iter()
        .find(|((s, e), _)| &APP[*s as usize..*e as usize] == "width")
        .expect("width typed");
    assert_eq!(r.atoms[width.1 as usize], "Num", "types flow through the import chain");

    // Navigation: the import target jumps INTO lib.ml's export.
    let use_scale = APP.find("use scale").unwrap() + "use ".len();
    let (duri, dspan) = db.definition("app.ml", use_scale as u32).expect("import resolves");
    assert_eq!(duri, "lib.ml");
    assert_eq!(&LIB[dspan.0 as usize..dspan.1 as usize], "scale");
    // A local use of the alias resolves to the LOCAL import def first.
    let start_use = APP.rfind("start").unwrap();
    let (duri, dspan) = db.definition("app.ml", start_use as u32).expect("alias resolves");
    assert_eq!(duri, "app.ml");
    assert_eq!(&APP[dspan.0 as usize..dspan.1 as usize], "start", "alias is the local binding");

    // The exported flag is data: lib's symbols carry it.
    let syms = db.symbols("lib.ml");
    let vis: Vec<(String, bool)> = syms
        .defs
        .iter()
        .filter(|d| d.top_level)
        .map(|d| (d.name.clone(), d.exported))
        .collect();
    assert!(vis.contains(&("scale".to_string(), true)));
    assert!(vis.contains(&("secret".to_string(), false)));
}

/// Visibility is enforced with a DEDICATED diagnostic: importing a
/// private name says "exists but not exported" (not "cannot find"),
/// a typo stays "cannot find", and plain refs never cross files.
#[test]
fn strict_semantics_private_typo_and_no_ambient() {
    let w = world();
    let mut db = SemDb::new(w.def.binding.clone());
    db.set_types(w.def.types.clone());
    open(&w, &mut db, "lib.ml", LIB);
    let app2 = "use secret;\nuse nothere;\nlet ambient = base + 1;\n";
    open(&w, &mut db, "app.ml", app2);

    let hidden = db.not_exported("app.ml");
    assert_eq!(hidden.len(), 1, "{hidden:?}");
    assert_eq!(hidden[0].0, "secret");
    let span = hidden[0].1;
    assert_eq!(&app2[span.0 as usize..span.1 as usize], "secret", "diag on the import name");

    let unresolved: Vec<String> = db.unresolved("app.ml").into_iter().map(|(n, _)| n).collect();
    assert!(unresolved.contains(&"nothere".to_string()), "typo is 'cannot find': {unresolved:?}");
    assert!(
        unresolved.contains(&"base".to_string()),
        "tier on ⇒ no ambient cross-file names — `base` needs an import: {unresolved:?}"
    );
    assert!(!unresolved.contains(&"secret".to_string()), "private ≠ unresolved");
}

/// `pub` is an incrementality contract: editing a PRIVATE def's body in
/// lib does not re-resolve app at all (its foreign fingerprint is built
/// from the EXPORT surface); removing a `pub` does — and app's import
/// flips to the access error.
#[test]
fn private_edits_do_not_invalidate_dependents() {
    let w = world();
    let mut db = SemDb::new(w.def.binding.clone());
    db.set_types(w.def.types.clone());
    let mut lib = open(&w, &mut db, "lib.ml", LIB);
    open(&w, &mut db, "app.ml", APP);
    assert!(db.unresolved("app.ml").is_empty());
    let _ = db.types("app.ml");

    // Private body edit: secret = 32 → 99.
    let li = lib.buf.lines.iter().position(|l| l.text.contains("let secret")).unwrap();
    let term = lib.buf.lines[li].term;
    lib.edit(&w.sg, &w.tables, &[LineEdit {
        start: li,
        end: li + 1,
        replacement: vec![Line::new("let secret = 99;", term)],
    }])
    .unwrap();
    db.set_tree("lib.ml", lib.tree().unwrap().clone());
    let before = db.stats.item_resolves_computed;
    assert!(db.unresolved("app.ml").is_empty());
    assert!(db.not_exported("app.ml").is_empty());
    let delta = db.stats.item_resolves_computed - before;
    assert_eq!(delta, 0, "private edit must not re-resolve the dependent (got {delta})");

    // Export-surface edit: scale loses `pub`.
    let li = lib.buf.lines.iter().position(|l| l.text.contains("pub fn scale")).unwrap();
    let term = lib.buf.lines[li].term;
    lib.edit(&w.sg, &w.tables, &[LineEdit {
        start: li,
        end: li + 1,
        replacement: vec![Line::new("fn scale(factor: Num) -> Num {", term)],
    }])
    .unwrap();
    db.set_tree("lib.ml", lib.tree().unwrap().clone());
    let hidden = db.not_exported("app.ml");
    assert_eq!(hidden.len(), 1, "un-exporting flips the dependent's import: {hidden:?}");
    assert_eq!(hidden[0].0, "scale");
}

/// No declarations, no tier: a grammar without @export/@import keeps
/// the open world — plain refs resolve ambiently across files.
#[test]
fn open_world_without_declarations() {
    const OPEN: &str = r#"
language Open

token WS    = /\s+/ @trivia
token NUMBER = /\d+/ @style(number)
token IDENT = /[\a_][\w_]*/ @specialize @style(variable)
token EQ    = "=" @style(operator)
token SEMI  = ";" @style(punctuation)
token PLUS  = "+" @style(operator)

keywords IDENT = let

prec left "+"

start file

rule file = File: stmts @scope(unordered)
rule stmts = stmt*
rule stmt = LetStmt: "let" name:IDENT "=" e:expr ";" @def(name) @type(def, e)
rule expr =
  | AddExpr: expr "+" expr @type(sig, Num, Num, Num)
  | NumLit:  NUMBER @type(Num)
  | NameRef: name:IDENT @ref(name) @type(ref)
"#;
    let tc = RgToolchain::new();
    let out = compile_source(&tc, OPEN);
    assert!(out.diags.is_empty(), "{:?}", out.diags);
    assert!(!out.def.binding.module_tier());
    let (lexer, tables) = certify(&out.def).unwrap();
    let mut db = SemDb::new(out.def.binding.clone());
    db.set_types(out.def.types.clone());
    let a = IncSession::new(&lexer, &out.def.sg, &tables, "let shared = 1;\n").unwrap();
    db.set_tree("a", a.tree().unwrap().clone());
    let b = IncSession::new(&lexer, &out.def.sg, &tables, "let use_it = shared + 1;\n").unwrap();
    db.set_tree("b", b.tree().unwrap().clone());
    assert!(db.unresolved("b").is_empty(), "ambient cross-file resolution without the tier");
    assert!(db.not_exported("b").is_empty());
}

/// The new forms carry compile-time cross-checks.
#[test]
fn module_forms_are_statically_checked() {
    let tc = RgToolchain::new();
    let refuse = |src: &str, expect: &str| {
        let out = compile_source(&tc, src);
        assert!(
            out.diags.iter().any(|d| d.msg.contains(expect)),
            "expected `{expect}`: {:?}",
            out.diags
        );
    };
    refuse(
        &MODLANG.replace("@def(name) @export @type(def, e)", "@export @type(def, e)"),
        "requires @def",
    );
    refuse(
        &MODLANG.replace("@import(name)", "@import(nolabel)"),
        "no symbol labeled `nolabel`",
    );
    refuse(
        &MODLANG.replace(
            "@def(name) @import(target)",
            "@def(name) @ref(target) @import(target)",
        ),
        "at most one reference",
    );
}
