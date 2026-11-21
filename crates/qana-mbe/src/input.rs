//! An invocation's tokens as the matcher reads them: a cursor over the
//! flattened trees, rustc's tests of which fragment a token can begin, and
//! the black-box parse of a fragment, by the grammar.

use std::collections::HashSet;
use std::sync::Arc;

use qana_grammar::{batch_parse_from, parse_prefix};
use qana_lang::EmbeddedLang;

use crate::render::{flatten, render, split_groups, wrappable, Flat, Rendering};
use crate::rules::Fragment;
use crate::token::{
    is_path_segment_keyword, is_reserved, Delim, ExprKind, MetaVarKind, PatKind, Semi, Token,
    TokenKind, Tree,
};
use crate::{Error, Origin};

/// Where the matcher stands: a flat, and how far into its token (rustc's
/// parser splits `>>` where a generic list ends, leaving `>`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Cursor {
    pub flat: usize,
    pub at: u32,
}

/// The token at a cursor, as the matcher compares and tests it.
#[derive(Clone, Debug)]
pub(crate) enum Cur {
    Token(Token),
    Open(Delim),
    Close(Delim),
    Eof,
}

/// What a fragment matched (rustc's `ParseNtResult`).
#[derive(Clone, Debug)]
pub(crate) enum Nt {
    Tt(Tree),
    Ident(Token),
    Lifetime(Token),
    /// A fragment of another kind: its trees, and the kind its invisible
    /// group gets when it is transcribed.
    Node {
        kind: MetaVarKind,
        trees: Vec<Tree>,
    },
}

pub(crate) struct Input<'l> {
    lang: &'l EmbeddedLang,
    pub flats: Vec<Flat>,
    /// The invisible groups (by opening flat) written in parentheses, which
    /// the grammar would otherwise not read as one node.
    wrapped: HashSet<usize>,
    rendering: Option<Rendering>,
}

impl<'l> Input<'l> {
    pub(crate) fn new(lang: &'l EmbeddedLang, trees: &[Tree]) -> Input<'l> {
        Input { lang, flats: flatten(trees), wrapped: HashSet::new(), rendering: None }
    }

    /// The flats written out, with the groups wrapped so far.
    fn ensure_rendering(&mut self) {
        if self.rendering.is_none() {
            self.rendering = Some(render(self.lang, &self.flats, &self.wrapped, &HashSet::new()));
        }
    }

    pub(crate) fn start(&self) -> Cursor {
        Cursor { flat: 0, at: 0 }
    }

    pub(crate) fn cur(&self, c: Cursor) -> Cur {
        match self.flats.get(c.flat) {
            None => Cur::Eof,
            Some(Flat::Token(t)) if c.at == 0 => Cur::Token(t.clone()),
            Some(Flat::Token(t)) => Cur::Token(slice_token(t, c.at, t.text.len() as u32)),
            Some(Flat::Open { group, .. }) => Cur::Open(group.delim),
            Some(Flat::Close { delim, .. }) => Cur::Close(*delim),
        }
    }

    pub(crate) fn bump(&self, c: Cursor) -> Cursor {
        Cursor { flat: c.flat + 1, at: 0 }
    }

    /// Where the matcher stands, for messages.
    pub(crate) fn origin(&self, c: Cursor) -> Option<Origin> {
        match self.flats.get(c.flat) {
            Some(Flat::Token(t)) => Some(t.origin),
            Some(Flat::Open { group, .. }) => Some(group.open),
            Some(Flat::Close { origin, .. }) => Some(*origin),
            None => self.flats.last().map(|f| match f {
                Flat::Token(t) => t.origin,
                Flat::Open { group, .. } => group.open,
                Flat::Close { origin, .. } => *origin,
            }),
        }
    }

    /// The trees from `start` to `end`, tokens cut where the cursors stand
    /// inside them.
    fn trees_between(&self, start: Cursor, end: Cursor) -> Vec<Tree> {
        let mut out = Vec::new();
        let mut i = start.flat;
        while i < end.flat || (i == end.flat && end.at > 0) {
            match &self.flats[i] {
                Flat::Token(t) => {
                    let lo = if i == start.flat { start.at } else { 0 };
                    let hi = if i == end.flat { end.at } else { t.text.len() as u32 };
                    out.push(Tree::Token(if lo == 0 && hi == t.text.len() as u32 {
                        t.clone()
                    } else {
                        slice_token(t, lo, hi)
                    }));
                    i += 1;
                }
                Flat::Open { group, close } => {
                    out.push(Tree::Group(group.clone()));
                    i = close + 1;
                }
                Flat::Close { .. } => unreachable!("a fragment's trees are balanced"),
            }
        }
        out
    }

    /// Parse a fragment of `kind` at `c`, as rustc's `parse_nonterminal`
    /// does: what it matched and where the matcher goes on.
    pub(crate) fn parse_nonterminal(
        &mut self,
        kind: Fragment,
        c: Cursor,
    ) -> Result<(Nt, Cursor), Error> {
        let here = self.origin(c);
        match kind {
            Fragment::Tt => match &self.flats.get(c.flat) {
                Some(Flat::Open { group, close }) => {
                    Ok((Nt::Tt(Tree::Group(group.clone())), Cursor { flat: close + 1, at: 0 }))
                }
                Some(Flat::Token(_)) => {
                    let Cur::Token(t) = self.cur(c) else { unreachable!() };
                    Ok((Nt::Tt(Tree::Token(t)), self.bump(c)))
                }
                _ => Err(Error::new("expected a token tree", here)),
            },
            Fragment::Ident => match self.cur(c) {
                Cur::Token(t) if matches!(t.ident(), Some((name, _)) if name != "_") => {
                    Ok((Nt::Ident(t), self.bump(c)))
                }
                _ => Err(Error::new("expected ident", here)),
            },
            Fragment::Lifetime => match self.cur(c) {
                Cur::Token(t) if t.kind == TokenKind::Lifetime => {
                    Ok((Nt::Lifetime(t), self.bump(c)))
                }
                _ => Err(Error::new("expected a lifetime", here)),
            },
            _ => {
                // A fragment of its kind, forwarded whole, is all rustc's
                // parser takes for some kinds (`eat_metavar_seq`).
                let whole = match self.flats.get(c.flat) {
                    Some(Flat::Open { group, close }) => match group.delim {
                        Delim::Invisible(k) if takes_whole(kind, k) => {
                            Some((group.clone(), *close))
                        }
                        _ => None,
                    },
                    _ => None,
                };
                if let Some((group, close)) = whole {
                    let trees = vec![Tree::Group(group)];
                    let end = Cursor { flat: close + 1, at: 0 };
                    let new_kind = self.node_kind(kind, &trees, c, end)?;
                    return Ok((Nt::Node { kind: new_kind, trees }, end));
                }
                let end = self.parse_by_grammar(kind, c)?;
                let trees = self.trees_between(c, end);
                let new_kind = self.node_kind(kind, &trees, c, end)?;
                Ok((Nt::Node { kind: new_kind, trees }, end))
            }
        }
    }

    /// The grammar's greedy parse of fragment `kind` from `c`: where it
    /// ends. A forwarded fragment inside it the grammar does not read as
    /// one node is wrapped in parentheses, and the parse done again.
    fn parse_by_grammar(&mut self, kind: Fragment, c: Cursor) -> Result<Cursor, Error> {
        let lang = self.lang;
        let rule = kind.rule().expect("a grammar fragment");
        let nt = lang.sg.fragment(rule).ok_or_else(|| {
            Error::new(format!("the grammar declares no fragment `{rule}`"), None)
        })?;
        let state = lang
            .sg
            .start_state(lang.tables, nt)
            .ok_or_else(|| Error::new(format!("fragment `{rule}` has no entry"), None))?;
        let here = self.origin(c);
        loop {
            let flats_len = self.flats.len();
            self.ensure_rendering();
            let r = self.rendering.as_ref().expect("rendered");
            let byte = r.spans.get(c.flat).map(|s| s.0 + c.at).unwrap_or(r.text.len() as u32);
            let g = r.sig_at(byte);
            let n = match parse_prefix(lang.sg, lang.tables, state, &r.ids[g..]) {
                Ok(n) => n,
                Err(k) => {
                    // A forwarded fragment before where the parse stopped
                    // may be what the grammar read otherwise (rustc keeps
                    // it one node): those not wrapped yet are, and the
                    // parse is done again.
                    let stop = r.sig_byte(g + k);
                    let retry: Vec<usize> = (c.flat..flats_len)
                        .filter(|&i| {
                            self.flats[i].invisible().is_some_and(wrappable)
                                && matches!(self.flats[i], Flat::Open { .. })
                                && !self.wrapped.contains(&i)
                                && r.spans[i].0 <= stop
                        })
                        .collect();
                    if !retry.is_empty() {
                        self.wrapped.extend(retry);
                        self.rendering = None;
                        continue;
                    }
                    // rustc's parser's message: what it expected, and the
                    // token it found where the fragment could not go on.
                    let found = if g + k >= r.ids.len() {
                        Cur::Eof
                    } else {
                        let at = r.sig_start[g + k];
                        let stop = cursor_at(&self.flats, r, c, at, flats_len);
                        self.cur(stop)
                    };
                    let origin = match found {
                        Cur::Eof => here,
                        _ => origin_at(&self.flats, r, r.sig_start[g + k], here),
                    };
                    return Err(Error::new(
                        format!("expected {}, found {}", expected(kind), token_descr(&found)),
                        origin,
                    ));
                }
            };
            let end_byte = if n == 0 {
                byte
            } else {
                let last = r.sig_full[g + n - 1];
                r.sig_start[g + n - 1] + r.full[last].text.len() as u32
            };
            let end = cursor_at(&self.flats, r, c, end_byte, flats_len);
            // An invisible group the parse ends inside of is split.
            let taken = &self.flats[c.flat..end.flat.min(self.flats.len())];
            let mut split: Vec<usize> = taken
                .iter()
                .enumerate()
                .filter(|(_, f)| {
                    matches!(f, Flat::Open { close, .. } if *close >= end.flat)
                        && f.invisible().is_some()
                })
                .map(|(k, _)| c.flat + k)
                .collect();
            let has_groups = taken.iter().any(|f| f.invisible().is_some());
            if split.is_empty() && has_groups && n > 0 {
                let first = r.sig_full[g];
                let last = r.sig_full[g + n - 1];
                let toks = &r.full[first..=last];
                let (tree, _, repairs) = batch_parse_from(lang.sg, lang.tables, state, toks)
                    .map_err(|e| Error::new(format!("parse failed: {e:?}"), here))?;
                split = split_groups(
                    lang,
                    &self.flats,
                    c.flat,
                    end.flat,
                    r,
                    &self.wrapped,
                    &tree,
                    r.sig_start[g],
                    &repairs,
                    g,
                )?;
            }
            if split.is_empty() {
                return Ok(end);
            }
            for i in split {
                let k = self.flats[i].invisible().expect("an invisible group");
                if !wrappable(k) {
                    return Err(Error::new(
                        "a forwarded fragment that is no one node where it stands",
                        self.origin(Cursor { flat: i, at: 0 }),
                    ));
                }
                self.wrapped.insert(i);
            }
            self.rendering = None;
        }
    }

    /// The kind a fragment's invisible group gets when it is transcribed:
    /// rustc's `MetaVarKind`, with what it records of the node.
    fn node_kind(
        &mut self,
        kind: Fragment,
        trees: &[Tree],
        start: Cursor,
        end: Cursor,
    ) -> Result<MetaVarKind, Error> {
        Ok(match kind {
            Fragment::Item => MetaVarKind::Item,
            Fragment::Block => MetaVarKind::Block,
            Fragment::Stmt => MetaVarKind::Stmt { semi: self.stmt_semi(trees, start, end)? },
            Fragment::Pat(p) => MetaVarKind::Pat(p),
            Fragment::Expr(e) => {
                let (lit, string) = expr_literal_flags(trees);
                MetaVarKind::Expr {
                    kind: e,
                    can_begin_literal_maybe_minus: lit,
                    can_begin_string_literal: string,
                }
            }
            Fragment::Ty => MetaVarKind::Ty { is_path: self.ty_is_path(trees, start, end)? },
            Fragment::Meta => MetaVarKind::Meta { has_meta_form: has_meta_form(trees) },
            Fragment::Path => MetaVarKind::Path,
            Fragment::Vis => MetaVarKind::Vis,
            Fragment::Literal => MetaVarKind::Literal,
            Fragment::Ident | Fragment::Lifetime | Fragment::Tt => {
                unreachable!("read as tokens")
            }
        })
    }

    /// Whether the grammar needs a `;` after a statement where rustc's tree
    /// needs none: a forwarded statement's is what it was; otherwise the
    /// grammar's reading of the statement says (a `let`, an expression).
    fn stmt_semi(&mut self, trees: &[Tree], start: Cursor, end: Cursor) -> Result<Semi, Error> {
        if let [Tree::Group(g)] = trees {
            if let Delim::Invisible(MetaVarKind::Stmt { semi }) = g.delim {
                return Ok(semi);
            }
        }
        let lang = self.lang;
        let Some(state) =
            lang.sg.fragment("stmt").and_then(|nt| lang.sg.start_state(lang.tables, nt))
        else {
            return Err(Error::new("the grammar declares no fragment `stmt`", None));
        };
        let flats_len = self.flats.len();
        self.ensure_rendering();
        let r = self.rendering.as_ref().expect("rendered");
        let s = r.spans.get(start.flat).map(|s| s.0 + start.at).unwrap_or(r.text.len() as u32);
        let e =
            if end.flat < flats_len { r.spans[end.flat].0 + end.at } else { r.text.len() as u32 };
        let (g0, g1) = (r.sig_at(s), r.sig_at(e));
        if g1 == g0 {
            return Ok(Semi::Never);
        }
        let toks = &r.full[r.sig_full[g0]..=r.sig_full[g1 - 1]];
        let (tree, _, _) = batch_parse_from(lang.sg, lang.tables, state, toks)
            .map_err(|e| Error::new(format!("parse failed: {e:?}"), None))?;
        Ok(match lang.sg.prods[tree.prod as usize].name.as_deref() {
            Some("LetStmtFragment") => Semi::Always,
            Some("ExprStmtFragment") => Semi::UnlessTail,
            _ => Semi::Never,
        })
    }

    /// Whether a type is a path without a qualified self (rustc's
    /// `TyKind::Path(None, _)`): a forwarded type is what it was, a
    /// forwarded path is one, and otherwise its tokens are one path by the
    /// grammar's `path` fragment and begin with no `<`.
    fn ty_is_path(&mut self, trees: &[Tree], start: Cursor, end: Cursor) -> Result<bool, Error> {
        if let [Tree::Group(g)] = trees {
            match g.delim {
                Delim::Invisible(MetaVarKind::Ty { is_path }) => return Ok(is_path),
                Delim::Invisible(MetaVarKind::Path) => return Ok(true),
                _ => {}
            }
        }
        match trees.first() {
            Some(Tree::Token(t)) if t.is_punct("<") || t.is_punct("<<") => return Ok(false),
            None => return Ok(false),
            _ => {}
        }
        let lang = self.lang;
        let Some(state) =
            lang.sg.fragment("path").and_then(|nt| lang.sg.start_state(lang.tables, nt))
        else {
            return Err(Error::new("the grammar declares no fragment `path`", None));
        };
        let flats_len = self.flats.len();
        self.ensure_rendering();
        let r = self.rendering.as_ref().expect("rendered");
        let s = r.spans.get(start.flat).map(|s| s.0 + start.at).unwrap_or(r.text.len() as u32);
        let e =
            if end.flat < flats_len { r.spans[end.flat].0 + end.at } else { r.text.len() as u32 };
        let (g0, g1) = (r.sig_at(s), r.sig_at(e));
        let tokens = &r.ids[g0..g1];
        Ok(parse_prefix(lang.sg, lang.tables, state, tokens) == Ok(tokens.len()))
    }
}

/// Where the parse that took the text up to byte `end_byte` leaves the
/// matcher: in a token's middle, or at the first flat it did not take. An
/// empty invisible group belongs to what follows it, the delimiter that
/// closes one the parse entered to what precedes.
fn cursor_at(flats: &[Flat], r: &Rendering, from: Cursor, end_byte: u32, len: usize) -> Cursor {
    let mut opened: HashSet<usize> = HashSet::new();
    let mut i = from.flat;
    while i < len {
        let (s, e) = r.spans[i];
        let s = if i == from.flat { s + from.at } else { s };
        let taken = match &flats[i] {
            _ if e > s => {
                if e <= end_byte {
                    true
                } else if s < end_byte {
                    return Cursor { flat: i, at: end_byte - r.spans[i].0 };
                } else {
                    false
                }
            }
            Flat::Open { .. } => {
                // Bare: its tokens come after it.
                s < end_byte
            }
            Flat::Close { open, .. } => opened.contains(open) && s <= end_byte,
            Flat::Token(_) => s < end_byte,
        };
        if !taken {
            break;
        }
        if let Flat::Open { .. } = &flats[i] {
            opened.insert(i);
        }
        i += 1;
    }
    Cursor { flat: i, at: 0 }
}

/// What rustc's parser says it expected where a fragment of `kind` does
/// not parse.
fn expected(kind: Fragment) -> &'static str {
    match kind {
        Fragment::Expr(_) => "expression",
        Fragment::Ty => "type",
        Fragment::Pat(_) => "pattern",
        Fragment::Item => "item",
        Fragment::Stmt => "statement",
        Fragment::Block => "`{`",
        Fragment::Literal => "literal",
        Fragment::Path | Fragment::Meta => "identifier",
        Fragment::Vis => "visibility",
        Fragment::Ident => "identifier",
        Fragment::Lifetime => "lifetime",
        Fragment::Tt => "token tree",
    }
}

/// The origin of the flat written at byte `at`.
fn origin_at(flats: &[Flat], r: &Rendering, at: u32, or: Option<Origin>) -> Option<Origin> {
    flats
        .iter()
        .zip(&r.spans)
        .find(|(_, &(s, e))| s <= at && at < e)
        .map(|(f, _)| match f {
            Flat::Token(t) => t.origin,
            Flat::Open { group, .. } => group.open,
            Flat::Close { origin, .. } => *origin,
        })
        .or(or)
}

/// Whether rustc's parser takes a forwarded fragment of kind `k` whole,
/// and nothing after it, for a fragment of `kind` (`eat_metavar_seq` where
/// a parse of that kind begins).
fn takes_whole(kind: Fragment, k: MetaVarKind) -> bool {
    match kind {
        Fragment::Item => k == MetaVarKind::Item,
        Fragment::Block => k == MetaVarKind::Block,
        Fragment::Stmt => matches!(k, MetaVarKind::Stmt { .. }),
        Fragment::Pat(PatKind::Param { .. }) => matches!(k, MetaVarKind::Pat(_)),
        Fragment::Literal => matches!(k, MetaVarKind::Expr { .. } | MetaVarKind::Literal),
        Fragment::Ty => matches!(k, MetaVarKind::Ty { .. }),
        Fragment::Meta => matches!(k, MetaVarKind::Meta { .. }),
        Fragment::Path => matches!(k, MetaVarKind::Path | MetaVarKind::Ty { is_path: true }),
        Fragment::Vis => k == MetaVarKind::Vis,
        Fragment::Pat(PatKind::WithOr) | Fragment::Expr(_) => false,
        Fragment::Ident | Fragment::Lifetime | Fragment::Tt => false,
    }
}

/// A token cut to bytes `lo..hi` of its text, at the matching part of its
/// span where its span is its text.
pub(crate) fn slice_token(t: &Token, lo: u32, hi: u32) -> Token {
    let text = &t.text[lo as usize..hi as usize];
    let origin = if t.origin.end - t.origin.start == t.text.len() as u32 {
        Origin { source: t.origin.source, start: t.origin.start + lo, end: t.origin.start + hi }
    } else {
        t.origin
    };
    Token { text: Arc::from(text), origin, ..t.clone() }
}

/// Outer attributes at the start of trees (`#[…]`) skipped.
fn skip_outer_attrs(mut trees: &[Tree]) -> &[Tree] {
    while let [Tree::Token(pound), Tree::Group(g), rest @ ..] = trees {
        if pound.is_punct("#") && g.delim == Delim::Bracket {
            trees = rest;
        } else {
            break;
        }
    }
    trees
}

fn literal_token(t: &Token) -> bool {
    t.kind == TokenKind::Literal || t.is_keyword("true") || t.is_keyword("false")
}

/// Whether an expression is a literal (`ExprKind::Lit`: it can begin a
/// literal and a string literal) or a negated one (only the first): what
/// rustc's `Expr` invisible group records.
fn expr_literal_flags(trees: &[Tree]) -> (bool, bool) {
    let trees = skip_outer_attrs(trees);
    // A forwarded fragment is the expression it holds.
    let inner = |g: &crate::token::Group| -> Option<(bool, bool)> {
        match g.delim {
            Delim::Invisible(MetaVarKind::Expr {
                can_begin_literal_maybe_minus,
                can_begin_string_literal,
                ..
            }) => Some((can_begin_literal_maybe_minus, can_begin_string_literal)),
            Delim::Invisible(MetaVarKind::Literal) => {
                let negated = matches!(g.trees.first(), Some(Tree::Token(t)) if t.is_punct("-"));
                Some(if negated { (true, false) } else { (true, true) })
            }
            _ => None,
        }
    };
    match trees {
        [Tree::Token(t)] if literal_token(t) => (true, true),
        [Tree::Group(g)] => inner(g).unwrap_or((false, false)),
        [Tree::Token(minus), Tree::Token(t)] if minus.is_punct("-") && literal_token(t) => {
            (true, false)
        }
        [Tree::Token(minus), Tree::Group(g)] if minus.is_punct("-") => match inner(g) {
            Some((true, true)) => (true, false),
            _ => (false, false),
        },
        _ => (false, false),
    }
}

/// Whether an attribute's item is a meta item (rustc's `meta_kind()` is
/// some): a path alone, a path and a parenthesized list of meta items and
/// literals, or a path, `=` and a literal.
fn has_meta_form(trees: &[Tree]) -> bool {
    let mut trees = trees;
    if let [Tree::Token(u), Tree::Group(g)] = trees {
        if u.is_keyword("unsafe") && g.delim == Delim::Paren {
            return has_meta_form(&g.trees);
        }
    }
    // The path: `::`? name (`::` name)*, or a forwarded path.
    match trees {
        [Tree::Group(g), rest @ ..] if matches!(g.delim, Delim::Invisible(MetaVarKind::Path)) => {
            trees = rest
        }
        _ => {
            let mut k = 0usize;
            if matches!(trees.first(), Some(Tree::Token(t)) if t.is_punct("::")) {
                k += 1;
            }
            loop {
                match trees.get(k) {
                    Some(Tree::Token(t)) if t.kind == TokenKind::Ident => k += 1,
                    _ => return false,
                }
                match trees.get(k) {
                    Some(Tree::Token(t)) if t.is_punct("::") => k += 1,
                    _ => break,
                }
            }
            trees = &trees[k..];
        }
    }
    match trees {
        [] => true,
        [Tree::Group(g)] if g.delim == Delim::Paren => meta_list(&g.trees),
        [Tree::Group(_)] => false,
        [Tree::Token(eq), value @ ..] if eq.is_punct("=") => match value {
            [Tree::Token(t)] => literal_token(t),
            [Tree::Group(g)] => matches!(
                g.delim,
                Delim::Invisible(MetaVarKind::Literal)
                    | Delim::Invisible(MetaVarKind::Expr { can_begin_string_literal: true, .. })
            ),
            _ => false,
        },
        _ => false,
    }
}

/// rustc's `MetaItemKind::list_from_tokens`.
fn meta_list(trees: &[Tree]) -> bool {
    let mut k = 0usize;
    while k < trees.len() {
        let Some(next) = meta_item_inner(trees, k) else { return false };
        k = next;
        match trees.get(k) {
            None => {}
            Some(Tree::Token(t)) if t.is_punct(",") => k += 1,
            _ => return false,
        }
    }
    true
}

/// A meta item or a literal at `trees[k..]`: where it ends.
fn meta_item_inner(trees: &[Tree], k: usize) -> Option<usize> {
    match trees.get(k)? {
        Tree::Token(t) if literal_token(t) && t.kind == TokenKind::Literal => return Some(k + 1),
        Tree::Token(t) if t.is_keyword("true") || t.is_keyword("false") => return Some(k + 1),
        Tree::Group(g) => {
            if let Delim::Invisible(MetaVarKind::Meta { has_meta_form: true }) = g.delim {
                let _ = meta_item_inner(&g.trees, 0)?;
                return Some(k + 1);
            }
            return None;
        }
        _ => {}
    }
    // A meta item: its path, then a list, `= literal`, or nothing.
    let mut i = k;
    if matches!(trees.get(i), Some(Tree::Token(t)) if t.is_punct("::")) {
        i += 1;
    }
    loop {
        match trees.get(i) {
            Some(Tree::Token(t)) if t.kind == TokenKind::Ident => i += 1,
            _ => return None,
        }
        match trees.get(i) {
            Some(Tree::Token(t)) if t.is_punct("::") => i += 1,
            _ => break,
        }
    }
    match trees.get(i) {
        Some(Tree::Group(g)) if g.delim == Delim::Paren => {
            if meta_list(&g.trees) {
                Some(i + 1)
            } else {
                None
            }
        }
        Some(Tree::Group(_)) => None,
        Some(Tree::Token(eq)) if eq.is_punct("=") => match trees.get(i + 1) {
            Some(Tree::Token(t)) if literal_token(t) => Some(i + 2),
            _ => None,
        },
        _ => Some(i),
    }
}

// ---- which fragments a token can begin (rustc's `nonterminal_may_begin_with`) ----

fn ident_can_begin_expr(name: &str, raw: bool, t: &Token) -> bool {
    (raw || !is_reserved(name, t.edition))
        || (!raw && is_path_segment_keyword(name))
        || [
            "async", "do", "box", "break", "const", "continue", "false", "for", "gen", "if", "let",
            "loop", "match", "move", "return", "true", "try", "unsafe", "while", "yield", "safe",
            "static",
        ]
        .contains(&name)
}

fn ident_can_begin_type(name: &str, raw: bool, t: &Token) -> bool {
    (raw || !is_reserved(name, t.edition))
        || (!raw && is_path_segment_keyword(name))
        || ["_", "for", "impl", "fn", "unsafe", "extern", "typeof", "dyn"].contains(&name)
}

pub(crate) fn can_begin_expr(cur: &Cur) -> bool {
    match cur {
        Cur::Token(t) => match t.kind {
            TokenKind::Ident => {
                let (name, raw) = t.ident().expect("an identifier");
                ident_can_begin_expr(name, raw, t)
            }
            TokenKind::Literal | TokenKind::Lifetime => true,
            TokenKind::Punct => matches!(
                &*t.text,
                "!" | "-"
                    | "*"
                    | "|"
                    | "||"
                    | "&"
                    | "&&"
                    | ".."
                    | "..."
                    | "..="
                    | "<"
                    | "<<"
                    | "::"
                    | "#"
            ),
            TokenKind::DocComment | TokenKind::Delimiter => false,
        },
        Cur::Open(Delim::Paren | Delim::Brace | Delim::Bracket) => true,
        Cur::Open(Delim::Invisible(k)) => matches!(
            k,
            MetaVarKind::Block
                | MetaVarKind::Expr { .. }
                | MetaVarKind::Literal
                | MetaVarKind::Path
        ),
        Cur::Close(_) | Cur::Eof => false,
    }
}

pub(crate) fn token_can_begin_type(t: &Token) -> bool {
    can_begin_type(&Cur::Token(t.clone()))
}

pub(crate) fn can_begin_type(cur: &Cur) -> bool {
    match cur {
        Cur::Token(t) => match t.kind {
            TokenKind::Ident => {
                let (name, raw) = t.ident().expect("an identifier");
                ident_can_begin_type(name, raw, t)
            }
            TokenKind::Lifetime => true,
            TokenKind::Punct => {
                matches!(&*t.text, "!" | "*" | "&" | "&&" | "?" | "<" | "<<" | "::")
            }
            _ => false,
        },
        Cur::Open(Delim::Paren | Delim::Bracket) => true,
        Cur::Open(Delim::Invisible(k)) => matches!(k, MetaVarKind::Ty { .. } | MetaVarKind::Path),
        _ => false,
    }
}

fn can_begin_pattern(cur: &Cur, pat: PatKind) -> bool {
    match cur {
        Cur::Token(t) => match t.kind {
            TokenKind::Ident | TokenKind::Literal => true,
            TokenKind::Punct => match &*t.text {
                "&" | "-" | "&&" | ".." | "..." | "::" | "<" | "<<" => true,
                "|" => pat == PatKind::WithOr,
                _ => false,
            },
            _ => false,
        },
        Cur::Open(Delim::Paren | Delim::Bracket) => true,
        Cur::Open(Delim::Invisible(k)) => matches!(
            k,
            MetaVarKind::Expr { .. }
                | MetaVarKind::Literal
                | MetaVarKind::Meta { .. }
                | MetaVarKind::Pat(_)
                | MetaVarKind::Path
                | MetaVarKind::Ty { .. }
        ),
        _ => false,
    }
}

fn can_begin_literal_maybe_minus(cur: &Cur) -> bool {
    match cur {
        Cur::Token(t) => {
            t.kind == TokenKind::Literal
                || t.is_punct("-")
                || t.is_keyword("true")
                || t.is_keyword("false")
        }
        Cur::Open(Delim::Invisible(MetaVarKind::Literal)) => true,
        Cur::Open(Delim::Invisible(MetaVarKind::Expr {
            can_begin_literal_maybe_minus, ..
        })) => *can_begin_literal_maybe_minus,
        _ => false,
    }
}

fn is_keyword(cur: &Cur, kw: &str) -> bool {
    matches!(cur, Cur::Token(t) if t.is_keyword(kw))
}

/// How rustc's messages name a token (`token_descr`): a keyword, a
/// reserved identifier or keyword, a fragment a macro put in.
pub(crate) fn token_descr(cur: &Cur) -> String {
    match cur {
        Cur::Token(t) => {
            let kind = match t.kind {
                TokenKind::Ident => {
                    let (name, raw) = t.ident().expect("an identifier");
                    let used = USED.contains(&name)
                        || (t.edition >= crate::Edition::E2018 && USED_2018.contains(&name));
                    if raw {
                        if t.interpolated {
                            "identifier"
                        } else {
                            ""
                        }
                    } else if name == "_" || name == "$crate" {
                        "reserved identifier"
                    } else if used {
                        "keyword"
                    } else if is_reserved(name, t.edition) {
                        "reserved keyword"
                    } else if t.interpolated {
                        "identifier"
                    } else {
                        ""
                    }
                }
                TokenKind::Lifetime if t.interpolated => "lifetime",
                TokenKind::DocComment => "doc comment",
                _ => "",
            };
            if kind.is_empty() {
                format!("`{}`", t.text)
            } else {
                format!("{kind} `{}`", t.text)
            }
        }
        Cur::Open(Delim::Invisible(k)) => {
            format!("`{}` metavariable", crate::render::kind_name(*k))
        }
        Cur::Open(d) => format!("`{}`", d.open_text()),
        Cur::Close(Delim::Invisible(k)) => {
            format!("`{}` metavariable", crate::render::kind_name(*k))
        }
        Cur::Close(d) => format!("`{}`", d.close_text()),
        Cur::Eof => "end of macro arguments".to_string(),
    }
}

const USED: &[&str] = &[
    "as", "break", "const", "continue", "crate", "else", "enum", "extern", "false", "fn", "for",
    "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return",
    "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where",
    "while",
];
const USED_2018: &[&str] = &["async", "await", "dyn"];

/// Whether a fragment of `kind` may begin with `cur`: `false` keeps the
/// matcher from calling the parser there, and lets another arm be tried.
pub(crate) fn may_begin_with(kind: Fragment, cur: &Cur) -> bool {
    // Whether a forwarded fragment of kind `k` may be one identifier.
    fn may_be_ident(k: MetaVarKind) -> bool {
        !matches!(k, MetaVarKind::Item | MetaVarKind::Block | MetaVarKind::Vis)
    }
    match kind {
        Fragment::Expr(ExprKind::Expr2021 { .. }) => {
            can_begin_expr(cur) && !is_keyword(cur, "let") && !is_keyword(cur, "const")
        }
        Fragment::Expr(ExprKind::Expr) => {
            (can_begin_expr(cur) || is_keyword(cur, "_")) && !is_keyword(cur, "let")
        }
        Fragment::Ty => can_begin_type(cur),
        Fragment::Ident => {
            matches!(cur, Cur::Token(t) if matches!(t.ident(), Some((n, _)) if n != "_"))
        }
        Fragment::Literal => can_begin_literal_maybe_minus(cur),
        Fragment::Vis => match cur {
            Cur::Token(t) if t.is_punct(",") || t.kind == TokenKind::Ident => true,
            Cur::Token(t) if t.kind == TokenKind::Lifetime && t.interpolated => true,
            Cur::Open(Delim::Invisible(_)) => true,
            _ => can_begin_type(cur),
        },
        Fragment::Block => match cur {
            Cur::Open(Delim::Brace) => true,
            Cur::Token(t) => t.kind == TokenKind::Lifetime && t.interpolated,
            Cur::Open(Delim::Invisible(k)) => matches!(
                k,
                MetaVarKind::Block
                    | MetaVarKind::Stmt { .. }
                    | MetaVarKind::Expr { .. }
                    | MetaVarKind::Literal
            ),
            _ => false,
        },
        Fragment::Path | Fragment::Meta => match cur {
            Cur::Token(t) => t.is_punct("::") || t.kind == TokenKind::Ident,
            Cur::Open(Delim::Invisible(k)) => may_be_ident(*k),
            _ => false,
        },
        Fragment::Pat(p) => can_begin_pattern(cur, p),
        Fragment::Lifetime => matches!(cur, Cur::Token(t) if t.kind == TokenKind::Lifetime),
        Fragment::Tt | Fragment::Item | Fragment::Stmt => !matches!(cur, Cur::Close(_)),
    }
}
