//! Incremental LR parsing over green trees, exact under error recovery.
//!
//! The parser consumes a SENTENTIAL FORM: a queue of subtrees salvaged
//! from the previous tree, interleaved with fresh tokens for the damaged
//! regions. A subtree is reused where the parse stands in the LR state its
//! own parse began in, and its last reductions are decided as they were on
//! the terminal that now follows it (state matching, `GreenNode::lr_ends`);
//! a subtree holding an error must also match its recovery record
//! (`GreenNode::recovery`). Otherwise it breaks down into its children
//! (an ERROR node into its tokens), and the parse continues at finer grain.
//! Lists are balanced trees of runs cut between repetition units, and a
//! run of whole units continues an open list of its kind.
//!
//! Trees are built during parsing (no intermediate PNode): stack entries
//! carry the material drained in front of them, so tokens sit exactly
//! where the batch weave puts them, and material pending in front of a
//! reused subtree goes down its left spine. The contract is that the
//! incremental tree is the batch tree, `semantic_eq` (structure, trivia
//! placement, and how each node was parsed), with batch's repairs; see
//! `docs/incremental_recovery.md`.

use crate::green::{
    flat_children, is_seq_prod, GreenChild, GreenNode, GreenToken, LrEnds, TokWithText, LIST_PROD,
    MAX_RUN, RUN_PROD,
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
    /// Number of subtrees broken into their children (not reusable where
    /// the parse stood).
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
/// this: batch appends one seed or repetition unit per reduce;
/// incremental also absorbs whole salvaged runs (the associative
/// concatenation Wagner's `B → B B` view licenses).
struct ListBuilder {
    nt: u16,
    pieces: Vec<Piece>,
    /// Whether the reduction that opened it was settled (see
    /// [`GreenNode::lr_ends`]): for an ε seed, the only thing that says so.
    seed_settled: bool,
    /// The state its last reduction (seed or unit) was decided in, and the
    /// production it reduced by.
    last: (u16, u16),
}

/// A piece of a list under construction.
enum Piece {
    /// One child: an element node, a token of the repetition (a
    /// separator), or material (trivia, an ERROR node) drained in front of
    /// one; and whether how it entered the list depended on nothing the
    /// parse took or decided after it (see [`GreenNode::lr_ends`]).
    Loose(GreenChild, bool),
    /// A reused run of whole repetition units, entered in the list's state
    /// (see [`ListBuilder::finalize`]).
    Leaf(Arc<GreenNode>),
}

/// The children a child contributes with pending material drained into
/// it, where batch puts that material: inside a node, before its first
/// symbol; before a token, at the token's level.
fn materialize(c: GreenChild, drains: Vec<GreenChild>) -> Vec<GreenChild> {
    if drains.is_empty() {
        return vec![c];
    }
    match c {
        GreenChild::Node(n) => vec![GreenChild::Node(prepend_trivia(&n, drains))],
        token => {
            let mut out = drains;
            out.push(token);
            out
        }
    }
}

/// Whether pending material drains into `c`: it holds a symbol and a real
/// terminal (a node holding only repair-inserted tokens leaves the
/// material pending).
fn takes_drain(c: &GreenChild) -> bool {
    holds_symbol(c)
        && match c {
            GreenChild::Node(n) => n.terms > 0,
            GreenChild::Token(t) => !t.is_missing(),
        }
}

/// Whether a child of a list is one of its symbols (a node other than an
/// ERROR node, or a shifted token, repair-inserted ones included) rather
/// than material.
fn is_structure(c: &GreenChild) -> bool {
    match c {
        GreenChild::Node(n) => n.nt != crate::green::ERROR_NT,
        GreenChild::Token(tk) => !tk.trivia,
    }
}

impl ListBuilder {
    fn new(nt: u16, seed_settled: bool, last: (u16, u16)) -> Self {
        ListBuilder { nt, pieces: Vec::new(), seed_settled, last }
    }

    /// Absorb a salvaged LIST or RUN node whose parse began where this
    /// one stands: its runs of whole units become leaf pieces (never
    /// elements: that keeps a splice O(runs)), and anything else its
    /// children. Pending material (`drains`) drains into the first piece
    /// that takes it, as batch drains it at the next shift.
    fn absorb_seq(&mut self, n: &Arc<GreenNode>, drains: &mut Vec<GreenChild>) {
        let leaf = n.prod == RUN_PROD
            && n.lr_ends.is_some()
            && n.children.iter().all(|c| !matches!(c, GreenChild::Node(m) if m.prod == RUN_PROD));
        if leaf {
            let run = if !drains.is_empty() && takes_drain(&GreenChild::Node(n.clone())) {
                prepend_trivia(n, std::mem::take(drains))
            } else {
                n.clone()
            };
            self.pieces.push(Piece::Leaf(run));
            return;
        }
        for c in &n.children {
            match c {
                GreenChild::Node(m) if m.prod == RUN_PROD => self.absorb_seq(m, drains),
                other => {
                    let taken = if !drains.is_empty() && takes_drain(other) {
                        std::mem::take(drains)
                    } else {
                        Vec::new()
                    };
                    // A list reused whole was entered settled throughout.
                    for c in materialize(other.clone(), taken) {
                        self.pieces.push(Piece::Loose(c, true));
                    }
                }
            }
        }
    }

    /// The list's tree, in the deterministic balanced form: its seed and
    /// repetition units, chunked into RUN nodes of at most `MAX_RUN`
    /// children, cut only between units, then grouped 16-ary until one
    /// node remains (a list of at most `MAX_RUN` loose children skips the
    /// run layer). `s0` is the state the list began in, `c` the state it
    /// stands in (`goto(s0, nt)`), where every repetition unit begins.
    ///
    /// Cutting between units is what lets a run be reused: a run of whole
    /// units entered in `c` leaves the parse in `c`, as parsing them would,
    /// so it begins in `c`; it ends where its last unit's reduction was
    /// decided, the state its symbols lead to from `c`. The run holding
    /// the seed began in `s0`
    /// with no list open, which no open list can continue: it has no
    /// [`LrEnds`]. Nor does a run cut inside a unit (a unit of more than
    /// `MAX_RUN` children, with material between its symbols), or one
    /// holding a piece that entered unsettled.
    fn finalize(self, g: &SynGrammar, t: &LrTables, s0: u16, c: u16) -> Arc<GreenNode> {
        let nt = self.nt;
        let shape = *t.lists.get(&nt).expect("a list builder builds a list nonterminal");
        let seed_len = g.prods[shape.seed as usize].rhs.len();
        let alpha_len = g.prods[shape.cons as usize].rhs.len() - 1;
        let settled_all = self.seed_settled
            && self.pieces.iter().all(|p| match p {
                Piece::Loose(_, settled) => *settled,
                Piece::Leaf(_) => true,
            });

        // The pieces as blocks: the seed and each unit, with the material
        // in front of their symbols; and reused runs.
        enum Block {
            Group { children: Vec<GreenChild>, settled: bool, unit: bool },
            Leaf(Arc<GreenNode>),
        }
        let mut blocks: Vec<Block> = Vec::new();
        let mut group: Vec<GreenChild> = Vec::new();
        let mut group_settled = true;
        let mut in_seed = seed_len > 0;
        let mut need = if in_seed { seed_len } else { alpha_len };
        let mut taken = 0usize;
        for p in self.pieces {
            match p {
                Piece::Leaf(run) => {
                    debug_assert!(group.is_empty() && !in_seed, "a reused run is whole units");
                    blocks.push(Block::Leaf(run));
                }
                Piece::Loose(ch, settled) => {
                    let symbol = is_structure(&ch);
                    group.push(ch);
                    group_settled &= settled;
                    if symbol {
                        taken += 1;
                        if taken == need {
                            blocks.push(Block::Group {
                                children: std::mem::take(&mut group),
                                settled: group_settled,
                                unit: !in_seed,
                            });
                            group_settled = true;
                            taken = 0;
                            in_seed = false;
                            need = alpha_len;
                        }
                    }
                }
            }
        }
        debug_assert!(group.is_empty() && !in_seed, "a list ends at a whole unit");

        // Where each block's last reduction was decided, and by what: a
        // group's symbols lead from the state it began in (the seed's from
        // `s0`, a unit's from `c`) to the state its reduction (the seed's or
        // the repetition's) was decided in.
        let ends: Vec<Option<(u16, u16)>> = blocks
            .iter()
            .map(|b| match b {
                Block::Leaf(run) => run.lr_ends.map(|e| (e.end, e.prod)),
                Block::Group { children, unit, .. } => {
                    let mut cur = if *unit { c } else { s0 };
                    for ch in children.iter().filter(|ch| is_structure(ch)) {
                        let next = match ch {
                            GreenChild::Node(m) => t.goto_[cur as usize].get(&m.nt).copied(),
                            GreenChild::Token(tk) => match t.action[cur as usize].get(&tk.id) {
                                Some(LrAct::Shift(n)) => Some(*n),
                                _ => None,
                            },
                        };
                        cur = next?;
                    }
                    Some((cur, if *unit { shape.cons } else { shape.seed }))
                }
            })
            .collect();
        debug_assert!(
            ends.last().copied().flatten().is_none_or(|e| e == self.last),
            "the last unit ends where the list's last reduction was decided"
        );

        let list_ends = match ends.last().copied() {
            Some(Some((end, prod))) if settled_all => Some(LrEnds { begin: s0, end, prod }),
            // An empty list: its seed is its last reduction.
            None if settled_all => Some(LrEnds { begin: s0, end: self.last.0, prod: self.last.1 }),
            _ => None,
        };
        let loose_total: Option<usize> = blocks.iter().try_fold(0usize, |n, b| match b {
            Block::Group { children, .. } => Some(n + children.len()),
            Block::Leaf(_) => None,
        });
        if loose_total.is_some_and(|n| n <= MAX_RUN) {
            let children: Vec<GreenChild> = blocks
                .into_iter()
                .flat_map(|b| match b {
                    Block::Group { children, .. } => children,
                    Block::Leaf(_) => unreachable!(),
                })
                .collect();
            return make_node(nt, LIST_PROD, children, list_ends);
        }

        // Chunk: whole blocks while they fit; a group larger than a run
        // is cut into runs no list can continue.
        struct Chunker {
            nt: u16,
            c: u16,
            leaves: Vec<Arc<GreenNode>>,
            buf: Vec<GreenChild>,
            // Whether the buffered children are whole units, all settled,
            // and where the last one's reduction was decided, by what.
            whole: bool,
            end: Option<(u16, u16)>,
        }
        impl Chunker {
            fn flush(&mut self) {
                if !self.buf.is_empty() {
                    let ends = match self.end {
                        Some((end, prod)) if self.whole => {
                            Some(LrEnds { begin: self.c, end, prod })
                        }
                        _ => None,
                    };
                    let chunk = std::mem::take(&mut self.buf);
                    self.leaves.push(make_node(self.nt, RUN_PROD, chunk, ends));
                }
                self.whole = true;
                self.end = None;
            }
        }
        let mut ch = Chunker { nt, c, leaves: Vec::new(), buf: Vec::new(), whole: true, end: None };
        for (b, end) in blocks.into_iter().zip(ends) {
            match b {
                Block::Leaf(run) => {
                    if !ch.buf.is_empty() && ch.buf.len() + run.children.len() <= MAX_RUN {
                        // Dissolve a small run into its neighbors so edit
                        // seams don't accumulate tiny runs.
                        ch.buf.extend(run.children.iter().cloned());
                        ch.whole &= run.lr_ends.is_some();
                        ch.end = end;
                    } else {
                        ch.flush();
                        ch.leaves.push(run);
                    }
                }
                Block::Group { children, settled, unit } => {
                    if children.len() > MAX_RUN {
                        ch.flush();
                        for piece in children.chunks(MAX_RUN) {
                            ch.leaves.push(make_node(nt, RUN_PROD, piece.to_vec(), None));
                        }
                        continue;
                    }
                    if ch.buf.len() + children.len() > MAX_RUN {
                        ch.flush();
                    }
                    ch.whole &= settled && unit;
                    ch.end = end;
                    ch.buf.extend(children);
                }
            }
        }
        ch.flush();
        let mut level: Vec<Arc<GreenNode>> = ch.leaves;
        while level.len() > MAX_RUN {
            let mut next = Vec::with_capacity(level.len() / MAX_RUN + 1);
            for chunk in level.chunks(MAX_RUN) {
                let ends = match (chunk[0].lr_ends, chunk[chunk.len() - 1].lr_ends) {
                    (Some(first), Some(last)) if chunk.iter().all(|r| r.lr_ends.is_some()) => {
                        Some(LrEnds { begin: first.begin, ..last })
                    }
                    _ => None,
                };
                let children = chunk.iter().cloned().map(GreenChild::Node).collect();
                next.push(make_node(nt, RUN_PROD, children, ends));
            }
            level = next;
        }
        make_node(nt, LIST_PROD, level.into_iter().map(GreenChild::Node).collect(), list_ends)
    }
}

enum SymSlot {
    Child(GreenChild),
    List(ListBuilder),
}

impl SymSlot {
    /// The child the slot holds; a list is finalized, with `s0` the state
    /// it began in and `c` the one it stands in.
    fn into_child(self, g: &SynGrammar, t: &LrTables, s0: u16, c: u16) -> GreenChild {
        match self {
            SymSlot::Child(ch) => ch,
            SymSlot::List(b) => GreenChild::Node(b.finalize(g, t, s0, c)),
        }
    }
}

struct Entry {
    /// Material drained in front of `sym` (when it is a shifted token),
    /// flattened before it on reduce.
    leading: Vec<GreenChild>,
    sym: SymSlot,
    /// Where it stands in the parse (for a list entry: from its first
    /// piece to its last).
    span: Span,
}

/// The children an entry contributes where it ends up as a whole (in a
/// node its production builds, or in an ERROR node recovery sets it aside
/// in): the material drained in front of its symbol, then the symbol.
/// `s0` and `c` are the states it was taken in and led to.
fn entry_children(e: Entry, g: &SynGrammar, t: &LrTables, s0: u16, c: u16) -> Vec<GreenChild> {
    let mut out = e.leading;
    out.push(e.sym.into_child(g, t, s0, c));
    out
}

fn make_node(
    nt: u16,
    prod: u16,
    children: Vec<GreenChild>,
    lr_ends: Option<LrEnds>,
) -> Arc<GreenNode> {
    let mut node = GreenNode {
        nt,
        prod,
        width: 0,
        terms: 0,
        n_toks: 0,
        has_err: nt == crate::green::ERROR_NT,
        lr_ends,
        children: Vec::new(),
        recovery: None,
    };
    for c in &children {
        count_child(&mut node, c);
    }
    node.children = children;
    Arc::new(node)
}

/// The parse's counting state at one point: terminals consumed, repairs
/// made, recoveries entered, and the repair-density state (where the last
/// recovery was, and how big its cluster is). Recovery's decisions depend
/// on all of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Snap {
    consumed_terms: usize,
    repairs_len: usize,
    books_len: usize,
    last_repair_at: usize,
    repair_cluster: u32,
}

/// Where a stack entry or list piece stands in the parse: `at`, just
/// before its own first symbol was taken (the material drained in front
/// of it already decided), and `end`, just after its last symbol was.
/// The repairs between them are its own.
#[derive(Clone, Copy, Debug)]
struct Span {
    at: Snap,
    end: Snap,
}

/// Recovery's insert search validates this many terminals ahead.
const K: usize = 3;
/// Past this many repairs in a document, the rest of the input becomes
/// error material.
const MAX_REPAIRS: usize = 200;
/// REPAIR-DENSITY HYSTERESIS: insert-search validates only K terminals
/// ahead, so structurally hopeless text (a naked expression at file scope
/// reads as declarator gymnastics for a few tokens at a time) can chain
/// plausible-but-wrong inserts clear through a document — 28 "winning"
/// repairs once deleted a fixture's `int` and buried 20 functions in the
/// error region. When errors keep clustering within WINDOW terminals of
/// the last recovery, stop trusting inserts and SKIP until clean shifting
/// resumes: contained garbage beats clever mangling.
const REPAIR_CLUSTER_WINDOW: usize = 6;
const REPAIR_CLUSTER_ESCALATE: u32 = 3;

/// How error recovery built a node: what an incremental parse needs to
/// reuse the node exactly as a batch parse of the same text would rebuild
/// it (see `docs/incremental_recovery.md`). A node carries one when it
/// holds an error and can be reused whole; a node holding an error
/// without one is always re-parsed.
///
/// Everything in it is invariant under reuse: positions are relative to
/// the node's first terminal, and the configuration the node began in is
/// kept only as far as recovery inside the node could tell it apart. So a
/// node reused under a configuration recovery cannot tell from the
/// recorded one, and every node beneath it, keeps a record equal to the
/// one a batch parse makes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recovery {
    /// The state stack beneath the node.
    beneath: Vec<u16>,
    /// The entries beneath it as recovery reads them (see [`EntryKind`]).
    kinds: Vec<EntryKind>,
    /// The cluster size of the last recovery before the node, when the
    /// node's first recovery lands within `REPAIR_CLUSTER_WINDOW`
    /// terminals of it (and so grows that cluster); `None` when the
    /// node's first recovery starts a cluster of its own, or there is
    /// none.
    density: Option<u32>,
    /// The repairs made before the node, when the budget is reached
    /// inside it; `None` when it is not.
    budget: Option<usize>,
    /// Its repairs, at terminals relative to its first.
    repairs: Vec<Repair>,
    /// Where its recoveries were entered, relative to its first terminal.
    books: Vec<usize>,
    /// The cluster size of its last recovery.
    end_cluster: u32,
}

/// What recovery reads of a stack entry besides its state: whether it is
/// a list under construction, and whether that list has pieces yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EntryKind {
    Other,
    EmptyList,
    List,
}

fn entry_kind(e: &Entry) -> EntryKind {
    match &e.sym {
        SymSlot::List(b) if b.pieces.is_empty() => EntryKind::EmptyList,
        SymSlot::List(_) => EntryKind::List,
        SymSlot::Child(_) => EntryKind::Other,
    }
}

/// The density state a node starting at `start` sees, as far as recovery
/// inside it can tell: whether its first recovery (`first_book`
/// terminals in) lands within the window of the last one before it, and
/// if so that cluster's size.
fn density_class(
    first_book: Option<usize>,
    start: usize,
    last_repair_at: usize,
    cluster: u32,
) -> Option<u32> {
    let f = first_book?;
    (last_repair_at != usize::MAX && start + f - last_repair_at <= REPAIR_CLUSTER_WINDOW)
        .then_some(cluster)
}

/// The budget a node with `own` repairs sees, as far as recovery inside
/// it can tell: nothing while the budget cannot be reached inside it, and
/// the exact count before it when it can.
fn budget_class(repairs_before: usize, own: usize) -> Option<usize> {
    (repairs_before + own >= MAX_REPAIRS).then_some(repairs_before)
}

/// Add what `c` holds to `node`'s counts, as a child of it: a node's
/// width, terms and tokens; a token's bytes, itself as a token, and as a
/// term unless trivia. A missing (repair-inserted) token counts nothing
/// but marks an error, and so does an error beneath. Every place that
/// gives a node a child counts it here, so stored counts are the sums
/// of the children's.
fn count_child(node: &mut GreenNode, c: &GreenChild) {
    match c {
        GreenChild::Node(n) => {
            node.width += n.width;
            node.terms += n.terms;
            node.n_toks += n.n_toks;
            node.has_err |= n.has_err;
        }
        GreenChild::Token(t) if t.is_missing() => node.has_err = true,
        GreenChild::Token(t) => {
            node.width += t.text.len() as u32;
            node.n_toks += 1;
            node.terms += u32::from(!t.trivia);
        }
    }
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
    /// The construct in progress was set aside as error material so a
    /// structural token an ENCLOSING construct was waiting for (a
    /// closer, a separator) could reach it.
    Unwound,
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
    simulate_depth(g, t, base, feed).0
}

/// Like [`simulate`], but also reports the stack depth WHERE THE FEED
/// ENDED UP. Two repairs can consume the lookahead equally well while
/// leaving the following text in different worlds: after
/// `struct Point P` + `;` the next function lives at file scope; after
/// `struct Point P` + `{` it lives inside a fabricated K&R body (and
/// after `(`, inside a fabricated parameter list) — both of which
/// absorb unboundedly. The final depth is the tell.
fn simulate_depth(g: &SynGrammar, t: &LrTables, base: &[u16], feed: &[TokenId]) -> (usize, usize) {
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
                    return (i, states.len());
                }
                states.truncate(states.len() - k);
                let top = *states.last().unwrap();
                match t.goto_[top as usize].get(&prod.lhs) {
                    Some(&n) => states.push(n),
                    None => return (i, states.len()),
                }
            }
            Some(LrAct::Accept) => return (feed.len(), states.len()),
            _ => return (i, states.len()),
        }
    }
    (i, states.len())
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
        // (seq, score, post-SIMULATION stack depth): among equal
        // scores, the repair that leaves the FOLLOWING TEXT at the
        // shallowest depth wins. A completing insert (`;` after
        // `struct Point P`) puts the next function back at file scope;
        // an opening insert (`(` or `{` in the same spot) fabricates a
        // viable-but-wrong prefix — a parameter list or K&R body —
        // that the rest of the document feeds without ever erroring
        // (each ate a 249k-line file). All of them simulate perfectly
        // inside the K-token horizon; where the lookahead ENDS UP is
        // the tell. Note the probe point: post-insert depth is not
        // enough — inserting `{` first REDUCES the declaration head
        // (landing shallow) and only then opens the door.
        // (Lexicographic order stays as the last tie-break for
        // determinism — it just no longer decides between a
        // terminator and an absorber by accident of token numbering.)
        let mut best_at_len: Option<(Vec<TokenId>, usize, usize)> = None;
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
                let (score, depth) = simulate_depth(g, t, &states2, peeked);
                if score >= 1 {
                    let better = match &best_at_len {
                        None => true,
                        Some((bseq, bscore, bdepth)) => {
                            score > *bscore
                                || (score == *bscore && depth < *bdepth)
                                || (score == *bscore && depth == *bdepth && seq2 < *bseq)
                        }
                    };
                    if better {
                        best_at_len = Some((seq2, score, depth));
                    }
                } else {
                    next.push((seq2, states2));
                }
            }
        }
        if let Some((seq, score, _)) = best_at_len {
            return Some((seq, score));
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
        None,
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

/// The real tokens beneath `c`, in order: trivia included, repair-
/// inserted (missing) tokens left out, as a batch parse reads them.
fn tokens_of(c: &GreenChild, out: &mut Vec<GreenChild>) {
    match c {
        GreenChild::Token(t) if t.is_missing() => {}
        GreenChild::Token(_) => out.push(c.clone()),
        GreenChild::Node(n) => {
            for ch in &n.children {
                tokens_of(ch, out);
            }
        }
    }
}

/// Whether a child holds a SYMBOL: a terminal the parser shifted, real
/// or repair-inserted (missing). Trivia and ERROR nodes are not symbols
/// but pending material, drained in front of the next shifted symbol;
/// an ε-node holds nothing.
fn holds_symbol(c: &GreenChild) -> bool {
    match c {
        GreenChild::Token(t) => !t.trivia,
        GreenChild::Node(m) if m.nt == crate::green::ERROR_NT => false,
        GreenChild::Node(m) => (m.terms > 0 && !m.has_err) || m.children.iter().any(holds_symbol),
    }
}

/// Whether a child is pending material: what the parser drains in front
/// of the next symbol it shifts. Trivia, and the ERROR nodes recovery
/// makes of skipped tokens and unwound entries (every ERROR node goes
/// through the pending queue).
fn is_pending_material(c: &GreenChild) -> bool {
    match c {
        GreenChild::Token(t) => t.trivia,
        GreenChild::Node(m) => m.nt == crate::green::ERROR_NT,
    }
}

/// Inject pending material (trivia, and ERROR nodes of skipped tokens)
/// down the left spine of a subtree so it sits where a batch parse
/// would drain it: immediately before the subtree's first SYMBOL (see
/// [`holds_symbol`]: a shifted terminal, real or repair-inserted), and
/// before that symbol's own contiguous run of drained material, because
/// the injected material is stream-earlier. A batch-built tree carries
/// pending material only there: in front of a shifted terminal, at that
/// terminal's level, after any ε-nodes reduced before the shift. Only
/// called for subtrees with ≥1 terminal.
///
/// An ERROR node is material, never the first symbol, even though it
/// counts its skipped tokens as terms: in front of `typedef`, a reused
/// node can hold `"\n" ERROR(])  "\n"`, all drained before `typedef`
/// when it was built, and new material goes before all three. (A node
/// reused while it holds an error has had the error material in front of
/// its first symbol taken out first, by `strip_lead`; ERROR nodes are
/// still material wherever else they stand.)
fn prepend_trivia(node: &Arc<GreenNode>, trivia: Vec<GreenChild>) -> Arc<GreenNode> {
    debug_assert!(node.terms > 0);
    let mut n = (**node).clone();
    // The injected material counts as any child counts: an ERROR node
    // brings its skipped tokens as terms and tokens, not one token for
    // the whole node.
    for c in &trivia {
        count_child(&mut n, c);
    }
    // First child holding a symbol (exists: terms > 0, and terms are
    // counted from shifted terminals or from ERROR material that was
    // drained in front of one).
    let idx = n
        .children
        .iter()
        .position(holds_symbol)
        .expect("terms > 0 implies a child holding a symbol");
    match &n.children[idx] {
        GreenChild::Token(_) => {
            // Back over the run of drained material that immediately
            // precedes the first symbol at this level; the new material
            // goes before it.
            let mut ins = idx;
            while ins > 0 && is_pending_material(&n.children[ins - 1]) {
                ins -= 1;
            }
            n.children.splice(ins..ins, trivia);
        }
        GreenChild::Node(m) => {
            // Children before the first symbol-holding node can only be
            // ε-nodes: drained material sits at the level of the symbol it
            // was drained in front of, which is inside this node. So
            // recursing into it preserves batch placement.
            debug_assert!(
                n.children[..idx]
                    .iter()
                    .all(|c| matches!(c, GreenChild::Node(e) if e.terms == 0 && !is_pending_material(c))),
                "unexpected material before the first symbol-holding child"
            );
            let m2 = prepend_trivia(m, trivia);
            n.children[idx] = GreenChild::Node(m2);
        }
    }
    Arc::new(n)
}

/// The recovery record of `node`, just reduced over `span`, with
/// `states` and `stack` what lies beneath it. `None` when it made no
/// repair of its own (an error in the material in front of it was decided
/// before it began: with that material taken out, it is a clean parse,
/// reused as any clean node is), or cannot be reused whole:
///
/// * it began at a terminal where a recovery had already been entered
///   (its first symbol was shifted, or inserted, by a recovery deciding
///   what came before it), so what it holds is bound up with decisions
///   that are not its own; or
/// * its own first recovery was entered at its first terminal, before its
///   first symbol was taken (after an ε-reduction it holds), where the
///   material that recovery makes is drained in front of its first symbol
///   among the material decided before it.
///
/// Such a node is re-parsed from its children, which carry records of
/// their own where they can be reused.
fn recovery_of(
    node: &GreenNode,
    span: &Span,
    states: &[u16],
    stack: &[Entry],
    repairs: &[Repair],
    books: &[usize],
) -> Option<Recovery> {
    let (at, end) = (span.at, span.end);
    if !node.has_err || node.terms == 0 || at.last_repair_at == at.consumed_terms {
        return None;
    }
    let own_books: Vec<usize> =
        books[at.books_len..end.books_len].iter().map(|b| b - at.consumed_terms).collect();
    if own_books.first() == Some(&0) {
        return None;
    }
    let own_repairs: Vec<Repair> = repairs[at.repairs_len..end.repairs_len]
        .iter()
        .map(|r| Repair { at_terminal: r.at_terminal - at.consumed_terms, kind: r.kind.clone() })
        .collect();
    if own_repairs.is_empty() {
        return None;
    }
    Some(Recovery {
        beneath: states.to_vec(),
        kinds: stack.iter().map(entry_kind).collect(),
        density: density_class(
            own_books.first().copied(),
            at.consumed_terms,
            at.last_repair_at,
            at.repair_cluster,
        ),
        budget: budget_class(at.repairs_len, own_repairs.len()),
        end_cluster: if own_books.is_empty() { 0 } else { end.repair_cluster },
        repairs: own_repairs,
        books: own_books,
    })
}

/// Take out the material in front of a node's first symbol (where the
/// parse drained what was pending when it took that symbol) when it holds
/// error material: what recovery decided before the node began, which a
/// parse reusing the node decides again. Yields the material, and what
/// goes back on the input after it in the node's place. `None` when the
/// material holds no error material (only trivia, which no parse decides:
/// it stays, and the new pending material goes in front of it).
///
/// What goes back is the node without the material, unless an
/// ε-reduction comes before its first symbol somewhere down its left
/// spine: an ε-node, or the ε seed of a list. An ε-reduction takes no
/// token to tell what it was made on: in front of error material, that
/// may have been a token recovery then skipped, or the one after the
/// material, and the tree is the same either way. So no node holding one
/// is reused whole: each goes back as its children down the spine (ε-nodes
/// among them are re-derived, never reused; see [`incremental_parse`]),
/// until the spine reaches a node with none in front of its first symbol.
fn strip_lead(
    node: &Arc<GreenNode>,
    g: &SynGrammar,
    t: &LrTables,
) -> Option<(Vec<GreenChild>, Vec<GreenChild>)> {
    let (stripped, lead) = strip_spine(node)?;
    let mut items = Vec::new();
    unroll_eps_spine(&stripped, g, t, &mut items);
    Some((items, lead))
}

/// `strip_lead` down the spine: the node without its lead material, and
/// the material.
fn strip_spine(node: &Arc<GreenNode>) -> Option<(Arc<GreenNode>, Vec<GreenChild>)> {
    let idx = node.children.iter().position(holds_symbol)?;
    let mut n = (**node).clone();
    let lead = match &node.children[idx] {
        GreenChild::Token(_) => {
            let mut from = idx;
            while from > 0 && is_pending_material(&node.children[from - 1]) {
                from -= 1;
            }
            if !node.children[from..idx].iter().any(|c| matches!(c, GreenChild::Node(_))) {
                return None;
            }
            n.children.drain(from..idx).collect()
        }
        GreenChild::Node(m) => {
            let (m2, lead) = strip_spine(m)?;
            n.children[idx] = GreenChild::Node(m2);
            lead
        }
    };
    recount(&mut n);
    Some((Arc::new(n), lead))
}

/// Whether an ε-reduction comes before a node's first symbol anywhere
/// down its left spine: an ε-node in front of it, or a whole list whose
/// seed is ε (the seed's reduction leaves no node).
fn eps_on_spine(node: &GreenNode, g: &SynGrammar, t: &LrTables) -> bool {
    let Some(idx) = node.children.iter().position(holds_symbol) else { return false };
    let eps_seed = node.prod == LIST_PROD
        && t.lists.get(&node.nt).is_some_and(|sh| g.prods[sh.seed as usize].rhs.is_empty());
    idx > 0
        || eps_seed
        || match &node.children[idx] {
            GreenChild::Node(m) => eps_on_spine(m, g, t),
            GreenChild::Token(_) => false,
        }
}

/// `node` as input items: whole, or, while an ε-reduction comes before
/// its first symbol down its spine, its children with the spine child
/// unrolled the same way.
fn unroll_eps_spine(
    node: &Arc<GreenNode>,
    g: &SynGrammar,
    t: &LrTables,
    out: &mut Vec<GreenChild>,
) {
    if !eps_on_spine(node, g, t) {
        out.push(GreenChild::Node(node.clone()));
        return;
    }
    let idx = node.children.iter().position(holds_symbol).expect("a spine has a symbol");
    for (i, c) in node.children.iter().enumerate() {
        match c {
            GreenChild::Node(m) if i == idx => unroll_eps_spine(m, g, t, out),
            other => out.push(other.clone()),
        }
    }
}

/// Recompute a node's counts from its children.
fn recount(n: &mut GreenNode) {
    n.width = 0;
    n.terms = 0;
    n.n_toks = 0;
    n.has_err = n.nt == crate::green::ERROR_NT;
    let children = std::mem::take(&mut n.children);
    for c in &children {
        count_child(n, c);
    }
    n.children = children;
}

/// Whether `n`'s last reductions are decided on `la` as they were (see
/// [`LrEnds`]): its own, and those of every node ending where it ends
/// (down its last symbol, and through the ε-nodes after it), each called
/// for in its state on `la`.
fn reduces_alike(t: &LrTables, n: &GreenNode, la: TokenId) -> bool {
    let Some(e) = n.lr_ends else { return false };
    if t.action[e.end as usize].get(&la) != Some(&LrAct::Reduce(e.prod)) {
        return false;
    }
    let children: Vec<&GreenChild> =
        if is_seq_prod(n.prod) { flat_children(n) } else { n.children.iter().collect() };
    for c in children.into_iter().rev() {
        match c {
            GreenChild::Token(tk) if tk.trivia => {}
            // The last symbol is a token: shifted, not reduced.
            GreenChild::Token(_) => return true,
            GreenChild::Node(m) => {
                if !reduces_alike(t, m, la) {
                    return false;
                }
                // An ε-node was reduced on the same terminal as what
                // comes before it.
                if m.terms > 0 {
                    return true;
                }
            }
        }
    }
    true
}

/// The terminal after the item at the front of the input: what a parse
/// reading that item next reads after it.
fn next_terminal(input: &VecDeque<Item>) -> TokenId {
    input
        .iter()
        .skip(1)
        .find_map(|item| match item {
            Item::Fresh(tok) => (!tok.trivia).then_some(tok.id),
            Item::Sub(c) => leftmost_term(c),
        })
        .unwrap_or(EOF)
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
    let mut stats = ReuseStats::default();
    let mut states: Vec<u16> = vec![0];
    let mut stack: Vec<Entry> = Vec::new();
    let mut pending: Vec<GreenChild> = Vec::new();
    let mut consumed_terms = 0usize;
    let mut repairs: Vec<Repair> = Vec::new();
    let mut last_repair_at = usize::MAX;
    let mut repair_cluster = 0u32;
    // Where each recovery was entered (the terminal it was entered at).
    let mut books: Vec<usize> = Vec::new();
    // Whether recovery is making the reductions: on a token it inserted,
    // or closing over default reductions at an error (which it skips over
    // budget). Such a reduction is recovery's decision, not the input's.
    let mut recovering = false;

    macro_rules! snap {
        () => {
            Snap {
                consumed_terms,
                repairs_len: repairs.len(),
                books_len: books.len(),
                last_repair_at,
                repair_cluster,
            }
        };
    }

    macro_rules! reduce {
        ($pidx:expr) => {{
            let pidx: u16 = $pidx;
            let prod = &g.prods[pidx as usize];
            let k = prod.rhs.len();
            let split = stack.len() - k;
            let popped: Vec<Entry> = stack.drain(split..).collect();
            // The state each popped entry was taken in, and the one it led
            // to.
            let around: Vec<(u16, u16)> =
                (0..k).map(|i| (states[split + i], states[split + i + 1])).collect();
            // The state the reduction was decided in.
            let decided_in = *states.last().unwrap();
            states.truncate(states.len() - k);
            let top = *states.last().unwrap();
            let next = *t.goto_[top as usize].get(&prod.lhs).expect("goto after reduce");
            let shape = t.lists.get(&prod.lhs).copied();
            // From the first popped symbol to the last (an ε reduce stands
            // where the parse is).
            let span = Span {
                at: popped.first().map_or(snap!(), |e| e.span.at),
                end: popped.last().map_or(snap!(), |e| e.span.end),
            };
            // Whether this reduction was decided by the symbols and the
            // terminal after them alone: recovery is not making it, and no
            // recovery came between the last popped symbol and it.
            let settled = !recovering
                && repairs.len() == span.end.repairs_len
                && books.len() == span.end.books_len;
            // The popped entries as list pieces (the cons head, the builder
            // itself, aside): material, then the symbol, settled as it was
            // and, for the last, as this reduction is.
            macro_rules! push_pieces {
                ($b:expr, $entries:expr) => {{
                    for (i, e) in $entries {
                        for m in e.leading {
                            $b.pieces.push(Piece::Loose(m, true));
                        }
                        let (s0, c) = around[i];
                        let ch = e.sym.into_child(g, t, s0, c);
                        let own = match &ch {
                            GreenChild::Node(n) => n.lr_ends.is_some(),
                            GreenChild::Token(_) => true,
                        };
                        $b.pieces.push(Piece::Loose(ch, own && (i + 1 < k || settled)));
                    }
                }};
            }
            let slot = match shape {
                // L4 cons: append α to the open builder instead of nesting.
                Some(sh) if pidx == sh.cons => {
                    let mut it = popped.into_iter().enumerate();
                    let (_, head) = it.next().unwrap();
                    debug_assert!(head.leading.is_empty(), "list slot never has leading");
                    let SymSlot::List(mut b) = head.sym else {
                        unreachable!("a list's cons reduces its open builder")
                    };
                    push_pieces!(b, it);
                    b.last = (decided_in, pidx);
                    SymSlot::List(b)
                }
                // L4 seed (incl. ε): open a fresh builder with the seed.
                Some(_) => {
                    let mut b = ListBuilder::new(prod.lhs, settled, (decided_in, pidx));
                    push_pieces!(b, popped.into_iter().enumerate());
                    SymSlot::List(b)
                }
                // Ordinary production: build the node.
                None => {
                    let mut children: Vec<GreenChild> = Vec::new();
                    for (i, e) in popped.into_iter().enumerate() {
                        let (s0, c) = around[i];
                        children.extend(entry_children(e, g, t, s0, c));
                    }
                    let ends = LrEnds { begin: top, end: decided_in, prod: pidx };
                    let node = make_node(prod.lhs, pidx, children, settled.then_some(ends));
                    let record = if settled {
                        recovery_of(&node, &span, &states, &stack, &repairs, &books)
                    } else {
                        None
                    };
                    let node = match record {
                        Some(r) => {
                            let mut n = Arc::unwrap_or_clone(node);
                            n.recovery = Some(Arc::new(r));
                            Arc::new(n)
                        }
                        None => node,
                    };
                    SymSlot::Child(GreenChild::Node(node))
                }
            };
            stack.push(Entry { leading: Vec::new(), sym: slot, span });
            states.push(next);
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
                    Some(LrAct::Reduce(p)) => {
                        recovering = true;
                        reduce!(p);
                        recovering = false;
                    }
                    Some(LrAct::Shift(n)) => {
                        stack.push(Entry {
                            leading: std::mem::take(&mut pending),
                            sym: SymSlot::Child(GreenChild::Token(GreenToken {
                                id: x,
                                trivia: false,
                                text: String::new(),
                            })),
                            span: Span { at: snap!(), end: snap!() },
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
                            let c = states.pop().expect("an entry has its state");
                            let s0 = *states.last().expect("the start state stays");
                            let kids = entry_children(e, g, t, s0, c);
                            pending.insert(
                                0,
                                GreenChild::Node(make_node(crate::green::ERROR_NT, crate::green::ERROR_PROD, kids, None)),
                            );
                        } else {
                            let kids = std::mem::take(&mut pending);
                            let root =
                                make_node(crate::green::ERROR_NT, crate::green::ERROR_PROD, kids, None);
                            stats.total_terms = consumed_terms as u32;
                            return Ok((root, stats, repairs));
                        }
                    }
                }
            } else {
                // ERROR-TIME DEFAULT-REDUCTION CLOSURE: reduces are
                // lookahead-gated, so with garbage lookahead the
                // automaton freezes MID-construct even when everything
                // before the garbage is complete (an empty pp_tokens
                // list refuses to reduce on `(` — the error fires
                // inside the directive, hiding the list boundary one
                // reduce-chain below the top). A state whose EVERY
                // action is the same reduce loses nothing by reducing
                // without a valid lookahead; closing over such states
                // surfaces the real boundary before repair chooses.
                let mut closure_steps = 0;
                while closure_steps < 32 {
                    let row = &t.action[*states.last().unwrap() as usize];
                    let mut only: Option<u16> = None;
                    let mut uniform = !row.is_empty();
                    for act in row.values() {
                        match act {
                            LrAct::Reduce(p) => match only {
                                None => only = Some(*p),
                                Some(q) if q == *p => {}
                                _ => {
                                    uniform = false;
                                    break;
                                }
                            },
                            _ => {
                                uniform = false;
                                break;
                            }
                        }
                    }
                    match (uniform, only) {
                        (true, Some(p)) => {
                            recovering = true;
                            reduce!(p);
                            recovering = false;
                            closure_steps += 1;
                        }
                        _ => break,
                    }
                }

                // Density bookkeeping: a repair landing within WINDOW
                // terminals of the previous one grows the cluster; a
                // clean stretch of shifting resets it.
                if last_repair_at != usize::MAX
                    && consumed_terms.saturating_sub(last_repair_at) <= REPAIR_CLUSTER_WINDOW
                {
                    repair_cluster += 1;
                } else {
                    repair_cluster = 1;
                }
                last_repair_at = consumed_terms;
                books.push(consumed_terms);
                let escalated = repair_cluster >= REPAIR_CLUSTER_ESCALATE;

                let peeked = peek_terms(&input, K + 1);
                let la = peeked.first().copied();
                // A lookahead some ENCLOSING state still wants (a
                // closer, a separator) is LOAD-BEARING: deleting it
                // hands everything that follows to the construct in
                // progress. The K-token simulate cannot see that
                // catastrophe past its horizon — deleting the `}` of
                // a two-line struct near the top of a 249k-line file
                // scored PERFECTLY (the next function parses as a
                // member declaration), the outline collapsed from 41k
                // marks to 2, and every keystroke inside the
                // incomplete member re-earned one document-sized
                // error region at 3-4 SECONDS each (the world-swallow,
                // measured by the keystroke lab). Such a token is
                // never deleted: a completing insert wins ties, and
                // failing that the in-progress item is set aside as
                // error material until the token can act.
                let la_wanted_below = la.is_some_and(|x| {
                    states[..states.len().saturating_sub(1)]
                        .iter()
                        .rev()
                        .take(128)
                        .any(|&s| t.action[s as usize].contains_key(&x))
                });
                let delete_score = if peeked.is_empty() {
                    0
                } else {
                    simulate(g, t, &states, &peeked[1..])
                };
                // LIST-BOUNDARY RESYNC: an error while the stack top
                // is a list under construction means the lookahead
                // could not START an element. Garbage must never
                // fabricate an element prefix there — a viable-but-
                // wrong prefix absorbs the GOOD constructs that follow
                // without ever erroring again (the file-scope orphan
                // expression read as declarator gymnastics and buried
                // twenty functions). Skipping is always the containing
                // choice at a boundary: real element starts shift.
                let at_list_boundary =
                    matches!(stack.last(), Some(Entry { sym: SymSlot::List(_), .. }));
                // Escalated clusters also skip the insert search: its
                // K-terminal validation is exactly what a repair
                // spiral exploits (and the BFS is the expensive part
                // of a repair storm).
                let insert_hit = if escalated || at_list_boundary {
                    None
                } else {
                    search_inserts(g, t, &states, &peeked)
                };
                if std::env::var_os("QANA_TRACE_REPAIR").is_some() {
                    let top = match stack.last() {
                        None => "-",
                        Some(Entry { sym: SymSlot::List(_), .. }) => "List",
                        Some(Entry { sym: SymSlot::Child(GreenChild::Node(_)), .. }) => "Node",
                        Some(Entry { sym: SymSlot::Child(GreenChild::Token(_)), .. }) => "Token",
                    };
                    // The pop ladder: for each depth, the entry popped
                    // (with its nonterminal or token name) and whether
                    // the exposed state acts on la.
                    let mut ladder = String::new();
                    if let Some(x) = la {
                        for k in 1..=stack.len().min(10) {
                            let name = match &stack[stack.len() - k].sym {
                                SymSlot::List(b) => format!(
                                    "L:{}",
                                    g.nt_names.get(b.nt as usize).map(|s| s.as_str()).unwrap_or("?")
                                ),
                                SymSlot::Child(GreenChild::Node(n)) => format!(
                                    "N:{}",
                                    g.nt_names.get(n.nt as usize).map(|s| s.as_str()).unwrap_or("?")
                                ),
                                SymSlot::Child(GreenChild::Token(tk)) => {
                                    format!("T:{}", g.term_name(tk.id))
                                }
                            };
                            let exposed = states[states.len() - 1 - k];
                            let acts = t.action[exposed as usize].contains_key(&x);
                            ladder.push_str(&name);
                            ladder.push(if acts { '+' } else { '.' });
                            ladder.push(' ');
                        }
                    }
                    eprintln!(
                        "[repair] term={consumed_terms} la={:?} top={top} depth={} ladder={ladder} list_boundary={at_list_boundary} wanted_below={la_wanted_below} delete_score={delete_score} insert={:?}",
                        la.map(|x| g.term_name(x)),
                        stack.len(),
                        insert_hit
                            .as_ref()
                            .map(|(seq, sc)| (seq.iter().map(|&x| g.term_name(x)).collect::<Vec<_>>(), *sc)),
                    );
                }
                // Choose: higher real consumption wins; ties prefer the
                // single delete over an insert sequence (lower cost) —
                // EXCEPT under a wanted-below lookahead, where the
                // insert is what keeps the wanted token alive and so
                // wins ties too.
                match insert_hit {
                    Some((seq, score))
                        if score > delete_score
                            || (la_wanted_below && score > 0 && score >= delete_score) =>
                    {
                        for &x in &seq {
                            repairs.push(Repair {
                                at_terminal: consumed_terms,
                                kind: RepairKind::Inserted(x),
                            });
                            apply_insert!(x);
                        }
                    }
                    _ => {
                        // Wanted below and no completing insert: set
                        // the in-progress item aside (pop into an
                        // error node) until the wanted token can act
                        // for its enclosing construct. EMPTY lists
                        // are transparent — an ε-seeded builder (the
                        // ptrs list of `struct P { i }`'s half-born
                        // declarator) protects nothing. A list WITH
                        // elements pops only as the LANDING step —
                        // when the wanted token acts directly beneath
                        // it, the list is the broken construct's own
                        // sub-structure (the spec+ holding `int` in
                        // `struct P { int }`). The probe never hunts
                        // PAST a non-empty list: elements at an outer
                        // level are good parses (a statement list, a
                        // field list of completed fields), and a
                        // wanted token belonging somewhere deeper
                        // never justifies dumping them.
                        let mut popped = false;
                        if let (true, Some(x)) = (la_wanted_below, la) {
                            let mut k = 0usize;
                            let depth = loop {
                                if k >= stack.len() {
                                    break None;
                                }
                                let nonempty_list = match &stack[stack.len() - 1 - k].sym {
                                    SymSlot::List(b) => !b.pieces.is_empty(),
                                    _ => false,
                                };
                                k += 1;
                                let exposed = states[states.len() - 1 - k];
                                if t.action[exposed as usize].contains_key(&x) {
                                    break Some(k);
                                }
                                if nonempty_list {
                                    break None;
                                }
                            };
                            if let Some(k) = depth {
                                repairs.push(Repair {
                                    at_terminal: consumed_terms,
                                    kind: RepairKind::Unwound,
                                });
                                for _ in 0..k {
                                    let e = stack.pop().expect("counted above");
                                    let c = states.pop().expect("an entry has its state");
                                    let s0 = *states.last().expect("the start state stays");
                                    let kids = entry_children(e, g, t, s0, c);
                                    pending.insert(
                                        0,
                                        GreenChild::Node(make_node(crate::green::ERROR_NT, crate::green::ERROR_PROD, kids, None)),
                                    );
                                }
                                popped = true;
                            }
                        }
                        if !popped {
                            delete_la!(); // skip: guaranteed progress
                        }
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
                        let GreenChild::Node(root) =
                            entry.sym.into_child(g, t, states[0], states[1])
                        else {
                            unreachable!("accept pops a rule node")
                        };
                        let mut root_owned = (*root).clone();
                        for c in pending.drain(..) {
                            count_child(&mut root_owned, &c);
                            root_owned.children.push(c);
                        }
                        stats.total_terms = consumed_terms as u32;
                        return Ok((Arc::new(root_owned), stats, repairs));
                    }
                    _ => recover!(true),
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
            // A repair-inserted (missing) token is not input: the parse
            // that inserted it invented it, and this parse re-derives (or
            // heals) it from the real tokens, as batch does. Salvage never
            // emits one; one reaches here when a reused node that holds
            // one is broken into its children (a node carrying an error
            // is reused away from the damage), and taking it for a
            // lookahead made recovery delete it, a repair batch never
            // makes.
            Some(Item::Sub(GreenChild::Token(tk))) if tk.is_missing() => {
                input.pop_front();
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
            debug_assert!(node.has_err || node.recovery.is_none(), "a record belongs to an error");

            // The material in front of a node carrying an error was decided
            // before the node began: it goes back to the input, to be
            // decided again where this parse stands. (Its own error
            // material is never there, so a node clean without it holds
            // no record anywhere beneath.)
            if !seq && node.has_err {
                if let Some((items, lead)) = strip_lead(&node, g, t) {
                    input.pop_front();
                    let back: Vec<GreenChild> = lead.into_iter().chain(items).collect();
                    for c in back.into_iter().rev() {
                        input.push_front(Item::Sub(c));
                    }
                    continue;
                }
            }

            // STATE MATCHING: a node is reused where the parse stands in
            // the state the node's own parse began in, and its last
            // reductions are decided as they were on the terminal after it
            // now (`LrEnds`). The automaton is deterministic and its tokens
            // are as they were, so a batch parse from here rebuilds it
            // exactly and reaches the state splicing it reaches. A node
            // carrying an error must also match its record: the rest of
            // what recovery inside it read.
            //
            // A node without a terminal (an ε-node, an empty list) is
            // never reused: there is nothing to save, and nothing in it
            // says what it was reduced on (see `strip_lead`). The parse
            // re-derives it from the terminal it now stands before, as
            // batch does.
            if node.terms > 0
                && node.lr_ends.is_some_and(|e| e.begin == state)
                && reduces_alike(t, &node, next_terminal(&input))
            {
                if seq {
                    // A list (whole, through its GOTO) or a run of whole
                    // units continuing the open list of its kind.
                    let open = matches!(
                        stack.last(),
                        Some(Entry { sym: SymSlot::List(b), .. }) if b.nt == node.nt
                    );
                    let seeds = node.prod == LIST_PROD;
                    let continues = node.prod == RUN_PROD && open;
                    if !node.has_err && (seeds || continues) {
                        input.pop_front();
                        let at = snap!();
                        let ends = node.lr_ends.expect("checked above");
                        if seeds {
                            let next = *t.goto_[state as usize]
                                .get(&node.nt)
                                .expect("the state a list began in has its GOTO");
                            stack.push(Entry {
                                leading: Vec::new(),
                                // Its pieces carry what the reused list's
                                // seed was.
                                sym: SymSlot::List(ListBuilder::new(
                                    node.nt,
                                    true,
                                    (ends.end, ends.prod),
                                )),
                                span: Span { at, end: at },
                            });
                            states.push(next);
                        }
                        let mut drains =
                            if node.terms == 0 { Vec::new() } else { std::mem::take(&mut pending) };
                        stats.reused_terms += node.terms;
                        stats.splices += 1;
                        consumed_terms += node.terms as usize;
                        let end = snap!();
                        let Some(Entry { sym: SymSlot::List(b), span, .. }) = stack.last_mut()
                        else {
                            unreachable!()
                        };
                        b.absorb_seq(&node, &mut drains);
                        b.last = (ends.end, ends.prod);
                        debug_assert!(drains.is_empty(), "a list with a terminal takes its drain");
                        span.end = end;
                        continue;
                    }
                } else {
                    let record = node.recovery.clone();
                    let reusable = match &record {
                        None => !node.has_err,
                        Some(rec) => {
                            states == rec.beneath
                                && stack.iter().map(entry_kind).eq(rec.kinds.iter().copied())
                                && density_class(
                                    rec.books.first().copied(),
                                    consumed_terms,
                                    last_repair_at,
                                    repair_cluster,
                                ) == rec.density
                                && budget_class(repairs.len(), rec.repairs.len()) == rec.budget
                        }
                    };
                    if reusable {
                        input.pop_front();
                        let next = *t.goto_[state as usize]
                            .get(&node.nt)
                            .expect("the state a node began in has its GOTO");
                        let at = snap!();
                        let drains =
                            if node.terms == 0 { Vec::new() } else { std::mem::take(&mut pending) };
                        if let Some(rec) = &record {
                            // What recovery did inside it, where it now
                            // stands.
                            let start = consumed_terms;
                            repairs.extend(rec.repairs.iter().map(|r| Repair {
                                at_terminal: start + r.at_terminal,
                                kind: r.kind.clone(),
                            }));
                            books.extend(rec.books.iter().map(|b| start + b));
                            if let Some(&last) = rec.books.last() {
                                last_repair_at = start + last;
                                repair_cluster = rec.end_cluster;
                            }
                        }
                        stats.reused_terms += node.terms;
                        stats.splices += 1;
                        consumed_terms += node.terms as usize;
                        let [child]: [GreenChild; 1] = materialize(GreenChild::Node(node), drains)
                            .try_into()
                            .unwrap_or_else(|_| unreachable!("a node takes its drain inside"));
                        stack.push(Entry {
                            leading: Vec::new(),
                            sym: SymSlot::Child(child),
                            span: Span { at, end: snap!() },
                        });
                        states.push(next);
                        continue;
                    }
                }
            }
            // Not reusable here. A reduce on its leftmost terminal is what
            // batch does next, and may bring the parse to the state the
            // node began in.
            if let Some(la) = leftmost_term(&GreenChild::Node(node.clone())) {
                if let Some(LrAct::Reduce(p)) = t.action[state as usize].get(&la).copied() {
                    reduce!(p);
                    continue;
                }
            }
            // Otherwise it is parsed from its children. An ERROR node (it
            // has no GOTO anywhere) breaks into its TOKENS: what recovery
            // wrapped in it is structure built before the parse gave up on
            // it, which a batch parse of the same text, reading tokens and
            // recovering afresh, need not build.
            input.pop_front();
            stats.breakdowns += 1;
            if node.nt == crate::green::ERROR_NT {
                let mut tokens = Vec::new();
                tokens_of(&GreenChild::Node(node.clone()), &mut tokens);
                for c in tokens.into_iter().rev() {
                    input.push_front(Item::Sub(c));
                }
            } else {
                for c in node.children.iter().rev() {
                    input.push_front(Item::Sub(c.clone()));
                }
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
                let at = snap!();
                consumed_terms += 1;
                stack.push(Entry {
                    leading: std::mem::take(&mut pending),
                    sym: SymSlot::Child(sym),
                    span: Span { at, end: snap!() },
                });
                states.push(next);
            }
            Some(LrAct::Reduce(p)) => reduce!(p),
            Some(LrAct::Accept) => unreachable!("accept only on EOF"),
            Some(LrAct::Error) | None => recover!(false),
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

/// The real terminals among the first `tok` tokens (trivia included) of
/// a tree.
fn terms_before(root: &GreenNode, tok: u32) -> usize {
    let mut terms = 0usize;
    let mut toks = 0u32;
    let mut node = root;
    'descend: loop {
        for c in &node.children {
            if toks >= tok {
                return terms;
            }
            match c {
                GreenChild::Node(n) if toks + n.n_toks <= tok => {
                    terms += n.terms as usize;
                    toks += n.n_toks;
                }
                GreenChild::Node(n) => {
                    node = n;
                    continue 'descend;
                }
                GreenChild::Token(t) if t.is_missing() => {}
                GreenChild::Token(t) => {
                    terms += usize::from(!t.trivia);
                    toks += 1;
                }
            }
        }
        return terms;
    }
}

/// The token index (trivia included) of real terminal `term` of a tree,
/// which has more than `term` terminals.
fn tok_of_term(root: &GreenNode, term: usize) -> u32 {
    let mut terms = 0usize;
    let mut toks = 0u32;
    let mut node = root;
    'descend: loop {
        for c in &node.children {
            match c {
                GreenChild::Node(n) if terms + n.terms as usize <= term => {
                    terms += n.terms as usize;
                    toks += n.n_toks;
                }
                GreenChild::Node(n) => {
                    node = n;
                    continue 'descend;
                }
                GreenChild::Token(t) if t.is_missing() => {}
                GreenChild::Token(t) if t.trivia => toks += 1,
                GreenChild::Token(_) => {
                    if terms == term {
                        return toks;
                    }
                    terms += 1;
                    toks += 1;
                }
            }
        }
        unreachable!("terminal {term} is past the tree's {terms}");
    }
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
    // For each region, the old token where the last `K + 1` terminals
    // before it begin: a node carrying an error must end at or before it
    // to be reused (see `walk`).
    let err_guards: Vec<u32> = regions
        .iter()
        .map(|r| match terms_before(old, r.old_toks.0).checked_sub(K + 1) {
            Some(term) => tok_of_term(old, term),
            None => 0,
        })
        .collect();
    let ctx = Ctx { regions, err_guards: &err_guards };
    walk(&GreenChild::Node(old.clone()), &ctx, &mut emitted, &mut tok_index, &mut out, false);
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

    /// `in_error`: `c` lies inside an ERROR node being dissolved. What
    /// recovery wrapped there (structure built before the parse gave up
    /// on it) is not structure a batch parse of the region would build:
    /// batch reads the region's tokens and recovers afresh. So inside a
    /// dissolved ERROR node only tokens are emitted, never a node whole.
    struct Ctx<'a> {
        regions: &'a [FreshRegion],
        err_guards: &'a [u32],
    }

    fn walk(
        c: &GreenChild,
        ctx: &Ctx,
        emitted: &mut usize,
        tok_index: &mut u32,
        out: &mut VecDeque<Item>,
        in_error: bool,
    ) {
        let regions = ctx.regions;
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
                // A node is reusable when the damage leaves its tokens as
                // they were: the parse checks the rest (the state it
                // stands in, and how the node's last reductions are
                // decided on the terminal after it now: `LrEnds`). A node
                // carrying an error needs the `K + 1` terminals after it
                // as they were too: recovery inside it reads that far (its
                // last recovery is entered at the latest at the terminal
                // after it, and reads K more), and its record checks what
                // it read before it (`docs/incremental_recovery.md`).
                // Re-running recovery for every error on every keystroke
                // cost a file with 27k transient errors ~2s per character.
                let reusable = !dirty(span, regions)
                    && (!n.has_err
                        || regions
                            .iter()
                            .zip(ctx.err_guards)
                            .all(|(r, &g)| r.old_toks.0 < span.1 || span.1 <= g));
                if reusable && !in_error {
                    // Maximal clean subtree: emit whole.
                    out.push_back(Item::Sub(c.clone()));
                    *tok_index += n.n_toks;
                } else {
                    let in_error = in_error || n.nt == crate::green::ERROR_NT;
                    for ch in &n.children {
                        walk(ch, ctx, emitted, tok_index, out, in_error);
                    }
                }
            }
        }
    }
}
