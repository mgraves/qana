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

impl GreenToken {
    /// A zero-width token inserted by error repair ("the parser pretended
    /// to see this"). Sound encoding: the empty-match lint guarantees no
    /// real token ever has empty text.
    pub fn is_missing(&self) -> bool {
        !self.trivia && self.text.is_empty()
    }
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
    /// Non-trivia terminals beneath (incremental-reuse bookkeeping).
    /// Missing (repair-inserted) tokens count zero — the counts mirror
    /// the BUFFER's real token stream exactly, which is what keeps
    /// salvage alignment sound on repaired trees.
    pub terms: u32,
    /// ALL real tokens beneath, trivia included (damage alignment).
    pub n_toks: u32,
    /// Contains error material (an ERROR node or missing token) anywhere
    /// beneath — such nodes never splice; salvage dissolves them.
    pub has_err: bool,
    pub children: Vec<GreenChild>,
}

impl GreenNode {
    pub fn text(&self) -> String {
        let mut s = String::with_capacity(self.width as usize);
        collect_text(self, &mut s);
        s
    }

    /// Children that correspond to grammar symbols: trivia, ERROR nodes,
    /// and repair-inserted missing tokens are all skipped, so typed
    /// accessors keep their positional meaning on repaired trees.
    pub fn symbol_children(&self) -> impl Iterator<Item = &GreenChild> {
        self.children.iter().filter(|c| match c {
            GreenChild::Token(t) => !t.trivia && !t.is_missing(),
            GreenChild::Node(n) => n.nt != ERROR_NT,
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
        node.n_toks += 1;
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
            let mut terms = 0u32;
            let mut n_toks = 0u32;
            for c in children {
                match c {
                    PNode::Tok { id, .. } => {
                        // Leading trivia becomes siblings before the terminal.
                        while *cur < all.len() && all[*cur].trivia {
                            let t = &all[*cur];
                            width += t.text.len() as u32;
                            n_toks += 1;
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
                        terms += 1;
                        n_toks += 1;
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
                        if let GreenChild::Node(n) = &child {
                            terms += n.terms;
                            n_toks += n.n_toks;
                        }
                        out.push(child);
                    }
                }
            }
            Ok(GreenChild::Node(Arc::new(GreenNode {
                nt: *nt,
                prod: *prod,
                width,
                terms,
                n_toks,
                // The PNode path parses error-free by construction.
                has_err: false,
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

// ---------------------------------------------------------------------------
// L4: balanced sequences — sentinel kinds, semantic equality, invariants
// ---------------------------------------------------------------------------

/// Production sentinel for a LIST node (the actual nonterminal node of a
/// list-shaped rule; children = elements directly, or balanced RUN nodes).
pub const LIST_PROD: u16 = u16::MAX - 1;
/// Production sentinel for a RUN node (an internal balanced chunk of a
/// list; same `nt` as its list; fanout ≤ [`MAX_RUN`]).
pub const RUN_PROD: u16 = u16::MAX - 2;
/// Kind of ERROR nodes (wrapping tokens skipped during repair). They
/// attach like trivia, are excluded from symbol positions, and poison
/// their ancestors' `has_err`.
pub const ERROR_NT: u16 = u16::MAX;
pub const ERROR_PROD: u16 = u16::MAX - 3;
/// Maximum fanout of RUN/LIST grouping.
pub const MAX_RUN: usize = 16;

pub fn is_seq_prod(p: u16) -> bool {
    p == LIST_PROD || p == RUN_PROD
}

/// Children with RUN nodes expanded inline — the flattened view under
/// which list association is (by declaration) meaningless.
pub fn flat_children(n: &GreenNode) -> Vec<&GreenChild> {
    let mut out = Vec::new();
    fn go<'a>(n: &'a GreenNode, out: &mut Vec<&'a GreenChild>) {
        for c in &n.children {
            match c {
                GreenChild::Node(m) if m.prod == RUN_PROD => go(m, out),
                other => out.push(other),
            }
        }
    }
    go(n, &mut out);
    out
}

/// Semantic tree equality: full structural equality everywhere, except
/// list nodes compare by their FLATTENED child sequences (run structure
/// is representation, not meaning — envelope L4). Trivia text and
/// placement still compare exactly at the flattened level.
pub fn semantic_eq(a: &GreenNode, b: &GreenNode) -> bool {
    if a.nt != b.nt {
        return false;
    }
    let (al, bl) = (a.prod == LIST_PROD, b.prod == LIST_PROD);
    if al != bl {
        return false;
    }
    if !al && a.prod != b.prod {
        return false;
    }
    let fa = flat_children(a);
    let fb = flat_children(b);
    if fa.len() != fb.len() {
        return false;
    }
    fa.iter().zip(&fb).all(|(x, y)| match (x, y) {
        (GreenChild::Token(t), GreenChild::Token(u)) => {
            t.id == u.id && t.trivia == u.trivia && t.text == u.text
        }
        (GreenChild::Node(m), GreenChild::Node(n)) => semantic_eq(m, n),
        _ => false,
    })
}

/// Balance invariants: every LIST/RUN node has fanout ≤ MAX_RUN; RUN
/// children never include LIST nodes; counts (width/terms/n_toks) are
/// consistent everywhere.
pub fn check_balance(n: &GreenNode) -> Result<(), String> {
    if is_seq_prod(n.prod) && n.children.len() > MAX_RUN {
        return Err(format!(
            "seq node nt={} has fanout {} > {}",
            n.nt,
            n.children.len(),
            MAX_RUN
        ));
    }
    let (mut w, mut te, mut tk) = (0u32, 0u32, 0u32);
    for c in &n.children {
        match c {
            GreenChild::Token(t) => {
                if t.is_missing() {
                    continue; // zero-count by the repair convention
                }
                w += t.text.len() as u32;
                tk += 1;
                if !t.trivia {
                    te += 1;
                }
            }
            GreenChild::Node(m) => {
                if n.prod == RUN_PROD && m.prod == LIST_PROD {
                    return Err("LIST node nested inside RUN".to_string());
                }
                check_balance(m)?;
                w += m.width;
                te += m.terms;
                tk += m.n_toks;
            }
        }
    }
    if (w, te, tk) != (n.width, n.terms, n.n_toks) {
        return Err(format!(
            "count mismatch at nt={} prod={}: stored ({},{},{}) computed ({w},{te},{tk})",
            n.nt, n.prod, n.width, n.terms, n.n_toks
        ));
    }
    Ok(())
}
