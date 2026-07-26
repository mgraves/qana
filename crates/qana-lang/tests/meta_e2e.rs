//! The meta tier, end to end: declared macros, binding-guided
//! substitution, fixpoint expansion, provenance — and the residues,
//! pinned. The committed examples/macrolang is the exerciser.

use qana_engine::IncSession;
use qana_lang::compile::certify;
use qana_lang::expand::expand_document;
use qana_lang::{compile_source, RgToolchain};
use qana_sem::macros::{tiles, SegKind};
use qana_sem::SemDb;

const MAC_RG: &str = include_str!("../../../examples/macrolang/mac.rg");
const DEMO: &str = include_str!("../../../examples/macrolang/demo.mac");

fn world() -> (qana_grammar::CompiledLexer, qana_lang::compile::LangDef, qana_grammar::LrTables)
{
    let tc = RgToolchain::new();
    let out = compile_source(&tc, MAC_RG);
    assert!(out.diags.is_empty(), "mac.rg compiles: {:?}", out.diags);
    let (lexer, tables) = certify(&out.def).expect("mac.rg certifies");
    (lexer, out.def, tables)
}

/// The whole loop on the committed demo: nested macros, macro-calling
/// macros, cpp-disciplined bodies — exact expected output, converging
/// in two passes, zero diagnostics, zero repairs.
#[test]
fn expansion_reaches_the_expected_fixpoint() {
    let (lexer, def, tables) = world();
    assert!(def.macros.declared(), "the tier is declared");
    let out = expand_document(&lexer, &def, &tables, DEMO, &[], 8).expect("expands");
    assert!(out.diags.is_empty(), "no diagnostics: {:?}", out.diags);
    assert_eq!(out.repairs, 0, "no pass ever parsed broken text");
    assert_eq!(out.passes, 2, "nested macros need exactly two passes");
    assert_eq!(out.substitutions, 10, "6 outer + 4 inner uses");

    // Definitions stay; uses open. Exact lines — including the parens
    // the expander adds to keep the author's grouping (`quad`'s body
    // says twice(y) + twice(y), so the second twice stays a unit).
    assert!(out.text.contains("let a = 3 + 3;"), "{}", out.text);
    assert!(out.text.contains("let b = 2 + 2 + (2 + 2);"), "{}", out.text);
    assert!(out.text.contains("let c = (a + 1) * unit;"), "{}", out.text);
    assert!(out.text.contains("let d = 4 + 4 + (4 + 4);"), "{}", out.text);
    assert!(out.text.contains("macro twice(x) => { x + x }"), "defs verbatim: {}", out.text);
}

/// The materialization promise: expansion output is an ORDINARY
/// document — it parses clean and every reference resolves. Full
/// editor intelligence inside generated text, the thing a token-based
/// expander can never give.
#[test]
fn expanded_output_is_an_ordinary_document() {
    let (lexer, def, tables) = world();
    let out = expand_document(&lexer, &def, &tables, DEMO, &[], 8).unwrap();
    let session = IncSession::new(&lexer, &def.sg, &tables, &out.text).unwrap();
    assert!(session.last_repairs.is_empty(), "output parses clean: {:?}", session.last_repairs);
    let mut db = SemDb::new(def.binding.clone());
    db.set_tree("exp", session.tree().unwrap().clone());
    assert!(db.unresolved("exp").is_empty(), "all refs resolve: {:?}", db.unresolved("exp"));

    // And expansion is IDEMPOTENT: the output has nothing left to expand.
    let again = expand_document(&lexer, &def, &tables, &out.text, &[], 8).unwrap();
    assert_eq!(again.substitutions, 0, "fixpoint reached");
    assert_eq!(again.text, out.text);
}

/// Provenance: the segments tile the output exactly, and they point
/// where they claim — an `Arg` byte maps into the use site's argument,
/// a `Body` byte into the macro definition's body.
#[test]
fn provenance_tiles_and_points_home() {
    let (lexer, def, tables) = world();
    let out = expand_document(&lexer, &def, &tables, DEMO, &[], 8).unwrap();
    assert!(tiles(&out.segs, out.text.len() as u32), "segments tile the output: {:?}", out.segs);

    // `let a = 3 + 3;` — the two 3s are Arg segments whose src is the
    // literal `3` inside `twice!(3)`; the ` + ` between them is Body,
    // mapping into twice's definition body.
    let a_out = out.text.find("let a = ").unwrap() + "let a = ".len();
    let arg_src = DEMO.find("twice!(3)").unwrap() + "twice!(".len();
    let seg_at = |off: u32| out.segs.iter().find(|s| s.out.0 <= off && off < s.out.1).unwrap();
    let first3 = seg_at(a_out as u32);
    assert_eq!(first3.kind, SegKind::Arg);
    assert_eq!(first3.src.0 as usize, arg_src, "the 3 comes from the use site");
    let plus = seg_at(a_out as u32 + 2);
    assert_eq!(plus.kind, SegKind::Body);
    let twice_body = DEMO.find("{ x + x }").unwrap();
    assert!(
        (plus.src.0 as usize) > twice_body && (plus.src.1 as usize) < twice_body + "{ x + x }".len(),
        "the + comes from twice's body: {:?}",
        plus
    );
}

/// The costs, pinned: arity mismatch diagnoses and leaves the use
/// verbatim; splicing a non-macro diagnoses; a self-recursive macro
/// hits the pass cap with a diagnostic instead of hanging — the
/// envelope's termination guarantee extends to the meta tier.
#[test]
fn diagnostics_and_termination_are_pinned() {
    let (lexer, def, tables) = world();

    let arity = "macro twice(x) => { x + x }\nlet a = twice!(1, 2);\n";
    let out = expand_document(&lexer, &def, &tables, arity, &[], 8).unwrap();
    assert_eq!(out.substitutions, 0);
    assert!(out.text.contains("twice!(1, 2)"), "left verbatim");
    assert!(
        out.diags.iter().any(|d| d.msg.contains("1 argument(s), 2 supplied")),
        "{:?}",
        out.diags
    );

    let notmac = "let f = 1;\nlet a = f!(2);\n";
    let out = expand_document(&lexer, &def, &tables, notmac, &[], 8).unwrap();
    assert!(
        out.diags.iter().any(|d| d.msg.contains("does not resolve to a macro")),
        "{:?}",
        out.diags
    );

    let loopy = "macro loopy(x) => { loopy!(x) }\nlet a = loopy!(1);\n";
    let out = expand_document(&lexer, &def, &tables, loopy, &[], 4).unwrap();
    assert!(
        out.diags.iter().any(|d| d.msg.contains("did not converge")),
        "the cap is a diagnostic, not a hang: {:?}",
        out.diags
    );
}

/// The COMMITTED materializations are drift-gated, astgen-style: the
/// checked-in .exp files must equal a fresh expansion of their
/// sources. If a demo or grammar changes, `qana expand` must be
/// re-run — this test is what makes the pair trustworthy to read.
#[test]
fn committed_materializations_are_current() {
    let (lexer, def, tables) = world();
    let out = expand_document(&lexer, &def, &tables, DEMO, &[], 8).unwrap();
    let committed = include_str!("../../../examples/macrolang/demo.exp.mac");
    assert_eq!(committed, out.text, "demo.exp.mac drifted — rerun `qana expand`");

    let c_rg = include_str!("../../../examples/c/c.rg");
    let c_demo = include_str!("../../../examples/c/demo.c");
    let tc = RgToolchain::new();
    let cdef = compile_source(&tc, c_rg);
    let (clexer, ctables) = certify(&cdef.def).unwrap();
    let cout = expand_document(&clexer, &cdef.def, &ctables, c_demo, &[], 8).unwrap();
    let ccommitted = include_str!("../../../examples/c/demo.exp.c");
    assert_eq!(ccommitted, cout.text, "demo.exp.c drifted — rerun `qana expand`");

    let sl_rg = include_str!("../../../examples/structs/structlang.rg");
    let sl_demo = include_str!("../../../examples/structs/demo.sl");
    let sdef = compile_source(&tc, sl_rg);
    let (slexer, stables) = certify(&sdef.def).unwrap();
    let sout = expand_document(&slexer, &sdef.def, &stables, sl_demo, &[], 8).unwrap();
    let scommitted = include_str!("../../../examples/structs/demo.exp.sl");
    assert_eq!(scommitted, sout.text, "demo.exp.sl drifted — rerun `qana expand`");
}

/// CROSS-FILE macros: a macro defined in a sibling expands here. The
/// body splices from the SIBLING's text — provenance names the file —
/// and the spliced output re-binds in this document's context, so a
/// body reference to the sibling's own top-level def keeps resolving
/// (cross-file, open world) after materialization.
#[test]
fn cross_file_macros_expand_with_foreign_provenance() {
    let (lexer, def, tables) = world();
    let lib = "let unit = 10;\nmacro twice(x) => { x + x }\nmacro scaled(v) => { (v) * unit }\n";
    let app = "let a = twice!(3);\nlet b = scaled!(a + 1);\nlet c = twice!(twice!(2));\n";
    let sibs = vec![("lib.mac".to_string(), lib.to_string())];
    let out = expand_document(&lexer, &def, &tables, app, &sibs, 8).expect("expands");
    assert!(out.diags.is_empty(), "{:?}", out.diags);
    assert!(out.text.contains("let a = 3 + 3;"), "{}", out.text);
    assert!(out.text.contains("let b = (a + 1) * unit;"), "{}", out.text);
    assert!(out.text.contains("let c = 2 + 2 + (2 + 2);"), "nested cross-file: {}", out.text);
    assert!(tiles(&out.segs, out.text.len() as u32));

    // Provenance: a body byte names the DEFINING file and maps into
    // the macro's body there; the arg byte stays file-local.
    let plus_at = out.text.find("3 + 3").unwrap() + 2;
    let seg = out.segs.iter().find(|s| s.out.0 <= plus_at as u32 && (plus_at as u32) < s.out.1).unwrap();
    assert_eq!(seg.kind, SegKind::Body);
    assert_eq!(seg.src_uri.as_deref(), Some("lib.mac"), "{seg:?}");
    let body_in_lib = lib.find("x + x").unwrap();
    assert!(
        (seg.src.0 as usize) >= body_in_lib && (seg.src.1 as usize) <= body_in_lib + "x + x".len(),
        "maps into lib's body: {seg:?}"
    );
    let three_at = out.text.find("3 + 3").unwrap();
    let aseg = out.segs.iter().find(|s| s.out.0 <= three_at as u32 && (three_at as u32) < s.out.1).unwrap();
    assert_eq!((aseg.kind, aseg.src_uri.as_deref()), (SegKind::Arg, None), "{aseg:?}");

    // The materialized app is an ordinary document IN ITS WORLD: with
    // the sibling present, `unit` (spliced from lib's body) resolves
    // cross-file.
    let session = IncSession::new(&lexer, &def.sg, &tables, &out.text).unwrap();
    assert!(session.last_repairs.is_empty(), "{:?}", session.last_repairs);
    let mut db = SemDb::new(def.binding.clone());
    let sib_session = IncSession::new(&lexer, &def.sg, &tables, lib).unwrap();
    db.set_tree("lib.mac", sib_session.tree().unwrap().clone());
    db.set_tree("app", session.tree().unwrap().clone());
    assert!(db.unresolved("app").is_empty(), "{:?}", db.unresolved("app"));
}

/// The C shape of the same fact: a #define in a header-style sibling
/// opens in this file's code — approximating what #include provides,
/// through the open world the binding tier already had.
#[test]
fn c_macros_cross_files_like_headers() {
    let c_rg = include_str!("../../../examples/c/c.rg");
    let tc = RgToolchain::new();
    let cdef = compile_source(&tc, c_rg);
    let (clexer, ctables) = certify(&cdef.def).unwrap();
    let header = "#define SCALE 4\n";
    let main_c = "int f(int x) { return x * SCALE; }\n";
    let sibs = vec![("defs.h".to_string(), header.to_string())];
    let out = expand_document(&clexer, &cdef.def, &ctables, main_c, &sibs, 8).unwrap();
    assert!(out.diags.is_empty(), "{:?}", out.diags);
    assert!(out.text.contains("return x * 4;"), "{}", out.text);
    let four = out.text.find("* 4;").unwrap() + 2;
    let seg = out.segs.iter().find(|s| s.out.0 <= four as u32 && (four as u32) < s.out.1).unwrap();
    assert_eq!(seg.src_uri.as_deref(), Some("defs.h"), "the 4 came from the header: {seg:?}");
}

/// REFLECTION: the meta tier meets the type tier. `@reflect(ty, sep)`
/// makes a splice iterate the resolved type's DECLARED members — the
/// member map is the type tier's own `deftype`/`def`/`member` forms,
/// not an engine schema — substituting parameter 1 at member-name
/// positions and parameter 2 at ordinary ref positions, per member,
/// joined by the grammar-declared separator. "Write the per-field
/// expression once" — the client's original struct-iterator dream, as
/// two annotations.
#[test]
fn reflection_iterates_declared_members() {
    let sl_rg = include_str!("../../../examples/structs/structlang.rg");
    let sl_demo = include_str!("../../../examples/structs/demo.sl");
    let tc = RgToolchain::new();
    let out = compile_source(&tc, sl_rg);
    assert!(out.diags.is_empty(), "structlang compiles: {:?}", out.diags);
    let (lexer, tables) = certify(&out.def).expect("structlang certifies");

    let exp = expand_document(&lexer, &out.def, &tables, sl_demo, &[], 8).expect("expands");
    assert!(exp.diags.is_empty(), "{:?}", exp.diags);
    assert!(
        exp.text.contains("let span: Num = origin.x + origin.y;"),
        "member names iterate: {}",
        exp.text
    );
    assert!(
        exp.text.contains("let grown: Num = probe(new Point) + probe(new Point);"),
        "member TYPES iterate: {}",
        exp.text
    );
    assert!(tiles(&exp.segs, exp.text.len() as u32));

    // Provenance points at the FIELDS: the generated `x` maps to the
    // struct's own `x: Num` declaration; the ` + ` join is synthesized
    // (kind Sep, no source).
    let gx = exp.text.find("origin.x + origin.y").unwrap() + "origin.".len();
    let seg = exp.segs.iter().find(|s| s.out.0 <= gx as u32 && (gx as u32) < s.out.1).unwrap();
    assert_eq!(seg.kind, SegKind::Arg);
    let field_x = sl_demo.find("x: Num").unwrap();
    assert_eq!(seg.src.0 as usize, field_x, "generated x points at the field: {seg:?}");
    let join = exp.text.find("origin.x + origin.y").unwrap() + "origin.x ".len();
    let jseg = exp.segs.iter().find(|s| s.out.0 <= join as u32 && (join as u32) < s.out.1).unwrap();
    assert_eq!(jseg.kind, SegKind::Sep, "{jseg:?}");

    // The materialized output is an ordinary TYPED document: it
    // parses, resolves, and TYPE-CHECKS clean — the instantiations
    // are checked even though the template is exempt.
    let session = IncSession::new(&lexer, &out.def.sg, &tables, &exp.text).unwrap();
    assert!(session.last_repairs.is_empty(), "{:?}", session.last_repairs);
    let mut db = SemDb::new(out.def.binding.clone());
    db.set_types(out.def.types.clone());
    db.set_macro_bodies(&out.def.macros);
    db.set_tree("exp", session.tree().unwrap().clone());
    assert!(db.unresolved("exp").is_empty(), "{:?}", db.unresolved("exp"));
    let report = db.types("exp");
    assert!(report.diags.is_empty(), "expanded output type-checks: {:?}", report.diags);

    // And the PRE-expansion document type-checks too, BECAUSE macro
    // bodies are templates: exempt at the definition, checked per
    // instantiation.
    let mut db2 = SemDb::new(out.def.binding.clone());
    db2.set_types(out.def.types.clone());
    db2.set_macro_bodies(&out.def.macros);
    let s2 = IncSession::new(&lexer, &out.def.sg, &tables, sl_demo).unwrap();
    db2.set_tree("src", s2.tree().unwrap().clone());
    assert!(db2.unresolved("src").is_empty(), "{:?}", db2.unresolved("src"));
    let r2 = db2.types("src");
    assert!(r2.diags.is_empty(), "templates are type-exempt at the def: {:?}", r2.diags);

    // Membership drift is a semantic edit: add a field, re-expand,
    // and the derived expression GROWS — reflection reads the
    // declarations, so the output follows them.
    let grown = sl_demo.replace("  x: Num,\n  y: Num", "  x: Num,\n  y: Num,\n  z: Num");
    let exp2 = expand_document(&lexer, &out.def, &tables, &grown, &[], 8).unwrap();
    assert!(
        exp2.text.contains("origin.x + origin.y + origin.z"),
        "a new field joins the derive: {}",
        exp2.text
    );

    // Reflecting a non-type is diagnosed and left intact.
    let bad = "struct P { x: Num }\nmacro m(f, t) => { zero.f }\nlet zero: Num = 0;\nlet q: Num = m!{zero};\n";
    let e3 = expand_document(&lexer, &out.def, &tables, bad, &[], 8).unwrap();
    assert!(
        e3.diags.iter().any(|d| d.msg.contains("does not resolve to a declared type")),
        "{:?}",
        e3.diags
    );
    assert!(e3.text.contains("m!{zero}"), "left intact: {}", e3.text);
}

/// CROSS-FILE REFLECTION: the reflected type may live in a sibling.
/// Its members come from ITS declarations, the substituted spans
/// splice from that file, and provenance points there — so a derive
/// written here follows a struct declared next door.
#[test]
fn reflection_crosses_files() {
    let sl_rg = include_str!("../../../examples/structs/structlang.rg");
    let tc = RgToolchain::new();
    let out = compile_source(&tc, sl_rg);
    let (lexer, tables) = certify(&out.def).unwrap();

    // lib declares the type AND the derive macro; app owns neither.
    let lib = "struct Vec3 {\n  x: Num,\n  y: Num,\n  z: Num\n}\nmacro coords(f, t) => { here.f }\n";
    let app = "let here: Vec3 = new Vec3;\nlet span: Num = coords!{Vec3};\n";
    let sibs = vec![("lib.sl".to_string(), lib.to_string())];
    let exp = expand_document(&lexer, &out.def, &tables, app, &sibs, 8).expect("expands");
    assert!(exp.diags.is_empty(), "{:?}", exp.diags);
    assert_eq!(
        exp.text, "let here: Vec3 = new Vec3;\nlet span: Num = here.x + here.y + here.z;\n",
        "a foreign struct's members drive the derive"
    );
    assert!(tiles(&exp.segs, exp.text.len() as u32));

    // Provenance: the generated `x` points at lib's FIELD, and the
    // macro body's `here.` prefix at lib's macro — both foreign, both
    // named. (The join is synthesized.)
    let gx = exp.text.find("here.x").unwrap() + "here.".len();
    let xseg = exp.segs.iter().find(|s| s.out.0 <= gx as u32 && (gx as u32) < s.out.1).unwrap();
    assert_eq!((xseg.kind, xseg.src_uri.as_deref()), (SegKind::Arg, Some("lib.sl")), "{xseg:?}");
    assert_eq!(xseg.src.0 as usize, lib.find("x: Num").unwrap(), "…at the field");
    let gh = exp.text.find("here.x").unwrap();
    let hseg = exp.segs.iter().find(|s| s.out.0 <= gh as u32 && (gh as u32) < s.out.1).unwrap();
    assert_eq!((hseg.kind, hseg.src_uri.as_deref()), (SegKind::Body, Some("lib.sl")), "{hseg:?}");

    // The materialized app is an ordinary document in its two-file
    // world: it resolves AND type-checks — the member accesses the
    // derive generated are checked against the foreign struct.
    let session = IncSession::new(&lexer, &out.def.sg, &tables, &exp.text).unwrap();
    assert!(session.last_repairs.is_empty(), "{:?}", session.last_repairs);
    let lib_session = IncSession::new(&lexer, &out.def.sg, &tables, lib).unwrap();
    let mut db = SemDb::new(out.def.binding.clone());
    db.set_types(out.def.types.clone());
    db.set_macro_bodies(&out.def.macros);
    db.set_tree("lib.sl", lib_session.tree().unwrap().clone());
    db.set_tree("app.sl", session.tree().unwrap().clone());
    assert!(db.unresolved("app.sl").is_empty(), "{:?}", db.unresolved("app.sl"));
    assert!(db.types("app.sl").diags.is_empty(), "{:?}", db.types("app.sl").diags);

    // Editing the SIBLING's membership changes this file's derive —
    // reflection reads declarations, wherever they live.
    let lib2 = lib.replace("  z: Num\n", "  z: Num,\n  w: Num\n");
    let sibs2 = vec![("lib.sl".to_string(), lib2)];
    let exp2 = expand_document(&lexer, &out.def, &tables, app, &sibs2, 8).unwrap();
    assert!(exp2.text.contains("here.x + here.y + here.z + here.w"), "{}", exp2.text);

    // A name that resolves cross-file but is NOT a type still
    // diagnoses (and leaves the use intact).
    let notty = "let coords: Num = 0;\nlet q: Num = coords!{scale};\n";
    let libf = "fn scale(v: Num) -> Num { return v; }\nmacro m(f, t) => { f }\n";
    let e3 = expand_document(
        &lexer,
        &out.def,
        &tables,
        notty,
        &[("l.sl".to_string(), libf.to_string())],
        8,
    )
    .unwrap();
    assert!(
        e3.diags.iter().any(|d| d.msg.contains("does not resolve to a declared type")),
        "{:?}",
        e3.diags
    );
}

/// SYNTAX-AWARE SUBSTITUTION. The expander preserves the parse SHAPE
/// the author wrote, on both sides of a splice: an argument that binds
/// weaker than the operator it lands next to gets parentheses, and so
/// does a body that binds weaker than the context it lands in. Both
/// facts come from the grammar's OWN `prec` declarations, and the
/// parentheses come from its OWN grouping production — the expander
/// invents nothing. These are exactly the two classic cpp traps.
#[test]
fn substitution_is_syntax_aware() {
    let (lexer, def, tables) = world();
    let ex = |doc: &str| expand_document(&lexer, &def, &tables, doc, &[], 8).unwrap();

    // Trap 1 — a weak ARGUMENT in a strong slot.
    let out = ex("macro square(v) => { v * v }\nlet e = square!(1 + 2);\n");
    assert!(out.text.contains("let e = (1 + 2) * (1 + 2);"), "{}", out.text);
    // Trap 2 — a weak RESULT in a strong slot.
    let out2 = ex("macro twice(x) => { x + x }\nlet f = 3 * twice!(4);\n");
    assert!(out2.text.contains("let f = 3 * (4 + 4);"), "{}", out2.text);

    // Not a paren more than the shape needs: a strong argument in a
    // weak slot, an equal-strength argument on the associating side,
    // and a fenced slot all stay bare.
    let out3 = ex("macro twice(x) => { x + x }\nlet g = twice!(2 * 5);\n");
    assert!(out3.text.contains("let g = 2 * 5 + 2 * 5;"), "{}", out3.text);
    let out4 = ex("macro twice(x) => { x + x }\nlet h = twice!(1) + 9;\n");
    assert!(out4.text.contains("let h = 1 + 1 + 9;"), "left-assoc, left side: {}", out4.text);
    let out5 = ex("macro id(x) => { x }\nlet i = id!(1 + 2);\nlet j = 2 * id!(3 + 4);\n");
    assert!(out5.text.contains("let i = 1 + 2;"), "fenced slot stays bare: {}", out5.text);
    assert!(out5.text.contains("let j = 2 * (3 + 4);"), "…but a strong slot wraps: {}", out5.text);

    // THE SHAPE ITSELF, not just the spelling: re-parse the expansion
    // and check the top operator. Textual splicing would leave `+` on
    // top of `1 + 2 * 1 + 2`; syntax-aware splicing keeps the `*` the
    // macro's body declared.
    let s = IncSession::new(&lexer, &def.sg, &tables, &out.text).unwrap();
    assert!(s.last_repairs.is_empty(), "{:?}", s.last_repairs);
    assert_eq!(
        let_rhs_prod(s.tree().unwrap(), &def.sg, "e").as_deref(),
        Some("Mul"),
        "the body's operator stays on top"
    );
    let s2 = IncSession::new(&lexer, &def.sg, &tables, &out2.text).unwrap();
    assert_eq!(
        let_rhs_prod(s2.tree().unwrap(), &def.sg, "f").as_deref(),
        Some("Mul"),
        "the use site's operator stays on top"
    );

    // Provenance names the added bytes for what they are: the expander
    // synthesized them, and they have no place in any source file.
    let paren_at = out.text.find("(1 + 2)").unwrap();
    let seg = out
        .segs
        .iter()
        .find(|s| s.out.0 <= paren_at as u32 && (paren_at as u32) < s.out.1)
        .unwrap();
    assert_eq!(seg.kind, SegKind::Paren, "{seg:?}");
    assert_eq!(seg.src, (0, 0), "synthesized bytes have no source");
    assert!(tiles(&out.segs, out.text.len() as u32));

    // And the honest refusal: a grammar with no grouping production
    // cannot be parenthesized, so the expander says so instead of
    // silently changing the meaning.
    let no_parens = MAC_RG.replace("  | Paren:   \"(\" expr \")\"\n", "");
    assert_ne!(no_parens, MAC_RG, "probe edit took");
    let tc = RgToolchain::new();
    let probe = compile_source(&tc, &no_parens);
    let (plexer, ptables) = certify(&probe.def).expect("the probe grammar certifies");
    let out6 = expand_document(
        &plexer,
        &probe.def,
        &ptables,
        "macro square(v) => { v * v }\nlet e = square!(1 + 2);\n",
        &[],
        8,
    )
    .unwrap();
    assert!(
        out6.diags.iter().any(|d| d.msg.contains("declares no grouping production")),
        "{:?}",
        out6.diags
    );
    assert!(out6.text.contains("let e = 1 + 2 * 1 + 2;"), "spliced textually: {}", out6.text);
}

/// The production name of `let NAME = <expr>;`'s right-hand side.
fn let_rhs_prod(
    n: &qana_grammar::GreenNode,
    sg: &qana_grammar::syn::SynGrammar,
    name: &str,
) -> Option<String> {
    use qana_grammar::GreenChild;
    if (n.prod as usize) < sg.prods.len() && sg.prod_name(n.prod as usize) == "LetDef" {
        let kids: Vec<&GreenChild> = n.symbol_children().collect();
        if let (Some(GreenChild::Token(t)), Some(GreenChild::Node(e))) = (kids.get(1), kids.get(3))
        {
            if t.text == name {
                return Some(sg.prod_name(e.prod as usize));
            }
        }
    }
    n.children.iter().find_map(|c| match c {
        GreenChild::Node(m) => let_rhs_prod(m, sg, name),
        _ => None,
    })
}

/// HYGIENE, repaired. One rule finds every capture — a reference that
/// survives expansion must resolve to the same definition afterwards
/// as where it was WRITTEN — and the repair follows from it: rename
/// the binder that swallowed it. Alpha-converting a binding and its
/// references changes nothing else, so the captured reference goes
/// back to meaning what it says, in both directions of capture.
#[test]
fn hygiene_repairs_capture_by_renaming() {
    let (lexer, def, tables) = world();
    let ex = |doc: &str| expand_document(&lexer, &def, &tables, doc, &[], 8).unwrap();

    // Direction 1 — the USER's local would swallow the body's free
    // name, so the user's local is the one that moves.
    let doc = "let unit = 10;\nmacro scaled(v) => { v * unit }\nlet z = { let unit = 99; scaled!(2) };\n";
    let out = ex(doc);
    assert!(out.text.contains("let z = { let unit_h1 = 99; 2 * unit };"), "{}", out.text);
    let notes: Vec<_> = out.diags.iter().filter(|d| d.note).collect();
    assert_eq!(notes.len(), 1, "{:?}", out.diags);
    assert!(notes[0].msg.contains("renamed `unit` to `unit_h1`"), "{}", notes[0].msg);
    assert!(!out.diags.iter().any(|d| !d.note), "nothing left to report: {:?}", out.diags);
    assert_eq!(notes[0].span.0 as usize, doc.rfind("unit = 99").unwrap(), "at the renamed binder");

    // The repair is checked the way the capture was found: in the
    // expansion, every name resolves where it should.
    let s = IncSession::new(&lexer, &def.sg, &tables, &out.text).unwrap();
    assert!(s.last_repairs.is_empty(), "{:?}", s.last_repairs);
    let mut db = SemDb::new(def.binding.clone());
    db.set_tree("o", s.tree().unwrap().clone());
    assert!(db.unresolved("o").is_empty(), "{:?}", db.unresolved("o"));
    let body_unit = out.text.rfind("unit };").unwrap();
    let (_, dspan) = db.definition("o", body_unit as u32).expect("resolves");
    assert_eq!(
        dspan.0 as usize,
        out.text.find("unit = 10").unwrap(),
        "the body's `unit` names the TOP-LEVEL binding again"
    );
    let local = out.text.find("unit_h1").unwrap();
    let (_, lspan) = db.definition("o", local as u32).expect("resolves");
    assert_eq!(lspan.0 as usize, local, "and the renamed local is its own definition");

    // Provenance keeps the link: a renamed name is its own segment,
    // still pointing at the name as the author wrote it.
    let rseg = out
        .segs
        .iter()
        .find(|g| g.out.0 <= local as u32 && (local as u32) < g.out.1)
        .unwrap();
    assert_eq!(rseg.kind, SegKind::Rename);
    assert_eq!(rseg.src.0 as usize, doc.rfind("unit = 99").unwrap(), "{rseg:?}");
    assert!(tiles(&out.segs, out.text.len() as u32));

    // Direction 2 — the BODY's local would swallow the argument, so
    // the body's local is the one that moves (Scheme's rule, derived).
    let out2 = ex("let n = 1;\nmacro shadow(x) => { { let n = 5; x } }\nlet z = shadow!(n);\n");
    assert!(out2.text.contains("let z = { let n_h1 = 5; n };"), "{}", out2.text);
    assert!(out2.diags.iter().all(|d| d.note), "{:?}", out2.diags);

    // NO FALSE POSITIVES, and no gratuitous renames: the committed
    // demo and a block that shadows something else are untouched.
    for clean in [DEMO, "let unit = 10;\nmacro scaled(v) => { v * unit }\nlet z = { let other = 99; scaled!(2) };\n"] {
        let r = ex(clean);
        assert!(r.diags.is_empty(), "{:?}", r.diags);
        assert!(!r.text.contains("_h1"), "nothing renamed: {}", r.text);
    }

    // THE HONEST FALLBACK: repair needs a name the grammar admits. A
    // language whose identifiers are letters only cannot spell
    // `unit_h1`, so the expander reports the capture instead of
    // emitting something that does not lex.
    let letters = MAC_RG.replace("/[\\a_][\\w_]*/", "/[\\a]+/");
    assert_ne!(letters, MAC_RG, "probe edit took");
    let tc = RgToolchain::new();
    let probe = compile_source(&tc, &letters);
    let (plexer, ptables) = certify(&probe.def).expect("the probe grammar certifies");
    let out3 = expand_document(&plexer, &probe.def, &ptables, doc, &[], 8).unwrap();
    assert!(!out3.text.contains("_h"), "no unlexable rename: {}", out3.text);
    assert!(
        out3.diags.iter().any(|d| !d.note && d.msg.contains("changes meaning when expanded")),
        "reported instead: {:?}",
        out3.diags
    );
}

/// REFLECTION FACETS and NAME POSITIONS. A reflection macro's
/// parameters bind to the facets its `@reflect` declares — the
/// member's name, its declared type, the owning type's name, its
/// index, the member count — three of them copies of the declaration
/// (so provenance points at it) and two computed from it. And a name
/// position accepts only a name: splicing an expression there is
/// refused, not emitted as nonsense.
#[test]
fn reflection_facets_and_name_positions() {
    let sl_rg = include_str!("../../../examples/structs/structlang.rg");
    let sl_demo = include_str!("../../../examples/structs/demo.sl");
    let tc = RgToolchain::new();
    let out = compile_source(&tc, sl_rg);
    assert!(out.diags.is_empty(), "{:?}", out.diags);
    let (lexer, tables) = certify(&out.def).unwrap();
    let ex = |doc: &str| expand_document(&lexer, &out.def, &tables, doc, &[], 8).unwrap();

    // The committed demo's faceted derive numbers its members — and
    // the join-aware parenthesization keeps each element a unit.
    let exp = ex(sl_demo);
    assert!(exp.diags.is_empty(), "{:?}", exp.diags);
    assert!(
        exp.text.contains("let tags: Num = origin.x + 0 + (origin.y + 1);"),
        "index facet, join-aware parens: {}",
        exp.text
    );

    // An index is COMPUTED (no source); a name is COPIED from the
    // field it reflects.
    let zero = exp.text.find("origin.x + 0").unwrap() + "origin.x + ".len();
    let zseg = exp.segs.iter().find(|s| s.out.0 <= zero as u32 && (zero as u32) < s.out.1).unwrap();
    assert_eq!((zseg.kind, zseg.src), (SegKind::Meta, (0, 0)), "{zseg:?}");
    let x_at = exp.text.find("origin.x + 0").unwrap() + "origin.".len();
    let xseg = exp.segs.iter().find(|s| s.out.0 <= x_at as u32 && (x_at as u32) < s.out.1).unwrap();
    assert_eq!(xseg.kind, SegKind::Arg);
    assert_eq!(xseg.src.0 as usize, sl_demo.find("x: Num").unwrap());
    assert!(tiles(&exp.segs, exp.text.len() as u32));

    // Facet count and parameter count must agree.
    let bad = "struct P {\n  a: Num\n}\nmacro two(f, t) => { origin.f }\nlet z: Num = two!!{P};\n";
    let e2 = ex(bad);
    assert!(
        e2.diags.iter().any(|d| d.msg.contains("3 facet(s) (name, type, index)")),
        "{:?}",
        e2.diags
    );

    // A NAME position takes a name…
    let ok = "struct P {\n  a: Num\n}\nlet origin: P = new P;\nmacro pick(f) => { origin.f }\nlet g: Num = pick!(a);\n";
    let e3 = ex(ok);
    assert!(e3.diags.is_empty(), "{:?}", e3.diags);
    assert!(e3.text.contains("let g: Num = origin.a;"), "{}", e3.text);

    // …and refuses an expression, instead of emitting `origin.(1 + 2)`.
    let nope = "struct P {\n  a: Num\n}\nlet origin: P = new P;\nmacro pick(f) => { origin.f }\nlet g: Num = pick!(1 + 2);\n";
    let e4 = ex(nope);
    assert!(
        e4.diags.iter().any(|d| d.msg.contains("cannot be substituted at a name position")),
        "{:?}",
        e4.diags
    );
    assert!(e4.text.contains("pick!(1 + 2)"), "left intact: {}", e4.text);
    let s = IncSession::new(&lexer, &out.def.sg, &tables, &e4.text).unwrap();
    assert!(s.last_repairs.is_empty(), "the refusal leaves parseable text");
}
