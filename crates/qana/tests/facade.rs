//! The facade computes nothing, so its failure mode is not a wrong
//! answer — it is a wrong re-export path or a wrong feature gate, and
//! that only ever surfaces in a CONSUMER's build, never in ours. So this
//! test is that consumer: it imports exclusively through `qana::` and
//! never names a member crate, then drives a real grammar end to end.

use qana::prelude::*;

const C_QANA: &str = include_str!("../../../examples/c/c.qana");
const DEMO: &str = include_str!("../../../examples/c/demo.c");

#[test]
fn the_prelude_alone_certifies_a_grammar_and_serves_a_document() {
    let tc = QanaToolchain::new();
    let out = compile_source(&tc, C_QANA);
    assert!(out.diags.is_empty(), "c.qana compiles: {:?}", out.diags);

    let (lexer, tables) = certify(&out.def).expect("the C subset is in the envelope");

    let session = IncSession::new(&lexer, &out.def.sg, &tables, DEMO).unwrap();
    let tree = session.tree().expect("total");
    assert_eq!(tree.text(), DEMO, "lossless, reached through the facade");

    let mut db = SemDb::new(out.def.binding.clone());
    db.set_tree("demo.c", tree.clone());
    assert!(
        db.unresolved("demo.c").is_empty(),
        "every reference resolves: {:?}",
        db.unresolved("demo.c")
    );
}

/// Not a behavioural claim — a wiring one. Every layer must be reachable
/// by its module path, including the two the test above never touches.
/// If a re-export is dropped or misgated, this stops compiling.
#[test]
fn every_layer_is_reachable_by_module_path() {
    fn reachable<T>() -> &'static str {
        std::any::type_name::<T>()
    }
    assert!(reachable::<qana::grammar::SynGrammar>().contains("SynGrammar"));
    assert!(reachable::<qana::sem::SemDb>().contains("SemDb"));
    assert!(reachable::<qana::services::Styles>().contains("Styles"));
    assert!(reachable::<qana::lang::QanaToolchain>().contains("QanaToolchain"));

    // linework is re-exported for convenience but depends on nothing —
    // an editor may depend on it alone, which is the point of it.
    assert!(reachable::<qana::linework::Paint>().contains("Paint"));
    assert_eq!(qana::linework::MOD_DEF, 1);
}
