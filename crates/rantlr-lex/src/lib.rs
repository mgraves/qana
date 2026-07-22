//! P0 spike for the rantlr envelope: line-anchored incremental lexing.
//!
//! Demonstrates, with measurable guarantees rather than heuristics:
//!
//! * **L1 — line-local tokens.** `lex_line` is a pure, total function of
//!   `(line text, entry state)`; no token can span a line break by
//!   construction.
//! * **L2 — finite line-start state.** The state carried across a line
//!   boundary is [`LineState`]: `Normal` or `BlockComment(depth ≤ 8)`.
//!   Multi-line constructs are represented in that state, not by
//!   line-spanning tokens.
//! * **Losslessness.** Every byte of the source belongs to exactly one
//!   token (trivia included; unrecognized bytes become `Unknown` trivia),
//!   so `reproduce()` rebuilds the input byte-for-byte. Tokens store
//!   lengths, not offsets, so unchanged lines relocate for free.
//! * **Incrementality with a batch oracle.** `apply_edits` applies a batch
//!   of scattered edits in one pass and relexes only damaged lines plus a
//!   reconvergence run per site (the VS Code line-state contract, made
//!   explicit). Tests assert the result is identical to lexing from
//!   scratch — the differential gate the full system will keep forever.
//! * **Containment preview.** `build_skeleton` derives the block structure
//!   (folding/outline candidates) from bracket tokens with local,
//!   non-cascading handling of imbalance.

/// Nested block comments deeper than this are a lint error in the real
/// tool (envelope L2: the line-start state must be drawn from a small
/// finite set). The spike saturates at the cap.
pub const MAX_COMMENT_NEST: u8 = 8;

/// State carried across a line boundary. Small, finite, `Eq` — the whole
/// point. Reconvergence testing is `==` on this type.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum LineState {
    #[default]
    Normal,
    /// Inside a block comment at the given nesting depth (1..=MAX_COMMENT_NEST).
    BlockComment(u8),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TokenKind {
    // ---- trivia ----
    Whitespace,
    LineComment,
    BlockComment,
    /// Bytes the demo language doesn't know. Trivia, so the token layer is
    /// total and losslessness holds for arbitrary input.
    Unknown,
    // ---- real tokens ----
    Ident,
    Keyword,
    Number,
    Str { terminated: bool },
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Punct,
}

impl TokenKind {
    pub fn is_trivia(self) -> bool {
        matches!(
            self,
            TokenKind::Whitespace
                | TokenKind::LineComment
                | TokenKind::BlockComment
                | TokenKind::Unknown
        )
    }
}

/// A token is a kind and a byte length. No positions: offsets are derived
/// by prefix sums, which is what makes unchanged lines reusable without
/// patching (the green-tree trick, applied to the token layer).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Token {
    pub kind: TokenKind,
    pub len: u32,
}

/// Line terminator, preserved exactly so mixed-ending files round-trip.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LineTerm {
    Lf,
    CrLf,
    Cr,
    /// Final line without a terminator.
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

/// One source line: text without its terminator, plus the exact terminator.
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

/// Exact line splitting: `join_lines(&split_lines(s)) == s` for every `s`.
/// `\r\n`, `\n`, and lone `\r` are all terminators; an empty input is one
/// empty unterminated line.
pub fn split_lines(src: &str) -> Vec<Line> {
    let bytes = src.as_bytes();
    let mut lines = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
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
    // Trailing content without terminator — or the empty file as one empty line.
    lines.push(Line::new(&src[start..], LineTerm::None));
    lines
}

pub fn join_lines(lines: &[Line]) -> String {
    let cap: usize = lines
        .iter()
        .map(|l| l.text.len() + l.term.as_str().len())
        .sum();
    let mut out = String::with_capacity(cap);
    for l in lines {
        out.push_str(&l.text);
        out.push_str(l.term.as_str());
    }
    out
}

const KEYWORDS: &[&str] = &[
    "fn", "let", "if", "else", "while", "for", "return", "struct", "enum",
    "impl", "match", "true", "false", "mut", "pub", "use",
];

/// Lex one line. Pure and total: consumes every byte of `text`, never
/// looks at any other line, and the only cross-line channel is the
/// returned exit [`LineState`].
///
/// Invariant (debug-asserted, property-tested): sum of token lengths
/// equals `text.len()`.
pub fn lex_line(text: &str, entry: LineState) -> (Vec<Token>, LineState) {
    let bytes = text.as_bytes();
    let n = bytes.len();
    let mut tokens = Vec::new();
    let mut i = 0usize;
    let mut state = entry;

    let push = |tokens: &mut Vec<Token>, kind: TokenKind, from: usize, to: usize| {
        debug_assert!(to > from, "empty token {kind:?}");
        tokens.push(Token { kind, len: (to - from) as u32 });
    };

    // Resume an open block comment from a previous line.
    if let LineState::BlockComment(depth0) = state {
        let start = i;
        let mut depth = depth0;
        while i < n {
            if bytes[i] == b'*' && i + 1 < n && bytes[i + 1] == b'/' {
                i += 2;
                depth -= 1;
                if depth == 0 {
                    break;
                }
            } else if bytes[i] == b'/' && i + 1 < n && bytes[i + 1] == b'*' {
                i += 2;
                depth = depth.saturating_add(1).min(MAX_COMMENT_NEST);
            } else {
                i += 1;
            }
        }
        if i > start {
            push(&mut tokens, TokenKind::BlockComment, start, i);
        }
        state = if depth == 0 {
            LineState::Normal
        } else {
            LineState::BlockComment(depth)
        };
        if state != LineState::Normal {
            // Whole line consumed inside the comment.
            debug_assert_eq!(i, n);
            return (tokens, state);
        }
    }

    while i < n {
        let c = text[i..].chars().next().unwrap();
        let start = i;
        match c {
            c if c == ' ' || c == '\t' || c.is_whitespace() => {
                while i < n {
                    let d = text[i..].chars().next().unwrap();
                    if d == ' ' || d == '\t' || d.is_whitespace() {
                        i += d.len_utf8();
                    } else {
                        break;
                    }
                }
                push(&mut tokens, TokenKind::Whitespace, start, i);
            }
            '/' if i + 1 < n && bytes[i + 1] == b'/' => {
                i = n;
                push(&mut tokens, TokenKind::LineComment, start, i);
            }
            '/' if i + 1 < n && bytes[i + 1] == b'*' => {
                i += 2;
                let mut depth: u8 = 1;
                while i < n && depth > 0 {
                    if bytes[i] == b'*' && i + 1 < n && bytes[i + 1] == b'/' {
                        i += 2;
                        depth -= 1;
                    } else if bytes[i] == b'/' && i + 1 < n && bytes[i + 1] == b'*' {
                        i += 2;
                        depth = depth.saturating_add(1).min(MAX_COMMENT_NEST);
                    } else {
                        i += 1;
                    }
                }
                push(&mut tokens, TokenKind::BlockComment, start, i);
                if depth > 0 {
                    // Open at end of line: carried in the line-start state,
                    // NOT in a line-spanning token (L1 + L2).
                    return (tokens, LineState::BlockComment(depth));
                }
            }
            '"' => {
                i += 1;
                let mut terminated = false;
                while i < n {
                    let d = text[i..].chars().next().unwrap();
                    if d == '\\' {
                        i += 1;
                        if i < n {
                            i += text[i..].chars().next().unwrap().len_utf8();
                        }
                    } else if d == '"' {
                        i += 1;
                        terminated = true;
                        break;
                    } else {
                        i += d.len_utf8();
                    }
                }
                // Design choice with containment consequences: strings are
                // single-line, so an unterminated string is an error token on
                // THIS line and the exit state is Normal — the next line is
                // untouched. No cascade.
                push(&mut tokens, TokenKind::Str { terminated }, start, i);
            }
            c if c.is_ascii_digit() => {
                while i < n && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                if i + 1 < n && bytes[i] == b'.' && bytes[i + 1].is_ascii_digit() {
                    i += 1;
                    while i < n && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                }
                push(&mut tokens, TokenKind::Number, start, i);
            }
            c if c.is_alphabetic() || c == '_' => {
                while i < n {
                    let d = text[i..].chars().next().unwrap();
                    if d.is_alphanumeric() || d == '_' {
                        i += d.len_utf8();
                    } else {
                        break;
                    }
                }
                let kind = if KEYWORDS.contains(&&text[start..i]) {
                    TokenKind::Keyword
                } else {
                    TokenKind::Ident
                };
                push(&mut tokens, kind, start, i);
            }
            '(' | ')' | '[' | ']' | '{' | '}' => {
                let kind = match c {
                    '(' => TokenKind::LParen,
                    ')' => TokenKind::RParen,
                    '[' => TokenKind::LBracket,
                    ']' => TokenKind::RBracket,
                    '{' => TokenKind::LBrace,
                    _ => TokenKind::RBrace,
                };
                i += 1;
                push(&mut tokens, kind, start, i);
            }
            c if c.is_ascii_punctuation() => {
                i += 1;
                push(&mut tokens, TokenKind::Punct, start, i);
            }
            c => {
                // Total coverage: anything else is Unknown trivia.
                i += c.len_utf8();
                push(&mut tokens, TokenKind::Unknown, start, i);
            }
        }
    }

    debug_assert_eq!(
        tokens.iter().map(|t| t.len as usize).sum::<usize>(),
        n,
        "lossless invariant: token lengths must cover the line exactly"
    );
    (tokens, LineState::Normal)
}

/// Tokens + exit state for one line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineTokens {
    pub tokens: Vec<Token>,
    pub exit: LineState,
}

/// A line-range edit: replace lines `start..end` with `replacement`.
/// (`start == end` is a pure insertion.) LSP `didChange` ranges map onto
/// this by expanding to whole lines — the token damage is identical
/// because tokens are line-local.
#[derive(Clone, Debug)]
pub struct LineEdit {
    pub start: usize,
    pub end: usize,
    pub replacement: Vec<Line>,
}

/// Damage accounting for one `apply_edits` call — the numbers the
/// envelope's lexing contract is stated in.
#[derive(Clone, Copy, Debug, Default)]
pub struct DamageReport {
    pub sites: usize,
    /// Lines that were textually replaced/inserted (must be relexed).
    pub replaced_lines: usize,
    /// Total lines relexed, including reconvergence runs.
    pub relexed_lines: usize,
    /// Carried lines relexed only to re-establish state agreement
    /// (`relexed - replaced`): the measured Σ Rᵢ.
    pub reconverged_extra: usize,
}

/// The lexed buffer: lines + per-line tokens/exit states, kept in lockstep.
pub struct LexedBuffer {
    pub lines: Vec<Line>,
    pub lexed: Vec<LineTokens>,
}

impl LexedBuffer {
    /// Cold (from-scratch) lex. Also the differential oracle.
    pub fn new(src: &str) -> Self {
        let lines = split_lines(src);
        let mut lexed = Vec::with_capacity(lines.len());
        let mut state = LineState::Normal;
        for line in &lines {
            let (tokens, exit) = lex_line(&line.text, state);
            lexed.push(LineTokens { tokens, exit });
            state = exit;
        }
        LexedBuffer { lines, lexed }
    }

    /// Byte-exact reproduction of the current text — the losslessness
    /// invariant, checked against the token layer.
    pub fn reproduce(&self) -> String {
        join_lines(&self.lines)
    }

    /// Verify that every line's tokens exactly cover its text.
    /// (Cheap enough to run in tests after every operation.)
    pub fn verify_coverage(&self) -> bool {
        self.lines.iter().zip(&self.lexed).all(|(l, t)| {
            t.tokens.iter().map(|t| t.len as usize).sum::<usize>() == l.text.len()
        })
    }

    pub fn entry_state(&self, line: usize) -> LineState {
        if line == 0 {
            LineState::Normal
        } else {
            self.lexed[line - 1].exit
        }
    }

    /// Apply a batch of non-overlapping, ascending line edits in one merge
    /// pass, then relex all damaged regions with per-site reconvergence.
    ///
    /// Reconvergence rule (the L1/L2 theorem in executable form): after
    /// relexing the replaced lines of a site, keep relexing carried lines
    /// while the freshly computed entry state differs from the entry state
    /// that line had before the edit; stop the moment they agree, because
    /// every subsequent line's tokenization is a pure function of
    /// (unchanged text, now-agreed entry state).
    pub fn apply_edits(&mut self, edits: &[LineEdit]) -> DamageReport {
        // -- validate batch shape --
        for w in edits.windows(2) {
            assert!(w[0].end <= w[1].start, "edits must be ascending and non-overlapping");
        }
        for e in edits {
            assert!(e.start <= e.end && e.end <= self.lines.len(), "edit out of range");
        }

        let mut report = DamageReport { sites: edits.len(), ..Default::default() };

        struct Gap {
            /// First line of the replacement, in NEW coordinates.
            new_start: usize,
            /// One past the last replacement line, in NEW coordinates.
            new_end: usize,
            /// Exit state of the OLD line immediately preceding the first
            /// carried line after this gap (= the old entry state of that
            /// carried line). `Normal` at buffer start.
            old_exit_before_carry: LineState,
        }

        let same_shape = edits.iter().all(|e| e.replacement.len() == e.end - e.start);
        let gaps: Vec<Gap> = if same_shape {
            // Fast path: line counts unchanged, so patch lines in place —
            // no buffer rebuild at all. This is the single-keystroke path.
            // (A rope/piece-tree text layer makes the general path equally
            // cheap; the spike's Vec storage makes the difference visible.)
            edits
                .iter()
                .map(|e| {
                    let old_exit_before_carry = if e.end > e.start {
                        self.lexed[e.end - 1].exit
                    } else if e.start > 0 {
                        self.lexed[e.start - 1].exit
                    } else {
                        LineState::Normal
                    };
                    for (k, l) in e.replacement.iter().enumerate() {
                        self.lines[e.start + k] = l.clone();
                    }
                    report.replaced_lines += e.replacement.len();
                    Gap { new_start: e.start, new_end: e.end, old_exit_before_carry }
                })
                .collect()
        } else {
            // General path: one-pass merge into fresh vectors, carrying old
            // lex results for untouched lines.
            let old_lines = std::mem::take(&mut self.lines);
            let old_lexed = std::mem::take(&mut self.lexed);
            let mut gaps: Vec<Gap> = Vec::with_capacity(edits.len());
            let mut new_lines: Vec<Line> = Vec::with_capacity(old_lines.len());
            let mut new_lexed: Vec<LineTokens> = Vec::with_capacity(old_lexed.len());

            let placeholder = || LineTokens { tokens: Vec::new(), exit: LineState::Normal };

            // Exit state of the last OLD line consumed (carried or dropped).
            // For any gap, this is exactly the old entry state of the first
            // carried line that follows it — including tricky shapes like an
            // insertion immediately after another edit's replacement.
            let mut prev_old_exit = LineState::Normal;
            let mut old_l = old_lines.into_iter();
            let mut old_t = old_lexed.into_iter();
            let mut cursor = 0usize; // old-coordinate scan position

            for e in edits {
                // Carry the untouched span before this edit.
                for _ in cursor..e.start {
                    let l = old_l.next().expect("line underflow");
                    let t = old_t.next().expect("lex underflow");
                    prev_old_exit = t.exit;
                    new_lines.push(l);
                    new_lexed.push(t);
                }
                // Drop the replaced span.
                for _ in e.start..e.end {
                    let _ = old_l.next().expect("line underflow");
                    prev_old_exit = old_t.next().expect("lex underflow").exit;
                }
                let new_start = new_lines.len();
                for l in &e.replacement {
                    new_lines.push(l.clone());
                    new_lexed.push(placeholder());
                }
                report.replaced_lines += e.replacement.len();
                gaps.push(Gap { new_start, new_end: new_lines.len(), old_exit_before_carry: prev_old_exit });
                cursor = e.end;
            }
            // Trailing carry.
            for l in old_l {
                new_lines.push(l);
                new_lexed.push(old_t.next().expect("lex underflow"));
            }
            self.lines = new_lines;
            self.lexed = new_lexed;
            gaps
        };

        // Canonical form: exactly the final line is unterminated, and no
        // lone-CR terminator is immediately followed by a line that joins
        // starting with '\n' (which would fuse into a CRLF on re-split).
        // Edits that violate either are ill-formed at this layer; the
        // char-range edit mapping above this API preserves both by
        // construction, because terminators are part of the char stream.
        debug_assert!(
            self.lines
                .iter()
                .enumerate()
                .all(|(i, l)| (l.term == LineTerm::None) == (i + 1 == self.lines.len())),
            "canonical line-term invariant violated by edit batch"
        );
        debug_assert!(
            self.lines.windows(2).all(|w| {
                !(w[0].term == LineTerm::Cr && w[1].text.is_empty() && w[1].term == LineTerm::Lf)
            }),
            "CR + \\n seam created by edit batch (would fuse into CRLF on re-split)"
        );

        // -- relex each damaged region with reconvergence --
        // Processing ascending means everything before gap.new_start is settled.
        let mut settled_until = 0usize; // lines below this are final
        for (gi, gap) in gaps.iter().enumerate() {
            // A previous site's reconvergence run stops no later than this
            // gap's start (it breaks on reaching the next gap), so
            // settled_until <= gap.new_start always; max() keeps that
            // explicit.
            let mut i = gap.new_start.max(settled_until);
            let mut st = self.entry_state(i);

            // Phase 1: replaced lines always relex.
            while i < gap.new_end {
                let (tokens, exit) = lex_line(&self.lines[i].text, st);
                st = exit;
                self.lexed[i] = LineTokens { tokens, exit };
                report.relexed_lines += 1;
                i += 1;
            }

            // Phase 2: reconvergence over carried lines. `expected_old_entry`
            // is what this carried line's entry state used to be; agreement
            // means every line below is already correct.
            let mut expected_old_entry = gap.old_exit_before_carry;
            let next_gap_start = gaps.get(gi + 1).map(|g| g.new_start).unwrap_or(usize::MAX);
            while i < self.lines.len() {
                if st == expected_old_entry {
                    break; // states agree: everything below is untouched
                }
                if i >= next_gap_start {
                    // Ran into the next edit site; its own pass continues.
                    break;
                }
                let old_exit = self.lexed[i].exit; // save before overwrite
                let (tokens, exit) = lex_line(&self.lines[i].text, st);
                st = exit;
                self.lexed[i] = LineTokens { tokens, exit };
                report.relexed_lines += 1;
                report.reconverged_extra += 1;
                expected_old_entry = old_exit;
                i += 1;
            }
            settled_until = i;
        }

        report
    }
}

// ---------------------------------------------------------------------------
// Block skeleton: containment preview
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BracketKind {
    Paren,
    Bracket,
    Brace,
}

/// A block in the skeleton. `close == None` means unclosed at EOF —
/// recorded locally, never cascading into sibling blocks.
#[derive(Clone, Debug)]
pub struct Block {
    pub kind: BracketKind,
    pub open: (usize, usize),          // (line, token index)
    pub close: Option<(usize, usize)>, // matched close, if any
    pub depth: u32,
}

#[derive(Clone, Debug, Default)]
pub struct Skeleton {
    pub blocks: Vec<Block>,
    pub max_depth: u32,
    pub unmatched_closes: usize,
}

impl Skeleton {
    /// Folding candidates: blocks spanning more than one line.
    pub fn folding_ranges(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        self.blocks.iter().filter_map(|b| {
            let (ol, _) = b.open;
            let (cl, _) = b.close?;
            (cl > ol).then_some((ol, cl))
        })
    }
}

/// Single linear pass over the token runs. Imbalance is handled locally:
/// a stray close is recorded and skipped (no stack unwind → no cascade);
/// unclosed opens simply never get a `close`.
pub fn build_skeleton(buf: &LexedBuffer) -> Skeleton {
    let mut sk = Skeleton::default();
    let mut stack: Vec<usize> = Vec::new(); // indices into sk.blocks
    for (li, lt) in buf.lexed.iter().enumerate() {
        for (ti, tok) in lt.tokens.iter().enumerate() {
            let open = match tok.kind {
                TokenKind::LParen => Some(BracketKind::Paren),
                TokenKind::LBracket => Some(BracketKind::Bracket),
                TokenKind::LBrace => Some(BracketKind::Brace),
                _ => None,
            };
            if let Some(kind) = open {
                let depth = stack.len() as u32 + 1;
                sk.max_depth = sk.max_depth.max(depth);
                stack.push(sk.blocks.len());
                sk.blocks.push(Block { kind, open: (li, ti), close: None, depth });
                continue;
            }
            let close = match tok.kind {
                TokenKind::RParen => Some(BracketKind::Paren),
                TokenKind::RBracket => Some(BracketKind::Bracket),
                TokenKind::RBrace => Some(BracketKind::Brace),
                _ => None,
            };
            if let Some(kind) = close {
                match stack.last() {
                    Some(&bi) if sk.blocks[bi].kind == kind => {
                        sk.blocks[bi].close = Some((li, ti));
                        stack.pop();
                    }
                    _ => sk.unmatched_closes += 1, // local error, no unwind
                }
            }
        }
    }
    sk
}
