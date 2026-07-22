//! Runtime support for generated typed ASTs: thin `Copy` references over
//! green nodes with symbol-indexed navigation. Generated code (see
//! `astgen`) is a set of zero-cost wrappers over these two types — the
//! mechanism by which a grammar change becomes downstream compile errors
//! ("ramification as type errors").

use crate::green::{GreenChild, GreenNode, GreenToken};
use crate::model::TokenId;

#[derive(Clone, Copy, Debug)]
pub struct NodeRef<'g>(pub &'g GreenNode);

#[derive(Clone, Copy, Debug)]
pub struct TokenRef<'g>(pub &'g GreenToken);

#[derive(Clone, Copy, Debug)]
pub enum SymbolChild<'g> {
    Node(NodeRef<'g>),
    Token(TokenRef<'g>),
}

impl<'g> NodeRef<'g> {
    pub fn nt(&self) -> u16 {
        self.0.nt
    }
    pub fn prod(&self) -> u16 {
        self.0.prod
    }
    pub fn width(&self) -> u32 {
        self.0.width
    }
    pub fn text(&self) -> String {
        self.0.text()
    }

    /// The k-th grammar-symbol child (trivia skipped) — the generated
    /// accessors' navigation primitive. Positions correspond 1:1 to the
    /// production's RHS symbols.
    pub fn symbol_child(&self, k: usize) -> Option<SymbolChild<'g>> {
        self.0
            .symbol_children()
            .nth(k)
            .map(|c| match c {
                GreenChild::Node(n) => SymbolChild::Node(NodeRef(n)),
                GreenChild::Token(t) => SymbolChild::Token(TokenRef(t)),
            })
    }

    pub fn child_node(&self, k: usize) -> Option<NodeRef<'g>> {
        match self.symbol_child(k)? {
            SymbolChild::Node(n) => Some(n),
            SymbolChild::Token(_) => None,
        }
    }

    pub fn child_token(&self, k: usize, expect: TokenId) -> Option<TokenRef<'g>> {
        match self.symbol_child(k)? {
            SymbolChild::Token(t) if t.0.id == expect => Some(t),
            _ => None,
        }
    }
}

impl<'g> TokenRef<'g> {
    pub fn id(&self) -> TokenId {
        self.0.id
    }
    pub fn text(&self) -> &'g str {
        &self.0.text
    }
}

/// Implemented by every generated typed wrapper.
pub trait AstNode<'g>: Sized {
    fn cast(node: NodeRef<'g>) -> Option<Self>;
    fn node(&self) -> NodeRef<'g>;
}
