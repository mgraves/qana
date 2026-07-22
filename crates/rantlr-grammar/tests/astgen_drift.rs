//! The drift gate: the checked-in typed AST must exactly match what the
//! generator produces from the current grammar value. When the grammar
//! changes, this fails first; regenerating then surfaces every
//! downstream use that no longer typechecks — ramification as compile
//! errors, enforced.

use rantlr_grammar::astgen::generate;
use rantlr_grammar::demo::{demo_grammar, demo_syn_grammar};
use rantlr_grammar::{build_lr, Vocab};

#[test]
fn checked_in_typed_ast_is_current() {
    let (g, ids) = demo_grammar();
    let vocab = Vocab::of(&g);
    let sg = demo_syn_grammar(&ids, &vocab);
    let tables = build_lr(&sg);
    let fresh = generate(&sg, &tables);
    let checked_in = include_str!("../src/demo_ast.rs");
    assert!(
        fresh == checked_in,
        "demo_ast.rs is stale — regenerate with:\n  cargo run -p rantlr-grammar --bin astgen > crates/rantlr-grammar/src/demo_ast.rs"
    );
}
