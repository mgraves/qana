//! A LIVE DOCUMENT: the whole certified pipeline behind one
//! [`linework::Limner`]. This is what an editor widget holds — through
//! the trait, never through this crate — to get keystroke-synchronous
//! paint, semantic modifier bits, hover facts, and inline hints from
//! a single `.qana` grammar.
//!
//! Layering (the point of the exercise): the editor depends on `linework`
//! alone; this crate depends on `linework` and implements it. The two
//! meet only at the trait.

use crate::folds::FoldIndex;
use crate::EmbeddedLang;
use linework::{FactCard, Hint, LineEdit, Limner, Mark, Paint, PaintDelta};
use qana_engine::{IncSession, Line, LineTerm};
use qana_grammar::green::{GreenChild, GreenNode, LIST_PROD, RUN_PROD};
use qana_sem::SemDb;
use qana_services::paint::{facts_at, type_hints, Painter};
use qana_services::OutlineConfig;
use std::sync::Arc;

/// A stopwatch for one stage of an edit, feeding the `QANA_TRACE_EDIT`
/// timings. It reads the clock only when the trace is on. An untraced
/// edit has no use for the time, and on `wasm32-unknown-unknown`
/// `std::time::Instant::now` panics, so an edit made in a browser must
/// never reach it. (No environment exists there, so the trace is off.)
struct StageClock(Option<std::time::Instant>);

impl StageClock {
    fn start(trace: bool) -> Self {
        StageClock(trace.then(std::time::Instant::now))
    }

    /// The time since [`StageClock::start`]. Zero when the trace is off,
    /// where nothing prints it.
    fn elapsed(&self) -> std::time::Duration {
        self.0.map(|started| started.elapsed()).unwrap_or_default()
    }
}

/// One open document over one language. Owns the incremental session,
/// the semantic database, and the painter; answers the [`Limner`]
/// contract from them.
pub struct LiveDoc {
    lang: Arc<EmbeddedLang>,
    uri: String,
    session: IncSession<'static>,
    db: SemDb,
    painter: Painter,
    /// Marks-channel revision: bumps on every edit and reopen. Keys
    /// the marks cache and answers [`Limner::marks_rev`] so editors
    /// gate their derived outline work on real structure changes.
    marks_rev: u64,
    /// The outline, kept across edits: see [`Outline`]. Editors query
    /// marks per cursor tick, and after an edit only the part of the
    /// tree the edit rebuilt is walked again.
    outline: Outline,
    /// The SHAPE revision: bumps only when the (name, category,
    /// nesting) sequence of the marks changes — spans shift on every
    /// keystroke, the shape survives body edits. Computed as a hash
    /// with the outline; consumers gate outline-tree rebuilds on this,
    /// so typing costs them nothing.
    shape_rev: u64,
    shape_hash: u64,
    /// The folds channel: what each line contributes, kept across edits,
    /// and the folds matched from it once per revision.
    folds: FoldIndex,
}

// ---------------------------------------------------------------------------
// The outline, kept across edits
// ---------------------------------------------------------------------------
//
// `qana_services::outline` walks a whole tree. An editor asks for the
// outline after every edit (breadcrumbs ask on every cursor move), and
// at 250,000 lines the walk is 25 ms: per keystroke, for an edit that
// changed one function.
//
// An edit leaves most of a tree as it was. The incremental parser hands
// back the same subtrees, by pointer, for everything it did not have to
// rebuild. And the outline of a subtree depends on nothing outside it:
// the marks of a top-level construct, relative to where it starts, are
// the same wherever it sits. So the outline is kept in UNITS, the
// top-level constructs of the file, and after an edit only the units
// that are new subtrees are walked. The rest keep their marks, moved to
// where the construct now starts.

/// One top-level construct of the file and the marks it contributes.
struct OutlineUnit {
    /// The construct. Identity (the same allocation is the same
    /// construct) and keepalive in one: while the unit holds it, its
    /// address cannot be handed to another subtree.
    node: Arc<GreenNode>,
    /// Where the construct starts, in bytes. Its marks are absolute, so
    /// when this moves they move with it.
    base: u32,
    /// One past the index of its last mark in [`Outline::marks`]. Its
    /// first is where the unit before it ended.
    mark_end: u32,
    /// The [`shape_fold`] of its marks, and `SHAPE_P` to the power of
    /// their count: what joins unit shapes into the file's.
    shape: u64,
    shape_pow: u64,
}

struct Outline {
    /// The [`LiveDoc::marks_rev`] this outline is of. Zero: never walked.
    rev: u64,
    /// Every mark of the document, in document order, absolute. Handed
    /// out by `Arc`; when nobody else holds one at the next edit, it is
    /// patched where it stands.
    marks: Arc<Vec<Mark>>,
    /// 1 when `marks[0]` is the mark of the ROOT node itself (a grammar
    /// whose start production is outline-worthy), else 0. Every other
    /// mark then lies inside it, one level deeper.
    head: usize,
    units: Vec<OutlineUnit>,
}

impl Outline {
    fn new() -> Self {
        Outline { rev: 0, marks: Arc::new(Vec::new()), head: 0, units: Vec::new() }
    }
}

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
/// `left * SHAPE_P^len(right) + right`: the shapes of the units join
/// into the shape of the file without reading any mark again, and the
/// result does not depend on where the units divide.
fn shape_fold(h: u64, element: u64) -> u64 {
    h.wrapping_mul(SHAPE_P).wrapping_add(element)
}

/// A symbol of `qana_services::outline`, as a mark at `base`.
fn mark_at(symbol: qana_services::Symbol, base: u32, kinds: &[&'static str]) -> Mark {
    Mark {
        category: kinds.iter().position(|k| *k == symbol.kind).unwrap_or(0) as u8,
        start: base + symbol.span.0,
        end: base + symbol.span.1,
        name_start: base + symbol.selection.0,
        name_end: base + symbol.selection.1,
        name: symbol.name,
    }
}

/// The units of a tree, in document order, each with where it starts:
/// the elements of the root's list children, and every other node child
/// of the root. Between the root and a unit there are only the root
/// itself and the list nodes (`LIST_PROD`, `RUN_PROD`), which no outline
/// entry can name, so `outline(tree)` is the root's own symbol followed
/// by the outline of each unit in turn.
fn collect_units<'t>(root: &'t GreenNode, out: &mut Vec<(&'t Arc<GreenNode>, u32)>) {
    fn list<'t>(n: &'t GreenNode, base: u32, out: &mut Vec<(&'t Arc<GreenNode>, u32)>) {
        let mut off = base;
        for c in &n.children {
            match c {
                GreenChild::Node(m) if m.prod == RUN_PROD => list(m, off, out),
                GreenChild::Node(m) => out.push((m, off)),
                GreenChild::Token(_) => {}
            }
            off += c.width();
        }
    }
    let mut off = 0u32;
    for c in &root.children {
        if let GreenChild::Node(n) = c {
            if n.prod == LIST_PROD {
                list(n, off, out);
            } else {
                out.push((n, off));
            }
        }
        off += c.width();
    }
}

/// Walk one unit: its marks, absolute, and its shape. `outer` is how
/// many marks enclose every unit (the root's own, if it has one).
fn walk_unit(
    node: &Arc<GreenNode>,
    base: u32,
    outer: usize,
    cfg: &OutlineConfig,
    kinds: &[&'static str],
    marks: &mut Vec<Mark>,
) -> (u64, u64) {
    let (mut shape, mut pow) = (0u64, 1u64);
    let mut open_ends: Vec<u32> = Vec::new();
    for symbol in qana_services::outline(node, cfg) {
        let m = mark_at(symbol, base, kinds);
        while open_ends.last().is_some_and(|&e| m.start >= e) {
            open_ends.pop();
        }
        shape = shape_fold(shape, shape_element(&m.name, m.category, outer + open_ends.len()));
        pow = pow.wrapping_mul(SHAPE_P);
        open_ends.push(m.end);
        marks.push(m);
    }
    (shape, pow)
}

impl LiveDoc {
    /// Open `text` under `lang`. The `uri` names the document in fact
    /// cards (and, later, in multi-document worlds).
    pub fn open(lang: Arc<EmbeddedLang>, uri: impl Into<String>, text: &str) -> LiveDoc {
        let uri = uri.into();
        let session = lang.session(text);
        let mut db = SemDb::new(lang.binding.clone());
        db.set_types(lang.types.clone());
        db.set_macro_bodies(&lang.macros);
        db.set_tree(&uri, session.tree().expect("total").clone());
        let (painter, _) = Painter::new(&session, &lang.styles, Some((&mut db, &uri)));
        let folds = FoldIndex::new(lang.lexer, |id| style_class(&lang, id), &session.buf);
        LiveDoc {
            lang,
            uri,
            session,
            db,
            painter,
            marks_rev: 1,
            outline: Outline::new(),
            shape_rev: 0,
            shape_hash: 0,
            folds,
        }
    }

    /// The outline marks for the CURRENT revision.
    ///
    /// Asked twice in one revision, the second answer is the first's
    /// `Arc`. Asked after an edit, the units the edit left alone keep
    /// their marks, and only the units that are new subtrees are walked.
    fn marks_now(&mut self) -> Arc<Vec<Mark>> {
        if self.outline.rev == self.marks_rev {
            return self.outline.marks.clone();
        }
        let cfg = &self.lang.outline;
        let kinds = mark_kinds(cfg);
        let tree = self.session.tree().expect("total");
        let outline = &mut self.outline;

        // The list nodes between the root and a unit have productions of
        // their own that no grammar's outline entry names. Were one ever
        // to (a grammar of 65,534 productions), the units would not be
        // the whole story; the tree is then one unit, walked whole.
        let spine_is_silent =
            !cfg.entries.iter().any(|e| e.prod == LIST_PROD || e.prod == RUN_PROD);

        // The root's own mark, when its production is outline-worthy.
        let root_mark = if spine_is_silent {
            qana_services::node_symbol(tree, cfg).map(|s| mark_at(s, 0, &kinds))
        } else {
            None
        };
        let head = root_mark.is_some() as usize;
        let marks = Arc::make_mut(&mut outline.marks);
        if head != outline.head {
            // Every unit just moved a level in or out, and its shape
            // with it: nothing kept is true any more.
            outline.units.clear();
            marks.clear();
            outline.head = head;
        }
        if let Some(root_mark) = root_mark {
            match marks.first_mut() {
                Some(first) => *first = root_mark,
                None => marks.push(root_mark),
            }
        }

        // Which units did the edits since the last walk leave alone?
        // The ones still at the front and the ones still at the back, by
        // identity; what lies between them is walked.
        let mut units: Vec<(&Arc<GreenNode>, u32)> = Vec::new();
        if spine_is_silent {
            collect_units(tree, &mut units);
        } else {
            units.push((tree, 0));
        }
        let (old_len, new_len) = (outline.units.len(), units.len());
        let mut lo = 0usize;
        while lo < old_len && lo < new_len && Arc::ptr_eq(&outline.units[lo].node, units[lo].0) {
            lo += 1;
        }
        let mut suffix = 0usize;
        while suffix < old_len - lo
            && suffix < new_len - lo
            && Arc::ptr_eq(&outline.units[old_len - 1 - suffix].node, units[new_len - 1 - suffix].0)
        {
            suffix += 1;
        }
        let (old_hi, new_hi) = (old_len - suffix, new_len - suffix);
        let first_mark = |units: &[OutlineUnit], i: usize| {
            if i == 0 {
                head
            } else {
                units[i - 1].mark_end as usize
            }
        };
        let (from, to) = (first_mark(&outline.units, lo), first_mark(&outline.units, old_hi));

        let mut window_marks: Vec<Mark> = Vec::new();
        let mut window_units: Vec<OutlineUnit> = Vec::with_capacity(new_hi - lo);
        for &(node, base) in &units[lo..new_hi] {
            let (shape, shape_pow) = walk_unit(node, base, head, cfg, &kinds, &mut window_marks);
            window_units.push(OutlineUnit {
                node: node.clone(),
                base,
                mark_end: (from + window_marks.len()) as u32,
                shape,
                shape_pow,
            });
        }
        let moved = (from + window_marks.len()) as i64 - to as i64;
        if moved != 0 {
            for unit in &mut outline.units[old_hi..] {
                unit.mark_end = (unit.mark_end as i64 + moved) as u32;
            }
        }
        marks.splice(from..to, window_marks);
        outline.units.splice(lo..old_hi, window_units);
        debug_assert_eq!(outline.units.len(), new_len);

        // A unit that was kept is the same construct; where it starts
        // may have moved, and its marks move with it. The shape of the
        // file is the units' shapes joined, in order.
        let mut shape = 0u64;
        if head == 1 {
            shape = shape_fold(shape, shape_element(&marks[0].name, marks[0].category, 0));
        }
        let mut at = head;
        for (unit, &(node, base)) in outline.units.iter_mut().zip(&units) {
            debug_assert!(Arc::ptr_eq(&unit.node, node));
            let end = unit.mark_end as usize;
            if unit.base != base {
                let by = base.wrapping_sub(unit.base);
                for m in &mut marks[at..end] {
                    m.start = m.start.wrapping_add(by);
                    m.end = m.end.wrapping_add(by);
                    m.name_start = m.name_start.wrapping_add(by);
                    m.name_end = m.name_end.wrapping_add(by);
                }
                unit.base = base;
            }
            shape = shape.wrapping_mul(unit.shape_pow).wrapping_add(unit.shape);
            at = end;
        }
        debug_assert_eq!(at, marks.len());

        // The SHAPE revision bumps only when the (name, category,
        // nesting) sequence changed. Spans are deliberately not part of
        // it: they shift on every keystroke while the shape survives
        // body edits.
        if shape != self.shape_hash {
            self.shape_hash = shape;
            self.shape_rev = self.shape_rev.wrapping_add(1);
        }
        outline.rev = self.marks_rev;
        outline.marks.clone()
    }
}

impl Limner for LiveDoc {
    fn open(&mut self, text: &str) -> Paint {
        self.marks_rev = self.marks_rev.wrapping_add(1);
        // A new document: nothing of the old tree is in the new one.
        // (The shape hash stays, so reopening a document of the same
        // shape does not announce a new one.)
        self.outline = Outline::new();
        self.session = self.lang.session(text);
        self.folds.reset(&self.session.buf);
        self.db = SemDb::new(self.lang.binding.clone());
        self.db.set_types(self.lang.types.clone());
        self.db.set_macro_bodies(&self.lang.macros);
        self.db.set_tree(&self.uri, self.session.tree().expect("total").clone());
        let (painter, paint) =
            Painter::new(&self.session, &self.lang.styles, Some((&mut self.db, &self.uri)));
        self.painter = painter;
        paint
    }

    fn edit(&mut self, edit: &LineEdit) -> PaintDelta {
        self.marks_rev = self.marks_rev.wrapping_add(1);
        // Terminators: interior replacement lines keep the replaced
        // lines' own, positionally (CRLF fidelity in mixed files);
        // extras get the document's prevailing flavor (the protocol
        // stays out of newline politics). THE CANONICAL-FORM CLAUSE:
        // when the edit reaches the document's end, the replacement's
        // last line becomes the new final line and must carry `None` —
        // and a positionally-reused `None` must never land anywhere
        // else. (Blind positional reuse turned a Return that split the
        // final line into two invariant violations at once: the old
        // final line's `None` landed on the now-interior left half,
        // and the new final line was padded with `Lf`.)
        let old_len = self.session.buf.lines.len();
        let prevailing = self
            .session
            .buf
            .lines
            .first()
            .map(|l| l.term)
            .filter(|t| !matches!(t, LineTerm::None))
            .unwrap_or(LineTerm::Lf);
        let (mut start, end) = (edit.start as usize, edit.end as usize);
        let reaches_end = end >= old_len;
        let n = edit.lines.len();
        let mut replacement: Vec<Line> = edit
            .lines
            .iter()
            .enumerate()
            .map(|(i, text)| {
                let term = if reaches_end && i + 1 == n {
                    LineTerm::None
                } else {
                    self.session
                        .buf
                        .lines
                        .get(start + i)
                        .filter(|_| start + i < end)
                        .map(|l| l.term)
                        .filter(|t| !matches!(t, LineTerm::None))
                        .unwrap_or(prevailing)
                };
                Line::new(text.clone(), term)
            })
            .collect();
        // A pure tail deletion would leave an old, terminated line as
        // the new final line; widen the batch by one line so the same
        // edit re-terminates it. (An emptied document keeps canonical
        // form as a single term-less empty line.)
        if reaches_end && replacement.is_empty() {
            if start > 0 {
                start -= 1;
                let kept = self.session.buf.lines[start].text.clone();
                replacement.push(Line::new(kept, LineTerm::None));
            } else {
                replacement.push(Line::new(String::new(), LineTerm::None));
            }
        }
        // `QANA_TRACE_EDIT=1` prints per-stage timings — the scale
        // harness's scalpel for attributing keystroke cost. The clock
        // is read only under the trace (see `StageClock`).
        let trace = std::env::var_os("QANA_TRACE_EDIT").is_some();
        let t0 = StageClock::start(trace);
        let outcome = self
            .session
            .edit(self.lang.sg, self.lang.tables, &[qana_engine::LineEdit {
                start,
                end,
                replacement,
            }])
            .expect("total under errors");
        let t_parse = t0.elapsed();
        self.folds.note_edit(
            &self.session.buf,
            &outcome.damage.regions,
            self.marks_rev.wrapping_sub(1),
            self.marks_rev,
        );
        let t1 = StageClock::start(trace);
        self.db.set_tree(&self.uri, self.session.tree().expect("total").clone());
        let t_sem = t1.elapsed();
        let t2 = StageClock::start(trace);
        let delta = self.painter.update(
            &self.session,
            &outcome.damage,
            &self.lang.styles,
            Some((&mut self.db, &self.uri)),
        );
        if trace {
            eprintln!(
                "[edit-trace] parse {t_parse:?} | sem set_tree {t_sem:?} | paint+resolve {:?}",
                t2.elapsed()
            );
        }
        delta
    }

    fn facts(&mut self, offset: u32) -> Option<FactCard> {
        let text = self.session.buf.reproduce();
        facts_at(&mut self.db, &self.uri, &text, offset)
    }

    fn hints(&mut self) -> Vec<Hint> {
        let text = self.session.buf.reproduce();
        type_hints(&mut self.db, &self.uri, &text)
    }

    fn legend(&self) -> Vec<String> {
        self.lang.styles.legend.iter().map(|s| s.to_string()).collect()
    }

    fn text(&mut self) -> String {
        self.session.buf.reproduce()
    }

    fn marks(&mut self) -> Vec<Mark> {
        // Trait-shaped copy of the cached vector; hot callers use
        // marks_shared and pay nothing.
        (*self.marks_now()).clone()
    }

    fn marks_shared(&mut self) -> Arc<Vec<Mark>> {
        self.marks_now()
    }

    fn marks_rev(&mut self) -> u64 {
        self.marks_rev
    }

    fn marks_shape_rev(&mut self) -> u64 {
        // The hash is a byproduct of the (cached) walk: make sure the
        // current revision has been walked, then report.
        let _ = self.marks_now();
        self.shape_rev
    }

    fn enclosing(&mut self, offset: u32) -> Vec<Mark> {
        // Containment filter over the CACHED outline walk. Document
        // order is pre-order, so the surviving chain is outermost-first
        // for free. The giant-document day the old comment predicted
        // arrived (27k marks, queried per cursor tick): the walk now
        // computes once per revision and this is a scan over the cache.
        self.marks_now()
            .iter()
            .filter(|m| m.start <= offset && offset < m.end)
            .cloned()
            .collect()
    }

    fn mark_legend(&self) -> Vec<String> {
        mark_kinds(&self.lang.outline).iter().map(|k| k.to_string()).collect()
    }

    fn folds_shared(&mut self) -> Arc<Vec<Mark>> {
        self.folds.folds(self.marks_rev)
    }

    fn folds_rev(&mut self) -> u64 {
        self.marks_rev
    }

    fn fold_legend(&self) -> Vec<String> {
        self.folds.legend().to_vec()
    }
}

/// The style class of a token of `lang`, if it has one.
fn style_class(lang: &EmbeddedLang, id: qana_grammar::model::TokenId) -> Option<&'static str> {
    lang.styles.class_of(id).map(|class| lang.styles.legend[class as usize])
}

/// Mark categories = the outline config's kinds, deduped in first-use
/// order. `marks` and `mark_legend` both derive from this so category
/// indices always agree.
fn mark_kinds(cfg: &qana_services::OutlineConfig) -> Vec<&'static str> {
    let mut kinds: Vec<&'static str> = Vec::new();
    for e in &cfg.entries {
        if !kinds.contains(&e.kind) {
            kinds.push(e.kind);
        }
    }
    kinds
}

#[cfg(test)]
mod outline_tests {
    //! The kept outline against the walk it replaces: after every step
    //! of a script, `marks_now` must be exactly what
    //! `qana_services::outline` says of the same tree, and the shape
    //! revision must move exactly when the shape does.

    use super::*;

    const C_QANA: &str = include_str!("../../../examples/c/c.qana");

    /// A grammar whose START production is outline-worthy: the root
    /// node is itself a mark, and every other mark is inside it.
    const MODULE_QANA: &str = r#"
language Mod

token WS = /\s+/ @trivia
token IDENT = /[\a_][\w_]*/ @specialize @style(variable)
token LBRACE = "{" @style(bracket)
token RBRACE = "}" @style(bracket)
token SEMI = ";" @style(punctuation)

keywords IDENT = module fn

pair LBRACE RBRACE

start file
rule file = File: "module" name:IDENT ";" items @outline(name, module)
rule items = item*
rule item = Fn: "fn" name:IDENT "{" calls "}" @outline(name, function)
rule calls = call*
rule call = Call: name:IDENT ";"
"#;

    /// What a full walk of the document's tree says its marks are.
    fn walked(doc: &LiveDoc) -> Vec<Mark> {
        let kinds = mark_kinds(&doc.lang.outline);
        qana_services::outline(doc.session.tree().expect("total"), &doc.lang.outline)
            .into_iter()
            .map(|s| mark_at(s, 0, &kinds))
            .collect()
    }

    /// The shape of a mark list, spelled out: what the shape hash stands
    /// for.
    fn shape_of(marks: &[Mark]) -> Vec<(String, u8, usize)> {
        let mut open_ends: Vec<u32> = Vec::new();
        marks
            .iter()
            .map(|m| {
                while open_ends.last().is_some_and(|&e| m.start >= e) {
                    open_ends.pop();
                }
                let depth = open_ends.len();
                open_ends.push(m.end);
                (m.name.clone(), m.category, depth)
            })
            .collect()
    }

    fn rng(mut seed: u64) -> impl FnMut(usize) -> usize {
        move |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n.max(1) as u64) as usize
        }
    }

    /// A document as a list of top-level items, each one or more whole
    /// lines, with whatever lines stand ahead of them (a header, blank
    /// lines). Every edit the script makes replaces whole items or the
    /// header with other whole items, so every step leaves a file that
    /// parses clean: these tests are about the outline, and a script
    /// that breaks the syntax would be testing the parser's recovery.
    struct Items {
        header: Vec<String>,
        items: Vec<Vec<String>>,
    }

    impl Items {
        fn text(&self) -> String {
            let mut out = String::new();
            for line in self.header.iter().chain(self.items.iter().flatten()) {
                out.push_str(line);
                out.push('\n');
            }
            out
        }
        fn first_line_of(&self, item: usize) -> u32 {
            (self.header.len() + self.items[..item].iter().map(Vec::len).sum::<usize>()) as u32
        }
    }

    fn lines(text: &str) -> Vec<String> {
        text.split('\n').map(String::from).collect()
    }

    /// Run a random script over `doc_items`: items inserted, replaced
    /// and removed, and the header replaced by another from `headers`.
    /// The marks are asked for after most steps (and after a run of
    /// several edits on the others), and every answer is held to the
    /// full walk. Returns the most marks the document had.
    fn hold_to_the_walk(
        grammar: &str,
        mut model: Items,
        headers: &[&[&str]],
        pool: &[&str],
        steps: usize,
        seed: u64,
    ) -> usize {
        let lang = Arc::new(EmbeddedLang::from_qana_source(grammar).expect("the grammar certifies"));
        let mut doc = LiveDoc::open(lang, "outline.test", &model.text());
        let mut rand = rng(seed);

        let mut shape = shape_of(&doc.marks_now());
        let mut shape_rev = doc.shape_rev;
        assert_eq!(*doc.marks_now(), walked(&doc), "the opened document");
        let mut most_marks = doc.marks_now().len();
        let (mut shape_changes, mut shape_kept) = (0usize, 0usize);

        for step in 0..steps {
            let n = model.items.len();
            let item = lines(pool[rand(pool.len())]);
            let edit = match rand(8) {
                // The header: everything after it moves, nothing after
                // it changes.
                0 if !headers.is_empty() => {
                    let header: Vec<String> =
                        headers[rand(headers.len())].iter().map(|l| l.to_string()).collect();
                    let edit =
                        LineEdit { start: 0, end: model.header.len() as u32, lines: header.clone() };
                    model.header = header;
                    edit
                }
                1 | 2 if n > 3 => {
                    let at = rand(n);
                    let start = model.first_line_of(at);
                    let removed = model.items.remove(at);
                    LineEdit { start, end: start + removed.len() as u32, lines: vec![] }
                }
                3 | 4 => {
                    let at = rand(n + 1);
                    let start = if at == n {
                        (model.header.len() + model.items.iter().map(Vec::len).sum::<usize>()) as u32
                    } else {
                        model.first_line_of(at)
                    };
                    model.items.insert(at, item.clone());
                    LineEdit { start, end: start, lines: item }
                }
                _ => {
                    let at = rand(n);
                    let start = model.first_line_of(at);
                    let old = std::mem::replace(&mut model.items[at], item.clone());
                    LineEdit { start, end: start + old.len() as u32, lines: item }
                }
            };
            doc.edit(&edit);
            assert_eq!(doc.session.buf.reproduce(), model.text(), "step {step}: the document");
            assert!(doc.session.last_repairs.is_empty(), "step {step} ({edit:?}) parses clean");
            if rand(5) == 0 {
                // Not asked after this edit: the next answer has to
                // account for more than one.
                continue;
            }

            let marks = doc.marks_now();
            assert_eq!(*marks, walked(&doc), "step {step} ({edit:?})");
            assert!(Arc::ptr_eq(&marks, &doc.marks_now()), "asked again, the same answer");
            most_marks = most_marks.max(marks.len());

            let now = shape_of(&marks);
            let rev = doc.shape_rev;
            assert_eq!(
                now != shape,
                rev != shape_rev,
                "step {step} ({edit:?}): the shape revision moves when the shape does, and only then"
            );
            if now != shape {
                shape_changes += 1;
            } else {
                shape_kept += 1;
            }
            shape = now;
            shape_rev = rev;
        }
        assert!(shape_changes > 0, "the script changed the shape at least once");
        assert!(shape_kept > 0, "and left it alone at least once");
        most_marks
    }

    /// C: functions, structs, enums, globals and macros, as items of one
    /// line and of several.
    #[test]
    fn the_kept_outline_is_the_walked_outline_in_c() {
        let pool = [
            "static int helper_a(void) { return 1; }",
            "static int helper_a(void) { return 1 + 1; }",
            "static int helper_b(int a) {\n    return a + helper_a();\n}",
            "static int helper_b(int a) {\n    return a;\n}",
            "struct box {\n    int w;\n    int h;\n};",
            "enum color { RED, GREEN };",
            "int g1, g2;",
            "#define TWICE(x) ((x) * 2)",
            "",
            "/* a comment */",
        ];
        let model = Items {
            header: vec![],
            items: (0..40).map(|i| lines(pool[i % pool.len()])).collect(),
        };
        let most = hold_to_the_walk(C_QANA, model, &[], &pool, 500, 0x0071_1E5E);
        assert!(most > 20, "the document has an outline to keep, {most} marks");
    }

    /// A grammar whose root is a mark: the head of the list is the
    /// root's own mark, every unit is one level deep, and a header of
    /// another length moves every unit without changing any.
    #[test]
    fn the_kept_outline_is_the_walked_outline_under_a_root_mark() {
        let pool = ["fn alpha { beta; }", "fn beta { alpha; gamma; }", "fn gamma { }", "fn alpha { }"];
        let headers: [&[&str]; 4] =
            [&["module main;"], &["", "module renamed;"], &["module m;"], &["", "", "module main;"]];
        let model = Items {
            header: vec!["module main;".into()],
            items: (0..12).map(|i| lines(pool[i % pool.len()])).collect(),
        };
        let text = model.text();
        let most = hold_to_the_walk(MODULE_QANA, model, &headers, &pool, 500, 0x00D0_C5ED);
        assert!(most >= 4, "the document has an outline to keep, {most} marks");

        // And the root's mark really is there, enclosing the rest.
        let lang = Arc::new(EmbeddedLang::from_qana_source(MODULE_QANA).unwrap());
        let mut doc = LiveDoc::open(lang, "m", &text);
        let marks = doc.marks_now();
        assert_eq!(marks[0].name, "main");
        assert_eq!(shape_of(&marks)[1], ("alpha".to_string(), marks[1].category, 1));
        assert_eq!(doc.outline.head, 1);
    }

    /// The root's mark can come and go: a document that does not parse
    /// as the root production has none. Losing it and getting it back
    /// puts every unit a level out and a level in again.
    #[test]
    fn the_root_mark_goes_and_comes_back() {
        let lang = Arc::new(EmbeddedLang::from_qana_source(MODULE_QANA).unwrap());
        let text = "module main;\nfn alpha { beta; }\nfn beta { alpha; }\n";
        let mut doc = LiveDoc::open(lang, "m", text);
        assert_eq!(doc.marks_now().len(), 3);
        assert_eq!(doc.outline.head, 1);
        let with_root = doc.shape_rev;

        // The header goes: whatever the parser makes of the rest, the
        // kept outline is what a walk of that tree says.
        doc.edit(&LineEdit { start: 0, end: 1, lines: vec![] });
        assert_eq!(*doc.marks_now(), walked(&doc));
        let head_without = doc.outline.head;

        // And comes back.
        doc.edit(&LineEdit { start: 0, end: 0, lines: vec!["module main;".into()] });
        assert_eq!(*doc.marks_now(), walked(&doc));
        assert_eq!(doc.outline.head, 1);
        assert_eq!(doc.marks_now()[0].name, "main");
        if head_without == 0 {
            assert_ne!(doc.shape_rev, with_root, "the shape changed, and changed back");
        }
    }

    /// The point of keeping it: typing in one function walks that
    /// function again, and no other.
    #[test]
    fn a_keystroke_walks_one_unit() {
        let lang = Arc::new(EmbeddedLang::from_qana_source(C_QANA).unwrap());
        let text: String = (0..300)
            .map(|i| format!("static int fn_{i}(int a) {{\n    return a + {i};\n}}\n\n"))
            .collect();
        let mut doc = LiveDoc::open(lang, "units.c", &text);
        let before = doc.marks_now();
        assert_eq!(before.len(), 300);
        let kept: Vec<*const GreenNode> =
            doc.outline.units.iter().map(|u| Arc::as_ptr(&u.node)).collect();
        drop(before);

        // Line 4·150 + 1 is the body of fn_150.
        doc.edit(&LineEdit { start: 601, end: 602, lines: vec!["    return a + 150 + 1;".into()] });
        let after = doc.marks_now();
        assert_eq!(*after, walked(&doc));
        let now: Vec<*const GreenNode> =
            doc.outline.units.iter().map(|u| Arc::as_ptr(&u.node)).collect();
        assert_eq!(now.len(), kept.len(), "the same number of units");
        let changed: Vec<usize> = (0..now.len()).filter(|&i| now[i] != kept[i]).collect();
        assert_eq!(changed.len(), 1, "one unit is a new subtree: {changed:?}");
        assert_eq!(after[changed[0]].name, "fn_150");
    }
}
