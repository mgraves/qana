//! qana-engine — the P0 spike's incremental machinery, generalized.
//!
//! The engine is generic over a [`LineLexer`]: any pure, total
//! `lex_line(text, entry_state)` with a small `Eq` state satisfies the
//! envelope's lexing contract, and the engine supplies incremental edit
//! application with per-site reconvergence, lossless reproduction, and
//! the block skeleton. `qana-grammar`'s [`CompiledLexer`] is the
//! canonical implementation; the P0 hand lexer doubles as a second one in
//! tests (the generated ≡ hand equivalence gate).
//!
//! The P0 crate (`qana-lex`) is intentionally left untouched as the
//! reference implementation; this crate supersedes it going forward.

use qana_grammar::{CompiledLexer, MStack, Token, Vocab};

/// The pure line-lexing contract (envelope L1/L2 in trait form).
pub trait LineLexer {
    type State: Copy + Eq + std::fmt::Debug + Default;
    fn lex_line(&self, text: &str, entry: Self::State) -> (Vec<Token>, Self::State);
}

impl LineLexer for CompiledLexer {
    type State = MStack;
    fn lex_line(&self, text: &str, entry: MStack) -> (Vec<Token>, MStack) {
        CompiledLexer::lex_line(self, text, entry)
    }
}

// ---------------------------------------------------------------------------
// Lines (identical semantics to the P0 spike)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LineTerm {
    Lf,
    CrLf,
    Cr,
    None,
}

impl LineTerm {
    pub fn as_str(self) -> &'static str {
        match self {
            LineTerm::Lf => "\n",
            LineTerm::CrLf => "\r\n",
            LineTerm::Cr => "\r",
            LineTerm::None => "",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub text: String,
    pub term: LineTerm,
}

impl Line {
    pub fn new(text: impl Into<String>, term: LineTerm) -> Self {
        Line { text: text.into(), term }
    }
}

pub fn split_lines(src: &str) -> Vec<Line> {
    let bytes = src.as_bytes();
    let mut lines = Vec::new();
    let (mut start, mut i) = (0usize, 0usize);
    while i < bytes.len() {
        match bytes[i] {
            b'\n' => {
                lines.push(Line::new(&src[start..i], LineTerm::Lf));
                i += 1;
                start = i;
            }
            b'\r' => {
                if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                    lines.push(Line::new(&src[start..i], LineTerm::CrLf));
                    i += 2;
                } else {
                    lines.push(Line::new(&src[start..i], LineTerm::Cr));
                    i += 1;
                }
                start = i;
            }
            _ => i += 1,
        }
    }
    lines.push(Line::new(&src[start..], LineTerm::None));
    lines
}

pub fn join_lines(lines: &[Line]) -> String {
    let cap: usize = lines.iter().map(|l| l.text.len() + l.term.as_str().len()).sum();
    let mut out = String::with_capacity(cap);
    for l in lines {
        out.push_str(&l.text);
        out.push_str(l.term.as_str());
    }
    out
}

// ---------------------------------------------------------------------------
// Incremental buffer, generic over the lexer
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineTokens<S> {
    pub tokens: Vec<Token>,
    pub exit: S,
}

#[derive(Clone, Debug)]
pub struct LineEdit {
    pub start: usize,
    pub end: usize,
    pub replacement: Vec<Line>,
}

/// One damaged region: the lines relexed by an edit site (replacement +
/// reconvergence run), in both coordinate systems. `old_lines` addresses
/// the pre-edit buffer (for incremental-parse salvage); `new_lines` the
/// post-edit buffer (for fresh-token harvest). A pure insertion has an
/// empty `old_lines` range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DamageRegion {
    pub old_lines: (usize, usize),
    pub new_lines: (usize, usize),
}

#[derive(Clone, Debug, Default)]
pub struct DamageReport {
    pub sites: usize,
    pub replaced_lines: usize,
    pub relexed_lines: usize,
    pub reconverged_extra: usize,
    /// Ascending, per-site damaged regions (may touch; consumers merge).
    pub regions: Vec<DamageRegion>,
}

pub struct LexedBuffer<'l, L: LineLexer> {
    pub lexer: &'l L,
    pub lines: Vec<Line>,
    pub lexed: Vec<LineTokens<L::State>>,
}

impl<'l, L: LineLexer> LexedBuffer<'l, L> {
    pub fn new(lexer: &'l L, src: &str) -> Self {
        let lines = split_lines(src);
        let mut lexed = Vec::with_capacity(lines.len());
        let mut state = L::State::default();
        for line in &lines {
            let (tokens, exit) = lexer.lex_line(&line.text, state);
            lexed.push(LineTokens { tokens, exit });
            state = exit;
        }
        LexedBuffer { lexer, lines, lexed }
    }

    pub fn reproduce(&self) -> String {
        join_lines(&self.lines)
    }

    pub fn verify_coverage(&self) -> bool {
        self.lines
            .iter()
            .zip(&self.lexed)
            .all(|(l, t)| t.tokens.iter().map(|t| t.len as usize).sum::<usize>() == l.text.len())
    }

    pub fn entry_state(&self, line: usize) -> L::State {
        if line == 0 {
            L::State::default()
        } else {
            self.lexed[line - 1].exit
        }
    }

    /// Batch edit application with per-site reconvergence — semantics
    /// identical to the P0 spike (see its documentation), generic over
    /// the lexer state.
    pub fn apply_edits(&mut self, edits: &[LineEdit]) -> DamageReport {
        for w in edits.windows(2) {
            assert!(w[0].end <= w[1].start, "edits must be ascending and non-overlapping");
        }
        for e in edits {
            assert!(e.start <= e.end && e.end <= self.lines.len(), "edit out of range");
        }

        let mut report = DamageReport { sites: edits.len(), ..Default::default() };

        struct Gap<S> {
            new_start: usize,
            new_end: usize,
            old_exit_before_carry: S,
            /// First replaced line in OLD coordinates.
            old_start: usize,
            /// Cumulative (new − old) line delta AFTER this gap's
            /// replacement — maps carried-line indices back to old ones.
            delta_after: isize,
        }

        let same_shape = edits.iter().all(|e| e.replacement.len() == e.end - e.start);
        let gaps: Vec<Gap<L::State>> = if same_shape {
            edits
                .iter()
                .map(|e| {
                    let old_exit_before_carry = if e.end > e.start {
                        self.lexed[e.end - 1].exit
                    } else if e.start > 0 {
                        self.lexed[e.start - 1].exit
                    } else {
                        L::State::default()
                    };
                    for (k, l) in e.replacement.iter().enumerate() {
                        self.lines[e.start + k] = l.clone();
                    }
                    report.replaced_lines += e.replacement.len();
                    Gap {
                        new_start: e.start,
                        new_end: e.end,
                        old_exit_before_carry,
                        old_start: e.start,
                        delta_after: 0,
                    }
                })
                .collect()
        } else {
            let old_lines = std::mem::take(&mut self.lines);
            let old_lexed = std::mem::take(&mut self.lexed);
            let mut gaps: Vec<Gap<L::State>> = Vec::with_capacity(edits.len());
            let mut new_lines: Vec<Line> = Vec::with_capacity(old_lines.len());
            let mut new_lexed: Vec<LineTokens<L::State>> = Vec::with_capacity(old_lexed.len());
            let mut prev_old_exit = L::State::default();
            let mut old_l = old_lines.into_iter();
            let mut old_t = old_lexed.into_iter();
            let mut cursor = 0usize;
            let mut cur_delta: isize = 0;

            for e in edits {
                for _ in cursor..e.start {
                    let l = old_l.next().expect("line underflow");
                    let t = old_t.next().expect("lex underflow");
                    prev_old_exit = t.exit;
                    new_lines.push(l);
                    new_lexed.push(t);
                }
                for _ in e.start..e.end {
                    let _ = old_l.next().expect("line underflow");
                    prev_old_exit = old_t.next().expect("lex underflow").exit;
                }
                let new_start = new_lines.len();
                for l in &e.replacement {
                    new_lines.push(l.clone());
                    new_lexed.push(LineTokens { tokens: Vec::new(), exit: L::State::default() });
                }
                report.replaced_lines += e.replacement.len();
                cur_delta += e.replacement.len() as isize - (e.end - e.start) as isize;
                gaps.push(Gap {
                    new_start,
                    new_end: new_lines.len(),
                    old_exit_before_carry: prev_old_exit,
                    old_start: e.start,
                    delta_after: cur_delta,
                });
                cursor = e.end;
            }
            for l in old_l {
                new_lines.push(l);
                new_lexed.push(old_t.next().expect("lex underflow"));
            }
            self.lines = new_lines;
            self.lexed = new_lexed;
            gaps
        };

        debug_assert!(
            self.lines
                .iter()
                .enumerate()
                .all(|(i, l)| (l.term == LineTerm::None) == (i + 1 == self.lines.len())),
            "canonical line-term invariant violated by edit batch"
        );

        let mut settled_until = 0usize;
        for (gi, gap) in gaps.iter().enumerate() {
            let mut i = gap.new_start.max(settled_until);
            let mut st = self.entry_state(i);

            while i < gap.new_end {
                let (tokens, exit) = self.lexer.lex_line(&self.lines[i].text, st);
                st = exit;
                self.lexed[i] = LineTokens { tokens, exit };
                report.relexed_lines += 1;
                i += 1;
            }

            let mut expected_old_entry = gap.old_exit_before_carry;
            let next_gap_start = gaps.get(gi + 1).map(|g| g.new_start).unwrap_or(usize::MAX);
            while i < self.lines.len() {
                if st == expected_old_entry {
                    break;
                }
                if i >= next_gap_start {
                    break;
                }
                let old_exit = self.lexed[i].exit;
                let (tokens, exit) = self.lexer.lex_line(&self.lines[i].text, st);
                st = exit;
                self.lexed[i] = LineTokens { tokens, exit };
                report.relexed_lines += 1;
                report.reconverged_extra += 1;
                expected_old_entry = old_exit;
                i += 1;
            }
            settled_until = i;

            // Record this site's damaged region in both coordinate systems.
            // Carried (reconverged) lines map old = new − delta_after.
            let old_end = (i as isize - gap.delta_after) as usize;
            let region = DamageRegion {
                old_lines: (gap.old_start, old_end.max(gap.old_start)),
                new_lines: (gap.new_start, i),
            };
            if region.old_lines.0 != region.old_lines.1 || region.new_lines.0 != region.new_lines.1
            {
                report.regions.push(region);
            }
        }

        report
    }
}

// ---------------------------------------------------------------------------
// Green-tree bridge
// ---------------------------------------------------------------------------

/// The FULL token stream for green-tree building: every lexed token with
/// its text, in order, plus synthesized [`qana_grammar::NEWLINE`]
/// trivia tokens carrying the exact line terminators (the engine's line
/// model stores those out-of-band). Concatenating the texts reproduces
/// the buffer byte-for-byte — which is exactly the invariant
/// `build_green` then lifts to the tree.
pub fn full_tokens(
    lexer: &CompiledLexer,
    buf: &LexedBuffer<'_, CompiledLexer>,
) -> Vec<qana_grammar::TokWithText> {
    let mut out = Vec::new();
    for (line, lt) in buf.lines.iter().zip(&buf.lexed) {
        let mut off = 0usize;
        for tok in &lt.tokens {
            let end = off + tok.len as usize;
            out.push(qana_grammar::TokWithText {
                id: tok.id,
                trivia: lexer.is_trivia(tok.id),
                text: line.text[off..end].to_string(),
            });
            off = end;
        }
        if line.term != LineTerm::None {
            out.push(qana_grammar::TokWithText {
                id: qana_grammar::NEWLINE,
                trivia: true,
                text: line.term.as_str().to_string(),
            });
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Block skeleton, vocab-driven
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Block {
    pub kind: qana_grammar::BracketKind,
    pub open: (usize, usize),
    pub close: Option<(usize, usize)>,
    pub depth: u32,
}

#[derive(Clone, Debug, Default)]
pub struct Skeleton {
    pub blocks: Vec<Block>,
    pub max_depth: u32,
    pub unmatched_closes: usize,
}

impl Skeleton {
    pub fn folding_ranges(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        self.blocks.iter().filter_map(|b| {
            let (ol, _) = b.open;
            let (cl, _) = b.close?;
            (cl > ol).then_some((ol, cl))
        })
    }
}

pub fn build_skeleton<L: LineLexer>(buf: &LexedBuffer<'_, L>, vocab: &Vocab) -> Skeleton {
    let mut sk = Skeleton::default();
    let mut stack: Vec<usize> = Vec::new();
    for (li, lt) in buf.lexed.iter().enumerate() {
        for (ti, tok) in lt.tokens.iter().enumerate() {
            let Some(&Some((kind, open))) = vocab.brackets.get(tok.id as usize) else {
                continue;
            };
            if open {
                let depth = stack.len() as u32 + 1;
                sk.max_depth = sk.max_depth.max(depth);
                stack.push(sk.blocks.len());
                sk.blocks.push(Block { kind, open: (li, ti), close: None, depth });
            } else {
                match stack.last() {
                    Some(&bi) if sk.blocks[bi].kind == kind => {
                        sk.blocks[bi].close = Some((li, ti));
                        stack.pop();
                    }
                    _ => sk.unmatched_closes += 1,
                }
            }
        }
    }
    sk
}

// ---------------------------------------------------------------------------
// Incremental session: lexer damage → salvage → Wagner parse
// ---------------------------------------------------------------------------

use qana_grammar::incremental::Repair;
use qana_grammar::{
    batch_parse_full, incremental_parse, salvage, FreshRegion, GreenNode, IncParseError,
    LrTables, ReuseStats, SynGrammar, TokWithText, NEWLINE,
};
use std::sync::Arc;

/// A live document: buffer + current lossless tree, kept in sync
/// incrementally. Error recovery makes parsing total, so the tree stays
/// valid while the user types through broken states; repairs are
/// surfaced for diagnostics, and error-poisoned regions re-derive on
/// each edit while clean regions keep splicing.
pub struct IncSession<'l> {
    pub buf: LexedBuffer<'l, CompiledLexer>,
    tree: Option<Arc<GreenNode>>,
    /// Repairs from the most recent parse (diagnostics substrate).
    pub last_repairs: Vec<Repair>,
    /// Per-line (full tokens incl. terminator, non-trivia terminals),
    /// matching the OLD state of `tree` (used for damage alignment).
    line_counts: Vec<(u32, u32)>,
}

#[derive(Clone, Debug)]
pub struct EditOutcome {
    pub damage: DamageReport,
    pub stats: ReuseStats,
    pub repairs: Vec<Repair>,
}

fn count_line(
    lexer: &CompiledLexer,
    buf: &LexedBuffer<'_, CompiledLexer>,
    li: usize,
) -> (u32, u32) {
    let (line, lt) = (&buf.lines[li], &buf.lexed[li]);
    let terms = lt.tokens.iter().filter(|t| !lexer.is_trivia(t.id)).count() as u32;
    let full = lt.tokens.len() as u32 + (line.term != LineTerm::None) as u32;
    (full, terms)
}

fn count_lines(lexer: &CompiledLexer, buf: &LexedBuffer<'_, CompiledLexer>) -> Vec<(u32, u32)> {
    (0..buf.lines.len()).map(|li| count_line(lexer, buf, li)).collect()
}

fn harvest_lines(
    lexer: &CompiledLexer,
    buf: &LexedBuffer<'_, CompiledLexer>,
    range: (usize, usize),
) -> Vec<TokWithText> {
    let mut out = Vec::new();
    for li in range.0..range.1 {
        let line = &buf.lines[li];
        let lt = &buf.lexed[li];
        let mut off = 0usize;
        for tok in &lt.tokens {
            let end = off + tok.len as usize;
            out.push(TokWithText {
                id: tok.id,
                trivia: lexer.is_trivia(tok.id),
                text: line.text[off..end].to_string(),
            });
            off = end;
        }
        if line.term != LineTerm::None {
            out.push(TokWithText {
                id: NEWLINE,
                trivia: true,
                text: line.term.as_str().to_string(),
            });
        }
    }
    out
}

fn merge_regions(regions: &[DamageRegion]) -> Vec<DamageRegion> {
    let mut out: Vec<DamageRegion> = Vec::new();
    for r in regions {
        match out.last_mut() {
            Some(prev) if r.old_lines.0 <= prev.old_lines.1 || r.new_lines.0 <= prev.new_lines.1 => {
                prev.old_lines.1 = prev.old_lines.1.max(r.old_lines.1);
                prev.new_lines.1 = prev.new_lines.1.max(r.new_lines.1);
            }
            _ => out.push(*r),
        }
    }
    out
}

impl<'l> IncSession<'l> {
    pub fn new(
        lexer: &'l CompiledLexer,
        sg: &SynGrammar,
        tables: &LrTables,
        src: &str,
    ) -> Result<Self, IncParseError> {
        let buf = LexedBuffer::new(lexer, src);
        let all = full_tokens(lexer, &buf);
        let (tree, _, repairs) = batch_parse_full(sg, tables, &all)?;
        let line_counts = count_lines(lexer, &buf);
        Ok(IncSession { buf, tree: Some(tree), last_repairs: repairs, line_counts })
    }

    pub fn tree(&self) -> Option<&Arc<GreenNode>> {
        self.tree.as_ref()
    }

    /// Apply an edit batch and incrementally reparse. Returns damage +
    /// reuse statistics; on a syntax error the session survives (buffer
    /// stays current) but the tree is invalid until an edit parses again.
    pub fn edit(
        &mut self,
        sg: &SynGrammar,
        tables: &LrTables,
        edits: &[LineEdit],
    ) -> Result<EditOutcome, IncParseError> {
        let lexer = self.buf.lexer;
        let damage = self.buf.apply_edits(edits);
        let old_counts = std::mem::take(&mut self.line_counts);
        // Splice-update the per-line counts: only damaged lines recount
        // (the old counts stay intact above for damage alignment).
        let merged = merge_regions(&damage.regions);
        let mut new_counts = old_counts.clone();
        for r in merged.iter().rev() {
            let fresh: Vec<(u32, u32)> =
                (r.new_lines.0..r.new_lines.1).map(|li| count_line(lexer, &self.buf, li)).collect();
            new_counts.splice(r.old_lines.0..r.old_lines.1, fresh);
        }
        debug_assert_eq!(new_counts.len(), self.buf.lines.len());

        let result = match self.tree.take() {
            Some(old_tree) => {
                let regions = merged;
                if regions.is_empty() {
                    // No-op batch: tree unchanged, full reuse.
                    let stats = ReuseStats {
                        reused_terms: old_tree.terms,
                        total_terms: old_tree.terms,
                        splices: 1,
                        breakdowns: 0,
                    };
                    let repairs = self.last_repairs.clone();
                    Ok((old_tree, stats, repairs))
                } else {
                    // Old full-token prefix sums for damage alignment.
                    let mut pref = Vec::with_capacity(old_counts.len() + 1);
                    let mut acc = 0u32;
                    pref.push(0);
                    for (full, _) in &old_counts {
                        acc += full;
                        pref.push(acc);
                    }
                    let fresh: Vec<FreshRegion> = regions
                        .iter()
                        .map(|r| FreshRegion {
                            old_toks: (pref[r.old_lines.0], pref[r.old_lines.1]),
                            tokens: harvest_lines(lexer, &self.buf, r.new_lines),
                        })
                        .collect();
                    let input = salvage(&old_tree, &fresh);
                    incremental_parse(sg, tables, input)
                }
            }
            None => {
                // Safety fallback (should not occur now that parsing is
                // total): full batch reparse.
                let all = full_tokens(lexer, &self.buf);
                batch_parse_full(sg, tables, &all).map(|(tree, _, repairs)| {
                    let stats = ReuseStats {
                        reused_terms: 0,
                        total_terms: tree.terms,
                        splices: 0,
                        breakdowns: 0,
                    };
                    (tree, stats, repairs)
                })
            }
        };

        self.line_counts = new_counts;
        match result {
            Ok((tree, stats, repairs)) => {
                self.tree = Some(tree);
                self.last_repairs = repairs.clone();
                Ok(EditOutcome { damage, stats, repairs })
            }
            Err(e) => {
                self.tree = None;
                Err(e)
            }
        }
    }
}
