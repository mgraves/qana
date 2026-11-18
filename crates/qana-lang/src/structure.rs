//! The structure of a live document, kept across edits: its outline
//! (`Limner::marks_shared`) and its folds (`Limner::folds_shared`), with
//! the outline's shape hash (see `crate::live`).
//!
//! `qana_services::outline` and `qana_services::folds` read a whole tree.
//! An editor asks for both after every edit (breadcrumbs on every cursor
//! move), and an edit leaves most of a tree as it was: the incremental
//! parser hands back the same subtrees, by pointer, for everything it did
//! not rebuild. What a subtree contributes depends on nothing outside it
//! but how it is entered: how many outline entries enclose it (its marks'
//! nesting), and what the tokens before it leave undecided for folding (a
//! comment run that has not ended: `qana_services::FoldState`). So the
//! structure is kept by subtree, at every depth: a subtree up to
//! [`PIECE_BYTES`] is read whole, a larger one child by child, and a
//! subtree that is the same allocation as before, entered as before, is
//! not read at all: its marks and folds are the ones it made last time,
//! moved to where it now starts.
//!
//! A large construct's own outline entry is read from the construct; its
//! own fold is put together from what its children report of themselves
//! (`qana_services::FoldWalk`): their line breaks, and their first and
//! last real tokens. A keystroke inside one function of a 100,000-line
//! module reads that function again and the chain of constructs around
//! it, and copies the rest.

use linework::Mark;
use qana_grammar::green::{GreenChild, GreenNode, GreenToken, ERROR_NT};
use qana_services::{
    end_folds, node_folds, token_folds, FoldConfig, FoldEntry, FoldState, OutlineConfig, RelFold,
};
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use std::ops::Range;
use std::sync::Arc;

/// Hashes the address a subtree is kept by. Addresses are distinct and
/// aligned, so one multiplication spreads them over the table; a keyed
/// hash buys nothing here, and costs the most of what keeping costs.
#[derive(Default)]
struct AddressHasher(u64);

impl Hasher for AddressHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0.rotate_left(8) ^ b as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        }
    }

    fn write_usize(&mut self, address: usize) {
        self.0 = (address as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15).rotate_right(29);
    }
}

/// Subtrees kept, by address.
type ByAddress = HashMap<usize, Kept, BuildHasherDefault<AddressHasher>>;

/// A subtree up to this many bytes is read whole; a larger one, child by
/// child. Small enough that reading one again is cheap next to a
/// keystroke, large enough that a document of megabytes is a few thousand
/// of them.
pub(crate) const PIECE_BYTES: u32 = 4096;

/// The multiplier of the shape hash: odd, so multiplying by it loses
/// nothing.
const SHAPE_P: u64 = 0x9E37_79B9_7F4A_7C15;

/// One mark's contribution to the shape: its name, its category and how
/// deeply it is nested. Its position is left out on purpose: positions
/// move with every keystroke, and the shape is what does not.
fn shape_element(name: &str, category: u8, depth: usize) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for &b in name.as_bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    for b in [0xff, category, depth as u8] {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h ^= h >> 32;
    h = h.wrapping_mul(0xD6E8_FEB8_6659_FD93);
    h ^ (h >> 32)
}

/// Add one element to a shape hash. The hash of a sequence is a
/// polynomial in [`SHAPE_P`], so the hash of two sequences joined is
/// `left * SHAPE_P^len(right) + right`: the shapes of subtrees join into
/// the shape of the document without reading any mark again, and the
/// result does not depend on where the subtrees divide.
fn shape_fold(h: u64, element: u64) -> u64 {
    h.wrapping_mul(SHAPE_P).wrapping_add(element)
}

/// A symbol of `qana_services::outline`, as a mark at `base`.
pub(crate) fn mark_at(symbol: qana_services::Symbol, base: u32, kinds: &[&'static str]) -> Mark {
    Mark {
        category: kinds.iter().position(|k| *k == symbol.kind).unwrap_or(0) as u8,
        start: base + symbol.span.0,
        end: base + symbol.span.1,
        name_start: base + symbol.selection.0,
        name_end: base + symbol.selection.1,
        name: symbol.name,
    }
}

/// A fold, at `base`, as a mark.
fn fold_mark(f: RelFold, base: u32) -> Mark {
    let f = f.at(base);
    Mark {
        category: f.category,
        start: f.span.0,
        end: f.span.1,
        name_start: f.head.0,
        name_end: f.head.1,
        name: String::new(),
    }
}

/// What a subtree, or a run of tokens, reports of itself to the
/// construct around it.
#[derive(Clone, Debug)]
struct Report {
    /// The fold state it leaves, relative to its end.
    left: FoldState,
    /// Its line breaks, and its first and last real tokens (relative to
    /// its start, with the line breaks before them).
    lines: u64,
    first_real: Option<(i64, i64, u64)>,
    last_real: Option<(i64, u64)>,
    /// The shape of its marks, and `SHAPE_P` to the power of their count.
    shape: u64,
    shape_pow: u64,
}

/// A subtree as a revision read it.
struct Kept {
    /// The subtree: identity and keepalive in one (while it is held, its
    /// address cannot be handed to another subtree).
    node: Arc<GreenNode>,
    /// Where it started.
    base: u32,
    /// How it was entered: the fold state (relative to its start), and
    /// how many marks enclosed it.
    entered: FoldState,
    outer: usize,
    /// Its marks and folds, in that revision's vectors.
    marks: Range<usize>,
    folds: Range<usize>,
    report: Report,
    /// For a subtree read child by child: its children that were kept
    /// too, so that they stay kept when it is reused whole (and can be
    /// reused on their own when it is not).
    children: Vec<usize>,
}

/// The outline and folds of one document.
pub(crate) struct Structure {
    /// The revision these are of. Zero: never read.
    rev: u64,
    marks: Arc<Vec<Mark>>,
    /// Every construct and comment fold, in document order, with a place
    /// held for each large construct's own fold (empty where it folds
    /// nothing). Of two folds over one span the outer comes first, and
    /// is the only one in [`Structure::folds`]: this, without the empty
    /// places and the inner ones.
    raw_folds: Vec<Option<Mark>>,
    folds: Arc<Vec<Mark>>,
    /// The hash of the outline's shape.
    pub(crate) shape: u64,
    kept: ByAddress,
    /// How many bytes of subtrees the last revision read whole: what
    /// keeping the rest saved is measured by it.
    pub(crate) read: usize,
}

/// How far a subtree reused whole moved: where it starts, and where its
/// marks and folds are in the vectors.
#[derive(Clone, Copy)]
struct Shift {
    base: u32,
    marks: usize,
    folds: usize,
}

/// Building one revision's structure.
struct Build<'a> {
    outline: &'a OutlineConfig,
    kinds: &'a [&'static str],
    fold_cfg: &'a FoldConfig,
    /// The last revision's subtrees and vectors. The vectors are the
    /// structure's own when no editor still holds the marks: what a kept
    /// subtree made is then moved out of them, not copied.
    old: ByAddress,
    old_marks: &'a mut [Mark],
    old_folds: &'a mut [Option<Mark>],
    kept: ByAddress,
    marks: Vec<Mark>,
    folds: Vec<Option<Mark>>,
    read: usize,
    /// The children being kept of each construct being read child by
    /// child, innermost last.
    children: Vec<Vec<usize>>,
}

impl Build<'_> {
    /// Read `node`, which starts at `base` and is entered in `st` with
    /// `outer` marks around it.
    fn subtree(&mut self, node: &Arc<GreenNode>, base: u32, st: FoldState, outer: usize) -> Report {
        let key = Arc::as_ptr(node) as usize;
        if let Some(siblings) = self.children.last_mut() {
            siblings.push(key);
        }
        let (mark_start, fold_start) = (self.marks.len(), self.folds.len());
        let entered = st.clone();
        let (report, children) = match self.old.remove(&key) {
            Some(k) if Arc::ptr_eq(&k.node, node) && k.entered == st && k.outer == outer => {
                // The same subtree, entered as before: what it made, moved.
                let by = base.wrapping_sub(k.base);
                let moved = |m: &mut Mark| Mark {
                    category: m.category,
                    start: m.start.wrapping_add(by),
                    end: m.end.wrapping_add(by),
                    name_start: m.name_start.wrapping_add(by),
                    name_end: m.name_end.wrapping_add(by),
                    name: std::mem::take(&mut m.name),
                };
                self.marks.extend(self.old_marks[k.marks.clone()].iter_mut().map(moved));
                self.folds.extend(self.old_folds[k.folds.clone()].iter_mut().map(|f| f.as_mut().map(moved)));
                // Its kept children are kept still, where it put them.
                let shift = Shift {
                    base: by,
                    marks: mark_start.wrapping_sub(k.marks.start),
                    folds: fold_start.wrapping_sub(k.folds.start),
                };
                for &child in &k.children {
                    self.carry(child, shift);
                }
                (k.report, k.children)
            }
            _ if node.width <= PIECE_BYTES
                || !node.children.iter().any(|c| matches!(c, GreenChild::Node(_))) =>
            {
                (self.whole(node, base, st, outer), Vec::new())
            }
            _ => {
                self.children.push(Vec::new());
                let report = self.by_children(node, base, st, outer);
                (report, self.children.pop().expect("pushed above"))
            }
        };
        self.kept.insert(
            key,
            Kept {
                node: node.clone(),
                base,
                entered,
                outer,
                marks: mark_start..self.marks.len(),
                folds: fold_start..self.folds.len(),
                report: report.clone(),
                children,
            },
        );
        report
    }

    /// Keep the kept subtree `key` of a subtree reused whole, and its
    /// kept children: moved by `shift`, as what it made was.
    fn carry(&mut self, key: usize, shift: Shift) {
        let Some(mut k) = self.old.remove(&key) else { return };
        k.base = k.base.wrapping_add(shift.base);
        k.marks = k.marks.start.wrapping_add(shift.marks)..k.marks.end.wrapping_add(shift.marks);
        k.folds = k.folds.start.wrapping_add(shift.folds)..k.folds.end.wrapping_add(shift.folds);
        for &child in &k.children {
            self.carry(child, shift);
        }
        self.kept.insert(key, k);
    }

    /// Read a subtree whole.
    fn whole(&mut self, node: &GreenNode, base: u32, st: FoldState, outer: usize) -> Report {
        self.read += node.width as usize;
        let (mut shape, mut shape_pow) = (0u64, 1u64);
        let mut open_ends: Vec<u32> = Vec::new();
        for symbol in qana_services::outline(node, self.outline) {
            let m = mark_at(symbol, base, self.kinds);
            while open_ends.last().is_some_and(|&e| m.start >= e) {
                open_ends.pop();
            }
            shape = shape_fold(shape, shape_element(&m.name, m.category, outer + open_ends.len()));
            shape_pow = shape_pow.wrapping_mul(SHAPE_P);
            open_ends.push(m.end);
            self.marks.push(m);
        }
        let walk = node_folds(node, self.fold_cfg, st);
        self.folds.extend(walk.folds.into_iter().map(|f| Some(fold_mark(f, base))));
        Report {
            left: walk.left,
            lines: walk.lines,
            first_real: walk.first_real,
            last_real: walk.last_real,
            shape,
            shape_pow,
        }
    }

    /// Read a large subtree child by child: its own mark, then its
    /// children's; its own fold, put together from what they report, at
    /// the place where the fold's first child begins.
    fn by_children(&mut self, node: &GreenNode, base: u32, st: FoldState, outer: usize) -> Report {
        let (mut shape, mut shape_pow) = (0u64, 1u64);
        let mut inner = outer;
        if let Some(symbol) = qana_services::node_symbol(node, self.outline) {
            let m = mark_at(symbol, base, self.kinds);
            shape = shape_fold(shape, shape_element(&m.name, m.category, outer));
            shape_pow = shape_pow.wrapping_mul(SHAPE_P);
            self.marks.push(m);
            inner += 1;
        }
        let mut g = Gather {
            report: Report { left: st, lines: 0, first_real: None, last_real: None, shape, shape_pow },
            place: None,
            waiting: false,
            first: None,
            last: None,
        };
        let entry = self.fold_cfg.entry(node.prod);
        if let Some(FoldEntry { from: None, .. }) = entry {
            // The fold is the whole construct's: it waits from the start.
            g.hold_place(&mut self.folds);
        }

        let mut off = 0u32;
        let mut symbol = 0usize;
        let mut tokens: Vec<&GreenToken> = Vec::new();
        let mut tokens_at = 0u32;
        for c in &node.children {
            let is_symbol = match c {
                GreenChild::Token(t) => !t.trivia && !t.is_missing(),
                GreenChild::Node(m) => m.nt != ERROR_NT,
            };
            if let (true, Some(FoldEntry { from: Some(k), .. })) = (is_symbol, entry) {
                if k == symbol {
                    // The fold begins with this child: its place comes
                    // before the child's own folds.
                    self.flush_tokens(&mut tokens, tokens_at, base, &mut g);
                    g.hold_place(&mut self.folds);
                }
            }
            match c {
                GreenChild::Token(t) => {
                    if tokens.is_empty() {
                        tokens_at = off;
                    }
                    tokens.push(t);
                }
                GreenChild::Node(m) => {
                    self.flush_tokens(&mut tokens, tokens_at, base, &mut g);
                    let st = std::mem::replace(&mut g.report.left, FoldState::start());
                    let child = self.subtree(m, base + off, st, inner);
                    g.absorb(&child, off as i64);
                }
            }
            off += c.width();
            if is_symbol {
                if let Some(FoldEntry { to: Some(k), .. }) = entry {
                    if k == symbol {
                        // The fold ends with this child.
                        self.flush_tokens(&mut tokens, tokens_at, base, &mut g);
                        g.end();
                    }
                }
                symbol += 1;
            }
        }
        self.flush_tokens(&mut tokens, tokens_at, base, &mut g);
        if let Some(FoldEntry { to: None, .. }) = entry {
            g.end();
        }
        if let (Some(e), Some(place), Some((start, head_end, at_start)), Some((end, at_end))) =
            (entry, g.place, g.first, g.last)
        {
            if end > start && at_end > at_start {
                let f = RelFold { category: e.category, span: (start, end), head: (start, head_end) };
                self.folds[place] = Some(fold_mark(f, base));
            }
        }
        g.report
    }

    /// Read the tokens gathered since the last child that was not one.
    fn flush_tokens(&mut self, tokens: &mut Vec<&GreenToken>, at: u32, base: u32, g: &mut Gather) {
        if tokens.is_empty() {
            return;
        }
        let st = std::mem::replace(&mut g.report.left, FoldState::start());
        let walk = token_folds(tokens.drain(..), self.fold_cfg, st);
        self.folds.extend(walk.folds.iter().map(|&f| Some(fold_mark(f, base + at))));
        let run = Report {
            left: walk.left,
            lines: walk.lines,
            first_real: walk.first_real,
            last_real: walk.last_real,
            shape: 0,
            shape_pow: 1,
        };
        g.absorb(&run, at as i64);
    }
}

/// A large construct being read child by child: what its children have
/// reported, and its own fold: the place held for it, whether it waits
/// for its first real token, and its first and last real tokens once
/// known (with the line breaks before them).
struct Gather {
    report: Report,
    place: Option<usize>,
    waiting: bool,
    first: Option<(i64, i64, u64)>,
    last: Option<(i64, u64)>,
}

impl Gather {
    fn hold_place(&mut self, folds: &mut Vec<Option<Mark>>) {
        folds.push(None);
        self.place = Some(folds.len() - 1);
        self.waiting = true;
    }

    /// Take in what a child, at `at` within the construct, reports. The
    /// first real token after the fold's place is the fold's first.
    fn absorb(&mut self, child: &Report, at: i64) {
        if let (true, Some((s, e, l))) = (self.waiting, child.first_real) {
            self.first = Some((s + at, e + at, self.report.lines + l));
            self.waiting = false;
        }
        absorb(&mut self.report, child, at);
    }

    /// The fold ends at the last real token reported so far. One whose
    /// first real token never came folds nothing.
    fn end(&mut self) {
        self.waiting = false;
        self.last = self.report.last_real;
    }
}

/// Join what `child`, at `at` within the construct, reports into what the
/// construct has gathered.
fn absorb(report: &mut Report, child: &Report, at: i64) {
    if report.first_real.is_none() {
        report.first_real =
            child.first_real.map(|(s, e, l)| (s + at, e + at, report.lines + l));
    }
    if let Some((e, l)) = child.last_real {
        report.last_real = Some((e + at, report.lines + l));
    }
    report.lines += child.lines;
    report.left = child.left.clone();
    report.shape = report.shape.wrapping_mul(child.shape_pow).wrapping_add(child.shape);
    report.shape_pow = report.shape_pow.wrapping_mul(child.shape_pow);
}

impl Structure {
    pub(crate) fn new() -> Structure {
        Structure {
            rev: 0,
            marks: Arc::new(Vec::new()),
            raw_folds: Vec::new(),
            folds: Arc::new(Vec::new()),
            shape: 0,
            kept: ByAddress::default(),
            read: 0,
        }
    }

    /// Bring the structure to revision `rev`, whose tree is `tree`: read
    /// again what changed, and keep the rest. Asked twice in one revision,
    /// the second time does nothing.
    pub(crate) fn update(
        &mut self,
        tree: &Arc<GreenNode>,
        outline: &OutlineConfig,
        kinds: &[&'static str],
        fold_cfg: &FoldConfig,
        rev: u64,
    ) {
        if self.rev == rev {
            return;
        }
        let old = std::mem::take(&mut self.kept);
        let kept_capacity = old.len();
        // The last revision's marks, the structure's own unless an editor
        // still holds them (then a copy: what is taken from it is moved).
        let mut old_marks: Vec<Mark> =
            Arc::try_unwrap(std::mem::take(&mut self.marks)).unwrap_or_else(|held| (*held).clone());
        let mut old_folds = std::mem::take(&mut self.raw_folds);
        let mut b = Build {
            outline,
            kinds,
            fold_cfg,
            old,
            old_marks: &mut old_marks,
            old_folds: &mut old_folds,
            kept: ByAddress::with_capacity_and_hasher(kept_capacity, Default::default()),
            marks: Vec::with_capacity(self.marks.len()),
            folds: Vec::with_capacity(self.raw_folds.len()),
            read: 0,
            children: Vec::new(),
        };
        let report = b.subtree(tree, 0, FoldState::start(), 0);
        let Build { kept, marks, folds: mut raw, read, .. } = b;
        // The end of the document closes what is still open.
        raw.extend(end_folds(fold_cfg, report.left).into_iter().map(|f| Some(fold_mark(f, tree.width))));

        // The folds: the places that hold one, in document order (a
        // comment run carried into a construct can come out after the
        // construct's own fold that it precedes; then they are put in
        // order, the outer of two at one start first), and of two over one
        // span the first, the outer one.
        let mut folds: Vec<Mark> = raw.iter().flatten().cloned().collect();
        if folds.windows(2).any(|w| (w[0].start, std::cmp::Reverse(w[0].end)) > (w[1].start, std::cmp::Reverse(w[1].end))) {
            folds.sort_by_key(|f| (f.start, std::cmp::Reverse(f.end)));
        }
        folds.dedup_by_key(|f| (f.start, f.end));

        self.marks = Arc::new(marks);
        self.raw_folds = raw;
        self.folds = Arc::new(folds);
        self.shape = report.shape;
        self.kept = kept;
        self.read = read;
        self.rev = rev;
    }

    /// The marks, as of the last [`update`](Self::update).
    pub(crate) fn marks(&self) -> Arc<Vec<Mark>> {
        self.marks.clone()
    }

    /// The folds, as of the last [`update`](Self::update).
    pub(crate) fn folds(&self) -> Arc<Vec<Mark>> {
        self.folds.clone()
    }
}
