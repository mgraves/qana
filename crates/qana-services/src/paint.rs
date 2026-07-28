//! THE NATIVE EDITOR PROTOCOL: paint + facts.
//!
//! LSP and tree-sitter remain supported exports, but they are
//! compatibility tiers — pull-based, serialized, and lossy against
//! what this engine actually knows. This module is the interface an
//! editor built FOR the engine consumes, designed around the two
//! facts the architecture guarantees and generic servers cannot:
//!
//!  * Damage is LINE-BOUNDED and known synchronously (L1/L2): an edit
//!    tells you exactly which lines' tokens changed, before any parse.
//!  * Every semantic fact is already MEMOIZED per item (binding,
//!    types, modules, provenance): hover is a lookup, not a request.
//!
//! So the protocol is LINE-KEYED and TWO-WAVE:
//!
//!  * WAVE 0 (lexical): each line is a run of 4-byte cells
//!    `{len, style, mods=0}` — recomputed for DAMAGED LINES ONLY,
//!    synchronously with the edit. The first frame after a keystroke
//!    paints from the same declared `@style` classes the final frame
//!    uses — there is no TextMate approximation to disagree with, so
//!    there is nothing to flicker.
//!  * WAVE 1 (semantic): the SAME runs gain modifier bits — def/ref,
//!    resolved/unresolved, public, foreign, typed — derived from the
//!    memoized binding and type tiers. Wave 1 never changes a style,
//!    only adds bits: refinement is monotone by construction (gated).
//!
//! A delta names the lines that changed and nothing else. Runs are
//! line-relative, so lines that merely SHIFTED re-emit nothing. The
//! wire form is the in-memory form, little-endian — decoding is a
//! bounds check, not a parse.

use crate::Styles;
use qana_engine::{DamageReport, IncSession, LexedBuffer};
use qana_grammar::CompiledLexer;
use qana_sem::{RefKind, SemDb, Target};
use std::sync::Arc;

// The protocol TYPES live in the engine-neutral `linework` crate — this
// module is qana's IMPLEMENTATION of them (the Painter, the facts
// assembly). Editors depend on `linework`; qana depends on `linework`;
// neither depends on the other.
pub use linework::{
    decode_lines, encode_lines, FactCard, Hint, Paint, PaintDelta, Run, MOD_DEF, MOD_FOREIGN,
    MOD_PUBLIC, MOD_REF, MOD_TYPED, MOD_UNRESOLVED, STYLE_NONE,
};

// ---------------------------------------------------------------------------
// Wave 0: lexical runs, straight off the line lexer
// ---------------------------------------------------------------------------

fn wave0_line(toks: &[qana_grammar::lexer::Token], term_len: u32, styles: &Styles) -> Vec<Run> {
    let mut runs: Vec<Run> = Vec::with_capacity(toks.len() + 1);
    let mut push = |len: u32, style: u8| {
        let mut left = len;
        while left > 0 {
            let chunk = left.min(u16::MAX as u32) as u16;
            match runs.last_mut() {
                // Merge equal-style neighbors (trivia between two
                // plain tokens is one run, not three).
                Some(r) if r.style == style && (r.len as u32 + chunk as u32) <= u16::MAX as u32 => {
                    r.len += chunk
                }
                _ => runs.push(Run { len: chunk, style, mods: 0 }),
            }
            left -= chunk as u32;
        }
    };
    for t in toks {
        let style = styles.class_of(t.id).map(|c| c as u8).unwrap_or(STYLE_NONE);
        push(t.len, style);
    }
    if term_len > 0 {
        push(term_len, STYLE_NONE);
    }
    runs
}

fn line_widths(buf: &LexedBuffer<'_, CompiledLexer>) -> Vec<u32> {
    buf.lines
        .iter()
        .map(|l| (l.text.len() + l.term.as_str().len()) as u32)
        .collect()
}

// ---------------------------------------------------------------------------
// Wave 1: the semantic overlay — bits from the memoized tiers
// ---------------------------------------------------------------------------

/// Split `runs` so `[a, b)` (line-relative) is covered exactly, and OR
/// `bits` into the covered runs. Styles never change: wave 1 is
/// additive by construction.
fn mark(runs: &mut Vec<Run>, a: u32, b: u32, bits: u8) {
    if b <= a {
        return;
    }
    let mut out = Vec::with_capacity(runs.len() + 2);
    let mut at = 0u32;
    for r in runs.iter() {
        let (s, e) = (at, at + r.len as u32);
        let lo = s.max(a);
        let hi = e.min(b);
        if lo >= hi {
            out.push(*r);
        } else {
            let seg = |len: u32, mods: u8, out: &mut Vec<Run>| {
                if len > 0 {
                    out.push(Run { len: len as u16, style: r.style, mods });
                }
            };
            seg(lo - s, r.mods, &mut out);
            seg(hi - lo, r.mods | bits, &mut out);
            seg(e - hi, r.mods, &mut out);
        }
        at = e;
    }
    *runs = out;
}

/// Everything wave 1 wants to say, as (absolute span, bits) marks —
/// one pass over tables the tiers already keep hot.
fn overlay_marks(db: &mut SemDb, uri: &str) -> Vec<((u32, u32), u8)> {
    let syms = db.symbols(uri);
    let res = db.resolve(uri);
    let mut marks: Vec<((u32, u32), u8)> = Vec::new();
    for d in syms.defs.iter() {
        let mut bits = MOD_DEF;
        if d.exported {
            bits |= MOD_PUBLIC;
        }
        marks.push((d.span, bits));
    }
    for (ri, r) in syms.refs.iter().enumerate() {
        let bits = match res.get(ri) {
            Some(Target::Local { .. }) => MOD_REF,
            Some(Target::Foreign { .. }) => MOD_REF | MOD_FOREIGN,
            // Import refs that share a def's token (`use x;`) stay
            // quiet when unresolved-as-import but present-as-def.
            _ if r.kind == RefKind::Qualified || r.kind == RefKind::Var
                || r.kind == RefKind::Call || r.kind == RefKind::Import =>
            {
                MOD_UNRESOLVED
            }
            _ => 0,
        };
        if bits != 0 {
            marks.push((r.span, bits));
        }
    }
    // The type tier, when declared: every def that carries a type.
    let report = db.types(uri);
    for &(span, _) in &report.def_types {
        marks.push((span, MOD_TYPED));
    }
    marks
}

fn apply_overlay(lines: &mut [Vec<Run>], widths: &[u32], marks: &[((u32, u32), u8)]) {
    // Line starts (absolute), once.
    let mut starts = Vec::with_capacity(widths.len() + 1);
    let mut acc = 0u32;
    for w in widths {
        starts.push(acc);
        acc += w;
    }
    starts.push(acc);
    for &((a, b), bits) in marks {
        // Tokens never cross lines (L1), so one line owns the span.
        let li = match starts.binary_search(&a) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        if li < lines.len() {
            mark(&mut lines[li], a - starts[li], b - starts[li], bits);
        }
    }
}

// ---------------------------------------------------------------------------
// The stateful painter: full once, then damage-shaped deltas
// ---------------------------------------------------------------------------

/// Owns the previous frame so an update can say exactly what changed.
pub struct Painter {
    /// Wave-0 runs per line (the lexical base, maintained by splice).
    base: Vec<Vec<Run>>,
    /// Emitted (overlaid) runs per line — what the editor is showing.
    shown: Vec<Vec<Run>>,
    rev: u64,
    /// Per-line widths (chars incl. terminator), maintained by splice —
    /// the identity-cached overlay path's coordinate backbone.
    widths: Vec<u32>,
    /// The identity-cached overlay: per item in document order, its
    /// paint key and derived marks (item-relative spans). An item whose
    /// `(frag_uid, res_uid)` pair reappears re-uses its marks by `Arc`
    /// clone; only misses re-derive, and only their lines repaint.
    /// `None` until the first typeless-grammar update populates it.
    cache: Option<Vec<(qana_sem::PaintKey, Arc<Vec<((u32, u32), u8)>>)>>,
}

/// One item's marks from its paint facts — the bits mapping shared by
/// the identity-cached path (and gated to equal the composed-view path
/// by the paint differential test).
fn item_marks(ip: &qana_sem::ItemPaint) -> Vec<((u32, u32), u8)> {
    let mut out = Vec::with_capacity(ip.defs.len() + ip.refs.len());
    for &(span, exported) in &ip.defs {
        let mut bits = MOD_DEF;
        if exported {
            bits |= MOD_PUBLIC;
        }
        out.push((span, bits));
    }
    for &(span, rp) in &ip.refs {
        let bits = match rp {
            qana_sem::RefPaint::Ref => MOD_REF,
            qana_sem::RefPaint::RefForeign => MOD_REF | MOD_FOREIGN,
            qana_sem::RefPaint::Unresolved => MOD_UNRESOLVED,
            qana_sem::RefPaint::Quiet => continue,
        };
        out.push((span, bits));
    }
    out
}

impl Painter {
    /// Paint everything once. `db` brings wave 1; `None` is the pure
    /// lexical painter (the first frame of a huge file, or a grammar
    /// with no binding tier).
    pub fn new(
        session: &IncSession<'_>,
        styles: &Styles,
        db: Option<(&mut SemDb, &str)>,
    ) -> (Painter, Paint) {
        let widths = line_widths(&session.buf);
        let base: Vec<Vec<Run>> = session
            .buf
            .lexed
            .iter()
            .zip(session.buf.lines.iter())
            .map(|(lt, l)| wave0_line(&lt.tokens, l.term.as_str().len() as u32, styles))
            .collect();
        let mut shown = base.clone();
        let mut cache = None;
        if let Some((db, uri)) = db {
            let marks = overlay_marks(db, uri);
            apply_overlay(&mut shown, &widths, &marks);
            if !db.has_types() {
                // Prefill the identity cache so the FIRST keystroke is
                // already the steady state — the all-miss fill belongs
                // to open, where the whole-document pass already lives.
                let keys = db.paint_sync(uri);
                cache = Some(
                    keys.iter()
                        .enumerate()
                        .map(|(i, k)| {
                            (*k, Arc::new(item_marks(&db.item_paint(uri, i as u32))))
                        })
                        .collect(),
                );
            }
        }
        let p = Painter { base, shown: shown.clone(), rev: 0, widths, cache };
        let paint = Paint { rev: 0, lines: shown };
        (p, paint)
    }

    /// Apply one edit's damage: wave 0 recomputes ONLY the damaged
    /// window; wave 1 (when `db` is given) re-derives the overlay and
    /// re-emits only lines whose runs actually changed. The returned
    /// delta is the whole difference between the frames.
    pub fn update(
        &mut self,
        session: &IncSession<'_>,
        damage: &DamageReport,
        styles: &Styles,
        db: Option<(&mut SemDb, &str)>,
    ) -> PaintDelta {
        self.rev += 1;
        let mut delta = PaintDelta { rev: self.rev, splice: None, repaints: Vec::new() };

        // ---- wave 0: splice the damaged window ----
        let window = if damage.regions.is_empty() {
            None
        } else {
            let old_lo = damage.regions.iter().map(|r| r.old_lines.0).min().unwrap();
            let old_hi = damage.regions.iter().map(|r| r.old_lines.1).max().unwrap();
            let new_lo = damage.regions.iter().map(|r| r.new_lines.0).min().unwrap();
            let new_hi = damage.regions.iter().map(|r| r.new_lines.1).max().unwrap();
            debug_assert_eq!(old_lo, new_lo);
            let fresh: Vec<Vec<Run>> = (new_lo..new_hi)
                .map(|li| {
                    wave0_line(
                        &session.buf.lexed[li].tokens,
                        session.buf.lines[li].term.as_str().len() as u32,
                        styles,
                    )
                })
                .collect();
            self.base.splice(old_lo..old_hi, fresh.clone());
            Some((old_lo, old_hi, new_lo, new_hi))
        };

        match db {
            None => {
                // Lexical-only consumer: shown IS base; the delta is
                // the splice alone. (Semantic bits, if any were shown
                // before, are the caller's concern — mixing modes on
                // one painter is not supported.)
                if let Some((old_lo, old_hi, new_lo, new_hi)) = window {
                    let repl: Vec<Vec<Run>> = self.base[new_lo..new_hi].to_vec();
                    self.shown.splice(old_lo..old_hi, repl.clone());
                    delta.splice = Some(((old_lo as u32, old_hi as u32), repl));
                }
            }
            Some((db, uri)) if !db.has_types() => {
                // The IDENTITY-CACHED overlay: typeless grammars (no
                // per-item identity for the type report yet) re-derive
                // marks only for items whose (fragment, resolution)
                // identity changed, and repaint only their lines plus
                // the damaged window. Keystroke cost is O(window +
                // changed items), independent of document size.
                self.overlay_via_items(session, window, db, uri, &mut delta);
            }
            Some((db, uri)) => {
                let trace = std::env::var_os("QANA_TRACE_EDIT").is_some();
                // Fresh overlay over the maintained base…
                let t0 = std::time::Instant::now();
                let widths = line_widths(&session.buf);
                let t_widths = t0.elapsed();
                let t1 = std::time::Instant::now();
                let mut next = self.base.clone();
                let t_clone = t1.elapsed();
                let t2 = std::time::Instant::now();
                let marks = overlay_marks(db, uri);
                let t_marks = t2.elapsed();
                let t3 = std::time::Instant::now();
                apply_overlay(&mut next, &widths, &marks);
                if trace {
                    eprintln!(
                        "[paint-trace] widths {t_widths:?} | clone {t_clone:?} | overlay_marks {t_marks:?} (n={}) | apply {:?}",
                        marks.len(),
                        t3.elapsed()
                    );
                }
                // …then emit exactly what differs from the last frame:
                // the window as a splice, everything else by diff
                // (alignment shifts by the window's line-count delta).
                let (old_lo, old_hi, new_lo, new_hi) = window.unwrap_or((0, 0, 0, 0));
                if window.is_some() {
                    delta.splice =
                        Some(((old_lo as u32, old_hi as u32), next[new_lo..new_hi].to_vec()));
                }
                for li in 0..next.len() {
                    if window.is_some() && li >= new_lo && li < new_hi {
                        continue;
                    }
                    let old_li = if window.is_none() || li < new_lo {
                        li
                    } else {
                        li - new_hi + old_hi
                    };
                    if self.shown.get(old_li) != Some(&next[li]) {
                        delta.repaints.push((li as u32, next[li].clone()));
                    }
                }
                self.shown = next;
            }
        }
        delta
    }

    /// The frame as currently shown (what a fresh viewer would paint).
    pub fn frame(&self) -> Paint {
        Paint { rev: self.rev, lines: self.shown.clone() }
    }

    /// The identity-cached overlay path (typeless grammars). See the
    /// dispatch site in [`update`](Self::update) for the contract.
    #[allow(clippy::too_many_arguments)]
    fn overlay_via_items(
        &mut self,
        session: &IncSession<'_>,
        window: Option<(usize, usize, usize, usize)>,
        db: &mut SemDb,
        uri: &str,
        delta: &mut PaintDelta,
    ) {
        let trace = std::env::var_os("QANA_TRACE_EDIT").is_some();
        let t0 = std::time::Instant::now();

        // Widths follow the window by splice; a missing or drifted
        // cache rebuilds them whole (first update, or a resync).
        match window {
            Some((old_lo, old_hi, new_lo, new_hi))
                if self.widths.len() == self.base.len() + (old_hi - old_lo) - (new_hi - new_lo) =>
            {
                let fresh: Vec<u32> = (new_lo..new_hi)
                    .map(|li| {
                        let l = &session.buf.lines[li];
                        (l.text.len() + l.term.as_str().len()) as u32
                    })
                    .collect();
                self.widths.splice(old_lo..old_hi, fresh);
            }
            _ => self.widths = line_widths(&session.buf),
        }
        debug_assert_eq!(self.widths.len(), self.base.len());

        // Line starts, absolute — O(lines) of u32 adds.
        let mut starts = Vec::with_capacity(self.widths.len() + 1);
        let mut acc = 0u32;
        for w in &self.widths {
            starts.push(acc);
            acc += w;
        }
        starts.push(acc);
        let total = acc;

        // Sync the item view; misses re-derive their marks.
        let keys = db.paint_sync(uri);
        let old: std::collections::HashMap<(u64, u64), Arc<Vec<((u32, u32), u8)>>> = self
            .cache
            .take()
            .map(|items| {
                items
                    .into_iter()
                    .map(|(k, m)| ((k.frag_uid, k.res_uid), m))
                    .collect()
            })
            .unwrap_or_default();
        let mut misses: Vec<usize> = Vec::new();
        let mut items: Vec<(qana_sem::PaintKey, Arc<Vec<((u32, u32), u8)>>)> =
            Vec::with_capacity(keys.len());
        for (i, k) in keys.iter().enumerate() {
            match old.get(&(k.frag_uid, k.res_uid)) {
                Some(m) => items.push((*k, m.clone())),
                None => {
                    let ip = db.item_paint(uri, i as u32);
                    items.push((*k, Arc::new(item_marks(&ip))));
                    misses.push(i);
                }
            }
        }
        let t_sync = t0.elapsed();

        // Touched lines: the damaged window plus every miss item's
        // line range (an item's span runs to the next item's start —
        // the trivia gap carries no marks, so over-coverage is safe).
        let t1 = std::time::Instant::now();
        let line_of = |off: u32| -> usize {
            match starts.binary_search(&off.min(total)) {
                Ok(i) => i.min(self.widths.len().saturating_sub(1)),
                Err(i) => i - 1,
            }
        };
        let mut touched: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();
        let (old_lo, old_hi, new_lo, new_hi) = window.unwrap_or((0, 0, 0, 0));
        if window.is_some() {
            touched.extend(new_lo..new_hi);
        }
        for &i in &misses {
            let a = items[i].0.start;
            let b = if i + 1 < items.len() { items[i + 1].0.start } else { total };
            if b > a && !self.widths.is_empty() {
                touched.extend(line_of(a)..=line_of(b.saturating_sub(1)));
            }
        }

        // Rebuild each touched line from its wave-0 base plus the marks
        // of every item intersecting it.
        let rebuild = |li: usize,
                       items: &[(qana_sem::PaintKey, Arc<Vec<((u32, u32), u8)>>)]|
         -> Vec<Run> {
            let mut fresh = self.base[li].clone();
            let (ls, le) = (starts[li], starts[li] + self.widths[li]);
            let mut ii = items.partition_point(|(k, _)| k.start <= ls);
            ii = ii.saturating_sub(1);
            while ii < items.len() {
                let a0 = items[ii].0.start;
                if a0 >= le {
                    break;
                }
                let b0 = if ii + 1 < items.len() { items[ii + 1].0.start } else { total };
                if b0 > ls {
                    for &((ra, rb), bits) in items[ii].1.iter() {
                        let (a, b) = (a0 + ra, a0 + rb);
                        let (a, b) = (a.max(ls), b.min(le));
                        if b > a {
                            mark(&mut fresh, a - ls, b - ls, bits);
                        }
                    }
                }
                ii += 1;
            }
            fresh
        };

        if window.is_some() {
            let repl: Vec<Vec<Run>> = (new_lo..new_hi).map(|li| rebuild(li, &items)).collect();
            self.shown.splice(old_lo..old_hi, repl.clone());
            delta.splice = Some(((old_lo as u32, old_hi as u32), repl));
        }
        for &li in &touched {
            if window.is_some() && li >= new_lo && li < new_hi {
                continue;
            }
            let fresh = rebuild(li, &items);
            if self.shown.get(li) != Some(&fresh) {
                delta.repaints.push((li as u32, fresh.clone()));
                self.shown[li] = fresh;
            }
        }
        debug_assert_eq!(self.shown.len(), self.base.len());

        if trace {
            eprintln!(
                "[paint-trace] items {} | misses {} | touched {} | sync {t_sync:?} | rebuild {:?}",
                items.len(),
                misses.len(),
                touched.len(),
                t1.elapsed()
            );
        }
        self.cache = Some(items);
    }
}

// ---------------------------------------------------------------------------
// Facts: the hover card, assembled from queries that are already warm
// ---------------------------------------------------------------------------

pub fn facts_at(db: &mut SemDb, uri: &str, text: &str, offset: u32) -> Option<FactCard> {
    let syms = db.symbols(uri);
    let within = |s: (u32, u32)| s.0 <= offset && offset < s.1;
    let slice = |s: (u32, u32)| text.get(s.0 as usize..s.1 as usize).unwrap_or("").to_string();

    let mut card = FactCard::default();
    if let Some(d) = syms.defs.iter().find(|d| within(d.span)) {
        card.span = d.span;
        card.name = d.name.clone();
        card.is_def = true;
        card.exported = d.exported;
        card.def_site = Some((uri.to_string(), d.span));
        card.ns = (!d.ns.is_empty()).then(|| d.ns.clone());
    } else if let Some(ri) = syms.refs.iter().position(|r| within(r.span)) {
        let r = &syms.refs[ri];
        card.span = r.span;
        card.name = r.name.clone();
        card.ns = (!r.ns.is_empty()).then(|| r.ns.clone());
        card.def_site = db.definition(uri, offset);
        if card.def_site.is_none() {
            let not_exp = db.not_exported(uri);
            card.problem = Some(if not_exp.iter().any(|(_, s)| *s == r.span) {
                format!("`{}` exists but is not exported", r.name)
            } else {
                format!("cannot find `{}`", r.name)
            });
        }
    } else {
        return None;
    }
    if card.name.is_empty() {
        card.name = slice(card.span);
    }

    // The type, when the tier is declared: the fact at this very
    // span, else the fact at the definition it names.
    let report = db.types(uri);
    let ty_at = |spans: &[((u32, u32), qana_sem::TypeId)], s: (u32, u32)| {
        spans.iter().find(|(sp, _)| *sp == s).map(|(_, t)| *t)
    };
    let tid = ty_at(&report.def_types, card.span)
        .or_else(|| ty_at(&report.types, card.span))
        .or_else(|| match &card.def_site {
            Some((u, s)) if u == uri => ty_at(&report.def_types, *s),
            _ => None,
        });
    card.ty = tid.and_then(|t| report.atoms.get(t as usize).cloned());
    Some(card)
}

/// Inline TYPE HINTS: one `(line, col, text)` per typed definition —
/// the decoration plane, derived from the same report the diagnostics
/// use. Empty when the grammar declares no type tier.
pub fn type_hints(db: &mut SemDb, uri: &str, text: &str) -> Vec<Hint> {
    let report = db.types(uri);
    if report.def_types.is_empty() {
        return Vec::new();
    }
    // Line starts from the text itself (hints are a per-frame pull).
    let mut starts = vec![0u32];
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i as u32 + 1);
        }
    }
    let mut out = Vec::new();
    for &((_, end), tid) in &report.def_types {
        let li = match starts.binary_search(&end) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        if let Some(name) = report.atoms.get(tid as usize) {
            out.push(Hint { line: li as u32, col: end - starts[li], text: format!(": {name}") });
        }
    }
    out.sort_by_key(|h| (h.line, h.col));
    out
}
