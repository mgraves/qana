//! The meta tier, end to end: declared macros, binding-guided
//! substitution, fixpoint expansion, provenance — and the residues,
//! pinned. The committed examples/macrolang is the exerciser.

use rantlr_engine::IncSession;
use rantlr_rg::compile::certify;
use rantlr_rg::expand::expand_document;
use rantlr_rg::{compile_source, RgToolchain};
use rantlr_sem::macros::{tiles, SegKind};
use rantlr_sem::SemDb;

const MAC_RG: &str = include_str!("../../../examples/macrolang/mac.rg");
const DEMO: &str = include_str!("../../../examples/macrolang/demo.mac");

fn world() -> (rantlr_grammar::CompiledLexer, rantlr_rg::compile::LangDef, rantlr_grammar::LrTables)
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
    let out = expand_document(&lexer, &def, &tables, DEMO, 8).expect("expands");
    assert!(out.diags.is_empty(), "no diagnostics: {:?}", out.diags);
    assert_eq!(out.repairs, 0, "no pass ever parsed broken text");
    assert_eq!(out.passes, 2, "nested macros need exactly two passes");
    assert_eq!(out.substitutions, 8, "4 outer + 4 inner uses");

    // Definitions stay; uses open. Exact lines:
    assert!(out.text.contains("let a = 3 + 3;"), "{}", out.text);
    assert!(out.text.contains("let b = 2 + 2 + 2 + 2;"), "{}", out.text);
    assert!(out.text.contains("let c = (a + 1) * unit;"), "{}", out.text);
    assert!(out.text.contains("let d = 4 + 4 + 4 + 4;"), "{}", out.text);
    assert!(out.text.contains("macro twice(x) => { x + x }"), "defs verbatim: {}", out.text);
}

/// The materialization promise: expansion output is an ORDINARY
/// document — it parses clean and every reference resolves. Full
/// editor intelligence inside generated text, the thing a token-based
/// expander can never give.
#[test]
fn expanded_output_is_an_ordinary_document() {
    let (lexer, def, tables) = world();
    let out = expand_document(&lexer, &def, &tables, DEMO, 8).unwrap();
    let session = IncSession::new(&lexer, &def.sg, &tables, &out.text).unwrap();
    assert!(session.last_repairs.is_empty(), "output parses clean: {:?}", session.last_repairs);
    let mut db = SemDb::new(def.binding.clone());
    db.set_tree("exp", session.tree().unwrap().clone());
    assert!(db.unresolved("exp").is_empty(), "all refs resolve: {:?}", db.unresolved("exp"));

    // And expansion is IDEMPOTENT: the output has nothing left to expand.
    let again = expand_document(&lexer, &def, &tables, &out.text, 8).unwrap();
    assert_eq!(again.substitutions, 0, "fixpoint reached");
    assert_eq!(again.text, out.text);
}

/// Provenance: the segments tile the output exactly, and they point
/// where they claim — an `Arg` byte maps into the use site's argument,
/// a `Body` byte into the macro definition's body.
#[test]
fn provenance_tiles_and_points_home() {
    let (lexer, def, tables) = world();
    let out = expand_document(&lexer, &def, &tables, DEMO, 8).unwrap();
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
    let out = expand_document(&lexer, &def, &tables, arity, 8).unwrap();
    assert_eq!(out.substitutions, 0);
    assert!(out.text.contains("twice!(1, 2)"), "left verbatim");
    assert!(
        out.diags.iter().any(|d| d.msg.contains("1 argument(s), 2 supplied")),
        "{:?}",
        out.diags
    );

    let notmac = "let f = 1;\nlet a = f!(2);\n";
    let out = expand_document(&lexer, &def, &tables, notmac, 8).unwrap();
    assert!(
        out.diags.iter().any(|d| d.msg.contains("does not resolve to a macro")),
        "{:?}",
        out.diags
    );

    let loopy = "macro loopy(x) => { loopy!(x) }\nlet a = loopy!(1);\n";
    let out = expand_document(&lexer, &def, &tables, loopy, 4).unwrap();
    assert!(
        out.diags.iter().any(|d| d.msg.contains("did not converge")),
        "the cap is a diagnostic, not a hang: {:?}",
        out.diags
    );
}

/// The COMMITTED materializations are drift-gated, astgen-style: the
/// checked-in .exp files must equal a fresh expansion of their
/// sources. If a demo or grammar changes, `rantlr expand` must be
/// re-run — this test is what makes the pair trustworthy to read.
#[test]
fn committed_materializations_are_current() {
    let (lexer, def, tables) = world();
    let out = expand_document(&lexer, &def, &tables, DEMO, 8).unwrap();
    let committed = include_str!("../../../examples/macrolang/demo.exp.mac");
    assert_eq!(committed, out.text, "demo.exp.mac drifted — rerun `rantlr expand`");

    let c_rg = include_str!("../../../examples/c/c.rg");
    let c_demo = include_str!("../../../examples/c/demo.c");
    let tc = RgToolchain::new();
    let cdef = compile_source(&tc, c_rg);
    let (clexer, ctables) = certify(&cdef.def).unwrap();
    let cout = expand_document(&clexer, &cdef.def, &ctables, c_demo, 8).unwrap();
    let ccommitted = include_str!("../../../examples/c/demo.exp.c");
    assert_eq!(ccommitted, cout.text, "demo.exp.c drifted — rerun `rantlr expand`");
}
