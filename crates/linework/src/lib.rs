//! LINEWORK — the drawn marks of an illustration, and here they are
//! keyed by line. The engine-neutral protocol for binding language
//! intelligence to an editor: line-keyed, two-wave code PAINT plus
//! on-demand FACTS, with a trait ([`Limner`]) an editor widget can be
//! generic over.
//!
//! This crate has NO dependencies and knows nothing about any
//! particular engine or editor. An intelligence engine (a parser
//! toolchain, a language runtime) implements [`Limner`]; an editor
//! consumes it. Neither needs the other's types.
//!
//! # The shape
//!
//! * Paint is LINE-KEYED: each line is a vector of [`Run`]s — 4-byte
//!   cells `{len, style, mods}` tiling the line's bytes exactly. Runs
//!   are line-relative, so a line that merely SHIFTS re-emits
//!   nothing.
//! * Paint is TWO-WAVE: `style` is the lexical class (wave 0 —
//!   available synchronously with a keystroke, from the damaged lines
//!   only), and `mods` carries semantic bits (wave 1 — definition,
//!   resolved/unresolved reference, public, foreign, typed) that
//!   ARRIVE WITHOUT EVER CHANGING A STYLE. Refinement is additive, so
//!   the first frame is already right and nothing flickers.
//! * A [`PaintDelta`] names what one edit changed and nothing else: a
//!   spliced window of lines, plus semantic-only repaints by line
//!   number. The repaint set is the edit's semantic blast radius,
//!   made visible.
//! * [`FactCard`]s answer "what IS this name" — definition site,
//!   problem, display type, namespace — and [`Hint`]s carry inline
//!   decorations. Both are lookups against warm state, not requests.
//!
//! Offsets and lengths are BYTES of UTF-8 text. Consumers that index
//! by character (ropes) convert at the boundary.
//!
//! The wire form is the memory form, little-endian; see
//! [`encode_lines`]/[`decode_lines`]. Decoding is bounds checks, not
//! parsing.
//!
//! # Both sides of the seam
//!
//! An engine implements [`Limner`]; an editor holds one and asks. This
//! toy engine paints each line's first word as style 0 and marks it a
//! definition — enough to show the shape without being a real
//! highlighter.
//!
//! ```
//! use linework::{FactCard, Hint, LineEdit, Limner, Paint, PaintDelta, Run,
//!                MOD_DEF, STYLE_NONE};
//!
//! fn paint_line(line: &str) -> Vec<Run> {
//!     if line.is_empty() {
//!         return Vec::new();
//!     }
//!     // Byte indices throughout — never char indices.
//!     let head = line.find(' ').unwrap_or(line.len()) as u16;
//!     let tail = line.len() as u16 - head;
//!     let mut runs = vec![Run { len: head, style: 0, mods: MOD_DEF }];
//!     if tail > 0 {
//!         runs.push(Run { len: tail, style: STYLE_NONE, mods: 0 });
//!     }
//!     runs
//! }
//!
//! #[derive(Default)]
//! struct FirstWord { rev: u64, text: String }
//!
//! impl Limner for FirstWord {
//!     fn open(&mut self, text: &str) -> Paint {
//!         self.text = text.to_string();
//!         self.rev += 1;
//!         Paint { rev: self.rev, lines: text.lines().map(paint_line).collect() }
//!     }
//!
//!     fn edit(&mut self, edit: &LineEdit) -> PaintDelta {
//!         self.rev += 1;
//!         let runs = edit.lines.iter().map(|l| paint_line(l)).collect();
//!         // Only the spliced window is re-emitted; untouched lines are
//!         // absent from the delta, not repeated.
//!         PaintDelta {
//!             rev: self.rev,
//!             splice: Some(((edit.start, edit.end), runs)),
//!             repaints: Vec::new(),
//!         }
//!     }
//!
//!     fn facts(&mut self, _offset: u32) -> Option<FactCard> { None }
//!     fn hints(&mut self) -> Vec<Hint> { Vec::new() }
//!     fn legend(&self) -> Vec<String> { vec!["keyword".to_string()] }
//!     fn text(&mut self) -> String { self.text.clone() }
//! }
//!
//! // --- the editor side: generic over the trait, blind to the engine ---
//! fn total_painted_bytes(limner: &mut dyn Limner, src: &str) -> u32 {
//!     let paint = limner.open(src);
//!     paint.lines.iter().flatten().map(|r| r.len as u32).sum()
//! }
//!
//! let mut engine = FirstWord::default();
//! assert_eq!(total_painted_bytes(&mut engine, "let x\nlet y"), 10);
//!
//! // An edit returns only what changed.
//! let delta = engine.edit(&LineEdit {
//!     start: 1,
//!     end: 2,
//!     lines: vec!["let zz".to_string()],
//! });
//! assert!(delta.repaints.is_empty());
//! let (range, lines) = delta.splice.unwrap();
//! assert_eq!(range, (1, 2));
//! assert_eq!(lines[0][0].mods, MOD_DEF);
//! ```

// ---------------------------------------------------------------------------
// Modifier bits (wave 1)
// ---------------------------------------------------------------------------

/// This run's bytes are a definition's name.
pub const MOD_DEF: u8 = 1 << 0;
/// A reference that RESOLVES (navigation will land somewhere).
pub const MOD_REF: u8 = 1 << 1;
/// A reference that does not resolve — including "exists but is not
/// exported" and broken qualified paths (the squiggle substrate).
pub const MOD_UNRESOLVED: u8 = 1 << 2;
/// The definition is exported (a module tier's `pub`).
pub const MOD_PUBLIC: u8 = 1 << 3;
/// The reference resolves into ANOTHER file.
pub const MOD_FOREIGN: u8 = 1 << 4;
/// The name carries a known type (a type tier has a fact here).
pub const MOD_TYPED: u8 = 1 << 5;

/// `style` value for bytes with no declared class (trivia, plain).
pub const STYLE_NONE: u8 = 0xFF;

// ---------------------------------------------------------------------------
// Paint
// ---------------------------------------------------------------------------

/// One horizontal span of identically-painted bytes. Four bytes, by
/// design: a dense 200-column line is a handful of cache lines, and a
/// 10k-line buffer's whole paint fits in ~KBs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    pub len: u16,
    /// Index into the provider's [`Limner::legend`], or [`STYLE_NONE`].
    pub style: u8,
    /// `MOD_*` bits (zero after wave 0).
    pub mods: u8,
}

/// A full paint: per-line runs tiling each line exactly (text plus
/// terminator bytes), tagged with the revision they describe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paint {
    pub rev: u64,
    pub lines: Vec<Vec<Run>>,
}

/// What one edit changed, and nothing else.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PaintDelta {
    pub rev: u64,
    /// Old line range `(lo, hi)` replaced by these new lines' runs.
    pub splice: Option<((u32, u32), Vec<Vec<Run>>)>,
    /// `(new line index, runs)` — semantic-only repaints outside the
    /// splice (a rename moved resolution, an edit changed a type).
    pub repaints: Vec<(u32, Vec<Run>)>,
}

// ---------------------------------------------------------------------------
// Edits (editor → limner)
// ---------------------------------------------------------------------------

/// Replace lines `[start, end)` with `lines` (texts WITHOUT
/// terminators). The limner preserves the replaced lines' terminators
/// positionally and uses its default for any extras — a pure-text
/// protocol stays out of the newline-politics business.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineEdit {
    pub start: u32,
    pub end: u32,
    pub lines: Vec<String>,
}

// ---------------------------------------------------------------------------
// Facts (hover) and hints (inline decoration)
// ---------------------------------------------------------------------------

/// Everything the tiers know about the name at an offset, in one
/// record. No transport, no markdown — the editor renders it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FactCard {
    /// The name's own byte span in its document.
    pub span: (u32, u32),
    pub name: String,
    /// True at a definition site (else it is a reference).
    pub is_def: bool,
    pub exported: bool,
    /// Where navigation lands: (uri, byte span) — for a def, itself.
    pub def_site: Option<(String, (u32, u32))>,
    /// "cannot find …", "… is not exported", when unresolved.
    pub problem: Option<String>,
    /// Display type, when a type tier knows one.
    pub ty: Option<String>,
    /// Declared namespace when not the default (`tag`, `label`).
    pub ns: Option<String>,
}

/// An inline decoration: `text` rendered at `(line, col)` — e.g. a
/// `: Num` type hint after a definition's name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hint {
    pub line: u32,
    pub col: u32,
    pub text: String,
}

// ---------------------------------------------------------------------------
// The trait: what an editor is generic over
// ---------------------------------------------------------------------------

/// One who limns: the thing that draws a document's linework. An
/// intelligence engine implements this; an editor widget holds one and
/// stays ignorant of everything behind it.
///
/// The contract, in editor terms:
/// * [`open`](Limner::open) once per document (or on wholesale
///   replacement) — full paint back.
/// * [`edit`](Limner::edit) per change, line-shaped — a delta back,
///   synchronously; the wave-0 portion of that delta is expected in
///   MICROSECONDS, so the caller may paint the same frame.
/// * [`facts`](Limner::facts)/[`hints`](Limner::hints) on demand
///   (hover, cursor move) — warm lookups.
/// * [`legend`](Limner::legend) maps `style` bytes to class names the
///   theme understands ("keyword", "string", …).
pub trait Limner {
    fn open(&mut self, text: &str) -> Paint;
    fn edit(&mut self, edit: &LineEdit) -> PaintDelta;
    fn facts(&mut self, offset: u32) -> Option<FactCard>;
    fn hints(&mut self) -> Vec<Hint>;
    fn legend(&self) -> Vec<String>;
    /// The document as the limner sees it (lossless engines reproduce
    /// it; useful for differential checks and byte↔char conversion).
    fn text(&mut self) -> String;
}

// ---------------------------------------------------------------------------
// Wire form
// ---------------------------------------------------------------------------

/// `[rev u64][n u32]` then per line `[line u32][nruns u32][runs…]`,
/// each run `[len u16][style u8][mods u8]`, all little-endian.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_roundtrips_and_refuses_truncation() {
        let lines = vec![
            (0u32, vec![Run { len: 3, style: 0, mods: MOD_DEF }, Run { len: 1, style: STYLE_NONE, mods: 0 }]),
            (7u32, vec![Run { len: 65535, style: 4, mods: MOD_REF | MOD_FOREIGN }]),
        ];
        let items: Vec<(u32, &[Run])> = lines.iter().map(|(l, r)| (*l, r.as_slice())).collect();
        let bytes = encode_lines(9, &items);
        let (rev, back) = decode_lines(&bytes).unwrap();
        assert_eq!(rev, 9);
        assert_eq!(back, lines);
        for cut in 1..bytes.len() {
            assert!(decode_lines(&bytes[..cut]).is_none(), "cut at {cut} must refuse");
        }
    }

    #[test]
    fn mod_bits_are_distinct() {
        let all = [MOD_DEF, MOD_REF, MOD_UNRESOLVED, MOD_PUBLIC, MOD_FOREIGN, MOD_TYPED];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_eq!(a & b, 0);
            }
        }
    }
}
