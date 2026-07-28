//! P3 increment 1: grammar-derived editor services over the incremental
//! session. Everything here is computed from artifacts the toolchain
//! already generates — token styles from the vocab, folding from the
//! block skeleton and line states, outline from the typed tree,
//! completion straight off the LR ACTION rows, diagnostics from repairs.
//! The functions are LSP-shaped (semantic-token quintuples with delta
//! edits, line/col ranges) but transport-free; the LSP server binary is
//! the next increment's plumbing.

pub mod paint;

use qana_engine::{DamageReport, IncSession, LexedBuffer};
use qana_grammar::green::{ancestor_spans, ERROR_NT};
use qana_grammar::incremental::{Repair, RepairKind};
use qana_grammar::{
    CompiledLexer, GreenChild, GreenNode, LrAct, LrTables, SynGrammar, TokenId, EOF,
};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Semantic tokens (tier 0: syntactic, from the token runs)
// ---------------------------------------------------------------------------

/// The canonical style legend: a CLOSED set of classes shared by every
/// grammar the server hosts (editor themes and the semantic-token
/// protocol need one stable legend). Grammar-declared styles must pick
/// from it — an unknown class is a compile diagnostic, not a surprise.
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
];

/// Token-id → style mapping plus the LSP legend it indexes into.
#[derive(Clone, Debug)]
pub struct Styles {
    pub legend: Vec<&'static str>,
    map: HashMap<TokenId, u32>,
}

impl Styles {
    pub fn new(legend: Vec<&'static str>) -> Self {
        Styles { legend, map: HashMap::new() }
    }
    pub fn set(&mut self, id: TokenId, class: &'static str) {
        let idx = self
            .legend
            .iter()
            .position(|c| *c == class)
            .expect("class must be in the legend") as u32;
        self.map.insert(id, idx);
    }
    pub fn class_of(&self, id: TokenId) -> Option<u32> {
        self.map.get(&id).copied()
    }
}

/// LSP `SemanticTokens.data`: flat quintuples
/// (deltaLine, deltaStart, length, tokenType, tokenModifiers).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SemanticTokens {
    pub data: Vec<u32>,
    /// Styled-token quintuple count per buffer line (delta bookkeeping).
    per_line: Vec<u32>,
}

pub fn semantic_tokens_full(
    lexer: &CompiledLexer,
    buf: &LexedBuffer<'_, CompiledLexer>,
    styles: &Styles,
) -> SemanticTokens {
    let mut data = Vec::new();
    let mut per_line = Vec::with_capacity(buf.lines.len());
    let mut prev_line = 0u32;
    let mut prev_start = 0u32;
    for (li, lt) in buf.lexed.iter().enumerate() {
        let mut count = 0u32;
        let mut col = 0u32;
        for tok in &lt.tokens {
            let len = tok.len;
            if let Some(class) = styles.class_of(tok.id) {
                let line = li as u32;
                let delta_line = line - prev_line;
                let delta_start = if delta_line == 0 { col - prev_start } else { col };
                data.extend_from_slice(&[delta_line, delta_start, len, class, 0]);
                prev_line = line;
                prev_start = col;
                count += 1;
            }
            col += len;
            let _ = lexer;
        }
        per_line.push(count);
    }
    SemanticTokens { data, per_line }
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
    /// deltaLine re-anchored. Returns the LSP-shaped edit.
    pub fn update(
        &mut self,
        _lexer: &CompiledLexer,
        buf: &LexedBuffer<'_, CompiledLexer>,
        styles: &Styles,
        damage: &DamageReport,
    ) -> Option<SemDelta> {
        if damage.regions.is_empty() {
            return None;
        }
        // Spanning region (regions are ascending; a single splice keeps
        // the delta simple — per-region splices are a refinement).
        let old_lo = damage.regions.iter().map(|r| r.old_lines.0).min().unwrap();
        let old_hi = damage.regions.iter().map(|r| r.old_lines.1).max().unwrap();
        let new_lo = damage.regions.iter().map(|r| r.new_lines.0).min().unwrap();
        let new_hi = damage.regions.iter().map(|r| r.new_lines.1).max().unwrap();
        debug_assert_eq!(old_lo, new_lo, "regions align at their start");

        // Quintuple index where the damaged span starts, in OLD data.
        let q_start: u32 = self.per_line[..old_lo].iter().sum::<u32>() * 5;
        let q_deleted: u32 = self.per_line[old_lo..old_hi].iter().sum::<u32>() * 5;
        // One more styled token after the span re-anchors its deltaLine
        // (its absolute line may have shifted with the line-count delta).
        let has_after = self.per_line[old_hi..].iter().any(|&c| c > 0);
        let extra = u32::from(has_after) * 5;

        // Encode replacement lines (and the re-anchored follower).
        let mut insert = Vec::new();
        // Absolute position of the styled token PRECEDING the span.
        let (mut prev_line, mut prev_start) = {
            let mut line = None;
            'outer: for li in (0..new_lo).rev() {
                if self.per_line[li] > 0 {
                    // Find its start col by rescanning that line.
                    let mut col = 0u32;
                    let mut last = None;
                    for tok in &buf.lexed[li].tokens {
                        if styles.class_of(tok.id).is_some() {
                            last = Some(col);
                        }
                        col += tok.len;
                    }
                    line = Some((li as u32, last.unwrap_or(0)));
                    break 'outer;
                }
            }
            line.unwrap_or((0, 0))
        };
        if self.per_line[..new_lo].iter().all(|&c| c == 0) {
            prev_line = 0;
            prev_start = 0;
        }
        let mut new_counts: Vec<u32> = Vec::with_capacity(new_hi - new_lo);
        for li in new_lo..new_hi {
            let mut count = 0u32;
            let mut col = 0u32;
            for tok in &buf.lexed[li].tokens {
                if let Some(class) = styles.class_of(tok.id) {
                    let line = li as u32;
                    let delta_line = line - prev_line;
                    let delta_start = if delta_line == 0 { col - prev_start } else { col };
                    insert.extend_from_slice(&[delta_line, delta_start, tok.len, class, 0]);
                    prev_line = line;
                    prev_start = col;
                    count += 1;
                }
                col += tok.len;
            }
            new_counts.push(count);
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
            let abs_start = {
                // deltaStart is relative only within the same line; the
                // follower is on a later line than prev unless...
                let mut col = 0u32;
                let li = abs_line_new as usize;
                let mut first = 0u32;
                for tok in &buf.lexed[li].tokens {
                    if styles.class_of(tok.id).is_some() {
                        first = col;
                        break;
                    }
                    col += tok.len;
                }
                first
            };
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FoldRange {
    pub start_line: u32,
    pub end_line: u32,
    pub kind: FoldKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FoldKind {
    Block,
    Comment,
}

/// Folding: bracket blocks from the skeleton + multi-line comment runs
/// from the line states (lines whose ENTRY state is inside a comment).
pub fn folding_ranges(
    lexer: &CompiledLexer,
    buf: &LexedBuffer<'_, CompiledLexer>,
) -> Vec<FoldRange> {
    let mut out = Vec::new();
    let sk = qana_engine::build_skeleton(buf, &lexer.vocab);
    for (start, end) in sk.folding_ranges() {
        out.push(FoldRange { start_line: start as u32, end_line: end as u32, kind: FoldKind::Block });
    }
    // Comment runs: entry state depth > 0 means "inside a block comment".
    let mut run_start: Option<usize> = None;
    for li in 0..buf.lines.len() {
        let inside = buf.entry_state(li).depth() > 0;
        match (inside, run_start) {
            (true, None) => run_start = Some(li.saturating_sub(1)), // opener line
            (false, Some(s)) => {
                if li > s + 1 {
                    out.push(FoldRange {
                        start_line: s as u32,
                        end_line: (li - 1) as u32,
                        kind: FoldKind::Comment,
                    });
                }
                run_start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = run_start {
        let last = buf.lines.len() - 1;
        if last > s {
            out.push(FoldRange { start_line: s as u32, end_line: last as u32, kind: FoldKind::Comment });
        }
    }
    out.sort_by_key(|f| (f.start_line, f.end_line));
    out
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
/// that child's subtree (pre-order). Delegation is what lets a C
/// function outline as `handler` when its name sits at unbounded depth
/// in the declarator nest — the binding tier already knows where it
/// is, so the outline borrows its answer.
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
        if let Some(e) = cfg.entries.iter().find(|e| e.nt == n.nt && e.prod == n.prod) {
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
                        let span =
                            significant_span(n, base).unwrap_or((base, base + n.width));
                        match c {
                            GreenChild::Token(t) => out.push(Symbol {
                                name: t.text.clone(),
                                kind: e.kind,
                                span,
                                selection: (off, off + w),
                            }),
                            // DELEGATION: the name is the first def
                            // inside this child's subtree.
                            GreenChild::Node(m) => {
                                if let Some((name, sel)) = first_def_in(m, off, &cfg.defs) {
                                    out.push(Symbol {
                                        name,
                                        kind: e.kind,
                                        span,
                                        selection: sel,
                                    });
                                }
                            }
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
                walk(m, off, cfg, out);
            }
            off += c.width();
        }
    }

    /// `[first, last)` byte span of a node's significant content —
    /// trivia and missing tokens trimmed from BOTH ends, recursively
    /// (a first/last child that is itself a node starts/ends with its
    /// own attached trivia). `None` for an all-trivia node.
    fn significant_span(n: &GreenNode, base: u32) -> Option<(u32, u32)> {
        let mut first: Option<u32> = None;
        let mut last: Option<u32> = None;
        let mut off = base;
        for c in &n.children {
            let w = c.width();
            let sub = match c {
                GreenChild::Token(t) if !t.trivia && !t.is_missing() => Some((off, off + w)),
                GreenChild::Node(m) => significant_span(m, off),
                _ => None,
            };
            if let Some((s, e)) = sub {
                if first.is_none() {
                    first = Some(s);
                }
                last = Some(e);
            }
            off += w;
        }
        Some((first?, last?))
    }
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
        .map(|&t| {
            let name = sg.term_name(t).to_string();
            let is_keyword = name.starts_with("KW_");
            let label = if is_keyword {
                name.trim_start_matches("KW_").to_lowercase()
            } else {
                name
            };
            CompletionItem { label, is_keyword }
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
