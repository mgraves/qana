//! Regenerate the `.qana` typed AST:
//!   cargo run -p qana-lang --bin qana_astgen > crates/qana-lang/src/qana_ast.rs

use qana_grammar::astgen::generate_with_paths;
use qana_grammar::{build_lr, CompiledLexer};
use qana_lang::bootstrap::{qana_lex_grammar, qana_syn_grammar};

fn main() {
    let (g, ids) = qana_lex_grammar();
    let lexer = CompiledLexer::build(&g).expect("qana grammar must be in envelope");
    let (sg, _) = qana_syn_grammar(&ids, &lexer.vocab);
    let tables = build_lr(&sg);
    assert!(tables.conflicts.is_empty(), "no types for conflicted grammars");
    print!("{}", generate_with_paths(&sg, &tables, "qana_grammar"));
}
