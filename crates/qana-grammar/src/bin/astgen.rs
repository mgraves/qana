//! Regenerate the demo typed AST:
//!   cargo run -p qana-grammar --bin astgen > crates/qana-grammar/src/demo_ast.rs

use qana_grammar::astgen::generate;
use qana_grammar::demo::{demo_grammar, demo_syn_grammar};
use qana_grammar::{build_lr, CompiledLexer, Vocab};

fn main() {
    let (g, ids) = demo_grammar();
    let vocab = Vocab::of(&g);
    // Certify before generating: no types for out-of-envelope grammars.
    CompiledLexer::build(&g).expect("demo grammar must be in envelope");
    let sg = demo_syn_grammar(&ids, &vocab);
    let tables = build_lr(&sg);
    assert!(tables.conflicts.is_empty(), "no types for conflicted grammars");
    print!("{}", generate(&sg, &tables));
}
