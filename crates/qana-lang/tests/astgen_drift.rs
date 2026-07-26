//! Drift gate: the checked-in typed AST for the `.qana` grammar must match
//! what the generator produces from the bootstrap values today.

use qana_grammar::astgen::generate_with_paths;
use qana_grammar::{build_lr, CompiledLexer};
use qana_lang::bootstrap::{qana_lex_grammar, qana_syn_grammar};

#[test]
fn generated_rg_ast_is_current() {
    let (g, ids) = qana_lex_grammar();
    let lexer = CompiledLexer::build(&g).unwrap();
    let (sg, _) = qana_syn_grammar(&ids, &lexer.vocab);
    let tables = build_lr(&sg);
    let want = generate_with_paths(&sg, &tables, "qana_grammar");
    assert_eq!(
        include_str!("../src/qana_ast.rs"),
        want,
        "regenerate: cargo run -p qana-lang --bin qana_astgen > crates/qana-lang/src/qana_ast.rs"
    );
}
