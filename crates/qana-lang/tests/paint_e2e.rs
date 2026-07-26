//! The native editor protocol, end to end: line-keyed two-wave paint,
//! damage-shaped deltas, hover facts, decorations — gated with the
//! house differential (incremental ≡ from-scratch) and exact damage
//! bounds. LSP and tree-sitter are compatibility exports; THIS is the
//! interface an editor built for the engine consumes.

use qana_engine::{IncSession, Line, LineEdit};
use qana_lang::compile::certify;
use qana_lang::{compile_source, QanaToolchain};
use qana_sem::SemDb;
use qana_services::paint::{
    decode_lines, encode_lines, facts_at, type_hints, Painter, MOD_DEF, MOD_FOREIGN, MOD_PUBLIC,
    MOD_REF, MOD_TYPED, MOD_UNRESOLVED,
};

const C_RG: &str = include_str!("../../../examples/c/c.qana");
const C_DEMO: &str = include_str!("../../../examples/c/demo.c");
const SL_RG: &str = include_str!("../../../examples/structs/structlang.qana");
const SL_DEMO: &str = include_str!("../../../examples/structs/demo.sl");

fn c_world() -> (qana_grammar::CompiledLexer, qana_lang::compile::LangDef, qana_grammar::LrTables)
{
    let tc = QanaToolchain::new();
    let out = compile_source(&tc, C_RG);
    let (lexer, tables) = certify(&out.def).unwrap();
    (lexer, out.def, tables)
}

/// Every line's runs tile that line exactly, and the incremental
/// painter agrees with a from-scratch paint after every edit — the
/// same differential that guards the parser, applied to the frames.
#[test]
fn paint_tiles_and_incremental_equals_fresh() {
    let (lexer, def, tables) = c_world();
    let mut session = IncSession::new(&lexer, &def.sg, &tables, C_DEMO).unwrap();
    let mut db = SemDb::new(def.binding.clone());
    db.set_tree("d", session.tree().unwrap().clone());
    let (mut painter, full) = Painter::new(&session, &def.styles, Some((&mut db, "d")));

    let widths: Vec<u32> = session
        .buf
        .lines
        .iter()
        .map(|l| (l.text.len() + l.term.as_str().len()) as u32)
        .collect();
    for (li, runs) in full.lines.iter().enumerate() {
        let sum: u32 = runs.iter().map(|r| r.len as u32).sum();
        assert_eq!(sum, widths[li], "line {li} tiles exactly");
    }

    // A body edit, then a structural edit (insert a line), then a
    // deletion — after each, the maintained frame equals a fresh one.
    let edits = [
        (30usize, "    word_t doubled = w + w + 1;".to_string()),
        (31usize, "    int inserted = 0; return doubled;".to_string()),
        (5usize, "#define LIMIT 900".to_string()),
    ];
    for (line, text) in edits {
        let term = session.buf.lines[line].term;
        let outcome = session
            .edit(&def.sg, &tables, &[LineEdit {
                start: line,
                end: line + 1,
                replacement: vec![Line::new(text, term)],
            }])
            .unwrap();
        db.set_tree("d", session.tree().unwrap().clone());
        let _delta = painter.update(&session, &outcome.damage, &def.styles, Some((&mut db, "d")));

        let mut db2 = SemDb::new(def.binding.clone());
        db2.set_tree("d", session.tree().unwrap().clone());
        let (_, fresh) = Painter::new(&session, &def.styles, Some((&mut db2, "d")));
        assert_eq!(painter.frame().lines, fresh.lines, "incremental ≡ fresh after edit");
    }
}

/// The deltas are DAMAGE-SHAPED: a one-line body edit splices O(1)
/// lines and repaints almost nothing; renaming a definition repaints
/// exactly the lines that referred to it — the blast radius, visible.
#[test]
fn deltas_are_damage_shaped_and_renames_show_their_blast_radius() {
    let (lexer, def, tables) = c_world();
    // A wide file: one function per line-pair, all calling `scale`.
    let mut doc = String::from("static int scale(int v, int factor) { return v * factor; }\n");
    for i in 0..400 {
        doc.push_str(&format!("static int use{i}(void) {{ return scale({i}, 2); }}\n"));
    }
    doc.push_str("int main(void) { return scale(1, 2); }\n");
    let mut session = IncSession::new(&lexer, &def.sg, &tables, &doc).unwrap();
    let mut db = SemDb::new(def.binding.clone());
    db.set_tree("d", session.tree().unwrap().clone());
    let (mut painter, _) = Painter::new(&session, &def.styles, Some((&mut db, "d")));

    // 1. A pure body edit inside ONE function: splice ≤ 2 lines,
    //    no semantic repaints anywhere else.
    let term = session.buf.lines[5].term;
    let outcome = session
        .edit(&def.sg, &tables, &[LineEdit {
            start: 5,
            end: 6,
            replacement: vec![Line::new(
                "static int use4(void) { return scale(400, 2); }".to_string(),
                term,
            )],
        }])
        .unwrap();
    db.set_tree("d", session.tree().unwrap().clone());
    let delta = painter.update(&session, &outcome.damage, &def.styles, Some((&mut db, "d")));
    let ((lo, hi), _) = delta.splice.clone().expect("a splice");
    assert!(hi - lo <= 2, "body edit splices O(1) lines, got {}", hi - lo);
    assert!(
        delta.repaints.is_empty(),
        "no semantic fallout from a body edit: {:?}",
        delta.repaints.iter().map(|(l, _)| l).collect::<Vec<_>>()
    );

    // 2. RENAME the definition: every caller's line — and only those
    //    lines — repaints (their refs went unresolved).
    let term = session.buf.lines[0].term;
    let outcome = session
        .edit(&def.sg, &tables, &[LineEdit {
            start: 0,
            end: 1,
            replacement: vec![Line::new(
                "static int scale2(int v, int factor) { return v * factor; }".to_string(),
                term,
            )],
        }])
        .unwrap();
    db.set_tree("d", session.tree().unwrap().clone());
    let delta = painter.update(&session, &outcome.damage, &def.styles, Some((&mut db, "d")));
    let repainted: Vec<u32> = delta.repaints.iter().map(|(l, _)| *l).collect();
    let expected: Vec<u32> = (1..=401).collect(); // every use line + main
    assert_eq!(repainted, expected, "the repaint set IS the blast radius");
    // …and those repaints carry the unresolved bit on the callee.
    let (_, runs) = &delta.repaints[0];
    assert!(
        runs.iter().any(|r| r.mods & MOD_UNRESOLVED != 0),
        "callers show `cannot find`: {runs:?}"
    );
}

/// Wave 1 is MONOTONE: it may add modifier bits, never change a
/// style — the two frames come from one grammar, so the first paint
/// is already right and refinement cannot flicker it.
#[test]
fn semantic_refinement_never_changes_a_style() {
    let (lexer, def, tables) = c_world();
    let session = IncSession::new(&lexer, &def.sg, &tables, C_DEMO).unwrap();
    let (_, wave0) = Painter::new(&session, &def.styles, None);
    let mut db = SemDb::new(def.binding.clone());
    db.set_tree("d", session.tree().unwrap().clone());
    let (_, both) = Painter::new(&session, &def.styles, Some((&mut db, "d")));

    for (l0, l1) in wave0.lines.iter().zip(both.lines.iter()) {
        // Flatten to per-byte styles: refinement may split runs, so
        // compare byte-wise.
        let bytes = |runs: &[qana_services::paint::Run]| {
            runs.iter().flat_map(|r| std::iter::repeat(r.style).take(r.len as usize)).collect::<Vec<u8>>()
        };
        assert_eq!(bytes(l0), bytes(l1), "styles identical; only mods differ");
    }
    // And the overlay actually said something: defs and refs exist.
    let any = |bit: u8| both.lines.iter().flatten().any(|r| r.mods & bit != 0);
    assert!(any(MOD_DEF) && any(MOD_REF), "the demo has marked defs and refs");
}

/// The wire form round-trips exactly.
#[test]
fn wire_roundtrip_is_identity() {
    let (lexer, def, tables) = c_world();
    let session = IncSession::new(&lexer, &def.sg, &tables, C_DEMO).unwrap();
    let mut db = SemDb::new(def.binding.clone());
    db.set_tree("d", session.tree().unwrap().clone());
    let (_, paint) = Painter::new(&session, &def.styles, Some((&mut db, "d")));
    let items: Vec<(u32, &[qana_services::paint::Run])> =
        paint.lines.iter().enumerate().map(|(i, r)| (i as u32, r.as_slice())).collect();
    let bytes = encode_lines(paint.rev, &items);
    let (rev, decoded) = decode_lines(&bytes).expect("decodes");
    assert_eq!(rev, paint.rev);
    assert_eq!(decoded.len(), paint.lines.len());
    for ((line, runs), (i, orig)) in decoded.iter().zip(paint.lines.iter().enumerate()) {
        assert_eq!(*line as usize, i);
        assert_eq!(runs, orig);
    }
    assert!(decode_lines(&bytes[..bytes.len() - 1]).is_none(), "truncation is refused");
}

/// The FACTS plane: hover cards assembled from the memoized tiers —
/// definition sites, problems, types, namespaces — plus the
/// decoration plane (inline type hints).
#[test]
fn facts_and_hints_answer_from_warm_tiers() {
    // C: a typedef use navigates; a typo explains itself; a tag names
    // its namespace.
    let (lexer, def, tables) = c_world();
    let session = IncSession::new(&lexer, &def.sg, &tables, C_DEMO).unwrap();
    let mut db = SemDb::new(def.binding.clone());
    db.set_tree("d", session.tree().unwrap().clone());

    let use_at = C_DEMO.find("word_t global_words").unwrap() as u32;
    let card = facts_at(&mut db, "d", C_DEMO, use_at).expect("card");
    assert_eq!(card.name, "word_t");
    assert!(!card.is_def);
    let (duri, dspan) = card.def_site.expect("resolves");
    assert_eq!(duri, "d");
    assert_eq!(dspan.0 as usize, C_DEMO.find("word_t;").unwrap(), "…to the typedef");
    assert!(card.problem.is_none());

    let tag_at = C_DEMO.find("struct point point_t").unwrap() + "struct ".len();
    let tag = facts_at(&mut db, "d", C_DEMO, tag_at as u32).expect("tag card");
    assert_eq!(tag.ns.as_deref(), Some("tag"), "namespaces surface");

    let bad_doc = "wrod x;\n";
    let s2 = IncSession::new(&lexer, &def.sg, &tables, bad_doc).unwrap();
    let mut db2 = SemDb::new(def.binding.clone());
    db2.set_tree("b", s2.tree().unwrap().clone());
    let card = facts_at(&mut db2, "b", bad_doc, 0).expect("card");
    assert_eq!(card.problem.as_deref(), Some("cannot find `wrod`"));

    // Structlang: types on cards and as inline hints.
    let tc = QanaToolchain::new();
    let sl = compile_source(&tc, SL_RG);
    let (slex, stab) = certify(&sl.def).unwrap();
    let s3 = IncSession::new(&slex, &sl.def.sg, &stab, SL_DEMO).unwrap();
    let mut db3 = SemDb::new(sl.def.binding.clone());
    db3.set_types(sl.def.types.clone());
    db3.set_macro_bodies(&sl.def.macros);
    db3.set_tree("s", s3.tree().unwrap().clone());

    let w_at = SL_DEMO.find("let width").unwrap() + "let ".len();
    let card = facts_at(&mut db3, "s", SL_DEMO, w_at as u32).expect("card");
    assert!(card.is_def);
    assert_eq!(card.ty.as_deref(), Some("Num"), "the def's type rides the card");

    let hints = type_hints(&mut db3, "s", SL_DEMO);
    assert!(!hints.is_empty());
    let width_line = SL_DEMO[..w_at].matches('\n').count() as u32;
    assert!(
        hints.iter().any(|h| h.line == width_line && h.text == ": Num"),
        "inline `: Num` hint on the width line: {hints:?}"
    );

    // MOD_TYPED shows up in the overlay for the same world.
    let (_, paint) = Painter::new(&s3, &sl.def.styles, Some((&mut db3, "s")));
    assert!(
        paint.lines.iter().flatten().any(|r| r.mods & MOD_TYPED != 0),
        "typed defs are painted as such"
    );
}

/// Cross-file bits: a `pub` definition paints PUBLIC, and a use in the
/// neighbor paints FOREIGN — the module tier, visible in the colors.
#[test]
fn module_facts_paint_public_and_foreign()  {
    let ml_qana = include_str!("../../../examples/modules/modlang.qana");
    let lib = include_str!("../../../examples/modules/lib.ml");
    let app = include_str!("../../../examples/modules/app.ml");
    let tc = QanaToolchain::new();
    let out = compile_source(&tc, ml_qana);
    let (lexer, tables) = certify(&out.def).unwrap();
    let ls = IncSession::new(&lexer, &out.def.sg, &tables, lib).unwrap();
    let as_ = IncSession::new(&lexer, &out.def.sg, &tables, app).unwrap();
    let mut db = SemDb::new(out.def.binding.clone());
    db.set_tree("lib.ml", ls.tree().unwrap().clone());
    db.set_tree("app.ml", as_.tree().unwrap().clone());

    let (_, lib_paint) = Painter::new(&ls, &out.def.styles, Some((&mut db, "lib.ml")));
    assert!(
        lib_paint.lines.iter().flatten().any(|r| r.mods & MOD_PUBLIC != 0),
        "exported defs paint PUBLIC"
    );
    let (_, app_paint) = Painter::new(&as_, &out.def.styles, Some((&mut db, "app.ml")));
    assert!(
        app_paint.lines.iter().flatten().any(|r| r.mods & MOD_FOREIGN != 0),
        "imports paint FOREIGN"
    );
}
