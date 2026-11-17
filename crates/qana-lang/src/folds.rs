//! The folds channel of a live document: every region an editor can
//! fold that is not outline structure. A block between a declared
//! bracket pair, a mode that runs over lines (a block comment, a
//! string), and a run of comment lines. See [`linework::Limner::folds_shared`].
//!
//! Folds come from tokens alone, and a line's tokens depend on nothing
//! but its text and the state it is entered in. So what a line
//! contributes is kept per line ([`LineFolds`]), and an edit recomputes
//! it for the lines the lexer relexed and no others. Matching brackets
//! is not local (one `{` typed at the top re-pairs every brace below
//! it), so the folds themselves are matched afresh when they are asked
//! for, once per revision, by one pass over what the lines kept: a few
//! words per line, no text, no tokens.

use linework::Mark;
use qana_engine::{DamageRegion, LexedBuffer};
use qana_grammar::model::{BracketKind, TokenId};
use qana_grammar::{CompiledLexer, MStack};
use std::sync::Arc;

/// The fold categories every language has, by index; the categories of
/// its multi-line modes follow them (see [`FoldIndex::new`]).
const BRACE: u8 = 0;
const BRACKET: u8 = 1;
const PAREN: u8 = 2;
const COMMENT_LINES: u8 = 3;
const FIXED_LEGEND: [&str; 4] = ["brace", "bracket", "paren", "comment lines"];

/// No category: a token that enters no mode.
const NOT_A_RUN: u8 = u8::MAX;

/// A bracket token on a line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Bracket {
    /// Byte offset of the token in its line, and its length.
    col: u32,
    len: u32,
    /// [`BRACE`], [`BRACKET`] or [`PAREN`].
    kind: u8,
    open: bool,
}

/// Where a line enters or leaves a mode from the base mode: the edges of
/// a RUN, the tokens from one that enters a mode to the one that brings
/// the lexer back to the base mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RunEdge {
    /// The token at `col` (`len` bytes) enters a mode from the base mode;
    /// the run's category is `category`.
    Open { col: u32, len: u32, category: u8 },
    /// The run ends at byte `end` of the line: after the token that pops
    /// back to the base mode, or at the end of the line's text when the
    /// line's end pops a line-bounded mode.
    Close { end: u32 },
}

/// What one line contributes to the folds. Recomputed only when the
/// lexer relexes the line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct LineFolds {
    /// The line's bracket tokens, in order.
    brackets: Vec<Bracket>,
    /// The line's run edges, in order.
    runs: Vec<RunEdge>,
    /// For a COMMENT LINE (entered and left in the base mode, nothing on
    /// it but trivia, and at least one comment): the bytes from the start
    /// of its first comment to the end of its last.
    comment: Option<(u32, u32)>,
}

/// One fold being matched: a reserved slot in the output, and what
/// closes it.
#[derive(Clone, Copy)]
enum Open {
    Bracket { slot: usize, kind: u8, line: usize },
    Run { slot: usize, line: usize },
}

/// The folds channel of one document.
pub(crate) struct FoldIndex {
    /// Per token id: its bracket, as [`BRACE`]/[`BRACKET`]/[`PAREN`] and
    /// whether it opens, or `None`.
    bracket_of: Vec<Option<(u8, bool)>>,
    /// Per token id: the category of the run it opens when it enters a
    /// mode from the base mode, or [`NOT_A_RUN`].
    run_category: Vec<u8>,
    /// Per token id: whether its style is `comment`.
    is_comment: Vec<bool>,
    /// Per token id: whether it is trivia.
    is_trivia: Vec<bool>,
    legend: Vec<String>,
    lines: Vec<LineFolds>,
    /// The revision `folds` was matched for, and the folds.
    matched_rev: u64,
    folds: Arc<Vec<Mark>>,
}

impl FoldIndex {
    /// The fold index of `buf`, whose language styles its tokens with
    /// `class_of` (a token id to its style class name, if it has one).
    pub(crate) fn new<'c>(
        lexer: &CompiledLexer,
        class_of: impl Fn(TokenId) -> Option<&'c str>,
        buf: &LexedBuffer<'_, CompiledLexer>,
    ) -> FoldIndex {
        let vocab = &lexer.vocab;
        let ids = 0..vocab.names.len() as TokenId;
        let bracket_of = ids
            .clone()
            .map(|id| {
                vocab.brackets.get(id as usize).copied().flatten().map(|(kind, open)| {
                    let kind = match kind {
                        BracketKind::Brace => BRACE,
                        BracketKind::Bracket => BRACKET,
                        BracketKind::Paren => PAREN,
                    };
                    (kind, open)
                })
            })
            .collect();

        // A run is named for the style of the token that opens it: a
        // block comment's opener is styled `comment`, a string's
        // `string`. One whose opener has no style is a `region`.
        let mut legend: Vec<String> = FIXED_LEGEND.iter().map(|s| s.to_string()).collect();
        let base = MStack::default();
        let run_category = ids
            .clone()
            .map(|id| {
                if lexer.after_token(base, id).depth() == 0 {
                    return NOT_A_RUN;
                }
                let name = class_of(id).unwrap_or("region");
                match legend.iter().position(|c| c == name) {
                    Some(i) => i as u8,
                    None => {
                        legend.push(name.to_string());
                        (legend.len() - 1) as u8
                    }
                }
            })
            .collect();
        let is_comment = ids.clone().map(|id| class_of(id) == Some("comment")).collect();
        let is_trivia = ids.map(|id| lexer.is_trivia(id)).collect();

        let mut index = FoldIndex {
            bracket_of,
            run_category,
            is_comment,
            is_trivia,
            legend,
            lines: Vec::new(),
            matched_rev: 0,
            folds: Arc::new(Vec::new()),
        };
        index.reset(buf);
        index
    }

    /// Read every line of `buf` afresh: a document opened anew.
    pub(crate) fn reset(&mut self, buf: &LexedBuffer<'_, CompiledLexer>) {
        self.lines = (0..buf.lines.len()).map(|i| self.read_line(buf, i)).collect();
        self.matched_rev = 0;
    }

    /// The lexer relexed `damage` (each region's old lines are replaced by
    /// its new ones): read those lines again. Every other line was
    /// entered in the state it was before and has the text it had, so
    /// what it contributes is unchanged.
    pub(crate) fn note_edit(
        &mut self,
        buf: &LexedBuffer<'_, CompiledLexer>,
        damage: &[DamageRegion],
    ) {
        // From the last region back, so that an earlier region's old line
        // numbers still address this vector when it is reached.
        for region in damage.iter().rev() {
            let (old_lo, old_hi) = region.old_lines;
            let (new_lo, new_hi) = region.new_lines;
            let fresh: Vec<LineFolds> = (new_lo..new_hi).map(|i| self.read_line(buf, i)).collect();
            self.lines.splice(old_lo..old_hi, fresh);
        }
        debug_assert_eq!(self.lines.len(), buf.lines.len(), "a fold entry for every line");
    }

    /// What line `i` of `buf` contributes.
    fn read_line(&self, buf: &LexedBuffer<'_, CompiledLexer>, i: usize) -> LineFolds {
        let lexer = buf.lexer;
        let (line, lexed) = (&buf.lines[i], &buf.lexed[i]);
        let entry = buf.entry_state(i);
        let mut out = LineFolds::default();

        let mut state = entry;
        let mut col = 0u32;
        let mut all_trivia = true;
        let mut comment: Option<(u32, u32)> = None;
        for token in &lexed.tokens {
            let id = token.id as usize;
            if let Some(&Some((kind, open))) = self.bracket_of.get(id) {
                out.brackets.push(Bracket { col, len: token.len, kind, open });
            }
            let after = lexer.after_token(state, token.id);
            if state.depth() == 0 && after.depth() > 0 {
                let category = self.run_category.get(id).copied().unwrap_or(NOT_A_RUN);
                out.runs.push(RunEdge::Open { col, len: token.len, category });
            } else if state.depth() > 0 && after.depth() == 0 {
                out.runs.push(RunEdge::Close { end: col + token.len });
            }
            // The unknown token (an id past the vocabulary's tokens) is
            // trivia and no comment.
            all_trivia &= self.is_trivia.get(id).copied().unwrap_or(true);
            if self.is_comment.get(id).copied().unwrap_or(false) {
                let end = col + token.len;
                comment = Some(comment.map_or((col, end), |(start, _)| (start, end)));
            }
            state = after;
            col += token.len;
        }
        // The end of the line pops the line-bounded modes, which can
        // bring the lexer back to the base mode here.
        if state.depth() > 0 && lexed.exit.depth() == 0 {
            out.runs.push(RunEdge::Close { end: line.text.len() as u32 });
        }
        if entry.depth() == 0 && lexed.exit.depth() == 0 && all_trivia {
            out.comment = comment;
        }
        out
    }

    pub(crate) fn legend(&self) -> &[String] {
        &self.legend
    }

    /// The folds of `buf` at revision `rev`, matched once per revision.
    pub(crate) fn folds(
        &mut self,
        buf: &LexedBuffer<'_, CompiledLexer>,
        rev: u64,
    ) -> Arc<Vec<Mark>> {
        if self.matched_rev != rev {
            self.folds = Arc::new(self.match_folds(buf));
            self.matched_rev = rev;
        }
        self.folds.clone()
    }

    /// Match the lines' brackets and runs into folds.
    ///
    /// Brackets pair as editors pair them: a close takes the nearest
    /// open of its kind, and the opens it passes over are left unclosed
    /// (`( … }` closes the `{` before the paren and leaves the paren
    /// open); a close with no open of its kind is ignored. A run is
    /// a wall: brackets opened inside one close inside it or not at all,
    /// and a bracket inside it never closes one opened before it. So the
    /// folds nest. Each fold takes a slot in the output when it opens,
    /// which keeps the output in document order; a slot is filled when
    /// its region closes on a later line, and stays empty otherwise.
    fn match_folds(&self, buf: &LexedBuffer<'_, CompiledLexer>) -> Vec<Mark> {
        let mut slots: Vec<Option<Mark>> = Vec::new();
        let mut stack: Vec<Open> = Vec::new();
        // The comment run being read: its slot, start byte, first line,
        // and its last comment line's line and end byte.
        let mut comments: Option<(usize, u32, usize, usize, u32)> = None;

        let fold = |category: u8, start: u32, end: u32, name_end: u32| Mark {
            category,
            start,
            end,
            name_start: start,
            name_end,
            name: String::new(),
        };
        // `opens[slot]`: where the region of that slot opened, and its
        // opening token's end, kept until it closes.
        let mut opens: Vec<(u8, u32, u32)> = Vec::new();

        let mut line_start = 0u32;
        for (i, (folds, line)) in self.lines.iter().zip(&buf.lines).enumerate() {
            // Comment lines: a run of them is one fold.
            match (folds.comment, &mut comments) {
                (Some((_, end)), Some(run)) => {
                    run.3 = i;
                    run.4 = line_start + end;
                }
                (Some((start, end)), None) => {
                    let slot = slots.len();
                    slots.push(None);
                    opens.push((COMMENT_LINES, line_start + start, line_start + end));
                    comments = Some((slot, line_start + start, i, i, line_start + end));
                }
                (None, _) => {
                    if let Some((slot, start, first, last, end)) = comments.take() {
                        if last > first {
                            slots[slot] = Some(fold(COMMENT_LINES, start, end, opens[slot].2));
                        }
                    }
                }
            }

            // Brackets and run edges, in the order they stand on the line.
            let (mut b, mut r) = (0usize, 0usize);
            loop {
                let bracket_col = folds.brackets.get(b).map(|x| x.col);
                let run_col = folds.runs.get(r).map(|e| match *e {
                    RunEdge::Open { col, .. } => col,
                    // A close is at the end of its token: it comes after
                    // any bracket that starts before that end.
                    RunEdge::Close { end } => end.saturating_sub(1),
                });
                let take_bracket = match (bracket_col, run_col) {
                    (None, None) => break,
                    (Some(_), None) => true,
                    (None, Some(_)) => false,
                    (Some(bc), Some(rc)) => bc <= rc,
                };
                if take_bracket {
                    let x = folds.brackets[b];
                    b += 1;
                    let at = line_start + x.col;
                    if x.open {
                        let slot = slots.len();
                        slots.push(None);
                        opens.push((x.kind, at, at + x.len));
                        stack.push(Open::Bracket { slot, kind: x.kind, line: i });
                        continue;
                    }
                    // The nearest open of this kind, above the nearest run.
                    let found = stack.iter().rposition(|o| match *o {
                        Open::Bracket { kind, .. } => kind == x.kind,
                        Open::Run { .. } => true,
                    });
                    if let Some(k) = found {
                        if let Open::Bracket { slot, line, .. } = stack[k] {
                            stack.truncate(k);
                            if line < i {
                                let (category, start, name_end) = opens[slot];
                                slots[slot] = Some(fold(category, start, at + x.len, name_end));
                            }
                        }
                    }
                } else {
                    let edge = folds.runs[r];
                    r += 1;
                    match edge {
                        RunEdge::Open { col, len, category } => {
                            let slot = slots.len();
                            slots.push(None);
                            let at = line_start + col;
                            opens.push((category, at, at + len));
                            stack.push(Open::Run { slot, line: i });
                        }
                        RunEdge::Close { end } => {
                            let found = stack.iter().rposition(|o| matches!(o, Open::Run { .. }));
                            if let Some(k) = found {
                                if let Open::Run { slot, line } = stack[k] {
                                    stack.truncate(k);
                                    let (category, start, name_end) = opens[slot];
                                    if line < i && category != NOT_A_RUN {
                                        slots[slot] =
                                            Some(fold(category, start, line_start + end, name_end));
                                    }
                                }
                            }
                        }
                    }
                }
            }
            line_start += (line.text.len() + line.term.as_str().len()) as u32;
        }
        if let Some((slot, start, first, last, end)) = comments {
            if last > first {
                slots[slot] = Some(fold(COMMENT_LINES, start, end, opens[slot].2));
            }
        }
        slots.into_iter().flatten().collect()
    }
}
