//! Regenerate the `.rg` typed AST:
//!   cargo run -p qana-rg --bin rg_astgen > crates/qana-rg/src/rg_ast.rs

use qana_grammar::astgen::generate_with_paths;
use qana_grammar::{build_lr, CompiledLexer};
use qana_rg::bootstrap::{rg_lex_grammar, rg_syn_grammar};

fn main() {
    let (g, ids) = rg_lex_grammar();
    let lexer = CompiledLexer::build(&g).expect("rg grammar must be in envelope");
    let (sg, _) = rg_syn_grammar(&ids, &lexer.vocab);
    let tables = build_lr(&sg);
    assert!(tables.conflicts.is_empty(), "no types for conflicted grammars");
    print!("{}", generate_with_paths(&sg, &tables, "qana_grammar"));
}
