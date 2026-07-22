//! Lossless green trees (rowan/Roslyn lineage, minimal P1 form).
//!
//! Green nodes are immutable, position-independent values: kind
//! (nonterminal + production), byte width, and children (nodes or
//! tokens). Tokens carry their text and a trivia flag, so the tree is
//! SELF-CONTAINED: `text()` reconstructs the source byte-for-byte with no
//! vocab in hand — the losslessness invariant, promoted from the token
//! layer to the tree layer.
//!
//! Trivia policy (P1, deterministic): a trivia run attaches as siblings
//! immediately BEFORE the next non-trivia token, at that token's tree
//! position; trailing trivia attaches at the end of the root. Line
//! terminators are synthesized as [`NEWLINE`] trivia tokens (the engine's
//! line model stores them out-of-band). Richer ownership views
//! (doc-comment binding, Roslyn-style trailing trivia) are derived
//! layers over this substrate, not different trees.
//!
//! `Arc` children are the seam Wagner-style incremental reuse (P2)
//! splices through.

use crate::model::TokenId;
use crate::parse::PNode;
use std::sync::Arc;

/// Synthetic token id for line terminators woven into trees.
/// (Distinct from `syn::EOF == u16::MAX`.)
pub const NEWLINE: TokenId = u16::MAX - 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GreenToken {
    pub id: TokenId,
    pub trivia: bool,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GreenChild {
    Node(Arc<GreenNode>),
    Token(GreenToken),
}

impl GreenChild {
    pub fn width(&self) -> u32 {
        match self {
            GreenChild::Node(n) => n.width,
            GreenChild::Token(t) => t.text.len() as u32,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GreenNode {
    /// Nonterminal id (the node's kind).
    pub nt: u16,
    /// Production index (the typed-AST variant discriminator).
    pub prod: u16,
    /// Total byte width including all trivia beneath.
    pub width: u32,
    pub children: Vec<GreenChild>,
}

impl GreenNode {
    pub fn text(&self) -> String {
        let mut s = String::with_capacity(self.width as usize);
        collect_text(self, &mut s);
        s
    }

    /// Children that correspond to grammar symbols (skips trivia).
    pub fn symbol_children(&self) -> impl Iterator<Item = &GreenChild> {
        self.children.iter().filter(|c| match c {
            GreenChild::Token(t) => !t.trivia,
            GreenChild::Node(_) => true,
        })
    }
}

fn collect_text(n: &GreenNode, out: &mut String) {
    for c in &n.children {
        match c {
            GreenChild::Node(m) => collect_text(m, out),
            GreenChild::Token(t) => out.push_str(&t.text),
        }
    }
}

/// A token with text and trivia classification — the builder's input
/// stream (everything the lexer produced, in order, plus synthesized
/// [`NEWLINE`] terminators).
#[derive(Clone, Debug)]
pub struct TokWithText {
    pub id: TokenId,
    pub trivia: bool,
    pub text: String,
}

#[derive(Debug)]
pub enum TreeError {
    /// Parser terminal didn't line up with the token stream — a bug, not
    /// an input condition.
    Desync { expected: TokenId, found: Option<TokenId> },
}

/// Build a lossless green tree from a parse tree + the FULL token stream
/// (trivia included). Consumes every token; the result's `text()` equals
/// the concatenation of all input token texts, byte for byte.
pub fn build_green(root: &PNode, all: &[TokWithText]) -> Result<Arc<GreenNode>, TreeError> {
    let mut cur = 0usize;
    let node = build_node(root, all, &mut cur)?;
    let GreenChild::Node(node) = node else {
        // Every accept wraps the start production in a Rule node, so a
        // bare-token root cannot occur; refusing keeps the trivia
        // accounting airtight.
        unreachable!("parse roots are always rule nodes");
    };
    let mut node = (*node).clone();
    // Trailing trivia (incl. final newline) attaches to the root.
    while cur < all.len() && all[cur].trivia {
        let t = &all[cur];
        node.width += t.text.len() as u32;
        node.children.push(GreenChild::Token(GreenToken {
            id: t.id,
            trivia: true,
            text: t.text.clone(),
        }));
        cur += 1;
    }
    if cur != all.len() {
        return Err(TreeError::Desync { expected: all[cur].id, found: None });
    }
    Ok(Arc::new(node))
}

fn build_node(
    pnode: &PNode,
    all: &[TokWithText],
    cur: &mut usize,
) -> Result<GreenChild, TreeError> {
    match pnode {
        PNode::Tok { .. } => {
            // Interior terminals are handled inside the Rule arm (so
            // leading trivia lands in the right parent), and roots are
            // always rules — this arm is structurally unreachable.
            unreachable!("bare-token parse node outside a rule")
        }
        PNode::Rule { prod, nt, children } => {
            let mut out: Vec<GreenChild> = Vec::with_capacity(children.len());
            let mut width = 0u32;
            for c in children {
                match c {
                    PNode::Tok { id, .. } => {
                        // Leading trivia becomes siblings before the terminal.
                        while *cur < all.len() && all[*cur].trivia {
                            let t = &all[*cur];
                            width += t.text.len() as u32;
                            out.push(GreenChild::Token(GreenToken {
                                id: t.id,
                                trivia: true,
                                text: t.text.clone(),
                            }));
                            *cur += 1;
                        }
                        let Some(t) = all.get(*cur) else {
                            return Err(TreeError::Desync { expected: *id, found: None });
                        };
                        if t.id != *id {
                            return Err(TreeError::Desync { expected: *id, found: Some(t.id) });
                        }
                        width += t.text.len() as u32;
                        out.push(GreenChild::Token(GreenToken {
                            id: t.id,
                            trivia: false,
                            text: t.text.clone(),
                        }));
                        *cur += 1;
                    }
                    rule => {
                        let child = build_node(rule, all, cur)?;
                        width += child.width();
                        out.push(child);
                    }
                }
            }
            Ok(GreenChild::Node(Arc::new(GreenNode {
                nt: *nt,
                prod: *prod,
                width,
                children: out,
            })))
        }
    }
}

// ---------------------------------------------------------------------------
// Offset queries (the red-tree preview: enough for selection expansion)
// ---------------------------------------------------------------------------

/// All ancestor spans (outermost → innermost) covering `offset`, ending
/// with the token span itself. This is LSP `selectionRange` in substrate
/// form: each span strictly contains the next.
pub fn ancestor_spans(root: &GreenNode, offset: u32) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    let mut node = root;
    let mut base = 0u32;
    if offset >= root.width {
        return out;
    }
    'descend: loop {
        out.push((base, base + node.width));
        let mut off = base;
        for c in &node.children {
            let w = c.width();
            if offset < off + w {
                match c {
                    GreenChild::Node(n) => {
                        base = off;
                        node = n;
                        continue 'descend;
                    }
                    GreenChild::Token(_) => {
                        out.push((off, off + w));
                        return out;
                    }
                }
            }
            off += w;
        }
        return out; // zero-width tail (shouldn't happen with coverage)
    }
}

/// The token containing `offset`, with its absolute span.
pub fn token_at_offset(root: &GreenNode, offset: u32) -> Option<(&GreenToken, u32, u32)> {
    if offset >= root.width {
        return None;
    }
    let mut node = root;
    let mut base = 0u32;
    'descend: loop {
        let mut off = base;
        for c in &node.children {
            let w = c.width();
            if offset < off + w {
                match c {
                    GreenChild::Node(n) => {
                        base = off;
                        node = n;
                        continue 'descend;
                    }
                    GreenChild::Token(t) => return Some((t, off, off + w)),
                }
            }
            off += w;
        }
        return None;
    }
}
