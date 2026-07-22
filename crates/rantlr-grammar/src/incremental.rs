//! Wagner-style incremental LR parsing over green trees (TOPLAS 1998
//! lineage, first increment).
//!
//! The parser consumes a SENTENTIAL FORM: a queue of clean subtrees
//! salvaged from the previous tree interleaved with fresh tokens for the
//! damaged regions. Clean subtrees are shifted wholesale when the LR
//! automaton has a GOTO for their nonterminal (Wagner's nonterminal-shift
//! check); otherwise — or when rooted at a FRAGILE production (one whose
//! shape was decided by precedence resolution, Wagner §6) — they break
//! down into their children and the parse continues at finer grain.
//! Reduce decisions under a subtree lookahead use the subtree's leftmost
//! terminal.
//!
//! Trees are built during parsing (no intermediate PNode): value-stack
//! entries carry leading trivia so tokens sit exactly where the batch
//! weave puts them, and trivia pending before a spliced subtree is
//! injected down its left spine — making `incremental ≡ batch` hold as
//! FULL tree equality, not just text equality. That equality is the
//! permanent differential gate.
//!
//! Deliberate scope notes: node reuse here is Wagner's bottom-up kind
//! (top-down reuse and the optimality postpass are refinements);
//! sequences are not yet auto-balanced (envelope L4 — next increment),
//! so clean-prefix splices are O(1) but suffix statements re-wrap
//! per-item; error recovery remains out of scope.

use crate::green::{
    is_seq_prod, GreenChild, GreenNode, GreenToken, TokWithText, LIST_PROD, MAX_RUN, RUN_PROD,
};
use crate::lr::{LrAct, LrTables};
use crate::model::TokenId;
use crate::syn::{SynGrammar, EOF};
use std::collections::VecDeque;
use std::sync::Arc;

/// One input item of the sentential form.
#[derive(Clone, Debug)]
pub enum Item {
    /// A subtree (or single token) salvaged from the previous tree.
    Sub(GreenChild),
    /// A freshly lexed token from a damaged region.
    Fresh(TokWithText),
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ReuseStats {
    /// Non-trivia terminals covered by spliced subtrees.
    pub reused_terms: u32,
    /// Total non-trivia terminals in the new parse.
    pub total_terms: u32,
    /// Number of whole-subtree splices performed.
    pub splices: u32,
    /// Number of breakdowns (dirty, unshiftable, or fragile).
    pub breakdowns: u32,
}

impl ReuseStats {
    pub fn reuse_fraction(&self) -> f64 {
        if self.total_terms == 0 {
            1.0
        } else {
            self.reused_terms as f64 / self.total_terms as f64
        }
    }
}

#[derive(Clone, Debug)]
pub struct IncParseError {
    /// Terminals consumed before the error.
    pub at_terminal: usize,
    pub found: String,
    pub expected: Vec<String>,
}

impl std::fmt::Display for IncParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "syntax error at terminal #{} ({}): expected one of {}",
            self.at_terminal,
            self.found,
            self.expected.join(", ")
        )
    }
}

/// L4: an under-construction balanced list. Batch and incremental share
/// this — batch appends elements one cons-reduce at a time; incremental
/// additionally absorbs whole salvaged runs (the associative
/// concatenation Wagner's `B → B B` nondeterministic view licenses).
struct ListBuilder {
    nt: u16,
    pieces: Vec<Piece>,
}

enum Piece {
    /// A single child (element node, separator/trivia token).
    Loose(GreenChild),
    /// A reused leaf run (fanout ≤ MAX_RUN, children are elements).
    Leaf(Arc<GreenNode>),
}

impl ListBuilder {
    fn new(nt: u16) -> Self {
        ListBuilder { nt, pieces: Vec::new() }
    }
    fn push_loose(&mut self, c: GreenChild) {
        self.pieces.push(Piece::Loose(c));
    }
    /// Absorb a salvaged LIST or RUN node: reused chunks flatten to leaf
    /// runs (never to elements — that's what keeps splices O(runs)).
    fn absorb_seq(&mut self, n: &Arc<GreenNode>) {
        let is_leaf = n.prod == RUN_PROD
            && n.children
                .iter()
                .all(|c| !matches!(c, GreenChild::Node(m) if m.prod == RUN_PROD));
        if is_leaf {
            self.pieces.push(Piece::Leaf(n.clone()));
            return;
        }
        for c in &n.children {
            match c {
                GreenChild::Node(m) if m.prod == RUN_PROD => self.absorb_seq(m),
                other => self.pieces.push(Piece::Loose(other.clone())),
            }
        }
    }
    /// Deterministic balanced form: loose pieces chunk into ≤MAX_RUN leaf
    /// runs (small neighbors of reused runs dissolve into them), then
    /// levels group 16-ary until one node remains. Lists of ≤MAX_RUN
    /// loose children skip the run layer entirely.
    fn finalize(self) -> Arc<GreenNode> {
        let nt = self.nt;
        if self.pieces.len() <= MAX_RUN
            && self.pieces.iter().all(|p| matches!(p, Piece::Loose(_)))
        {
            let children: Vec<GreenChild> = self
                .pieces
                .into_iter()
                .map(|p| match p {
                    Piece::Loose(c) => c,
                    Piece::Leaf(_) => unreachable!(),
                })
                .collect();
            return make_node(nt, LIST_PROD, children);
        }
        let mut leaves: Vec<Arc<GreenNode>> = Vec::new();
        let mut buf: Vec<GreenChild> = Vec::new();
        fn flush(nt: u16, buf: &mut Vec<GreenChild>, leaves: &mut Vec<Arc<GreenNode>>) {
            while !buf.is_empty() {
                let take = buf.len().min(MAX_RUN);
                let chunk: Vec<GreenChild> = buf.drain(..take).collect();
                leaves.push(make_node(nt, RUN_PROD, chunk));
            }
        }
        for p in self.pieces {
            match p {
                Piece::Loose(c) => buf.push(c),
                Piece::Leaf(r) => {
                    if !buf.is_empty() && buf.len() + r.children.len() <= MAX_RUN {
                        // Dissolve the run into the pending buffer so edit
                        // seams don't accumulate tiny runs.
                        buf.extend(r.children.iter().cloned());
                    } else {
                        flush(nt, &mut buf, &mut leaves);
                        leaves.push(r);
                    }
                }
            }
        }
        flush(nt, &mut buf, &mut leaves);
        let mut level: Vec<GreenChild> = leaves.into_iter().map(GreenChild::Node).collect();
        while level.len() > MAX_RUN {
            let mut next = Vec::with_capacity(level.len() / MAX_RUN + 1);
            for chunk in level.chunks(MAX_RUN) {
                next.push(GreenChild::Node(make_node(nt, RUN_PROD, chunk.to_vec())));
            }
            level = next;
        }
        make_node(nt, LIST_PROD, level)
    }
}

enum SymSlot {
    Child(GreenChild),
    List(ListBuilder),
}

impl SymSlot {
    fn into_child(self) -> GreenChild {
        match self {
            SymSlot::Child(c) => c,
            SymSlot::List(b) => GreenChild::Node(b.finalize()),
        }
    }
}

struct Entry {
    /// Trivia preceding `sym`, flattened before it on reduce.
    leading: Vec<GreenChild>,
    sym: SymSlot,
}

fn make_node(nt: u16, prod: u16, children: Vec<GreenChild>) -> Arc<GreenNode> {
    let mut width = 0u32;
    let mut terms = 0u32;
    let mut n_toks = 0u32;
    for c in &children {
        match c {
            GreenChild::Node(n) => {
                width += n.width;
                terms += n.terms;
                n_toks += n.n_toks;
            }
            GreenChild::Token(t) => {
                width += t.text.len() as u32;
                n_toks += 1;
                if !t.trivia {
                    terms += 1;
                }
            }
        }
    }
    Arc::new(GreenNode { nt, prod, width, terms, n_toks, children })
}

/// Leftmost non-trivia terminal beneath a child, if any. Iterative
/// descent along the first terminal-bearing child — O(depth), and only
/// computed when actually needed (it's O(spine) on unbalanced lists, so
/// the hot splice path must never touch it).
fn leftmost_term(c: &GreenChild) -> Option<TokenId> {
    let mut cur = c;
    loop {
        match cur {
            GreenChild::Token(t) => return (!t.trivia).then_some(t.id),
            GreenChild::Node(n) => {
                cur = n.children.iter().find(|ch| match ch {
                    GreenChild::Token(t) => !t.trivia,
                    GreenChild::Node(m) => m.terms > 0,
                })?;
            }
        }
    }
}

/// Inject pending trivia down the left spine of a subtree so it sits
/// immediately before the subtree's first terminal — AND before that
/// terminal's own contiguous leading-trivia run, because pending trivia
/// is stream-earlier. (Batch-shaped trees carry trivia only immediately
/// before tokens at the same level, which is what makes this placement
/// exactly the batch builder's.) Only called for subtrees with ≥1
/// terminal.
fn prepend_trivia(node: &Arc<GreenNode>, trivia: Vec<GreenChild>) -> Arc<GreenNode> {
    debug_assert!(node.terms > 0);
    let mut n = (**node).clone();
    let extra_w: u32 = trivia.iter().map(|c| c.width()).sum();
    let extra_t = trivia.len() as u32;
    n.width += extra_w;
    n.n_toks += extra_t;
    // First child containing a terminal (exists: terms > 0).
    let idx = n
        .children
        .iter()
        .position(|c| match c {
            GreenChild::Token(t) => !t.trivia,
            GreenChild::Node(m) => m.terms > 0,
        })
        .expect("terms > 0 implies a symbol child with terminals");
    match &n.children[idx] {
        GreenChild::Token(_) => {
            // Back over the trivia run that immediately precedes the
            // first terminal at this level; pending goes before it.
            let mut ins = idx;
            while ins > 0
                && matches!(&n.children[ins - 1], GreenChild::Token(t) if t.trivia)
            {
                ins -= 1;
            }
            n.children.splice(ins..ins, trivia);
        }
        GreenChild::Node(m) => {
            // Children before the first terminal-bearing node can only be
            // zero-width ε-nodes (batch trees never put trivia before a
            // node child), so recursing into it preserves batch placement.
            debug_assert!(
                n.children[..idx]
                    .iter()
                    .all(|c| matches!(c, GreenChild::Node(e) if e.terms == 0)),
                "unexpected material before the first terminal-bearing child"
            );
            let m2 = prepend_trivia(m, trivia);
            n.children[idx] = GreenChild::Node(m2);
        }
    }
    Arc::new(n)
}

/// Incremental (and batch — pass all-fresh input) LR parse producing a
/// lossless green tree.
pub fn incremental_parse(
    g: &SynGrammar,
    t: &LrTables,
    mut input: VecDeque<Item>,
) -> Result<(Arc<GreenNode>, ReuseStats), IncParseError> {
    let mut stats = ReuseStats::default();
    let mut states: Vec<u16> = vec![0];
    let mut stack: Vec<Entry> = Vec::new();
    let mut pending: Vec<GreenChild> = Vec::new();
    let mut consumed_terms = 0usize;

    macro_rules! reduce {
        ($pidx:expr) => {{
            let pidx: u16 = $pidx;
            let prod = &g.prods[pidx as usize];
            let k = prod.rhs.len();
            let split = stack.len() - k;
            let popped: Vec<Entry> = stack.drain(split..).collect();
            states.truncate(states.len() - k);
            let top = *states.last().unwrap();
            let next = *t.goto_[top as usize].get(&prod.lhs).expect("goto after reduce");
            let shape = t.lists.get(&prod.lhs).copied();
            let slot = match shape {
                // L4 cons: append α to the open builder instead of nesting.
                Some(sh) if pidx == sh.cons => {
                    let mut it = popped.into_iter();
                    let head = it.next().unwrap();
                    debug_assert!(head.leading.is_empty(), "list slot never has leading");
                    let mut b = match head.sym {
                        SymSlot::List(b) => b,
                        SymSlot::Child(GreenChild::Node(n)) if is_seq_prod(n.prod) => {
                            let mut b = ListBuilder::new(prod.lhs);
                            b.absorb_seq(&n);
                            b
                        }
                        SymSlot::Child(other) => {
                            let mut b = ListBuilder::new(prod.lhs);
                            b.push_loose(other);
                            b
                        }
                    };
                    for e in it {
                        for tr in e.leading {
                            b.push_loose(tr);
                        }
                        b.push_loose(e.sym.into_child());
                    }
                    SymSlot::List(b)
                }
                // L4 seed (incl. ε): open a fresh builder with the seed.
                Some(_) => {
                    let mut b = ListBuilder::new(prod.lhs);
                    for e in popped {
                        for tr in e.leading {
                            b.push_loose(tr);
                        }
                        b.push_loose(e.sym.into_child());
                    }
                    SymSlot::List(b)
                }
                // Ordinary production: build the node.
                None => {
                    let mut children: Vec<GreenChild> = Vec::new();
                    for e in popped {
                        children.extend(e.leading);
                        children.push(e.sym.into_child());
                    }
                    SymSlot::Child(GreenChild::Node(make_node(prod.lhs, pidx, children)))
                }
            };
            stack.push(Entry { leading: Vec::new(), sym: slot });
            states.push(next);
        }};
    }

    loop {
        let state = *states.last().unwrap();
        match input.front() {
            None => {
                // End of input: EOF actions.
                match t.action[state as usize].get(&EOF).copied() {
                    Some(LrAct::Reduce(p)) => reduce!(p),
                    Some(LrAct::Accept) => {
                        debug_assert_eq!(stack.len(), 1);
                        let entry = stack.pop().unwrap();
                        debug_assert!(entry.leading.is_empty(), "leading on root entry");
                        let GreenChild::Node(root) = entry.sym.into_child() else {
                            unreachable!("accept pops a rule node")
                        };
                        let mut root_owned = (*root).clone();
                        for c in pending.drain(..) {
                            root_owned.width += c.width();
                            root_owned.n_toks += 1;
                            root_owned.children.push(c);
                        }
                        stats.total_terms = consumed_terms as u32;
                        return Ok((Arc::new(root_owned), stats));
                    }
                    _ => {
                        return Err(IncParseError {
                            at_terminal: consumed_terms,
                            found: "<eof>".to_string(),
                            expected: t.expected_tokens(state, g),
                        })
                    }
                }
                continue;
            }
            Some(Item::Fresh(tok)) if tok.trivia => {
                let tok = tok.clone();
                input.pop_front();
                pending.push(GreenChild::Token(GreenToken {
                    id: tok.id,
                    trivia: true,
                    text: tok.text,
                }));
                continue;
            }
            Some(Item::Sub(GreenChild::Token(tk))) if tk.trivia => {
                let tk = tk.clone();
                input.pop_front();
                pending.push(GreenChild::Token(tk));
                continue;
            }
            _ => {}
        }

        // Non-trivia lookahead: terminal or subtree.
        let (la_term, is_node) = match input.front().unwrap() {
            Item::Fresh(tok) => (Some(tok.id), false),
            Item::Sub(GreenChild::Token(tk)) => (Some(tk.id), false),
            Item::Sub(GreenChild::Node(_)) => (None, true), // computed lazily below
        };

        if is_node {
            let Some(Item::Sub(GreenChild::Node(node))) = input.front() else { unreachable!() };
            let node = node.clone();
            let seq = is_seq_prod(node.prod);

            // L4 associative splice: a salvaged LIST/RUN of a list
            // nonterminal either CONCATENATES into the open builder on
            // top of the stack (Wagner's B → B B, no state change) or
            // seeds a new builder via GOTO.
            if seq && t.lists.contains_key(&node.nt) {
                let continues = matches!(
                    stack.last(),
                    Some(Entry { sym: SymSlot::List(b), .. }) if b.nt == node.nt
                );
                if continues {
                    input.pop_front();
                    // Pending trivia belongs INSIDE the run's first
                    // element (where batch drains it at the next token
                    // shift) — never at list level.
                    let spliced = if pending.is_empty() || node.terms == 0 {
                        node.clone()
                    } else {
                        prepend_trivia(&node, std::mem::take(&mut pending))
                    };
                    stats.reused_terms += spliced.terms;
                    stats.splices += 1;
                    consumed_terms += spliced.terms as usize;
                    let Some(Entry { sym: SymSlot::List(b), .. }) = stack.last_mut() else {
                        unreachable!()
                    };
                    b.absorb_seq(&spliced);
                    continue;
                }
                if let Some(next) = t.goto_[state as usize].get(&node.nt).copied() {
                    input.pop_front();
                    let spliced = if pending.is_empty() || node.terms == 0 {
                        node.clone()
                    } else {
                        prepend_trivia(&node, std::mem::take(&mut pending))
                    };
                    let mut b = ListBuilder::new(node.nt);
                    stats.reused_terms += spliced.terms;
                    stats.splices += 1;
                    consumed_terms += spliced.terms as usize;
                    b.absorb_seq(&spliced);
                    stack.push(Entry { leading: Vec::new(), sym: SymSlot::List(b) });
                    states.push(next);
                    continue;
                }
                // Fall through to reduce-enable/breakdown below.
            }

            let fragile = !seq
                && (node.prod as usize) < t.fragile.len()
                && t.fragile[node.prod as usize];
            // Wagner's order: SPLICE the moment the automaton has a GOTO
            // for this nonterminal (fragile nodes never splice); reduces
            // are only performed to ENABLE a future splice when the node
            // isn't directly shiftable.
            if !fragile && !seq {
                if let Some(next) = t.goto_[state as usize].get(&node.nt).copied() {
                    input.pop_front();
                    let spliced = if pending.is_empty() || node.terms == 0 {
                        node.clone()
                    } else {
                        prepend_trivia(&node, std::mem::take(&mut pending))
                    };
                    stats.reused_terms += spliced.terms;
                    stats.splices += 1;
                    consumed_terms += spliced.terms as usize;
                    stack.push(Entry {
                        leading: Vec::new(),
                        sym: SymSlot::Child(GreenChild::Node(spliced)),
                    });
                    states.push(next);
                    continue;
                }
            }
            if !fragile {
                // Not shiftable here — a reduce on the leftmost terminal
                // may bring the automaton to a state where it is.
                if let Some(la) = leftmost_term(&GreenChild::Node(node.clone())) {
                    if let Some(LrAct::Reduce(p)) = t.action[state as usize].get(&la).copied() {
                        reduce!(p);
                        continue;
                    }
                }
            }
            // Fragile or genuinely unshiftable: break into children.
            input.pop_front();
            stats.breakdowns += 1;
            for c in node.children.iter().rev() {
                input.push_front(Item::Sub(c.clone()));
            }
            continue;
        }

        // Terminal lookahead.
        let la = la_term.unwrap();
        match t.action[state as usize].get(&la).copied() {
            Some(LrAct::Shift(next)) => {
                let sym = match input.pop_front().unwrap() {
                    Item::Fresh(tok) => GreenChild::Token(GreenToken {
                        id: tok.id,
                        trivia: false,
                        text: tok.text,
                    }),
                    Item::Sub(c) => {
                        // A salvaged token is reuse too (finer-grained
                        // than a subtree splice, but not fresh work).
                        stats.reused_terms += 1;
                        c
                    }
                };
                consumed_terms += 1;
                stack.push(Entry {
                    leading: std::mem::take(&mut pending),
                    sym: SymSlot::Child(sym),
                });
                states.push(next);
            }
            Some(LrAct::Reduce(p)) => reduce!(p),
            Some(LrAct::Accept) => unreachable!("accept only on EOF"),
            Some(LrAct::Error) | None => {
                let found = match input.front().unwrap() {
                    Item::Fresh(tok) => format!("`{}`", tok.text),
                    Item::Sub(GreenChild::Token(tk)) => format!("`{}`", tk.text),
                    _ => unreachable!(),
                };
                return Err(IncParseError {
                    at_terminal: consumed_terms,
                    found,
                    expected: t.expected_tokens(state, g),
                });
            }
        }
    }
}

/// Batch parse through the same builder: all-fresh input. This is the
/// oracle side of the incremental ≡ batch gate.
pub fn batch_parse_green(
    g: &SynGrammar,
    t: &LrTables,
    all: &[TokWithText],
) -> Result<Arc<GreenNode>, IncParseError> {
    let input: VecDeque<Item> = all.iter().cloned().map(Item::Fresh).collect();
    incremental_parse(g, t, input).map(|(tree, _)| tree)
}

// ---------------------------------------------------------------------------
// Salvage: turn (old tree + damage intervals + fresh regions) into the
// sentential-form input queue.
// ---------------------------------------------------------------------------

/// A damaged region in OLD full-token coordinates, with the fresh tokens
/// (trivia included) that replace it. `old_toks.0 == old_toks.1` encodes
/// a pure insertion point.
#[derive(Clone, Debug)]
pub struct FreshRegion {
    pub old_toks: (u32, u32),
    pub tokens: Vec<TokWithText>,
}

/// Walk the old tree emitting maximal clean subtrees, skipping tokens in
/// damaged intervals, and splicing each region's fresh tokens at its
/// position. Regions must be sorted and non-overlapping.
pub fn salvage(old: &Arc<GreenNode>, regions: &[FreshRegion]) -> VecDeque<Item> {
    let mut out = VecDeque::new();
    // Fresh-emission cursor. NOTE: the dirtiness test always scans ALL
    // regions — a region must keep marking old tokens dirty after its
    // fresh tokens were emitted, or a node starting exactly at a region
    // boundary would be judged clean and duplicated alongside its
    // replacement.
    let mut emitted = 0usize;
    let mut tok_index = 0u32;
    walk(&GreenChild::Node(old.clone()), regions, &mut emitted, &mut tok_index, &mut out);
    // Trailing regions (insertions at EOF).
    while emitted < regions.len() {
        for tok in &regions[emitted].tokens {
            out.push_back(Item::Fresh(tok.clone()));
        }
        emitted += 1;
    }
    return out;

    fn flush_fresh_at(pos: u32, regions: &[FreshRegion], emitted: &mut usize, out: &mut VecDeque<Item>) {
        while *emitted < regions.len() && regions[*emitted].old_toks.0 <= pos {
            for tok in &regions[*emitted].tokens {
                out.push_back(Item::Fresh(tok.clone()));
            }
            *emitted += 1;
        }
    }

    fn dirty(span: (u32, u32), regions: &[FreshRegion]) -> bool {
        // Does [span.0, span.1) intersect any region — counting pure
        // insertion points strictly inside the span?
        regions.iter().any(|r| {
            let (a, b) = r.old_toks;
            if a == b {
                span.0 < a && a < span.1
            } else {
                span.0 < b && a < span.1
            }
        })
    }

    fn walk(
        c: &GreenChild,
        regions: &[FreshRegion],
        emitted: &mut usize,
        tok_index: &mut u32,
        out: &mut VecDeque<Item>,
    ) {
        flush_fresh_at(*tok_index, regions, emitted, out);
        match c {
            GreenChild::Token(t) => {
                let here = *tok_index;
                *tok_index += 1;
                // Skip tokens inside damaged intervals; fresh replaces them.
                let in_dirty = regions.iter().any(|r| {
                    let (a, b) = r.old_toks;
                    a <= here && here < b
                });
                if !in_dirty {
                    out.push_back(Item::Sub(GreenChild::Token(t.clone())));
                }
            }
            GreenChild::Node(n) => {
                let span = (*tok_index, *tok_index + n.n_toks);
                if !dirty(span, regions) {
                    // Maximal clean subtree: emit whole.
                    out.push_back(Item::Sub(c.clone()));
                    *tok_index += n.n_toks;
                } else {
                    for ch in &n.children {
                        walk(ch, regions, emitted, tok_index, out);
                    }
                }
            }
        }
    }
}
