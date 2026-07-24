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
    flat_children, is_seq_prod, GreenChild, GreenNode, GreenToken, TokWithText, LIST_PROD,
    MAX_RUN, RUN_PROD,
};
use crate::lr::{ListShape, LrAct, LrTables, UnitSym};
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
    /// A single child (element node, separator/trivia token). The flag
    /// records REUSED provenance: only reused pieces may unwind on a
    /// right-edge error (fresh reduces are lookahead-correct).
    Loose(GreenChild, bool),
    /// A reused leaf run (fanout ≤ MAX_RUN, children are elements).
    Leaf(Arc<GreenNode>),
}

impl ListBuilder {
    fn new(nt: u16) -> Self {
        ListBuilder { nt, pieces: Vec::new() }
    }
    fn push_loose(&mut self, c: GreenChild, spliced: bool) {
        self.pieces.push(Piece::Loose(c, spliced));
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
                other => self.pieces.push(Piece::Loose(other.clone(), true)),
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
            && self.pieces.iter().all(|p| matches!(p, Piece::Loose(..)))
        {
            let children: Vec<GreenChild> = self
                .pieces
                .into_iter()
                .map(|p| match p {
                    Piece::Loose(c, _) => c,
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
                Piece::Loose(c, _) => buf.push(c),
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

/// How a salvaged RUN chunk may enter an associative list splice.
///
/// Runs are arbitrary ≤MAX_RUN cuts of a list's flattened children, so a
/// chunk can begin or end MID repetition-unit (e.g. a dangling trailing
/// separator). The blind splice is sound only for whole units — the LR
/// state does not advance over absorbed content (Wagner §7), so a
/// dangling separator would leave the automaton one shift behind the
/// tree and surface as a spurious repair (or a silently wrong tree).
enum RunFit {
    /// Aligned: absorb the whole node (trailing trivia included).
    /// `tail = 0`; otherwise absorb the flat prefix as loose pieces and
    /// push the last `tail` flattened children back onto the input for
    /// ordinary state-checked shifting.
    Take { tail: usize },
    /// Misaligned start or no unit boundary at all: break down instead.
    Reject,
}

fn unit_matches(c: &GreenChild, want: UnitSym, elem: u16) -> bool {
    match (c, want) {
        (GreenChild::Node(n), UnitSym::Elem) => n.nt == elem,
        (GreenChild::Token(t), UnitSym::Tok(id)) => {
            !t.trivia && !t.is_missing() && t.id == id
        }
        _ => false,
    }
}

fn is_symbol(c: &GreenChild) -> bool {
    match c {
        GreenChild::Token(t) => !t.trivia && !t.is_missing(),
        GreenChild::Node(_) => true,
    }
}

/// Alignment of a salvaged RUN against its list's repetition unit.
/// `lead` is what the chunk must begin with in the current position
/// (α-first when continuing an open list, seed-first when starting one).
fn run_fit(node: &GreenNode, shape: &ListShape, lead: UnitSym) -> RunFit {
    if node.terms == 0 {
        return RunFit::Take { tail: 0 }; // pure trivia: no state impact
    }
    let flat = flat_children(node);
    match flat.iter().find(|c| is_symbol(c)) {
        Some(first) if unit_matches(first, lead, shape.elem) => {}
        _ => return RunFit::Reject,
    }
    let Some(i) = flat.iter().rposition(|c| unit_matches(c, shape.alpha_last, shape.elem)) else {
        return RunFit::Reject;
    };
    if flat[i + 1..].iter().all(|c| !is_symbol(c)) {
        RunFit::Take { tail: 0 } // dangle is trivia-only: absorb whole
    } else {
        RunFit::Take { tail: flat.len() - 1 - i }
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
    /// True when `sym` is a subtree REUSED from the previous tree (a
    /// nonterminal splice). Reused subtrees' right-spine reductions
    /// assumed the OLD following lookahead, so they may unwind (Wagner
    /// right breakdown) when the parse errors just after them.
    spliced: bool,
}

fn make_node(nt: u16, prod: u16, children: Vec<GreenChild>) -> Arc<GreenNode> {
    let mut width = 0u32;
    let mut terms = 0u32;
    let mut n_toks = 0u32;
    let mut has_err = nt == crate::green::ERROR_NT;
    for c in &children {
        match c {
            GreenChild::Node(n) => {
                width += n.width;
                terms += n.terms;
                n_toks += n.n_toks;
                has_err |= n.has_err;
            }
            GreenChild::Token(t) => {
                if t.is_missing() {
                    has_err = true;
                    continue; // zero width, zero counts — not a buffer token
                }
                width += t.text.len() as u32;
                n_toks += 1;
                if !t.trivia {
                    terms += 1;
                }
            }
        }
    }
    Arc::new(GreenNode { nt, prod, width, terms, n_toks, has_err, children })
}

/// Leftmost non-trivia terminal beneath a child, if any. Iterative
/// descent along the first terminal-bearing child — O(depth), and only
/// computed when actually needed (it's O(spine) on unbalanced lists, so
/// the hot splice path must never touch it).
fn leftmost_term(c: &GreenChild) -> Option<TokenId> {
    let mut cur = c;
    loop {
        match cur {
            GreenChild::Token(t) => return (!t.trivia && !t.is_missing()).then_some(t.id),
            GreenChild::Node(n) => {
                cur = n.children.iter().find(|ch| match ch {
                    GreenChild::Token(t) => !t.trivia && !t.is_missing(),
                    GreenChild::Node(m) => m.terms > 0,
                })?;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Error recovery (bounded repair; CPCT+-inspired single-op candidates)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RepairKind {
    /// The token (with this text) was skipped into an ERROR node.
    Deleted(String),
    /// A zero-width token of this kind was pretended into existence.
    Inserted(TokenId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repair {
    /// Terminal index (into the real terminal stream) where the repair
    /// applied — services map this to a source span.
    pub at_terminal: usize,
    pub kind: RepairKind,
}

/// Peek the next `k` REAL terminal ids from the input (flattening
/// salvaged subtrees), for repair validation.
fn peek_terms(input: &VecDeque<Item>, k: usize) -> Vec<TokenId> {
    fn collect(n: &GreenNode, out: &mut Vec<TokenId>, k: usize) -> bool {
        for c in &n.children {
            match c {
                GreenChild::Token(t) if !t.trivia && !t.is_missing() => {
                    out.push(t.id);
                    if out.len() == k {
                        return true;
                    }
                }
                GreenChild::Node(m) => {
                    if collect(m, out, k) {
                        return true;
                    }
                }
                _ => {}
            }
        }
        false
    }
    let mut out = Vec::with_capacity(k);
    for item in input {
        match item {
            Item::Fresh(t) => {
                if !t.trivia {
                    out.push(t.id);
                }
            }
            Item::Sub(GreenChild::Token(t)) => {
                if !t.trivia && !t.is_missing() {
                    out.push(t.id);
                }
            }
            Item::Sub(GreenChild::Node(n)) => {
                collect(n, &mut out, k);
            }
        }
        if out.len() >= k {
            break;
        }
    }
    out
}

/// States-only LR run over a terminal feed; returns how many feed tokens
/// were consumed before erroring (Accept counts as consuming the rest).
fn simulate(g: &SynGrammar, t: &LrTables, base: &[u16], feed: &[TokenId]) -> usize {
    let mut states = base.to_vec();
    let mut i = 0usize;
    let mut steps = 0usize;
    while i < feed.len() && steps < 400 {
        steps += 1;
        let s = *states.last().unwrap();
        match t.action[s as usize].get(&feed[i]).copied() {
            Some(LrAct::Shift(n)) => {
                states.push(n);
                i += 1;
            }
            Some(LrAct::Reduce(p)) => {
                let prod = &g.prods[p as usize];
                let k = prod.rhs.len();
                if states.len() <= k {
                    return i;
                }
                states.truncate(states.len() - k);
                let top = *states.last().unwrap();
                match t.goto_[top as usize].get(&prod.lhs) {
                    Some(&n) => states.push(n),
                    None => return i,
                }
            }
            Some(LrAct::Accept) => return feed.len(),
            _ => return i,
        }
    }
    i
}

/// Apply one insertion on a cloned state stack (reduces, then the shift).
fn apply_insert_sim(g: &SynGrammar, t: &LrTables, base: &[u16], x: TokenId) -> Option<Vec<u16>> {
    let mut states = base.to_vec();
    let mut steps = 0usize;
    loop {
        steps += 1;
        if steps > 200 {
            return None;
        }
        let s = *states.last().unwrap();
        match t.action[s as usize].get(&x).copied() {
            Some(LrAct::Shift(n)) => {
                states.push(n);
                return Some(states);
            }
            Some(LrAct::Reduce(p)) => {
                let prod = &g.prods[p as usize];
                let k = prod.rhs.len();
                if states.len() <= k {
                    return None;
                }
                states.truncate(states.len() - k);
                let top = *states.last().unwrap();
                match t.goto_[top as usize].get(&prod.lhs) {
                    Some(&n) => states.push(n),
                    None => return None,
                }
            }
            _ => return None,
        }
    }
}

/// Bounded search for an INSERT SEQUENCE (length ≤ 3) that lets the parse
/// consume ≥1 real upcoming terminal — the mini-CPCT+ core. Prefers
/// shorter sequences, then higher real consumption, then
/// lexicographically smaller sequences (determinism).
fn search_inserts(
    g: &SynGrammar,
    t: &LrTables,
    base: &[u16],
    peeked: &[TokenId],
) -> Option<(Vec<TokenId>, usize)> {
    let mut frontier: Vec<(Vec<TokenId>, Vec<u16>)> = vec![(Vec::new(), base.to_vec())];
    let mut budget = 600usize;
    for _len in 1..=3usize {
        let mut next: Vec<(Vec<TokenId>, Vec<u16>)> = Vec::new();
        let mut best_at_len: Option<(Vec<TokenId>, usize)> = None;
        for (seq, states) in &frontier {
            let s = *states.last().unwrap();
            let mut cands: Vec<TokenId> =
                t.action[s as usize].keys().copied().filter(|&c| c != EOF).collect();
            cands.sort_unstable();
            cands.truncate(16);
            for cand in cands {
                if budget == 0 {
                    break;
                }
                budget -= 1;
                let Some(states2) = apply_insert_sim(g, t, states, cand) else { continue };
                let mut seq2 = seq.clone();
                seq2.push(cand);
                let score = simulate(g, t, &states2, peeked);
                if score >= 1 {
                    let better = match &best_at_len {
                        None => true,
                        Some((bseq, bscore)) => {
                            score > *bscore || (score == *bscore && seq2 < *bseq)
                        }
                    };
                    if better {
                        best_at_len = Some((seq2, score));
                    }
                } else {
                    next.push((seq2, states2));
                }
            }
        }
        if let Some(hit) = best_at_len {
            return Some(hit);
        }
        frontier = next;
        if frontier.is_empty() || budget == 0 {
            break;
        }
    }
    None
}

fn error_node_of(tok: GreenToken) -> GreenChild {
    GreenChild::Node(make_node(
        crate::green::ERROR_NT,
        crate::green::ERROR_PROD,
        vec![GreenChild::Token(tok)],
    ))
}

/// EOF-repair scoring: optionally insert one token, then run EOF reduces
/// to a fixpoint. Returns (reaches accept, remaining stack depth) —
/// `usize::MAX` depth when the insertion can't even shift.
fn eof_progress(
    g: &SynGrammar,
    t: &LrTables,
    base: &[u16],
    insert: Option<TokenId>,
) -> (bool, usize) {
    let mut states = base.to_vec();
    if let Some(x) = insert {
        let mut steps = 0;
        loop {
            steps += 1;
            if steps > 200 {
                return (false, usize::MAX);
            }
            let s = *states.last().unwrap();
            match t.action[s as usize].get(&x).copied() {
                Some(LrAct::Shift(n)) => {
                    states.push(n);
                    break;
                }
                Some(LrAct::Reduce(p)) => {
                    let prod = &g.prods[p as usize];
                    let k = prod.rhs.len();
                    if states.len() <= k {
                        return (false, usize::MAX);
                    }
                    states.truncate(states.len() - k);
                    let top = *states.last().unwrap();
                    match t.goto_[top as usize].get(&prod.lhs) {
                        Some(&n) => states.push(n),
                        None => return (false, usize::MAX),
                    }
                }
                _ => return (false, usize::MAX),
            }
        }
    }
    let mut steps = 0;
    loop {
        steps += 1;
        if steps > 200 {
            return (false, states.len());
        }
        let s = *states.last().unwrap();
        match t.action[s as usize].get(&EOF).copied() {
            Some(LrAct::Reduce(p)) => {
                let prod = &g.prods[p as usize];
                let k = prod.rhs.len();
                if states.len() <= k {
                    return (false, states.len());
                }
                states.truncate(states.len() - k);
                let top = *states.last().unwrap();
                match t.goto_[top as usize].get(&prod.lhs) {
                    Some(&n) => states.push(n),
                    None => return (false, states.len()),
                }
            }
            Some(LrAct::Accept) => return (true, states.len()),
            _ => return (false, states.len()),
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
            GreenChild::Token(t) => !t.trivia && !t.is_missing(),
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
/// lossless green tree. TOTAL under syntax errors: bounded repair
/// (validated single-token insert/delete, CPCT+-flavored) keeps the parse
/// going, skipped tokens land in ERROR nodes, inserted tokens are
/// zero-width, and every repair is reported.
pub fn incremental_parse(
    g: &SynGrammar,
    t: &LrTables,
    mut input: VecDeque<Item>,
) -> Result<(Arc<GreenNode>, ReuseStats, Vec<Repair>), IncParseError> {
    const K: usize = 3;
    const MAX_REPAIRS: usize = 200;
    let mut stats = ReuseStats::default();
    let mut states: Vec<u16> = vec![0];
    let mut stack: Vec<Entry> = Vec::new();
    let mut pending: Vec<GreenChild> = Vec::new();
    let mut consumed_terms = 0usize;
    let mut repairs: Vec<Repair> = Vec::new();

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
                            let spliced = head.spliced;
                            b.push_loose(other, spliced);
                            b
                        }
                    };
                    for e in it {
                        for tr in e.leading {
                            b.push_loose(tr, false);
                        }
                        let spliced = e.spliced;
                        b.push_loose(e.sym.into_child(), spliced);
                    }
                    SymSlot::List(b)
                }
                // L4 seed (incl. ε): open a fresh builder with the seed.
                Some(_) => {
                    let mut b = ListBuilder::new(prod.lhs);
                    for e in popped {
                        for tr in e.leading {
                            b.push_loose(tr, false);
                        }
                        let spliced = e.spliced;
                        b.push_loose(e.sym.into_child(), spliced);
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
            stack.push(Entry { leading: Vec::new(), sym: slot, spliced: false });
            states.push(next);
        }};
    }

    // Wagner right breakdown: a reused subtree's right-spine reductions
    // assumed the OLD tree's following lookahead. If the parse errors
    // immediately after reused structure, un-splice the nearest reused
    // piece (top spliced value-stack entry, or the open list builder's
    // trailing reused piece) and push its CHILDREN back onto the input:
    // they re-parse under the ACTUAL lookahead. Only reused provenance
    // unwinds — fresh reduces are lookahead-correct by construction —
    // and every unwind replaces a node by its children, so the total
    // work is bounded by the reused subtree's size. Yields `true` if
    // something was unwound (caller retries before invoking repair).
    macro_rules! try_unwind {
        () => {{
            let mut done = false;
            let mut pop_empty_list = false;
            match stack.last_mut() {
                Some(Entry { spliced: entry_spliced, sym: SymSlot::List(b), .. }) => {
                    let entry_spliced = *entry_spliced;
                    // Step over trailing trivia pieces (state-inert) to
                    // reach the last structural piece.
                    let mut trailing: Vec<GreenChild> = Vec::new();
                    while matches!(
                        b.pieces.last(),
                        Some(Piece::Loose(GreenChild::Token(tk), _)) if tk.trivia
                    ) {
                        let Some(Piece::Loose(c, _)) = b.pieces.pop() else { unreachable!() };
                        trailing.push(c);
                    }
                    let unwound: Option<Vec<GreenChild>> = match b.pieces.last() {
                        Some(Piece::Leaf(_)) => {
                            let Some(Piece::Leaf(run)) = b.pieces.pop() else { unreachable!() };
                            consumed_terms -= run.terms as usize;
                            stats.reused_terms -= run.terms;
                            Some(run.children.clone())
                        }
                        Some(Piece::Loose(GreenChild::Node(n), true)) => {
                            let terms = n.terms;
                            let Some(Piece::Loose(GreenChild::Node(n), _)) = b.pieces.pop()
                            else {
                                unreachable!()
                            };
                            consumed_terms -= terms as usize;
                            stats.reused_terms -= terms;
                            Some(n.children.clone())
                        }
                        _ => None,
                    };
                    match unwound {
                        Some(children) => {
                            // Byte order: children, then the trailing
                            // trivia we stepped over, then any pending
                            // trivia already read PAST the unwound node.
                            let pend = std::mem::take(&mut pending);
                            for c in pend.into_iter().rev() {
                                input.push_front(Item::Sub(c));
                            }
                            for c in trailing.into_iter() {
                                input.push_front(Item::Sub(c));
                            }
                            for c in children.into_iter().rev() {
                                input.push_front(Item::Sub(c));
                            }
                            done = true;
                        }
                        None => {
                            if b.pieces.is_empty() && entry_spliced {
                                // Unwinding emptied a list entry that
                                // only exists because of reuse: the
                                // entry AND its GOTO state unwind too
                                // (the automaton returns to the
                                // pre-list state, where batch would
                                // be). Its stepped-over trivia rejoins
                                // the input.
                                let pend = std::mem::take(&mut pending);
                                for c in pend.into_iter().rev() {
                                    input.push_front(Item::Sub(c));
                                }
                                for c in trailing.into_iter() {
                                    input.push_front(Item::Sub(c));
                                }
                                pop_empty_list = true;
                                done = true;
                            } else {
                                // Not unwindable: restore the trivia.
                                for c in trailing.into_iter().rev() {
                                    b.pieces.push(Piece::Loose(c, false));
                                }
                            }
                        }
                    }
                }
                Some(Entry { spliced: true, sym: SymSlot::Child(GreenChild::Node(_)), .. }) => {
                    let Some(Entry { leading, sym: SymSlot::Child(GreenChild::Node(n)), .. }) =
                        stack.pop()
                    else {
                        unreachable!()
                    };
                    states.pop();
                    consumed_terms -= n.terms as usize;
                    stats.reused_terms -= n.terms;
                    let pend = std::mem::take(&mut pending);
                    for c in pend.into_iter().rev() {
                        input.push_front(Item::Sub(c));
                    }
                    for c in n.children.iter().rev() {
                        input.push_front(Item::Sub(c.clone()));
                    }
                    for tr in leading.into_iter().rev() {
                        input.push_front(Item::Sub(tr));
                    }
                    done = true;
                }
                _ => {}
            }
            if pop_empty_list {
                stack.pop();
                states.pop();
            }
            done
        }};
    }

    macro_rules! delete_la {
        () => {{
            match input.pop_front() {
                Some(Item::Fresh(tok)) => {
                    repairs.push(Repair {
                        at_terminal: consumed_terms,
                        kind: RepairKind::Deleted(tok.text.clone()),
                    });
                    consumed_terms += 1;
                    pending.push(error_node_of(GreenToken {
                        id: tok.id,
                        trivia: false,
                        text: tok.text,
                    }));
                }
                Some(Item::Sub(GreenChild::Token(tk))) => {
                    repairs.push(Repair {
                        at_terminal: consumed_terms,
                        kind: RepairKind::Deleted(tk.text.clone()),
                    });
                    consumed_terms += 1;
                    pending.push(error_node_of(tk));
                }
                _ => {}
            }
        }};
    }

    macro_rules! apply_insert {
        ($x:expr) => {{
            let x = $x;
            loop {
                let s2 = *states.last().unwrap();
                match t.action[s2 as usize].get(&x).copied() {
                    Some(LrAct::Reduce(p)) => reduce!(p),
                    Some(LrAct::Shift(n)) => {
                        stack.push(Entry {
                            leading: std::mem::take(&mut pending),
                            sym: SymSlot::Child(GreenChild::Token(GreenToken {
                                id: x,
                                trivia: false,
                                text: String::new(),
                            })),
                            spliced: false,
                        });
                        states.push(n);
                        break;
                    }
                    _ => break,
                }
            }
        }};
    }

    macro_rules! recover {
        ($at_eof:expr) => {{
            if repairs.len() >= MAX_REPAIRS && !$at_eof {
                // Repair budget exhausted: dump the rest of the input into
                // error material and let EOF handling finish the tree.
                while let Some(item) = input.pop_front() {
                    match item {
                        Item::Fresh(tok) => {
                            if tok.trivia {
                                pending.push(GreenChild::Token(GreenToken {
                                    id: tok.id,
                                    trivia: true,
                                    text: tok.text,
                                }));
                            } else {
                                pending.push(error_node_of(GreenToken {
                                    id: tok.id,
                                    trivia: false,
                                    text: tok.text,
                                }));
                            }
                        }
                        Item::Sub(GreenChild::Token(tk)) => {
                            if tk.trivia {
                                pending.push(GreenChild::Token(tk));
                            } else if !tk.is_missing() {
                                pending.push(error_node_of(tk));
                            }
                        }
                        Item::Sub(GreenChild::Node(n)) => {
                            for c in n.children.iter().rev() {
                                input.push_front(Item::Sub(c.clone()));
                            }
                        }
                    }
                }
            } else if $at_eof {
                // EOF repair: prefer insertions that reach accept, then
                // ones that strictly shrink the stack under EOF reduces
                // (each missing closer heals one per round); otherwise
                // unwind the stack into error material. Over budget →
                // straight to unwind (never loop).
                let over_budget = repairs.len() >= MAX_REPAIRS;
                let baseline = eof_progress(g, t, &states, None).1;
                let state_now = *states.last().unwrap();
                let mut cands: Vec<TokenId> = t.action[state_now as usize]
                    .keys()
                    .copied()
                    .filter(|&c| c != EOF)
                    .collect();
                cands.sort_unstable();
                cands.truncate(24);
                let mut best: Option<(bool, usize, TokenId)> = None;
                if !over_budget {
                    for cand in cands {
                        let (acc, depth) = eof_progress(g, t, &states, Some(cand));
                        if depth == usize::MAX {
                            continue;
                        }
                        let better = match best {
                            None => true,
                            Some((ba, bd, bid)) => {
                                (acc, depth, cand) != (ba, bd, bid)
                                    && (acc && !ba
                                        || (acc == ba
                                            && (depth < bd || (depth == bd && cand < bid))))
                            }
                        };
                        if better {
                            best = Some((acc, depth, cand));
                        }
                    }
                }
                match best {
                    Some((acc, depth, x)) if acc || depth < baseline => {
                        repairs.push(Repair {
                            at_terminal: consumed_terms,
                            kind: RepairKind::Inserted(x),
                        });
                        apply_insert!(x);
                    }
                    _ => {
                        if let Some(e) = stack.pop() {
                            states.pop();
                            let mut kids = e.leading;
                            kids.push(e.sym.into_child());
                            pending.insert(
                                0,
                                GreenChild::Node(make_node(
                                    crate::green::ERROR_NT,
                                    crate::green::ERROR_PROD,
                                    kids,
                                )),
                            );
                        } else {
                            let kids = std::mem::take(&mut pending);
                            let root =
                                make_node(crate::green::ERROR_NT, crate::green::ERROR_PROD, kids);
                            stats.total_terms = consumed_terms as u32;
                            return Ok((root, stats, repairs));
                        }
                    }
                }
            } else {
                let peeked = peek_terms(&input, K + 1);
                let delete_score = if peeked.is_empty() {
                    0
                } else {
                    simulate(g, t, &states, &peeked[1..])
                };
                let insert_hit = search_inserts(g, t, &states, &peeked);
                // Choose: higher real consumption wins; ties prefer the
                // single delete over an insert sequence (lower cost).
                match insert_hit {
                    Some((seq, score)) if score > delete_score => {
                        for &x in &seq {
                            repairs.push(Repair {
                                at_terminal: consumed_terms,
                                kind: RepairKind::Inserted(x),
                            });
                            apply_insert!(x);
                        }
                    }
                    _ if delete_score > 0 => {
                        delete_la!();
                    }
                    _ => {
                        delete_la!(); // panic-skip: guaranteed progress
                    }
                }
            }
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
                        return Ok((Arc::new(root_owned), stats, repairs));
                    }
                    _ => {
                        if !try_unwind!() {
                            recover!(true);
                        }
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
            // seeds a new builder via GOTO. Complete LIST nodes are
            // whole numbers of repetition units by construction; RUN
            // chunks are arbitrary cuts and must pass the alignment
            // check first — a dangling tail (e.g. a trailing separator
            // the state has not shifted over) re-enters the input.
            if seq {
                if let Some(shape) = t.lists.get(&node.nt) {
                    let continues = matches!(
                        stack.last(),
                        Some(Entry { sym: SymSlot::List(b), .. }) if b.nt == node.nt
                    );
                    let goto_next = t.goto_[state as usize].get(&node.nt).copied();
                    let fit = if !continues && goto_next.is_none() {
                        RunFit::Reject // not spliceable here at all
                    } else if node.prod != RUN_PROD {
                        RunFit::Take { tail: 0 }
                    } else {
                        let lead = if continues {
                            shape.alpha_first
                        } else {
                            shape.seed_first.unwrap_or(shape.alpha_first)
                        };
                        run_fit(&node, shape, lead)
                    };
                    if let RunFit::Take { tail } = fit {
                        input.pop_front();
                        // Pending trivia belongs INSIDE the run's first
                        // element (where batch drains it at the next
                        // token shift) — never at list level.
                        let spliced = if pending.is_empty() || node.terms == 0 {
                            node.clone()
                        } else {
                            prepend_trivia(&node, std::mem::take(&mut pending))
                        };
                        if !continues {
                            // `spliced: true` — this list entry exists
                            // because of REUSED structure; if unwinding
                            // later empties it, the entry itself (and
                            // its GOTO state) must unwind too.
                            stack.push(Entry {
                                leading: Vec::new(),
                                sym: SymSlot::List(ListBuilder::new(node.nt)),
                                spliced: true,
                            });
                            states.push(goto_next.unwrap());
                        }
                        let Some(Entry { sym: SymSlot::List(b), .. }) = stack.last_mut() else {
                            unreachable!()
                        };
                        stats.splices += 1;
                        if tail == 0 {
                            stats.reused_terms += spliced.terms;
                            consumed_terms += spliced.terms as usize;
                            b.absorb_seq(&spliced);
                        } else {
                            // Absorb the aligned prefix loosely; the
                            // dangle shifts through the automaton.
                            let flat = flat_children(&spliced);
                            let cut = flat.len() - tail;
                            let mut absorbed = 0u32;
                            for c in &flat[..cut] {
                                absorbed += match c {
                                    GreenChild::Node(n) => n.terms,
                                    GreenChild::Token(tk) => {
                                        u32::from(!tk.trivia && !tk.is_missing())
                                    }
                                };
                                b.push_loose((*c).clone(), true);
                            }
                            let tail_items: Vec<GreenChild> =
                                flat[cut..].iter().map(|c| (*c).clone()).collect();
                            stats.reused_terms += absorbed;
                            consumed_terms += absorbed as usize;
                            for c in tail_items.into_iter().rev() {
                                input.push_front(Item::Sub(c));
                            }
                        }
                        continue;
                    }
                    // Reject: fall through to reduce-enable/breakdown.
                }
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
                        spliced: true,
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
                    spliced: false,
                });
                states.push(next);
            }
            Some(LrAct::Reduce(p)) => reduce!(p),
            Some(LrAct::Accept) => unreachable!("accept only on EOF"),
            Some(LrAct::Error) | None => {
                if !try_unwind!() {
                    recover!(false);
                }
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
    incremental_parse(g, t, input).map(|(tree, _, _)| tree)
}

/// Batch parse returning repairs as well (the session's fallback path
/// and diagnostics consumers want them).
pub fn batch_parse_full(
    g: &SynGrammar,
    t: &LrTables,
    all: &[TokWithText],
) -> Result<(Arc<GreenNode>, ReuseStats, Vec<Repair>), IncParseError> {
    let input: VecDeque<Item> = all.iter().cloned().map(Item::Fresh).collect();
    incremental_parse(g, t, input)
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
                if t.is_missing() {
                    // Repair-invented: zero-count, never re-emitted — the
                    // reparse re-derives (or heals) it from real tokens.
                    return;
                }
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
                // Error-poisoned nodes always dissolve: their real tokens
                // re-enter the parse as plain lookaheads (ERROR wrappers
                // evaporate), so recovery re-runs against current text.
                if !n.has_err && !dirty(span, regions) {
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
