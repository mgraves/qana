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
use rantlr_engine::{DamageReport, IncSession, LexedBuffer};
use rantlr_grammar::CompiledLexer;
use rantlr_sem::{RefKind, SemDb, Target};

/// This run's bytes are a definition's name.
pub const MOD_DEF: u8 = 1 << 0;
/// A reference that RESOLVES (navigation will land somewhere).
pub const MOD_REF: u8 = 1 << 1;
/// A reference that does not resolve — including "exists but is not
/// exported" and broken qualified paths (the squiggle substrate).
pub const MOD_UNRESOLVED: u8 = 1 << 2;
/// The definition is `@export`ed (the module tier's `pub`).
pub const MOD_PUBLIC: u8 = 1 << 3;
/// The reference resolves into ANOTHER file.
pub const MOD_FOREIGN: u8 = 1 << 4;
/// The name carries a known type (the type tier has a fact here).
pub const MOD_TYPED: u8 = 1 << 5;

/// `style` value for bytes with no declared class (trivia, plain).
pub const STYLE_NONE: u8 = 0xFF;

/// One horizontal span of identically-painted bytes. Four bytes, by
/// design: a 200-column line of dense code is a handful of cache
/// lines, and a whole 10k-line buffer's paint fits in ~KBs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    pub len: u16,
    /// Index into [`crate::LEGEND`], or [`STYLE_NONE`].
    pub style: u8,
    /// `MOD_*` bits (wave 1; zero after wave 0).
    pub mods: u8,
}

/// A full paint of a buffer: per-line runs, tiling each line exactly
/// (text plus terminator), tagged with the revision they describe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paint {
    pub rev: u64,
    pub lines: Vec<Vec<Run>>,
}

/// What one edit changed, and nothing else: the damaged window is
/// REPLACED (old line range → new lines), and any line OUTSIDE the
/// window whose semantic overlay changed is re-emitted by number.
/// Lines that only shifted appear nowhere.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PaintDelta {
    pub rev: u64,
    /// (old_lo, old_hi) replaced by `replacement` (new-line runs).
    pub splice: Option<((u32, u32), Vec<Vec<Run>>)>,
    /// (new line index, runs) — semantic-only repaints (wave 1 bits
    /// moved: a rename changed which references resolve, an edit
    /// changed a type). This IS the blast radius, made visible.
    pub repaints: Vec<(u32, Vec<Run>)>,
}

// ---------------------------------------------------------------------------
// Wave 0: lexical runs, straight off the line lexer
// ---------------------------------------------------------------------------

fn wave0_line(toks: &[rantlr_grammar::lexer::Token], term_len: u32, styles: &Styles) -> Vec<Run> {
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
        if let Some((db, uri)) = db {
            let marks = overlay_marks(db, uri);
            apply_overlay(&mut shown, &widths, &marks);
        }
        let p = Painter { base, shown: shown.clone(), rev: 0 };
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
            Some((db, uri)) => {
                // Fresh overlay over the maintained base…
                let widths = line_widths(&session.buf);
                let mut next = self.base.clone();
                let marks = overlay_marks(db, uri);
                apply_overlay(&mut next, &widths, &marks);
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
}

// ---------------------------------------------------------------------------
// Wire form: the memory layout, little-endian
// ---------------------------------------------------------------------------

/// `[rev u64][n u32]` then per line `[line u32][nruns u32][runs…]`,
/// each run `[len u16][style u8][mods u8]`. A full paint is the same
/// encoding with lines 0..n. Decoding is bounds checks.
pub fn encode_lines(rev: u64, items: &[(u32, &[Run])]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&rev.to_le_bytes());
    out.extend_from_slice(&(items.len() as u32).to_le_bytes());
    for (line, runs) in items {
        out.extend_from_slice(&line.to_le_bytes());
        out.extend_from_slice(&(runs.len() as u32).to_le_bytes());
        for r in *runs {
            out.extend_from_slice(&r.len.to_le_bytes());
            out.push(r.style);
            out.push(r.mods);
        }
    }
    out
}

pub fn decode_lines(bytes: &[u8]) -> Option<(u64, Vec<(u32, Vec<Run>)>)> {
    let mut at = 0usize;
    let take = |at: &mut usize, n: usize| -> Option<&[u8]> {
        let s = bytes.get(*at..*at + n)?;
        *at += n;
        Some(s)
    };
    let rev = u64::from_le_bytes(take(&mut at, 8)?.try_into().ok()?);
    let n = u32::from_le_bytes(take(&mut at, 4)?.try_into().ok()?);
    let mut items = Vec::with_capacity(n as usize);
    for _ in 0..n {
        let line = u32::from_le_bytes(take(&mut at, 4)?.try_into().ok()?);
        let nruns = u32::from_le_bytes(take(&mut at, 4)?.try_into().ok()?);
        let mut runs = Vec::with_capacity(nruns as usize);
        for _ in 0..nruns {
            let len = u16::from_le_bytes(take(&mut at, 2)?.try_into().ok()?);
            let style = *take(&mut at, 1)?.first()?;
            let mods = *take(&mut at, 1)?.first()?;
            runs.push(Run { len, style, mods });
        }
        items.push((line, runs));
    }
    (at == bytes.len()).then_some((rev, items))
}

// ---------------------------------------------------------------------------
// Facts: the hover card, assembled from queries that are already warm
// ---------------------------------------------------------------------------

/// What a name at some offset IS — everything the tiers know, in one
/// record, no transport. `text` is the caller's buffer (lossless
/// trees make it reproducible, but the editor already has it).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FactCard {
    /// The name's own span and text.
    pub span: (u32, u32),
    pub name: String,
    /// True at a definition site (else it is a reference).
    pub is_def: bool,
    pub exported: bool,
    /// Where navigation lands: (uri, span) — for a def, itself.
    pub def_site: Option<(String, (u32, u32))>,
    /// "cannot find", "not exported", or similar when unresolved.
    pub problem: Option<String>,
    /// The display type, when the type tier knows one.
    pub ty: Option<String>,
    /// The declared namespace, when not the default (`tag`, `label`).
    pub ns: Option<String>,
}

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
    let ty_at = |spans: &[((u32, u32), rantlr_sem::TypeId)], s: (u32, u32)| {
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
pub fn type_hints(db: &mut SemDb, uri: &str, text: &str) -> Vec<(u32, u32, String)> {
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
            out.push((li as u32, end - starts[li], format!(": {name}")));
        }
    }
    out.sort();
    out
}
