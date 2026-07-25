//! The C-clone exerciser gate: the committed C-subset grammar
//! certifies (this very grammar forced Pager merging into the LR
//! construction — canonical LR(1) never terminated on it), and the
//! committed demo parses losslessly with every reference resolving.

use rantlr_engine::IncSession;
use rantlr_rg::compile::certify;
use rantlr_rg::{compile_source, RgToolchain};
use rantlr_sem::SemDb;

const C_RG: &str = include_str!("../../../examples/c/c.rg");
const DEMO: &str = include_str!("../../../examples/c/demo.c");

#[test]
fn c_subset_certifies_and_serves_the_demo() {
    let tc = RgToolchain::new();
    let out = compile_source(&tc, C_RG);
    assert!(out.diags.is_empty(), "c.rg compiles: {:?}", out.diags);
    let (lexer, tables) = certify(&out.def).expect("C subset is in the envelope");
    assert!(
        tables.n_states < 1000,
        "Pager keeps C tractable (got {} states)",
        tables.n_states
    );
    assert!(!tables.lists.is_empty(), "C's lists are L4-balanced");

    let session = IncSession::new(&lexer, &out.def.sg, &tables, DEMO).unwrap();
    let tree = session.tree().expect("total");
    assert_eq!(tree.text(), DEMO, "lossless");
    assert!(session.last_repairs.is_empty(), "demo parses clean: {:?}", session.last_repairs);

    let mut db = SemDb::new(out.def.binding.clone());
    db.set_tree("demo.c", tree.clone());
    assert!(
        db.unresolved("demo.c").is_empty(),
        "every reference resolves: {:?}",
        db.unresolved("demo.c")
    );
    let syms = db.symbols("demo.c");
    let names: Vec<&str> = syms.defs.iter().map(|d| d.name.as_str()).collect();
    for expected in ["scale", "apply", "main", "point", "color", "LIMIT", "op"] {
        assert!(names.contains(&expected), "def {expected} found: {names:?}");
    }
}
