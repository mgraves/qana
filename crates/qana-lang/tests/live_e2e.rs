//! The Limner contract, exercised the way an EDITOR will: through the
//! trait object alone, with no knowledge of the engine behind it.
//! This is the seam Synkro's RichCodeArea holds.

use linework::{LineEdit, Limner, MOD_DEF, MOD_TYPED, MOD_UNRESOLVED};
use qana_lang::live::LiveDoc;
use qana_lang::EmbeddedLang;
use std::sync::Arc;

const SL_RG: &str = include_str!("../../../examples/structs/structlang.qana");
const SL_DEMO: &str = include_str!("../../../examples/structs/demo.sl");

fn limner() -> Box<dyn Limner> {
    let lang = Arc::new(EmbeddedLang::from_qana_source(SL_RG).expect("certifies"));
    Box::new(LiveDoc::open(lang, "demo.sl", SL_DEMO))
}

/// Open → paint; edit → delta; hover → facts; hints — all through
/// `dyn Limner`, and every answer coherent with the others.
#[test]
fn the_trait_carries_the_whole_experience() {
    let mut l = limner();
    let paint = l.open(SL_DEMO);
    assert_eq!(paint.lines.len(), SL_DEMO.lines().count() + usize::from(SL_DEMO.ends_with('\n')));
    assert!(paint.lines.iter().flatten().any(|r| r.mods & MOD_DEF != 0));
    assert!(paint.lines.iter().flatten().any(|r| r.mods & MOD_TYPED != 0));
    assert!(!l.legend().is_empty());
    assert_eq!(l.text(), SL_DEMO, "lossless mirror");

    // Edit: introduce a typo'd reference on the width line; the delta
    // must carry the unresolved bit, synchronously.
    let width_line = SL_DEMO[..SL_DEMO.find("let width").unwrap()].matches('\n').count() as u32;
    let delta = l.edit(&LineEdit {
        start: width_line,
        end: width_line + 1,
        lines: vec!["let width: Num = scal(2) + 3;".to_string()],
    });
    let ((lo, hi), repl) = delta.splice.expect("spliced");
    assert!(lo <= width_line && width_line < hi);
    assert!(
        repl.iter().flatten().any(|r| r.mods & MOD_UNRESOLVED != 0),
        "the typo paints unresolved in the SAME delta"
    );

    // Facts at the typo: the problem is named.
    let text = l.text();
    let at = text.find("scal(").unwrap() as u32;
    let card = l.facts(at).expect("card");
    assert_eq!(card.problem.as_deref(), Some("cannot find `scal`"));

    // Facts at a healthy typed def: the type rides along.
    let at = text.find("let ox").unwrap() + "let ".len();
    let card = l.facts(at as u32).expect("card");
    assert!(card.is_def);
    assert_eq!(card.ty.as_deref(), Some("Num"));

    // Hints include the same fact as a decoration.
    assert!(l.hints().iter().any(|h| h.text == ": Num"));

    // Repair the typo; the unresolved bit must clear in the delta.
    let delta = l.edit(&LineEdit {
        start: width_line,
        end: width_line + 1,
        lines: vec!["let width: Num = scale(2) + 3;".to_string()],
    });
    let (_, repl) = delta.splice.expect("spliced");
    assert!(
        repl.iter().flatten().all(|r| r.mods & MOD_UNRESOLVED == 0),
        "repair clears the squiggle bit"
    );

    // Structural edit: insert a line; the maintained text mirrors it.
    let n_before = l.text().lines().count();
    let delta = l.edit(&LineEdit {
        start: 0,
        end: 0,
        lines: vec!["# a fresh comment line".to_string()],
    });
    assert!(delta.splice.is_some());
    assert_eq!(l.text().lines().count(), n_before + 1);
}

/// The marks channel: flat, document-ordered, containment-nested
/// structure — the one substrate breadcrumbs, structure views, sticky
/// context, and folding all read.
#[test]
fn marks_carry_the_document_structure() {
    let mut l = limner();
    l.open(SL_DEMO);

    let legend = l.mark_legend();
    assert!(legend.iter().any(|k| k == "struct"), "legend: {legend:?}");
    assert!(legend.iter().any(|k| k == "function"), "legend: {legend:?}");

    let marks = l.marks();
    assert!(!marks.is_empty());
    assert!(marks.iter().all(|m| (m.category as usize) < legend.len()));

    // Document order, and well-nested: any two spans are disjoint or
    // one contains the other — containment IS the hierarchy.
    for w in marks.windows(2) {
        assert!(w[0].start <= w[1].start, "document order");
    }
    for a in &marks {
        for b in &marks {
            let disjoint = a.end <= b.start || b.end <= a.start;
            let a_in_b = b.start <= a.start && a.end <= b.end;
            let b_in_a = a.start <= b.start && b.end <= a.end;
            assert!(disjoint || a_in_b || b_in_a, "well-nested: {a:?} vs {b:?}");
        }
    }

    // The demo's named constructs all surface.
    let names: Vec<&str> = marks.iter().map(|m| m.name.as_str()).collect();
    for expect in ["Point", "Label", "scale", "origin", "tag"] {
        assert!(names.contains(&expect), "missing {expect} in {names:?}");
    }

    // `Point`: struct-categorized, name span inside the full span.
    let point = marks.iter().find(|m| m.name == "Point").unwrap();
    assert_eq!(legend[point.category as usize], "struct");
    assert!(point.start <= point.name_start && point.name_end <= point.end);

    // The breadcrumb query: enclosing chain at a field INSIDE Point,
    // outermost-first, every link containing the offset.
    let x_field = SL_DEMO.find("x: Num").unwrap() as u32;
    let chain = l.enclosing(x_field);
    assert!(chain.iter().any(|m| m.name == "Point"), "{chain:?}");
    for w in chain.windows(2) {
        assert!(
            w[0].start <= w[1].start && w[1].end <= w[0].end,
            "outermost first: {chain:?}"
        );
    }
    assert!(chain.iter().all(|m| m.start <= x_field && x_field < m.end));

    // Marks track edits: a line inserted at the top shifts every span.
    let before = l.marks();
    let delta = l.edit(&LineEdit {
        start: 0,
        end: 0,
        lines: vec!["# pushed down".to_string()],
    });
    assert!(delta.splice.is_some());
    let after = l.marks();
    assert_eq!(before.len(), after.len());
    let shift = "# pushed down\n".len() as u32;
    for (b, a) in before.iter().zip(&after) {
        assert_eq!(b.name, a.name);
        assert_eq!(a.start, b.start + shift, "span shifted by the inserted line");
    }
}

/// Nesting is what makes the enclosing chain a PATH: `mod` scopes give
/// structlang depth, and the chain walks root-at-left to leaf-at-right.
#[test]
fn enclosing_chain_walks_nested_scopes() {
    let mut l = limner();
    let src = "mod colors {\n  struct RGBA {\n    r:Num,\n  }\n}\n";
    l.open(src);

    let at = src.find("r:Num").unwrap() as u32;
    let chain = l.enclosing(at);
    let labels: Vec<&str> = chain.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(
        labels,
        vec!["colors", "RGBA", "r"],
        "root-at-left, leaf-at-right — fields are chain LEAVES"
    );

    let legend = l.mark_legend();
    assert_eq!(legend[chain[0].category as usize], "module");
    assert_eq!(legend[chain[1].category as usize], "struct");
    assert_eq!(legend[chain[2].category as usize], "field");
}

/// The canonical-form clause: an edit that REACHES THE DOCUMENT'S END
/// changes which line is final, and only the final line goes without a
/// terminator. These are the exact batches an editor's Return and
/// Backspace produce at EOF — the first shipped LiveDoc reused
/// terminators positionally, so a Return splitting the final line
/// copied its `None` onto an interior line and padded the new final
/// line with `Lf`, tripping the engine's invariant assert on the very
/// first Return pressed at EOF in a real editor.
#[test]
fn edits_reaching_the_document_end_keep_terms_canonical() {
    let mut l = limner();
    let n = l.open(SL_DEMO).lines.len() as u32; // canonical line count
    let before = l.text();

    // Return on the (empty, term-less) final line: 1 line → 2.
    let delta = l.edit(&LineEdit {
        start: n - 1,
        end: n,
        lines: vec![String::new(), String::new()],
    });
    assert!(delta.splice.is_some());
    assert_eq!(l.text(), format!("{before}\n"), "Return at EOF appends one newline");

    // Backspace joining at EOF: 2 lines → 1 restores the original.
    let delta = l.edit(&LineEdit {
        start: n - 1,
        end: n + 1,
        lines: vec![String::new()],
    });
    assert!(delta.splice.is_some());
    assert_eq!(l.text(), before, "the join restores the original");

    // A document whose final line carries TEXT (no trailing newline):
    // Return after its last character splits that line in two.
    let trimmed = SL_DEMO.strip_suffix('\n').expect("demo ends with newline");
    l.open(trimmed);
    let n = trimmed.matches('\n').count() as u32 + 1;
    let last = trimmed.rsplit_once('\n').expect("multi-line").1;
    let delta = l.edit(&LineEdit {
        start: n - 1,
        end: n,
        lines: vec![last.to_string(), String::new()],
    });
    assert!(delta.splice.is_some());
    assert_eq!(l.text(), format!("{trimmed}\n"), "split after the last char");

    // Pure tail deletion (empty replacement reaching EOF): the line
    // BEFORE the deleted tail becomes final and must shed its term.
    let delta = l.edit(&LineEdit { start: n, end: n + 1, lines: vec![] });
    assert!(delta.splice.is_some());
    assert_eq!(l.text(), trimmed, "tail deletion re-terminates the new final line");
}

/// The marks cache: editors query marks per CURSOR TICK, and at 27k
/// symbols the outline walk per query froze typing. marks_shared
/// hands out the same Arc until an edit bumps the revision — the
/// per-tick cost is O(1), and derived-work consumers gate on
/// marks_rev instead of recomputing to find out nothing changed.
#[test]
fn marks_are_cached_per_revision() {
    let mut l = limner();
    l.open(SL_DEMO);

    let rev0 = l.marks_rev();
    let a = l.marks_shared();
    let b = l.marks_shared();
    assert!(std::sync::Arc::ptr_eq(&a, &b), "same revision: same Arc, zero recompute");
    assert_eq!(l.marks_rev(), rev0, "queries do not bump the revision");

    // An edit invalidates: new revision, fresh marks reflecting it.
    let delta = l.edit(&LineEdit {
        start: 0,
        end: 0,
        lines: vec!["struct Fresh { a: Num }".to_string()],
    });
    let _ = delta;
    assert_ne!(l.marks_rev(), rev0, "edits bump the revision");
    let c = l.marks_shared();
    assert!(!std::sync::Arc::ptr_eq(&a, &c), "new revision: recomputed marks");
    assert!(
        c.iter().any(|m| m.name == "Fresh"),
        "fresh marks see the edit: {:?}",
        c.iter().map(|m| m.name.clone()).collect::<Vec<_>>()
    );

    // enclosing() rides the same cache and stays correct.
    let fresh = c.iter().find(|m| m.name == "Fresh").unwrap();
    let chain = l.enclosing(fresh.name_start);
    assert!(chain.iter().any(|m| m.name == "Fresh"), "chain: {chain:?}");
}
