//! Drift gate: the checked-in typed AST for the `.rg` grammar must match
//! what the generator produces from the bootstrap values today.

use rantlr_grammar::astgen::generate_with_paths;
use rantlr_grammar::{build_lr, CompiledLexer};
use rantlr_rg::bootstrap::{rg_lex_grammar, rg_syn_grammar};

#[test]
fn generated_rg_ast_is_current() {
    let (g, ids) = rg_lex_grammar();
    let lexer = CompiledLexer::build(&g).unwrap();
    let (sg, _) = rg_syn_grammar(&ids, &lexer.vocab);
    let tables = build_lr(&sg);
    let want = generate_with_paths(&sg, &tables, "rantlr_grammar");
    assert_eq!(
        include_str!("../src/rg_ast.rs"),
        want,
        "regenerate: cargo run -p rantlr-rg --bin rg_astgen > crates/rantlr-rg/src/rg_ast.rs"
    );
}
