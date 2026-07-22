//! rantlr-grammar — P1: grammars as first-class values, compiled to
//! certified table-driven lexers.
//!
//! Pipeline: [`model::LexGrammar`] (a plain value) → per-mode DFAs
//! ([`dfa`]) → envelope lints with counterexamples ([`lints`]) →
//! [`lexer::CompiledLexer`] exposing the same pure line-local
//! `lex_line(text, entry_state)` contract the P0 spike hand-implemented.
//!
//! An out-of-envelope grammar does not produce a lexer — it produces an
//! error carrying a witness (L1: a string the offending token matches
//! across a line break; L2: the unbounded mode-push cycle).

pub mod astgen;
pub mod dfa;
pub mod demo;
pub mod demo_ast;
pub mod green;
pub mod incremental;
pub mod lexer;
pub mod lints;
pub mod lr;
pub mod model;
pub mod parse;
pub mod pat;
pub mod syn;
pub mod typed;

pub use green::{build_green, GreenChild, GreenNode, GreenToken, TokWithText, NEWLINE};
pub use incremental::{
    batch_parse_green, incremental_parse, salvage, FreshRegion, IncParseError, Item, ReuseStats,
};
pub use lexer::{CompiledLexer, MStack, Token, MAX_STACK};
pub use lr::{build_lr, LrAct, LrTables};
pub use model::{Action, BracketKind, LexGrammar, TokenDef, TokenId, Vocab};
pub use parse::{parse, sexpr, PNode, TermTok};
pub use pat::{ClassSet, Pat};
pub use syn::{Assoc, NtId, Sym, SynGrammar, EOF};
pub use typed::{AstNode, NodeRef, SymbolChild, TokenRef};
