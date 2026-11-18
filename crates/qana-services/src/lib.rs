//! P3 increment 1: grammar-derived editor services over the incremental
//! session. Everything here is computed from artifacts the toolchain
//! already generates — token styles from the vocab and the tree,
//! folding and the outline from the typed tree,
//! completion straight off the LR ACTION rows, diagnostics from repairs.
//! The functions are LSP-shaped (semantic-token quintuples with delta
//! edits, line/col ranges) but transport-free; the LSP server binary is
//! the next increment's plumbing.

pub mod paint;
pub mod syntax;

pub use syntax::syntax_classes;
use syntax::SyntaxLines;

use qana_engine::{DamageReport, IncSession, LexedBuffer};
use qana_grammar::green::{ancestor_spans, ERROR_NT};
use qana_grammar::incremental::{Repair, RepairKind};
use qana_grammar::{
    CompiledLexer, GreenChild, GreenNode, GreenToken, LrAct, LrTables, SynGrammar, TokenId, EOF,
};
use std::collections::HashMap;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Semantic tokens (tier 0: syntactic, from the token runs)
// ---------------------------------------------------------------------------

/// The canonical style legend: a CLOSED set of classes shared by every
/// grammar the server hosts (editor themes and the semantic-token
/// protocol need one stable legend). Grammar-declared styles must pick
/// from it — an unknown class is a compile diagnostic, not a surprise.
///
/// A class is given two ways. A token's `@style(class)` gives it to every
/// token of that kind, wherever it stands (keywords, literals, comments).
/// An alternative's `@style(class, label…)` gives it to the symbols it
/// labels, by where they stand in the tree: a name is a `type` in a type
/// and a `function` where it is called (see [`syntax`]). The classes are
/// LSP's semantic token types where LSP has one; [`Styles::lsp_encode`]
/// says how each is sent.
///
/// New classes are added at the end, so a class keeps its index.
pub const LEGEND: &[&str] = &[
    "keyword",
    "variable",
    "number",
    "string",
    "comment",
    "operator",
    "punctuation",
    "bracket",
    "regexp",
    // A name that denotes a type: a type's own name where it is defined,
    // and the name a path in a type position ends in.
    "type",
    // A function's name where it is defined, and the name a call calls.
    "function",
    // The name a method call calls (`x.name()`).
    "method",
    // A macro's name where it is invoked or defined.
    "macro",
    // An attribute or annotation (`#[derive(Debug)]`).
    "decorator",
    // A lifetime (`'a`).
    "lifetime",
    // The label a loop or a block is given, and a `break` names.
    "label",
    // A comment that documents what follows it (`///`, `/** */`).
    "doc_comment",
];

/// The token modifiers of the LSP legend, in order: bit `i` of a token's
/// modifier set is `LSP_MODIFIERS[i]`.
pub const LSP_MODIFIERS: &[&str] = &["documentation"];

/// How LSP is sent a class that is not a token type of its own name: a
/// doc comment is LSP's `comment` with the `documentation` modifier.
/// Every other class is sent as the type of its name: LSP's standard
/// types, and the ones qana adds (`punctuation`, `bracket`, `lifetime`),
/// which a client declares (the VS Code extension does, with the standard
/// type each falls back to).
const LSP_ENCODING: &[(&str, &str, u32)] = &[("doc_comment", "comment", 1 << 0)];

fn lsp_encoding_of(class: &'static str) -> (&'static str, u32) {
    LSP_ENCODING
        .iter()
        .find(|(c, _, _)| *c == class)
        .map(|&(_, ty, mods)| (ty, mods))
        .unwrap_or((class, 0))
}

/// A symbol its production gives no class.
const ROUTE_NONE: u8 = u8::MAX;
/// A symbol painted the class its production's node is given.
const ROUTE_INHERIT: u8 = u8::MAX - 1;

/// A language's styles: the class each kind of token is painted
/// (`@style` on a token), the classes its syntax gives (`@style` on an
/// alternative), and the legend both index into.
#[derive(Clone, Debug)]
pub struct Styles {
    pub legend: Vec<&'static str>,
    map: HashMap<TokenId, u32>,
    /// By production, by symbol position: the class an alternative gives
    /// the symbol (an index into the legend), [`ROUTE_INHERIT`] for the
    /// class the production's node is given, or [`ROUTE_NONE`]. `None`
    /// for a production that gives none.
    syntax: Vec<Option<Box<[u8]>>>,
    /// By class: its LSP token type (an index into `lsp_types`) and its
    /// modifier bits.
    lsp: Vec<(u32, u32)>,
    lsp_types: Vec<&'static str>,
}

impl Styles {
    pub fn new(legend: Vec<&'static str>) -> Self {
        assert!(legend.len() < ROUTE_INHERIT as usize, "a legend has at most 253 classes");
        let mut lsp_types: Vec<&'static str> = Vec::new();
        let lsp = legend
            .iter()
            .map(|&class| {
                let (ty, mods) = lsp_encoding_of(class);
                let at = match lsp_types.iter().position(|t| *t == ty) {
                    Some(at) => at,
                    None => {
                        lsp_types.push(ty);
                        lsp_types.len() - 1
                    }
                };
                (at as u32, mods)
            })
            .collect();
        Styles { legend, map: HashMap::new(), syntax: Vec::new(), lsp, lsp_types }
    }

    /// The index of `class` in the legend.
    ///
    /// # Panics
    /// When the legend has no such class.
    pub fn class_index(&self, class: &str) -> u32 {
        self.legend
            .iter()
            .position(|c| *c == class)
            .expect("class must be in the legend") as u32
    }

    pub fn set(&mut self, id: TokenId, class: &'static str) {
        let idx = self.class_index(class);
        self.map.insert(id, idx);
    }

    pub fn class_of(&self, id: TokenId) -> Option<u32> {
        self.map.get(&id).copied()
    }

    /// The symbol at `position` of production `prod` is painted `class`,
    /// wherever the production stands (`@style(class, label)` on an
    /// alternative). A token there takes the class; a rule there is given
    /// it, and its own alternatives say which of its symbols take it (see
    /// [`Styles::pass_style`]).
    pub fn paint_symbol(&mut self, prod: u16, position: usize, class: &'static str) {
        let idx = self.class_index(class) as u8;
        self.set_route(prod, position, idx);
    }

    /// The symbol at `position` of production `prod` is painted the class
    /// the production's node is given, when it is given one
    /// (`@style(inherit, label)`): how a path hands the class it is given
    /// to the segment that names it.
    pub fn pass_style(&mut self, prod: u16, position: usize) {
        self.set_route(prod, position, ROUTE_INHERIT);
    }

    fn set_route(&mut self, prod: u16, position: usize, route: u8) {
        let prod = prod as usize;
        if self.syntax.len() <= prod {
            self.syntax.resize(prod + 1, None);
        }
        let routes = self.syntax[prod].get_or_insert_with(Box::default);
        if routes.len() <= position {
            let mut grown = routes.to_vec();
            grown.resize(position + 1, ROUTE_NONE);
            *routes = grown.into_boxed_slice();
        }
        routes[position] = route;
    }

    /// Whether the syntax gives any class. A language whose classes are
    /// all its tokens' is painted without reading its tree.
    pub fn has_syntax(&self) -> bool {
        self.syntax.iter().flatten().any(|r| r.iter().any(|&x| x != ROUTE_NONE))
    }

    /// The class the symbol at `position` of `prod` is painted, when the
    /// production's node is given `given`: an index into the legend, or
    /// `None` where the syntax gives it none.
    pub(crate) fn route(&self, prod: u16, position: usize, given: Option<u8>) -> Option<u8> {
        match self.syntax.get(prod as usize)?.as_ref()?.get(position).copied()? {
            ROUTE_NONE => None,
            ROUTE_INHERIT => given,
            class => Some(class),
        }
    }

    /// The token types of the LSP legend, in order: what
    /// [`Styles::lsp_encode`] sends each class as.
    pub fn lsp_types(&self) -> &[&'static str] {
        &self.lsp_types
    }

    /// How LSP is sent `class`: its token type (an index into
    /// [`Styles::lsp_types`]) and its modifier bits (over
    /// [`LSP_MODIFIERS`]).
    pub fn lsp_encode(&self, class: u32) -> (u32, u32) {
        self.lsp[class as usize]
    }
}

/// LSP `SemanticTokens.data`: flat quintuples
/// (deltaLine, deltaStart, length, tokenType, tokenModifiers), the type
/// and modifiers as [`Styles::lsp_encode`] sends each class.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SemanticTokens {
    pub data: Vec<u32>,
    /// Styled-token quintuple count per buffer line (delta bookkeeping).
    per_line: Vec<u32>,
    /// What the syntax paints, kept across edits; `None` for a language
    /// whose classes are all its tokens' ([`Styles::has_syntax`]).
    syntax: Option<SyntaxLines>,
}

/// The styled tokens of line `li`, as (column, length, class): each the
/// class its syntax gives it, and failing that its kind's.
fn styled_line(
    buf: &LexedBuffer<'_, CompiledLexer>,
    li: usize,
    styles: &Styles,
    syntax: Option<&SyntaxLines>,
) -> Vec<(u32, u32, u32)> {
    let painted = syntax.map(|s| s.line(li)).unwrap_or(&[]);
    let mut out = Vec::new();
    let mut next = 0usize;
    let mut col = 0u32;
    for tok in &buf.lexed[li].tokens {
        let class = match painted.get(next) {
            Some(&(at, len, class)) if at == col => {
                debug_assert_eq!(len, tok.len, "the syntax paints whole tokens");
                next += 1;
                Some(class as u32)
            }
            _ => styles.class_of(tok.id),
        };
        if let Some(class) = class {
            out.push((col, tok.len, class));
        }
        col += tok.len;
    }
    debug_assert_eq!(next, painted.len(), "every token the syntax paints is a token of its line");
    out
}

pub fn semantic_tokens_full(
    lexer: &CompiledLexer,
    buf: &LexedBuffer<'_, CompiledLexer>,
    tree: &Arc<GreenNode>,
    styles: &Styles,
) -> SemanticTokens {
    let _ = lexer;
    let syntax = styles.has_syntax().then(|| SyntaxLines::new(tree, styles, buf.lines.len()));
    let mut data = Vec::new();
    let mut per_line = Vec::with_capacity(buf.lines.len());
    let mut prev_line = 0u32;
    let mut prev_start = 0u32;
    for li in 0..buf.lexed.len() {
        let styled = styled_line(buf, li, styles, syntax.as_ref());
        for &(col, len, class) in &styled {
            let line = li as u32;
            let delta_line = line - prev_line;
            let delta_start = if delta_line == 0 { col - prev_start } else { col };
            let (ty, mods) = styles.lsp_encode(class);
            data.extend_from_slice(&[delta_line, delta_start, len, ty, mods]);
            prev_line = line;
            prev_start = col;
        }
        per_line.push(styled.len() as u32);
    }
    SemanticTokens { data, per_line, syntax }
}

/// An LSP `SemanticTokensEdit`: splice `insert` over
/// `data[start..start + delete_count]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SemDelta {
    pub start: u32,
    pub delete_count: u32,
    pub insert: Vec<u32>,
}

impl SemanticTokens {
    /// Recompute after an edit batch: damaged line ranges are re-encoded
    /// and spliced; the first styled token AFTER the damage gets its
    /// deltaLine re-anchored. A line the edit did not touch whose classes
    /// its syntax changed (typing `(` after a name makes it a call) is
    /// re-encoded with them: the span spliced reaches it. Returns the
    /// LSP-shaped edit.
    pub fn update(
        &mut self,
        _lexer: &CompiledLexer,
        buf: &LexedBuffer<'_, CompiledLexer>,
        tree: &Arc<GreenNode>,
        styles: &Styles,
        damage: &DamageReport,
    ) -> Option<SemDelta> {
        if damage.regions.is_empty() {
            return None;
        }
        // Spanning region (regions are ascending; a single splice keeps
        // the delta simple — per-region splices are a refinement).
        let old_lo = damage.regions.iter().map(|r| r.old_lines.0).min().unwrap();
        let mut old_hi = damage.regions.iter().map(|r| r.old_lines.1).max().unwrap();
        let new_lo = damage.regions.iter().map(|r| r.new_lines.0).min().unwrap();
        let mut new_hi = damage.regions.iter().map(|r| r.new_lines.1).max().unwrap();
        debug_assert_eq!(old_lo, new_lo, "regions align at their start");
        let changed = match &mut self.syntax {
            Some(syntax) => syntax.update(tree, styles, Some((old_lo, old_hi, new_lo, new_hi))),
            None => Vec::new(),
        };
        // The span reaches the lines whose classes changed outside the
        // damage. A line before it has one number in both revisions; one
        // after it is moved by the damage's change in line count.
        let mut new_lo = new_lo;
        if let Some(&first) = changed.first() {
            new_lo = new_lo.min(first);
        }
        if let Some(&last) = changed.last() {
            if last >= new_hi {
                let reach = last + 1 - new_hi;
                new_hi += reach;
                old_hi += reach;
            }
        }
        let old_lo = new_lo;
        let syntax = self.syntax.as_ref();

        // Quintuple index where the damaged span starts, in OLD data.
        let q_start: u32 = self.per_line[..old_lo].iter().sum::<u32>() * 5;
        let q_deleted: u32 = self.per_line[old_lo..old_hi].iter().sum::<u32>() * 5;
        // One more styled token after the span re-anchors its deltaLine
        // (its absolute line may have shifted with the line-count delta).
        let has_after = self.per_line[old_hi..].iter().any(|&c| c > 0);
        let extra = u32::from(has_after) * 5;

        // Encode replacement lines (and the re-anchored follower).
        let mut insert = Vec::new();
        // Absolute position of the styled token PRECEDING the span: the
        // last one on the nearest line before it that has one (lines
        // before the span are as they were).
        let (mut prev_line, mut prev_start) = (0..new_lo)
            .rev()
            .find(|&li| self.per_line[li] > 0)
            .map(|li| {
                let last = styled_line(buf, li, styles, syntax).last().map_or(0, |s| s.0);
                (li as u32, last)
            })
            .unwrap_or((0, 0));
        let mut new_counts: Vec<u32> = Vec::with_capacity(new_hi - new_lo);
        for li in new_lo..new_hi {
            let styled = styled_line(buf, li, styles, syntax);
            for &(col, len, class) in &styled {
                let line = li as u32;
                let delta_line = line - prev_line;
                let delta_start = if delta_line == 0 { col - prev_start } else { col };
                let (ty, mods) = styles.lsp_encode(class);
                insert.extend_from_slice(&[delta_line, delta_start, len, ty, mods]);
                prev_line = line;
                prev_start = col;
            }
            new_counts.push(styled.len() as u32);
        }
        // Re-anchor the follower (first styled token after the span).
        if has_after {
            let old_after_idx = (q_start + q_deleted) as usize;
            let mut follower: [u32; 5] = self.data[old_after_idx..old_after_idx + 5]
                .try_into()
                .unwrap();
            // Its absolute line in NEW coordinates.
            let mut abs_line_old = 0u32;
            let mut seen = 0u32;
            for (li, &c) in self.per_line.iter().enumerate() {
                seen += c * 5;
                if seen > q_start + q_deleted {
                    abs_line_old = li as u32;
                    break;
                }
            }
            let delta_lines = new_hi as i64 - old_hi as i64;
            let abs_line_new = (abs_line_old as i64 + delta_lines) as u32;
            // Its column: the first styled token of its line (it is the
            // first after the span, and the span ends with a line).
            let abs_start = styled_line(buf, abs_line_new as usize, styles, syntax)
                .first()
                .map_or(0, |s| s.0);
            follower[0] = abs_line_new - prev_line;
            follower[1] = if follower[0] == 0 { abs_start - prev_start } else { abs_start };
            insert.extend_from_slice(&follower);
        }

        // Apply the splice to our copy.
        let start = q_start as usize;
        let del = (q_deleted + extra) as usize;
        self.data.splice(start..start + del, insert.iter().copied());
        self.per_line.splice(old_lo..old_hi, new_counts);
        Some(SemDelta { start: q_start, delete_count: q_deleted + extra, insert })
    }
}
// ---------------------------------------------------------------------------
// Folding
// ---------------------------------------------------------------------------
//
// A document folds where its syntax says it does: the constructs its
// grammar marks `@fold`, wherever one runs over lines. Comments are not
// syntax (they are trivia, kept in the tree but skipped by the parser),
// so they fold by their tokens: a comment that runs over lines, and two
// or more lines in a row holding nothing but comments.

/// What folds in a language's documents: the constructs its grammar
/// declares foldable (`@fold`) and its comments.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FoldConfig {
    /// By production index: the fold its constructs make, if any.
    pub by_prod: Vec<Option<FoldEntry>>,
    /// Category names, indexed by [`Fold::category`]: the grammar's fold
    /// kinds in the order it first uses them, then `comment` if it does
    /// not.
    pub legend: Vec<String>,
    /// The category of comment folds.
    pub comment: u8,
    /// By token id: whether it is a comment (a trivia token styled
    /// `comment` or `doc_comment`).
    pub comments: Vec<bool>,
    /// By token id: whether the lexer reads it in a mode other than the
    /// base mode (`Vocab::modes`), within a construct of tokens of its
    /// own such as a block comment's body.
    pub in_mode: Vec<bool>,
}

/// The fold a foldable construct makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FoldEntry {
    pub category: u8,
    /// The symbol position the fold begins at (a construct's body, its
    /// opening bracket); `None` for the construct's start.
    pub from: Option<usize>,
    /// The symbol position it ends at (a closing bracket with more of the
    /// construct after it); `None` for the construct's end.
    pub to: Option<usize>,
}

impl FoldConfig {
    /// The fold configuration of a language: `kinds`, its fold kinds in
    /// the order it first uses them; `entries`, its foldable productions
    /// (the production, the index of its kind, and the symbol positions
    /// its fold begins and ends at) among `prods`. Its comments are the
    /// trivia tokens of `vocab` that `styles` classes `comment` or
    /// `doc_comment`.
    pub fn new(
        kinds: Vec<String>,
        entries: &[(u16, u8, Option<usize>, Option<usize>)],
        prods: usize,
        vocab: &qana_grammar::Vocab,
        styles: &Styles,
    ) -> FoldConfig {
        let mut legend = kinds;
        let comment = match legend.iter().position(|k| k == "comment") {
            Some(i) => i,
            None => {
                legend.push("comment".to_string());
                legend.len() - 1
            }
        } as u8;
        let mut by_prod: Vec<Option<FoldEntry>> = vec![None; prods];
        for &(prod, category, from, to) in entries {
            by_prod[prod as usize] = Some(FoldEntry { category, from, to });
        }
        // Doc comments are comments.
        let comment_classes: Vec<u32> = styles
            .legend
            .iter()
            .enumerate()
            .filter(|(_, c)| matches!(**c, "comment" | "doc_comment"))
            .map(|(i, _)| i as u32)
            .collect();
        let ids = 0..vocab.names.len() as TokenId;
        FoldConfig {
            by_prod,
            legend,
            comment,
            comments: ids
                .clone()
                .map(|id| {
                    vocab.is_trivia(id)
                        && styles.class_of(id).is_some_and(|c| comment_classes.contains(&c))
                })
                .collect(),
            in_mode: ids.map(|id| vocab.modes[id as usize] != 0).collect(),
        }
    }

    /// The fold a construct of production `prod` makes, if any.
    pub fn entry(&self, prod: u16) -> Option<FoldEntry> {
        self.by_prod.get(prod as usize).copied().flatten()
    }

    fn is_comment(&self, id: TokenId) -> bool {
        self.comments.get(id as usize).copied().unwrap_or(false)
    }

    fn is_in_mode(&self, id: TokenId) -> bool {
        self.in_mode.get(id as usize).copied().unwrap_or(false)
    }
}

/// A region of a document that runs over lines, which an editor can fold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fold {
    /// Index into [`FoldConfig::legend`].
    pub category: u8,
    /// Byte span, from the start of its first token to the end of its
    /// last.
    pub span: (u32, u32),
    /// Its head: a construct's first token, a comment's opening token,
    /// the comments of a run's first line.
    pub head: (u32, u32),
}

/// Where a fold walk stands between two pieces of a document: what the
/// tokens before them leave undecided. Offsets are relative to where the
/// next piece begins, so a piece walked from a state is walked again only
/// when the state it is entered in differs. A piece that follows a real
/// token (the usual case: trivia lead the token after them, so a node
/// ends at a token) is entered in [`FoldState::after_code`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoldState {
    /// Line breaks since the last token that was not one. Whether they
    /// end lines or run inside a comment waits on the next token: one
    /// read in a mode continues the construct the breaks are in.
    breaks: u32,
    /// The line being read: whether a real token is on it, the span of
    /// its comments, and whether it runs over a break inside a mode.
    code: bool,
    comments: Option<(i64, i64)>,
    spans: bool,
    /// The run of comment lines being read: its start, its first line's
    /// comments' end, its last line's comments' end, its lines.
    run: Option<(i64, i64, i64, u32)>,
    /// The comment being read: its start, its opening token's end, its
    /// last token's end, and whether it runs over a break.
    block: Option<(i64, i64, i64, bool)>,
}

impl FoldState {
    /// The state at the start of a document.
    pub fn start() -> FoldState {
        FoldState { breaks: 0, code: false, comments: None, spans: false, run: None, block: None }
    }

    /// The state after a real token, with nothing undecided.
    pub fn after_code() -> FoldState {
        FoldState { code: true, ..FoldState::start() }
    }

    fn shifted(&self, by: i64) -> FoldState {
        FoldState {
            comments: self.comments.map(|(s, e)| (s + by, e + by)),
            run: self.run.map(|(s, h, e, n)| (s + by, h + by, e + by, n)),
            block: self.block.map(|(s, h, e, b)| (s + by, h + by, e + by, b)),
            ..self.clone()
        }
    }
}

/// A fold whose offsets are relative to where a walk began, and may lie
/// before it (a comment run carried into the piece).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RelFold {
    pub category: u8,
    pub span: (i64, i64),
    pub head: (i64, i64),
}

impl RelFold {
    /// The fold at `base`.
    pub fn at(self, base: u32) -> Fold {
        let abs = |x: i64| (x + base as i64) as u32;
        Fold {
            category: self.category,
            span: (abs(self.span.0), abs(self.span.1)),
            head: (abs(self.head.0), abs(self.head.1)),
        }
    }
}

/// Reads the tokens of a document, or of a piece of one in order, and
/// finds its folds.
struct FoldWalker<'c> {
    cfg: &'c FoldConfig,
    st: FoldState,
    /// Offset of the next token, relative to where the walk began.
    at: i64,
    /// Line breaks passed in this walk.
    lines: u64,
    /// The first real token's start and end, and the line breaks passed
    /// before it.
    first_real: Option<(i64, i64, u64)>,
    /// The last real token's end, and the line breaks passed before it.
    last_real: Option<(i64, u64)>,
    /// Folds of constructs whose first real token has not come yet.
    waiting: Vec<usize>,
    /// Construct folds, in the order they were entered.
    slots: Vec<Slot>,
    comments: Vec<RelFold>,
}

/// A construct's fold being read.
struct Slot {
    category: u8,
    /// Its first real token's start and end, and the line breaks before
    /// it, once it comes.
    first: Option<(i64, i64, u64)>,
    /// Its last real token's end, and the line breaks before it, once the
    /// fold ends.
    last: Option<(i64, u64)>,
}

impl<'c> FoldWalker<'c> {
    fn new(cfg: &'c FoldConfig, st: FoldState) -> Self {
        FoldWalker {
            cfg,
            st,
            at: 0,
            lines: 0,
            first_real: None,
            last_real: None,
            waiting: Vec::new(),
            slots: Vec::new(),
            comments: Vec::new(),
        }
    }

    fn node(&mut self, n: &GreenNode) {
        let entry = self.cfg.entry(n.prod);
        let slot = entry.map(|e| {
            self.slots.push(Slot { category: e.category, first: None, last: None });
            self.slots.len() - 1
        });
        if let (Some(slot), Some(FoldEntry { from: None, .. })) = (slot, entry) {
            self.waiting.push(slot);
        }
        let (from, to) = (entry.and_then(|e| e.from), entry.and_then(|e| e.to));
        let mut symbol = 0usize;
        for c in &n.children {
            let is_symbol = match c {
                GreenChild::Token(t) => !t.trivia && !t.is_missing(),
                GreenChild::Node(m) => m.nt != ERROR_NT,
            };
            if let (true, Some(slot), Some(k)) = (is_symbol, slot, from) {
                if k == symbol {
                    self.waiting.push(slot);
                }
            }
            match c {
                GreenChild::Node(m) => self.node(m),
                GreenChild::Token(t) => self.token(t),
            }
            if is_symbol {
                if let (Some(slot), Some(k)) = (slot, to) {
                    if k == symbol {
                        self.end_fold(slot);
                    }
                }
                symbol += 1;
            }
        }
        if let (Some(slot), None) = (slot, to) {
            self.end_fold(slot);
        }
    }

    /// The fold of `slot` ends at the last real token read. One with no
    /// real token since its start (all of it inserted by recovery) folds
    /// nothing; so does one whose end never comes (the symbol it ends at
    /// was set aside by recovery).
    fn end_fold(&mut self, slot: usize) {
        self.waiting.retain(|&w| w != slot);
        if let (Some((start, _, _)), Some((end, lines))) = (self.slots[slot].first, self.last_real) {
            if end > start {
                self.slots[slot].last = Some((end, lines));
            }
        }
    }

    fn token(&mut self, t: &GreenToken) {
        let w = t.text.len() as i64;
        if t.is_missing() {
            return;
        }
        if t.id == qana_grammar::green::NEWLINE {
            self.st.breaks += 1;
            self.lines += 1;
            self.at += w;
            return;
        }
        let in_mode = self.cfg.is_in_mode(t.id);
        if self.st.breaks > 0 {
            if in_mode {
                // The breaks run inside the construct this token
                // continues: the line goes on over them.
                self.st.spans = true;
                if let Some(block) = &mut self.st.block {
                    block.3 = true;
                }
                self.st.breaks = 0;
            } else {
                self.breaks_end_lines();
            }
        }
        if !in_mode {
            self.close_block();
        }
        let comment = t.trivia && self.cfg.is_comment(t.id);
        let (start, end) = (self.at, self.at + w);
        if comment {
            match &mut self.st.block {
                Some(block) if in_mode => block.2 = end,
                _ => self.st.block = Some((start, end, end, false)),
            }
            self.st.comments = Some(self.st.comments.map_or((start, end), |(s, _)| (s, end)));
        }
        if !t.trivia {
            self.st.code = true;
            for slot in self.waiting.drain(..) {
                self.slots[slot].first = Some((start, end, self.lines));
            }
            if self.first_real.is_none() {
                self.first_real = Some((start, end, self.lines));
            }
            self.last_real = Some((end, self.lines));
        }
        self.at = end;
    }

    /// The line breaks waiting end lines: the line being read, and a
    /// blank one for each further break.
    fn breaks_end_lines(&mut self) {
        for _ in 0..std::mem::take(&mut self.st.breaks) {
            self.end_line();
        }
    }

    fn end_line(&mut self) {
        let st = &mut self.st;
        match st.comments {
            Some((start, end)) if !st.code && !st.spans => {
                st.run = Some(match st.run {
                    Some((s, head, _, n)) => (s, head, end, n + 1),
                    None => (start, end, end, 1),
                });
            }
            _ => self.close_run(),
        }
        let st = &mut self.st;
        st.code = false;
        st.comments = None;
        st.spans = false;
    }

    fn close_run(&mut self) {
        if let Some((start, head, end, lines)) = self.st.run.take() {
            if lines >= 2 {
                let category = self.cfg.comment;
                self.comments.push(RelFold { category, span: (start, end), head: (start, head) });
            }
        }
    }

    fn close_block(&mut self) {
        if let Some((start, head, end, spans)) = self.st.block.take() {
            if spans {
                let category = self.cfg.comment;
                self.comments.push(RelFold { category, span: (start, end), head: (start, head) });
            }
        }
    }

    /// The end of the document: the breaks waiting end lines, the last
    /// line ends, and so does a comment or run still open.
    fn end_of_document(&mut self) {
        self.breaks_end_lines();
        self.end_line();
        self.close_block();
        self.close_run();
    }

    /// The folds found, in document order (an enclosing fold before the
    /// ones it encloses; of two constructs with the same span, the outer
    /// one only), the state the walk ends in, and what it read.
    fn finish(self) -> FoldWalk {
        let mut folds: Vec<(RelFold, usize)> = self
            .slots
            .iter()
            .enumerate()
            .filter_map(|(i, slot)| {
                let (start, head_end, lines_at_start) = slot.first?;
                let (end, lines_at_end) = slot.last?;
                (lines_at_end > lines_at_start).then_some((
                    RelFold { category: slot.category, span: (start, end), head: (start, head_end) },
                    i,
                ))
            })
            .chain(self.comments.iter().map(|&f| (f, usize::MAX)))
            .collect();
        folds.sort_by_key(|(f, order)| (f.span.0, std::cmp::Reverse(f.span.1), *order));
        folds.dedup_by_key(|(f, _)| f.span);
        FoldWalk {
            folds: folds.into_iter().map(|(f, _)| f).collect(),
            left: self.st.shifted(-self.at),
            lines: self.lines,
            first_real: self.first_real,
            last_real: self.last_real,
        }
    }
}

/// What a fold walk of a piece of a document found: its folds, the state
/// it leaves (offsets relative to its end), and what a fold enclosing it
/// needs to know of it: its line breaks, and its first and last real
/// tokens (offsets relative to its start, with the line breaks before
/// them). A walk of a document's pieces in order, joined by these, is a
/// walk of the document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoldWalk {
    pub folds: Vec<RelFold>,
    pub left: FoldState,
    pub lines: u64,
    /// The first real token's start and end, and the line breaks before it.
    pub first_real: Option<(i64, i64, u64)>,
    /// The last real token's end, and the line breaks before it.
    pub last_real: Option<(i64, u64)>,
}

/// The folds of a whole document.
pub fn folds(tree: &GreenNode, cfg: &FoldConfig) -> Vec<Fold> {
    let mut walker = FoldWalker::new(cfg, FoldState::start());
    walker.node(tree);
    walker.end_of_document();
    walker.finish().folds.into_iter().map(|f| f.at(0)).collect()
}

/// The fold walk of one node of a document read in order, entered in `st`
/// (offsets relative to the node's start). A fold of a comment run or a
/// comment carried in from before the node may begin before it; one still
/// open at its end is carried out in the state it leaves.
pub fn node_folds(n: &GreenNode, cfg: &FoldConfig, st: FoldState) -> FoldWalk {
    let mut walker = FoldWalker::new(cfg, st);
    walker.node(n);
    walker.finish()
}

/// The same for a run of tokens.
pub fn token_folds<'t>(
    tokens: impl IntoIterator<Item = &'t GreenToken>,
    cfg: &FoldConfig,
    st: FoldState,
) -> FoldWalk {
    let mut walker = FoldWalker::new(cfg, st);
    for t in tokens {
        walker.token(t);
    }
    walker.finish()
}

/// The folds the end of a document closes, left open in `st` (offsets
/// relative to the end).
pub fn end_folds(cfg: &FoldConfig, st: FoldState) -> Vec<RelFold> {
    let mut walker = FoldWalker::new(cfg, st);
    walker.end_of_document();
    walker.finish().folds
}
// ---------------------------------------------------------------------------
// Outline (document symbols)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub kind: &'static str,
    /// Byte span of the whole construct.
    pub span: (u32, u32),
    /// Byte span of the name token (LSP selectionRange).
    pub selection: (u32, u32),
}

/// Outline configuration: which (nt, prod) pairs are outline-worthy and
/// which RHS symbol position carries the name — a TOKEN child directly,
/// or a NODE child by DELEGATION: the name is the first `@def` inside
/// that child's subtree (pre-order), and where it holds none, the
/// child's own text. Delegation is what lets a C function outline as
/// `handler` when its name sits at unbounded depth in the declarator
/// nest — the binding tier already knows where it is, so the outline
/// borrows its answer. A child with no definition in it names its
/// construct by what it says: a Rust function's `name`, an impl's type
/// (`Point<T>`).
#[derive(Clone, Debug, Default)]
pub struct OutlineConfig {
    pub entries: Vec<OutlineEntry>,
    /// The binding tier's def sites, (nt, prod, symbol-child of the
    /// name token) — the delegation walker's map. Filled by the
    /// grammar compiler; empty means delegated entries never resolve.
    pub defs: Vec<(u16, u16, usize)>,
}

#[derive(Clone, Debug)]
pub struct OutlineEntry {
    pub nt: u16,
    pub prod: u16,
    pub name_child: usize,
    pub kind: &'static str,
}

/// The first `@def` name inside `n`'s subtree, pre-order: the
/// delegation resolver for outline entries whose name child is a NODE.
/// In a declarator nest, pre-order meets the declared name before any
/// parameter it binds — `int (*handler(int sig))(void)` resolves to
/// `handler`, not `sig`.
fn first_def_in(
    n: &GreenNode,
    base: u32,
    defs: &[(u16, u16, usize)],
) -> Option<(String, (u32, u32))> {
    if let Some(&(_, _, k)) = defs.iter().find(|d| d.0 == n.nt && d.1 == n.prod) {
        let mut off = base;
        let mut sym_idx = 0usize;
        for c in &n.children {
            let w = c.width();
            let is_symbol = match c {
                GreenChild::Token(t) => !t.trivia && !t.is_missing(),
                GreenChild::Node(m) => m.nt != ERROR_NT,
            };
            if is_symbol {
                if sym_idx == k {
                    if let GreenChild::Token(t) = c {
                        return Some((t.text.clone(), (off, off + w)));
                    }
                    break;
                }
                sym_idx += 1;
            }
            off += w;
        }
    }
    let mut off = base;
    for c in &n.children {
        if let GreenChild::Node(m) = c {
            if m.nt != ERROR_NT {
                if let Some(hit) = first_def_in(m, off, defs) {
                    return Some(hit);
                }
            }
        }
        off += c.width();
    }
    None
}

pub fn outline(tree: &GreenNode, cfg: &OutlineConfig) -> Vec<Symbol> {
    let mut out = Vec::new();
    walk(tree, 0, cfg, &mut out);
    return out;

    fn walk(n: &GreenNode, base: u32, cfg: &OutlineConfig, out: &mut Vec<Symbol>) {
        if let Some(symbol) = own_symbol(n, base, cfg) {
            out.push(symbol);
        }
        let mut off = base;
        for c in &n.children {
            if let GreenChild::Node(m) = c {
                walk(m, off, cfg, out);
            }
            off += c.width();
        }
    }
}

/// The outline symbol `n` ITSELF is, if its production is
/// outline-worthy: what [`outline`] emits for `n` before it descends
/// into `n`'s children, with spans relative to `n`'s start.
///
/// `outline(tree)` is this for every node of the tree, in pre-order. A
/// caller that keeps the outlines of subtrees an edit left alone (see
/// `qana_lang::live`) asks this for the nodes above them.
pub fn node_symbol(n: &GreenNode, cfg: &OutlineConfig) -> Option<Symbol> {
    own_symbol(n, 0, cfg)
}

fn own_symbol(n: &GreenNode, base: u32, cfg: &OutlineConfig) -> Option<Symbol> {
    let e = cfg.entries.iter().find(|e| e.nt == n.nt && e.prod == n.prod)?;
    // Locate the name child among SYMBOL children with offsets.
    let mut off = base;
    let mut sym_idx = 0usize;
    for c in &n.children {
        let w = c.width();
        let is_symbol = match c {
            GreenChild::Token(t) => !t.trivia && !t.is_missing(),
            GreenChild::Node(m) => m.nt != ERROR_NT,
        };
        if is_symbol {
            if sym_idx == e.name_child {
                // The construct's SIGNIFICANT span, not its
                // trivia-padded node span: leading comments
                // attach inside the following node, and outline
                // consumers (folding, breadcrumbs,
                // documentSymbol ranges) want the construct,
                // not its gutter of trivia.
                let span = significant_span(n, base).unwrap_or((base, base + n.width));
                return match c {
                    GreenChild::Token(t) => Some(Symbol {
                        name: t.text.clone(),
                        kind: e.kind,
                        span,
                        selection: (off, off + w),
                    }),
                    // DELEGATION: the name is the first def
                    // inside this child's subtree; failing that,
                    // the child's own text.
                    GreenChild::Node(m) => first_def_in(m, off, &cfg.defs)
                        .or_else(|| significant_text(m, off))
                        .map(|(name, sel)| Symbol { name, kind: e.kind, span, selection: sel }),
                };
            }
            sym_idx += 1;
        }
        off += w;
    }
    None
}

/// The text of a node's real tokens, trivia between two of them read as
/// one space (`HashMap<K,\n    V>` is `HashMap<K, V>`), and their span:
/// what a construct named by the node is called. `None` for a node with
/// no real token.
fn significant_text(n: &GreenNode, base: u32) -> Option<(String, (u32, u32))> {
    fn walk(n: &GreenNode, off: &mut u32, text: &mut String, gap: &mut bool, span: &mut Option<(u32, u32)>) {
        for c in &n.children {
            match c {
                // The child's tokens from where it starts; the loop moves
                // past it below.
                GreenChild::Node(m) => walk(m, &mut { *off }, text, gap, span),
                GreenChild::Token(t) if t.trivia => *gap = true,
                GreenChild::Token(t) if t.is_missing() => {}
                GreenChild::Token(t) => {
                    if span.is_some() && *gap {
                        text.push(' ');
                    }
                    text.push_str(&t.text);
                    let end = *off + t.text.len() as u32;
                    *span = Some(span.map_or((*off, end), |(s, _)| (s, end)));
                    *gap = false;
                }
            }
            *off += c.width();
        }
    }
    let (mut off, mut text, mut gap, mut span) = (base, String::new(), false, None);
    walk(n, &mut off, &mut text, &mut gap, &mut span);
    span.map(|span| (text, span))
}

/// `[first, last)` byte span of a node's significant content —
/// trivia and missing tokens trimmed from BOTH ends, recursively
/// (a first/last child that is itself a node starts/ends with its
/// own attached trivia). `None` for an all-trivia node.
///
/// It descends to the first real token and to the last, through the
/// children that hold one (a node's `terms` counts its real tokens), so
/// it costs the node's depth, not its size: the outline asks it of a
/// module that is a whole file on every revision.
fn significant_span(n: &GreenNode, base: u32) -> Option<(u32, u32)> {
    let real = |c: &GreenChild| match c {
        GreenChild::Token(t) => !t.trivia && !t.is_missing(),
        GreenChild::Node(m) => m.terms > 0,
    };
    // The first real token's start.
    let (mut node, mut off) = (n, base);
    let first = loop {
        let mut at = off;
        let c = node.children.iter().find(|c| {
            let hit = real(c);
            if !hit {
                at += c.width();
            }
            hit
        })?;
        match c {
            GreenChild::Token(_) => break at,
            GreenChild::Node(m) => (node, off) = (m, at),
        }
    };
    // The last real token's end.
    let (mut node, mut end) = (n, base + n.width);
    let last = loop {
        let mut at = end;
        let c = node.children.iter().rev().find(|c| {
            let hit = real(c);
            if !hit {
                at -= c.width();
            }
            hit
        })?;
        match c {
            GreenChild::Token(_) => break at,
            GreenChild::Node(m) => (node, end) = (m, at),
        }
    };
    Some((first, last))
}

// ---------------------------------------------------------------------------
// Completion (expected terminals from the ACTION row at the caret)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionItem {
    pub label: String,
    pub is_keyword: bool,
}

/// Completion at a byte offset: run the states-only LR automaton over the
/// terminal prefix before the caret, then read the ACTION row. O(prefix)
/// — per-line state checkpoints are the documented optimization when a
/// client needs it.
pub fn completion_at(
    lexer: &CompiledLexer,
    buf: &LexedBuffer<'_, CompiledLexer>,
    sg: &SynGrammar,
    tables: &LrTables,
    offset: u32,
) -> Vec<CompletionItem> {
    // Collect terminal ids strictly before `offset`.
    let mut terms: Vec<TokenId> = Vec::new();
    let mut pos = 0u32;
    'outer: for (line, lt) in buf.lines.iter().zip(&buf.lexed) {
        let mut col = 0u32;
        for tok in &lt.tokens {
            if pos + col + tok.len > offset {
                break 'outer;
            }
            if !lexer.is_trivia(tok.id) {
                terms.push(tok.id);
            }
            col += tok.len;
        }
        pos += line.text.len() as u32 + line.term.as_str().len() as u32;
        if pos > offset {
            break;
        }
    }
    // States-only run (errors: stop at the last healthy state).
    let mut states: Vec<u16> = vec![0];
    let mut i = 0usize;
    let mut guard = 0usize;
    while i < terms.len() && guard < terms.len() * 8 + 64 {
        guard += 1;
        let s = *states.last().unwrap();
        match tables.action[s as usize].get(&terms[i]).copied() {
            Some(LrAct::Shift(n)) => {
                states.push(n);
                i += 1;
            }
            Some(LrAct::Reduce(p)) => {
                let prod = &sg.prods[p as usize];
                let k = prod.rhs.len();
                if states.len() <= k {
                    break;
                }
                states.truncate(states.len() - k);
                let top = *states.last().unwrap();
                match tables.goto_[top as usize].get(&prod.lhs) {
                    Some(&n) => states.push(n),
                    None => break,
                }
            }
            _ => break,
        }
    }
    let s = *states.last().unwrap();
    let mut items: Vec<CompletionItem> = tables.action[s as usize]
        .keys()
        .filter(|&&t| t != EOF)
        .map(|&t| match lexer.keyword_text(t) {
            Some(text) => CompletionItem { label: text.to_string(), is_keyword: true },
            None => CompletionItem { label: sg.term_name(t).to_string(), is_keyword: false },
        })
        .collect();
    items.sort_by(|a, b| a.label.cmp(&b.label));
    items.dedup();
    items
}

// ---------------------------------------------------------------------------
// Diagnostics from repairs
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub span: (u32, u32),
    pub message: String,
}

/// Map repairs (terminal indices) to byte spans + human messages.
pub fn diagnostics(
    lexer: &CompiledLexer,
    buf: &LexedBuffer<'_, CompiledLexer>,
    sg: &SynGrammar,
    repairs: &[Repair],
) -> Vec<Diagnostic> {
    // Byte span of the k-th terminal (fallback: end of file).
    let span_of = |k: usize| -> (u32, u32) {
        let mut idx = 0usize;
        let mut pos = 0u32;
        for (line, lt) in buf.lines.iter().zip(&buf.lexed) {
            let mut col = 0u32;
            for tok in &lt.tokens {
                if !lexer.is_trivia(tok.id) {
                    if idx == k {
                        return (pos + col, pos + col + tok.len);
                    }
                    idx += 1;
                }
                col += tok.len;
            }
            pos += line.text.len() as u32 + line.term.as_str().len() as u32;
        }
        (pos, pos)
    };
    repairs
        .iter()
        .map(|r| {
            let span = span_of(r.at_terminal);
            let message = match &r.kind {
                RepairKind::Deleted(text) => format!("unexpected `{text}`"),
                RepairKind::Inserted(id) => format!("missing {}", sg.term_name(*id)),
                RepairKind::Unwound => "incomplete construct".to_string(),
            };
            Diagnostic { span, message }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Selection expansion (re-exported substrate)
// ---------------------------------------------------------------------------

/// LSP selectionRange: nested spans at an offset, outermost first.
pub fn selection_ranges(session: &IncSession<'_>, offset: u32) -> Vec<(u32, u32)> {
    session.tree().map(|t| ancestor_spans(t, offset)).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Demo-language glue (style map + outline config for the demo grammar)
// ---------------------------------------------------------------------------

pub mod demo_glue {
    use super::{OutlineConfig, OutlineEntry, Styles};
    use qana_grammar::demo::DemoIds;
    use qana_grammar::SynGrammar;

    pub fn demo_styles(ids: &DemoIds) -> Styles {
        let mut s = Styles::new(super::LEGEND.to_vec());
        for &kw in &ids.kw {
            s.set(kw, "keyword");
        }
        s.set(ids.ident, "variable");
        s.set(ids.number, "number");
        s.set(ids.string, "string");
        s.set(ids.string_unterm, "string");
        s.set(ids.line_comment, "comment");
        s.set(ids.block_open, "comment");
        s.set(ids.b_open, "comment");
        s.set(ids.b_close, "comment");
        s.set(ids.b_content, "comment");
        s.set(ids.b_misc, "comment");
        for &op in &[ids.plus, ids.minus, ids.star, ids.slash, ids.eq] {
            s.set(op, "operator");
        }
        for &p in &[ids.semi, ids.comma, ids.punct] {
            s.set(p, "punctuation");
        }
        for &b in &[ids.lparen, ids.rparen, ids.lbracket, ids.rbracket, ids.lbrace, ids.rbrace] {
            s.set(b, "bracket");
        }
        s
    }

    /// Folding for the demo language: a block, and the bracketed lists
    /// of a call, a list literal and a parenthesised expression.
    pub fn demo_fold_config(
        sg: &SynGrammar,
        vocab: &qana_grammar::Vocab,
        styles: &Styles,
    ) -> super::FoldConfig {
        let mut entries: Vec<(u16, u8, Option<usize>, Option<usize>)> = Vec::new();
        for i in 0..sg.prods.len() {
            match sg.prod_name(i).as_str() {
                "Block" => entries.push((i as u16, 0, None, None)),
                "CallExpr" => entries.push((i as u16, 1, Some(1), None)),
                "ListExpr" | "ParenExpr" => entries.push((i as u16, 1, None, None)),
                _ => {}
            }
        }
        super::FoldConfig::new(
            vec!["block".to_string(), "list".to_string()],
            &entries,
            sg.prods.len(),
            vocab,
            styles,
        )
    }

    pub fn demo_outline_config(sg: &SynGrammar) -> OutlineConfig {
        let mut cfg = OutlineConfig::default();
        for i in 0..sg.prods.len() {
            if sg.prod_name(i) == "LetStmt" {
                cfg.entries.push(OutlineEntry {
                    nt: sg.prods[i].lhs,
                    prod: i as u16,
                    name_child: 1, // KW_LET IDENT ...
                    kind: "variable",
                });
            }
        }
        cfg
    }
}
