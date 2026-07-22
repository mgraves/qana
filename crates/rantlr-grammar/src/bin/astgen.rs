//! Regenerate the demo typed AST:
//!   cargo run -p rantlr-grammar --bin astgen > crates/rantlr-grammar/src/demo_ast.rs

use rantlr_grammar::astgen::generate;
use rantlr_grammar::demo::{demo_grammar, demo_syn_grammar};
use rantlr_grammar::{CompiledLexer, Vocab};

fn main() {
    let (g, ids) = demo_grammar();
    let vocab = Vocab::of(&g);
    // Certify before generating: no types for out-of-envelope grammars.
    CompiledLexer::build(&g).expect("demo grammar must be in envelope");
    let sg = demo_syn_grammar(&ids, &vocab);
    print!("{}", generate(&sg));
}
