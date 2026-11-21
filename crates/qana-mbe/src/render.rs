//! Token trees as text the grammar can parse: written out with the spacing
//! that keeps each token what it is, lexed back, and each tree's span in
//! the text kept, so a parse of the text says which tokens it took.
//!
//! A fragment a transcriber put in (an invisible group) is written bare;
//! where the grammar does not read it as one node, as rustc keeps it, it is
//! written again in parentheses, for the kinds Rust's grammar groups that
//! way (an expression, a pattern, a type that is no path, a literal).

use std::collections::HashSet;

use qana_grammar::{GreenChild, GreenNode, Repair, TokWithText, TokenId};
use qana_lang::EmbeddedLang;

use crate::token::{Delim, Group, MetaVarKind, Semi, Token, TokenKind, Tree};
use crate::{Edition, Error, Origin};

/// A token tree flattened: tokens, and each group's delimiters.
#[derive(Clone, Debug)]
pub(crate) enum Flat {
    Token(Token),
    Open { group: Group, close: usize },
    Close { delim: Delim, origin: Origin, edition: Edition, mark: u32, open: usize },
}

impl Flat {
    pub(crate) fn invisible(&self) -> Option<MetaVarKind> {
        match self {
            Flat::Open { group, .. } => match group.delim {
                Delim::Invisible(k) => Some(k),
                _ => None,
            },
            Flat::Close { delim: Delim::Invisible(k), .. } => Some(*k),
            _ => None,
        }
    }
}

pub(crate) fn flatten(trees: &[Tree]) -> Vec<Flat> {
    fn go(trees: &[Tree], out: &mut Vec<Flat>) {
        for tree in trees {
            match tree {
                Tree::Token(t) => out.push(Flat::Token(t.clone())),
                Tree::Group(g) => {
                    let open = out.len();
                    out.push(Flat::Open { group: g.clone(), close: 0 });
                    go(&g.trees, out);
                    let close = out.len();
                    out.push(Flat::Close {
                        delim: g.delim,
                        origin: g.close,
                        edition: g.edition,
                        mark: g.mark,
                        open,
                    });
                    if let Flat::Open { close: c, .. } = &mut out[open] {
                        *c = close;
                    }
                }
            }
        }
    }
    let mut out = Vec::new();
    go(trees, &mut out);
    out
}

/// Whether a fragment of this kind is wrapped in parentheses where the
/// grammar would not read it as one node: an expression, a pattern, a type
/// (a path too: `(u8)` is a type, and `$t::MAX`, where it would be none,
/// rustc refuses), a literal.
pub(crate) fn wrappable(kind: MetaVarKind) -> bool {
    matches!(
        kind,
        MetaVarKind::Expr { .. }
            | MetaVarKind::Pat(_)
            | MetaVarKind::Ty { .. }
            | MetaVarKind::Literal
    )
}

/// The first flat after `i` that is written as something: not an
/// invisible group's bare delimiter, nor one skipped.
fn next_written<'f>(
    flats: &'f [Flat],
    i: usize,
    wrapped: &HashSet<usize>,
    skip: &HashSet<usize>,
) -> Option<(usize, &'f Flat)> {
    let mut j = i + 1;
    while let Some(f) = flats.get(j) {
        let bare = match f {
            Flat::Open { group, .. } => {
                matches!(group.delim, Delim::Invisible(_)) && !wrapped.contains(&j)
            }
            Flat::Close { delim: Delim::Invisible(MetaVarKind::Stmt { .. }), .. } => false,
            Flat::Close { delim: Delim::Invisible(_), open, .. } => !wrapped.contains(open),
            Flat::Close { .. } | Flat::Token(_) => false,
        };
        if !bare && !skip.contains(&j) {
            return Some((j, f));
        }
        j += 1;
    }
    None
}

/// Whether a statement fragment closed at flat `i` is written with a `;`:
/// where the grammar needs one and none follows. An expression that ends
/// its block (or the stream) is its tail, and takes none.
fn needs_semi(
    flats: &[Flat],
    i: usize,
    semi: Semi,
    wrapped: &HashSet<usize>,
    skip: &HashSet<usize>,
) -> bool {
    if semi == Semi::Never {
        return false;
    }
    match next_written(flats, i, wrapped, skip) {
        Some((j, Flat::Token(t))) if t.is_punct(";") && !empty_statement(flats, j) => false,
        None | Some((_, Flat::Close { delim: Delim::Brace, .. })) => semi == Semi::Always,
        Some(_) => true,
    }
}

/// Whether the `;` at flat `j` is a statement fragment of its own (`$s`
/// that matched an empty statement), which ends no statement before it.
fn empty_statement(flats: &[Flat], j: usize) -> bool {
    j > 0
        && matches!(
            (&flats[j - 1], flats.get(j + 1)),
            (
                Flat::Open { group, .. },
                Some(Flat::Close { delim: Delim::Invisible(MetaVarKind::Stmt { .. }), .. })
            ) if matches!(group.delim, Delim::Invisible(MetaVarKind::Stmt { .. }))
        )
}

/// Whether the text from byte `a` to `b` is outer attributes only (`#[…]`,
/// doc comments), an invisible group's delimiters aside.
fn attributes_only(flats: &[Flat], r: &Rendering, a: u32, b: u32) -> bool {
    let inside: Vec<usize> = (0..flats.len())
        .filter(|&k| r.spans[k].1 > r.spans[k].0 && r.spans[k].0 >= a && r.spans[k].1 <= b)
        .collect();
    let mut i = 0usize;
    while i < inside.len() {
        match &flats[inside[i]] {
            Flat::Token(t) if t.kind == TokenKind::DocComment => i += 1,
            Flat::Token(t) if t.is_punct("#") => {
                let Some(&open) = inside.get(i + 1) else { return false };
                let Flat::Open { group, close } = &flats[open] else { return false };
                if group.delim != Delim::Bracket {
                    return false;
                }
                // Past the attribute's brackets.
                match inside.iter().position(|&k| k == *close) {
                    Some(p) => i = p + 1,
                    None => return false,
                }
            }
            _ => return false,
        }
    }
    true
}

/// Flats written out and lexed back.
pub(crate) struct Rendering {
    pub text: String,
    /// The grammar's tokens of the text, trivia among them.
    pub full: Vec<TokWithText>,
    /// For each of the grammar's non-trivia tokens: its index in `full`,
    /// its id and where it begins.
    pub sig_full: Vec<usize>,
    pub ids: Vec<TokenId>,
    pub sig_start: Vec<u32>,
    /// Each flat's span in the text; an invisible group's delimiters are
    /// empty unless wrapped.
    pub spans: Vec<(u32, u32)>,
}

impl Rendering {
    /// The first of the grammar's non-trivia tokens at or after byte `at`.
    pub(crate) fn sig_at(&self, at: u32) -> usize {
        self.sig_start.partition_point(|&s| s < at)
    }

    /// Where the grammar's non-trivia token `k` begins; the text's end past
    /// the last.
    pub(crate) fn sig_byte(&self, k: usize) -> u32 {
        self.sig_start.get(k).copied().unwrap_or(self.text.len() as u32)
    }

    /// The span its tokens take of an invisible group opened at flat
    /// `open` and closed at `close`, inside any parentheses: `None` for an
    /// empty one.
    pub(crate) fn content_span(&self, open: usize, close: usize) -> Option<(u32, u32)> {
        let mut span: Option<(u32, u32)> = None;
        for &(s, e) in &self.spans[open + 1..close] {
            if e > s {
                span = Some(match span {
                    None => (s, e),
                    Some((a, b)) => (a.min(s), b.max(e)),
                });
            }
        }
        span
    }
}

/// Write `flats` out (all but those `skip` names), the invisible groups
/// `wrapped` names (by their opening flat) in parentheses, and lex the
/// text.
pub(crate) fn render(
    lang: &EmbeddedLang,
    flats: &[Flat],
    wrapped: &HashSet<usize>,
    skip: &HashSet<usize>,
) -> Rendering {
    let mut text = String::new();
    let mut spans = Vec::with_capacity(flats.len());
    // What was written last: its source and end, when a token or bracket
    // of a source was, so that one adjacent to it there is written
    // adjacent here; and whether it was a line comment, which a newline
    // must end.
    let mut last: Option<Option<(u32, u32)>> = None;
    let mut newline = false;
    let mut write = |text: &mut String,
                     spans: &mut Vec<(u32, u32)>,
                     piece: &str,
                     origin: Option<Origin>,
                     line_comment: bool| {
        if let Some(prev) = last {
            let adjacent = matches!((prev, origin), (Some((src, end)), Some(o)) if src == o.source && end == o.start);
            if newline {
                text.push('\n');
            } else if !adjacent {
                text.push(' ');
            }
        }
        let start = text.len() as u32;
        text.push_str(piece);
        spans.push((start, text.len() as u32));
        last = Some(origin.map(|o| (o.source, o.end)));
        newline = line_comment;
    };
    for (i, flat) in flats.iter().enumerate() {
        if skip.contains(&i) {
            let at = text.len() as u32;
            spans.push((at, at));
            continue;
        }
        match flat {
            Flat::Token(t) => {
                let line_comment = t.kind == TokenKind::DocComment && t.text.starts_with("//");
                write(&mut text, &mut spans, &t.text, Some(t.origin), line_comment)
            }
            Flat::Open { group, .. } => match group.delim {
                Delim::Invisible(_) if !wrapped.contains(&i) => {
                    let at = text.len() as u32;
                    spans.push((at, at));
                }
                Delim::Invisible(_) => write(&mut text, &mut spans, "(", None, false),
                d => write(&mut text, &mut spans, d.open_text(), Some(group.open), false),
            },
            Flat::Close { delim, origin, open, .. } => match delim {
                Delim::Invisible(MetaVarKind::Stmt { semi })
                    if needs_semi(flats, i, *semi, wrapped, skip) =>
                {
                    // The `;` the grammar needs and rustc's tree does not.
                    write(&mut text, &mut spans, ";", None, false)
                }
                Delim::Invisible(_) if !wrapped.contains(open) => {
                    let at = text.len() as u32;
                    spans.push((at, at));
                }
                Delim::Invisible(_) => write(&mut text, &mut spans, ")", None, false),
                d => write(&mut text, &mut spans, d.close_text(), Some(*origin), false),
            },
        }
    }
    let full = lang.tokens(&text);
    let mut sig_full = Vec::new();
    let mut ids = Vec::new();
    let mut sig_start = Vec::new();
    let mut pos = 0u32;
    for (k, t) in full.iter().enumerate() {
        if !t.trivia {
            sig_full.push(k);
            ids.push(t.id);
            sig_start.push(pos);
        }
        pos += t.text.len() as u32;
    }
    Rendering { text, full, sig_full, ids, sig_start, spans }
}

/// The spans of a parse's nodes, from each one's first non-trivia token to
/// its last; and those of its token trees (nodes of rule `tts`: an
/// invocation's arguments, an attribute's), whose tokens are no syntax.
pub(crate) struct NodeSpans {
    pub nodes: HashSet<(u32, u32)>,
    pub trees: Vec<(u32, u32)>,
}

/// The [`NodeSpans`] of `tree`, which begins at byte `base`.
pub(crate) fn node_spans(tree: &GreenNode, base: u32, tts: Option<u16>) -> NodeSpans {
    fn go(
        n: &GreenNode,
        base: u32,
        tts: Option<u16>,
        out: &mut HashSet<(u32, u32)>,
        trees: &mut Vec<(u32, u32)>,
    ) -> Option<(u32, u32)> {
        let mut off = base;
        let mut span: Option<(u32, u32)> = None;
        for c in &n.children {
            let w = c.width();
            let inner = match c {
                GreenChild::Token(t) if !t.trivia && w > 0 => Some((off, off + w)),
                GreenChild::Token(_) => None,
                GreenChild::Node(m) => go(m, off, tts, out, trees),
            };
            if let Some((s, e)) = inner {
                span = Some(match span {
                    None => (s, e),
                    Some((a, _)) => (a, e),
                });
            }
            off += w;
        }
        if let Some(s) = span {
            out.insert(s);
            if Some(n.nt) == tts {
                trees.push(s);
            }
        }
        span
    }
    let mut nodes = HashSet::new();
    let mut trees = Vec::new();
    go(tree, base, tts, &mut nodes, &mut trees);
    NodeSpans { nodes, trees }
}

/// The invisible groups among `flats[from..to]` the parse `tree` (of the
/// text from byte `base`, with `repairs` at the grammar's non-trivia tokens
/// from `first_sig`) splits: an operand that is no one node of it, another
/// fragment a node straddles, or one where a repair stands. One in a token
/// tree is no syntax there, and splits nothing: the macro it is passed to
/// reads it whole. Those that can be wrapped are returned; one that cannot
/// is an error, rustc keeping as one node what the grammar cannot.
#[allow(clippy::too_many_arguments)]
pub(crate) fn split_groups(
    lang: &EmbeddedLang,
    flats: &[Flat],
    from: usize,
    to: usize,
    r: &Rendering,
    wrapped: &HashSet<usize>,
    tree: &GreenNode,
    base: u32,
    repairs: &[Repair],
    first_sig: usize,
) -> Result<Vec<usize>, Error> {
    let tts = lang.sg.nt_names.iter().position(|n| n == "tts").map(|i| i as u16);
    let NodeSpans { nodes: exact, trees } = node_spans(tree, base, tts);
    let nodes: Vec<(u32, u32)> = exact.iter().copied().collect();
    let repair_at: Vec<u32> =
        repairs.iter().map(|rep| r.sig_byte(first_sig + rep.at_terminal)).collect();
    let mut split = Vec::new();
    for i in from..to {
        let Flat::Open { group, close } = &flats[i] else { continue };
        let Delim::Invisible(kind) = group.delim else { continue };
        if wrapped.contains(&i) || *close > to {
            continue;
        }
        let Some((s, e)) = r.content_span(i, *close) else { continue };
        if trees.iter().any(|&(a, b)| a <= s && e <= b) {
            continue;
        }
        // An operand (an expression, a pattern, a type, a literal) must be
        // one node, or the grammar grouped it with what is around it. What
        // has no operators (an item, a statement, a block, a path, a
        // visibility, an attribute) needs only that no node straddles its
        // edges: the attributes before an item, the `;` after a statement
        // and a visibility's place in a field are the grammar's to group.
        // An item's or a statement's attributes are one list with those
        // written before it (rustc puts them on it): that list may straddle
        // its start.
        let attributed = matches!(kind, MetaVarKind::Item | MetaVarKind::Stmt { .. });
        let straddled = || {
            nodes.iter().any(|&(a, b)| {
                (a < s && s < b && b < e && !(attributed && attributes_only(flats, r, a, b)))
                    || (s < a && a < e && e < b)
            })
        };
        let regrouped = if wrappable(kind) { !exact.contains(&(s, e)) } else { straddled() };
        let broken = regrouped || repair_at.iter().any(|&b| b >= s && b <= e);
        if !broken {
            continue;
        }
        if wrappable(kind) {
            split.push(i);
        } else {
            return Err(Error::new(
                format!("a `{}` fragment that is no one node where it stands", kind_name(kind)),
                Some(group.open),
            ));
        }
    }
    Ok(split)
}

pub(crate) fn kind_name(kind: MetaVarKind) -> &'static str {
    match kind {
        MetaVarKind::Item => "item",
        MetaVarKind::Block => "block",
        MetaVarKind::Stmt { .. } => "stmt",
        MetaVarKind::Pat(_) => "pat",
        MetaVarKind::Expr { .. } => "expr",
        MetaVarKind::Ty { .. } => "ty",
        MetaVarKind::Meta { .. } => "meta",
        MetaVarKind::Path => "path",
        MetaVarKind::Vis => "vis",
        MetaVarKind::Literal => "literal",
    }
}
