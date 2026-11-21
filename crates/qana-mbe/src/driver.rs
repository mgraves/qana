//! Expanding an invocation and the invocations its expansion holds, in
//! turn, to rustc's recursion limit.
//!
//! Where an expansion's invocations stand, and what each expands to, the
//! grammar says (`@expansion`); a `macro_rules!` an expansion defines is
//! seen after it, as far as its scope reaches (a block, a module unless
//! `#[macro_use]` is on it), before any the caller resolves.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use qana_grammar::{GreenChild, GreenNode};
use qana_lang::EmbeddedLang;

use crate::parsed::Parsed;
use crate::render::Flat;
use crate::rules::{Form, MacroRules};
use crate::token::{Token, TokenKind, TokenStream, Tree};
use crate::{expand, Error};

/// rustc's default `recursion_limit`: how deep expansions may nest.
pub const RECURSION_LIMIT: usize = 128;

/// An invocation expanded: the tokens its arm transcribed (marked with
/// `mark`), the parse of them where it stands, and the invocations in them.
#[derive(Clone, Debug)]
pub struct Expansion {
    pub mark: u32,
    pub tokens: TokenStream,
    pub parsed: Parsed,
    pub nested: Vec<Nested>,
}

/// An invocation inside an expansion.
#[derive(Clone, Debug)]
pub struct Nested {
    /// Its span in the expansion's text ([`Parsed::text`]).
    pub span: (u32, u32),
    /// The path of the macro it invokes.
    pub path: Vec<Token>,
    /// What it passes the macro: the tokens inside its delimiters.
    pub input: TokenStream,
    /// The rule its expansion is parsed at.
    pub entry: String,
    pub outcome: Outcome,
}

#[derive(Clone, Debug)]
pub enum Outcome {
    Expanded(Box<Expansion>),
    /// No `macro_rules!` the expansion defines, nor the caller, says what
    /// the path names (a built-in macro, a procedural one).
    Unresolved,
    Failed(Error),
}

/// Expand `input` by `rules` and parse the expansion at `entry`, then each
/// invocation in it in turn: those a `macro_rules!` of the expansions
/// defines, before them, by it, the rest by what `resolve` says the path
/// names. Marks are numbered from 1, the top expansion's, in the order the
/// expansions are made.
pub fn expand_nested(
    lang: &EmbeddedLang,
    rules: &MacroRules,
    input: &TokenStream,
    entry: &str,
    resolve: &mut dyn FnMut(&[Token]) -> Option<Arc<MacroRules>>,
) -> Result<Expansion, Error> {
    let mut driver = Driver::new(lang, resolve);
    driver.run(rules, input, entry, 1)
}

/// Parse `stream` at `entry` (a file's items, say) and expand each
/// invocation in it as [`expand_nested`] does, in textual order: those a
/// `macro_rules!` before it in the stream or in an expansion defines by
/// it, the rest by what `resolve` says. Marks are numbered from 1.
pub fn expand_all(
    lang: &EmbeddedLang,
    stream: &TokenStream,
    entry: &str,
    resolve: &mut dyn FnMut(&[Token]) -> Option<Arc<MacroRules>>,
) -> Result<(Parsed, Vec<Nested>), Error> {
    let parsed = stream.parse(lang, entry)?;
    let mut driver = Driver::new(lang, resolve);
    let mut nested = Vec::new();
    let tree = parsed.tree.clone();
    driver.walk(&parsed, &tree, 0, 0, &mut nested);
    Ok((parsed, nested))
}

type Local = (String, Result<Arc<MacroRules>, Error>);

struct Driver<'a, 'r> {
    lang: &'a EmbeddedLang,
    resolve: &'r mut dyn FnMut(&[Token]) -> Option<Arc<MacroRules>>,
    next_mark: u32,
    /// The `macro_rules!` seen so far, in textual order, still in scope.
    locals: Vec<Local>,
    /// (nt, prod) → (the entry its invocation expands at, the child that
    /// holds the invocation).
    expansions: HashMap<(u16, u16), (u16, Option<usize>)>,
    /// Productions that are invocations.
    invocations: HashSet<(u16, u16)>,
    /// Productions that define a macro.
    macro_defs: HashSet<(u16, u16)>,
    /// Productions that open a scope, and those that are a module with its
    /// body.
    scopes: HashSet<(u16, u16)>,
    modules: HashSet<(u16, u16)>,
}

impl<'a, 'r> Driver<'a, 'r> {
    fn new(
        lang: &'a EmbeddedLang,
        resolve: &'r mut dyn FnMut(&[Token]) -> Option<Arc<MacroRules>>,
    ) -> Self {
        let b = &lang.binding;
        let mut expansions = HashMap::new();
        let mut invocations: HashSet<(u16, u16)> = b.expands.iter().copied().collect();
        for &(nt, prod, entry, child) in &b.expansions {
            expansions.insert((nt, prod), (entry, child));
            if child.is_none() {
                invocations.insert((nt, prod));
            }
        }
        let in_macro_ns: HashSet<(u16, u16)> = b
            .namespaces
            .iter()
            .filter(|(_, _, ns)| ns == "macro")
            .map(|&(nt, prod, _)| (nt, prod))
            .collect();
        let macro_defs = b
            .defs
            .iter()
            .map(|&(nt, prod, _)| (nt, prod))
            .filter(|k| in_macro_ns.contains(k))
            .collect();
        let scopes = b.scopes.iter().map(|&(nt, prod, _, _)| (nt, prod)).collect();
        let modules = b
            .units
            .iter()
            .filter(|(_, _, _, body)| body.is_some())
            .map(|&(nt, prod, _, _)| (nt, prod))
            .collect();
        Driver {
            lang,
            resolve,
            next_mark: 1,
            locals: Vec::new(),
            expansions,
            invocations,
            macro_defs,
            scopes,
            modules,
        }
    }

    fn run(
        &mut self,
        rules: &MacroRules,
        input: &TokenStream,
        entry: &str,
        depth: usize,
    ) -> Result<Expansion, Error> {
        let mark = self.next_mark;
        self.next_mark += 1;
        let tokens = expand(self.lang, rules, input, mark)?;
        let parsed = tokens.parse(self.lang, entry)?;
        let mut nested = Vec::new();
        let tree = parsed.tree.clone();
        self.walk(&parsed, &tree, 0, depth, &mut nested);
        Ok(Expansion { mark, tokens, parsed, nested })
    }

    fn walk(
        &mut self,
        parsed: &Parsed,
        node: &GreenNode,
        base: u32,
        depth: usize,
        out: &mut Vec<Nested>,
    ) {
        let key = (node.nt, node.prod);
        if self.macro_defs.contains(&key) {
            self.define(parsed, node, base);
            return;
        }
        if let Some(&(entry, child)) = self.expansions.get(&key) {
            match child {
                None => {
                    self.invoke(parsed, node, base, entry, depth, out);
                    return;
                }
                Some(c) => {
                    if let Some((index, inv, inv_base)) = self.held_invocation(node, base, c) {
                        // The rest of the node, then the invocation where it
                        // stands among it.
                        let mut off = base;
                        let mut k = 0usize;
                        for ch in &node.children {
                            let w = ch.width();
                            if is_symbol(ch) {
                                if k == index {
                                    self.invoke(parsed, inv, inv_base, entry, depth, out);
                                } else if let GreenChild::Node(m) = ch {
                                    self.walk(parsed, m, off, depth, out);
                                }
                                k += 1;
                            } else if let GreenChild::Node(m) = ch {
                                self.walk(parsed, m, off, depth, out);
                            }
                            off += w;
                        }
                        return;
                    }
                }
            }
        }
        let scoped = self.scopes.contains(&key);
        let keep = scoped && self.modules.contains(&key) && macro_use(parsed, node, base);
        let level = self.locals.len();
        let mut off = base;
        for ch in &node.children {
            if let GreenChild::Node(m) = ch {
                self.walk(parsed, m, off, depth, out);
            }
            off += ch.width();
        }
        if scoped && !keep {
            self.locals.truncate(level);
        }
    }

    /// The invocation symbol child `c` of `node` holds, through nodes that
    /// hold only it: the child's index, the node, where it begins.
    fn held_invocation<'n>(
        &self,
        node: &'n GreenNode,
        base: u32,
        c: usize,
    ) -> Option<(usize, &'n GreenNode, u32)> {
        let mut off = base;
        let mut k = 0usize;
        let mut found = None;
        for ch in &node.children {
            if is_symbol(ch) {
                if k == c {
                    if let GreenChild::Node(m) = ch {
                        found = Some((m.as_ref(), off));
                    }
                    break;
                }
                k += 1;
            }
            off += ch.width();
        }
        let (mut n, mut at) = found?;
        loop {
            if self.invocations.contains(&(n.nt, n.prod)) {
                return Some((c, n, at));
            }
            // Down through a node that holds only one node.
            let mut inner = None;
            let mut count = 0usize;
            let mut o = at;
            for ch in &n.children {
                if is_symbol(ch) {
                    count += 1;
                    if let GreenChild::Node(m) = ch {
                        inner = Some((m.as_ref(), o));
                    }
                }
                o += ch.width();
            }
            match (count, inner) {
                (1, Some((m, o))) => {
                    n = m;
                    at = o;
                }
                _ => return None,
            }
        }
    }

    /// The tokens of the flats a node spans: its first and last flat.
    fn flat_range(parsed: &Parsed, node: &GreenNode, base: u32) -> Option<(usize, usize)> {
        let (s, e) = content_span(node, base)?;
        let first = parsed.flat_spans.iter().position(|&(a, b)| b > a && a >= s)?;
        let last = parsed.flat_spans.iter().rposition(|&(a, b)| b > a && b <= e)?;
        Some((first, last))
    }

    /// A `macro_rules! name { … }`: read it and put it in scope.
    fn define(&mut self, parsed: &Parsed, node: &GreenNode, base: u32) {
        let Some((first, last)) = Self::flat_range(parsed, node, base) else { return };
        let flats = &parsed.flats[first..=last];
        let Some(bang) = flats.iter().position(|f| matches!(f, Flat::Token(t) if t.is_punct("!")))
        else {
            return;
        };
        let path: Vec<&Token> = flats[..bang]
            .iter()
            .filter_map(|f| match f {
                Flat::Token(t) => Some(t),
                _ => None,
            })
            .collect();
        if !(path.len() == 1 && path[0].is_keyword("macro_rules")) {
            // `path! name { … }`: an invocation of another macro, which the
            // grammar reads as a definition's shape; rustc refuses it.
            return;
        }
        let Some(Flat::Token(name)) = flats.get(bang + 1) else { return };
        let Some((name, _)) = name.ident() else { return };
        let body = trees_of(&flats[bang + 2..]);
        let rules = MacroRules::read(&TokenStream::new(body), Form::MacroRules).map(Arc::new);
        self.locals.push((name.to_string(), rules));
    }

    fn invoke(
        &mut self,
        parsed: &Parsed,
        node: &GreenNode,
        base: u32,
        entry: u16,
        depth: usize,
        out: &mut Vec<Nested>,
    ) {
        let Some((first, last)) = Self::flat_range(parsed, node, base) else { return };
        let span = (parsed.flat_spans[first].0, parsed.flat_spans[last].1);
        let flats = &parsed.flats[first..=last];
        let entry = self.lang.sg.nt_names[entry as usize].clone();
        let Some(bang) = flats.iter().position(|f| matches!(f, Flat::Token(t) if t.is_punct("!")))
        else {
            return;
        };
        let path: Vec<Token> = flats[..bang]
            .iter()
            .filter_map(|f| match f {
                Flat::Token(t) => Some(t.clone()),
                _ => None,
            })
            .collect();
        let Some(Flat::Open { group, .. }) = flats.get(bang + 1) else { return };
        let args = TokenStream::new(group.trees.to_vec());
        let local = match path.as_slice() {
            [one] if one.kind == TokenKind::Ident && &*one.text != "$crate" => {
                let (name, _) = one.ident().expect("an identifier");
                self.locals.iter().rev().find(|(n, _)| n == name).map(|(_, r)| r.clone())
            }
            _ => None,
        };
        let rules = match local {
            Some(Ok(rules)) => Some(rules),
            Some(Err(e)) => {
                out.push(Nested { span, path, input: args, entry, outcome: Outcome::Failed(e) });
                return;
            }
            None => (self.resolve)(&path),
        };
        let Some(rules) = rules else {
            out.push(Nested { span, path, input: args, entry, outcome: Outcome::Unresolved });
            return;
        };
        if depth + 1 > RECURSION_LIMIT {
            let name = path.last().map(|t| t.text.to_string()).unwrap_or_default();
            let at = path.last().map(|t| t.origin);
            out.push(Nested {
                span,
                path,
                input: args,
                entry,
                outcome: Outcome::Failed(Error::new(
                    format!("recursion limit reached while expanding `{name}!`"),
                    at,
                )),
            });
            return;
        }
        let outcome = match self.run(&rules, &args, &entry, depth + 1) {
            Ok(expansion) => Outcome::Expanded(Box::new(expansion)),
            Err(e) => Outcome::Failed(e),
        };
        out.push(Nested { span, path, input: args, entry, outcome });
    }
}

fn is_symbol(ch: &GreenChild) -> bool {
    match ch {
        GreenChild::Token(t) => !t.trivia && !t.is_missing(),
        GreenChild::Node(n) => n.nt != qana_grammar::green::ERROR_NT,
    }
}

/// A node's span from its first non-trivia token to its last.
fn content_span(node: &GreenNode, base: u32) -> Option<(u32, u32)> {
    fn go(n: &GreenNode, base: u32, span: &mut Option<(u32, u32)>) {
        let mut off = base;
        for c in &n.children {
            let w = c.width();
            match c {
                GreenChild::Token(t) if !t.trivia && w > 0 => {
                    *span = Some(match *span {
                        None => (off, off + w),
                        Some((a, _)) => (a, off + w),
                    })
                }
                GreenChild::Token(_) => {}
                GreenChild::Node(m) => go(m, off, span),
            }
            off += w;
        }
    }
    let mut span = None;
    go(node, base, &mut span);
    span
}

/// The trees the flats `flats` (balanced) are.
fn trees_of(flats: &[Flat]) -> Vec<Tree> {
    let mut out = Vec::new();
    let mut i = 0usize;
    // `close` indices are the whole expansion's; a group's flats are
    // skipped by counting its depth.
    while i < flats.len() {
        match &flats[i] {
            Flat::Token(t) => {
                out.push(Tree::Token(t.clone()));
                i += 1;
            }
            Flat::Open { group, .. } => {
                out.push(Tree::Group(group.clone()));
                let mut depth = 0usize;
                loop {
                    match &flats[i] {
                        Flat::Open { .. } => depth += 1,
                        Flat::Close { .. } => depth -= 1,
                        Flat::Token(_) => {}
                    }
                    i += 1;
                    if depth == 0 {
                        break;
                    }
                }
            }
            Flat::Close { .. } => i += 1,
        }
    }
    out
}

/// Whether a module carries `#[macro_use]`: its macros are then seen after
/// it. The attributes stand before its `mod`, after any visibility.
fn macro_use(parsed: &Parsed, node: &GreenNode, base: u32) -> bool {
    let Some((s, _)) = content_span(node, base) else { return false };
    let tokens = &parsed.tokens;
    let Some(mut j) = tokens.iter().position(|t| t.span.0 >= s) else { return false };
    // The node may begin with its attributes: look from its `mod`.
    while j < tokens.len() && !tokens[j].token.is_keyword("mod") {
        j += 1;
    }
    // Back over `unsafe`, a visibility, then each attribute.
    let mut k = j;
    let prev = |k: usize| k.checked_sub(1).map(|p| &tokens[p].token);
    if prev(k).is_some_and(|t| t.is_keyword("unsafe")) {
        k -= 1;
    }
    if prev(k).is_some_and(|t| t.kind == TokenKind::Delimiter && &*t.text == ")") {
        // `pub(…)`: back to its `(`, then `pub`.
        let mut d = 0usize;
        while k > 0 {
            k -= 1;
            let t = &tokens[k].token;
            if t.kind == TokenKind::Delimiter && &*t.text == ")" {
                d += 1;
            } else if t.kind == TokenKind::Delimiter && &*t.text == "(" {
                d -= 1;
                if d == 0 {
                    break;
                }
            }
        }
    }
    if prev(k).is_some_and(|t| t.is_keyword("pub")) {
        k -= 1;
    }
    loop {
        match prev(k) {
            Some(t) if t.kind == TokenKind::DocComment => k -= 1,
            Some(t) if t.kind == TokenKind::Delimiter && &*t.text == "]" => {
                let close = k - 1;
                let mut d = 0usize;
                let mut open = close;
                loop {
                    let t = &tokens[open].token;
                    if t.kind == TokenKind::Delimiter && &*t.text == "]" {
                        d += 1;
                    } else if t.kind == TokenKind::Delimiter && &*t.text == "[" {
                        d -= 1;
                        if d == 0 {
                            break;
                        }
                    }
                    if open == 0 {
                        return false;
                    }
                    open -= 1;
                }
                if open == 0 || !tokens[open - 1].token.is_punct("#") {
                    return false;
                }
                if tokens.get(open + 1).is_some_and(|t| t.token.is_keyword("macro_use")) {
                    return true;
                }
                k = open - 1;
            }
            _ => return false,
        }
    }
}
