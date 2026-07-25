//! The META TIER, v0: declared macros with binding-guided substitution.
//!
//! The toolchain predefines no macro system. A grammar DECLARES one:
//! `@macro(params, body)` marks a definition production (its `@def` is
//! the macro's name, the defs inside `params` are its parameters, and
//! `body` is the template — REAL syntax of the language, parsed and
//! bound like any other code), and `@splice(name[, args])` marks the
//! productions where expansion may happen.
//!
//! The elegance thesis, cashed: SUBSTITUTION IS BINDING. A parameter
//! occurrence in the body is an ordinary `@ref` that resolves to the
//! parameter's ordinary `@def` — so expanding means "replace each
//! body reference that RESOLVES to parameter i with argument i's
//! text". No token pattern-matching, no positional guesswork: the
//! binding tier the grammar already declared IS the macro semantics.
//! (A body typo is a parse error at the DEFINITION, and a stray name
//! is an unresolved-reference diagnostic — before any use exists.)
//!
//! One pass expands outermost uses only; uses inside macro BODIES are
//! never expanded at the definition (they expand after splicing, at
//! use sites) — the fixpoint driver in `rantlr-rg` re-parses and
//! re-binds each pass's output, which is what makes expansion "the
//! same engine, run again", and gives generated text full editor
//! intelligence for free. Every output byte carries PROVENANCE: a
//! segment list mapping it to the original text (verbatim runs, body
//! pieces from the definition, argument pieces from the use site),
//! composable across passes because every segment is a copy.

use crate::{SemDb, SymbolTable, Target};
use rantlr_grammar::green::{GreenChild, GreenNode, LIST_PROD};
use std::collections::HashMap;

/// The declared macro tier. Empty vectors = no tier (open world).
#[derive(Clone, Debug, Default)]
pub struct MacroConfig {
    /// (nt, prod, params child index or None, body child index) —
    /// `@macro([params,] body)` on a `@def`-bearing production.
    pub defs: Vec<(u16, u16, Option<usize>, usize)>,
    /// (nt, prod, callee child index, args child index or None) —
    /// `@splice(name[, args])`. Expansion happens ONLY at splice
    /// sites; a ref without `@splice` never expands (which is what
    /// keeps C's `#ifdef LIMIT` intact while `return LIMIT;` opens).
    pub uses: Vec<(u16, u16, usize, Option<usize>)>,
}

impl MacroConfig {
    pub fn declared(&self) -> bool {
        !self.defs.is_empty()
    }
}

/// Where an output byte came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegKind {
    /// Copied from the original text outside any expansion.
    Verbatim,
    /// Copied from a macro definition's body.
    Body,
    /// Copied from a use site's argument.
    Arg,
}

/// One provenance segment: output bytes `out` are a copy of source
/// bytes `src` (equal lengths — every segment is a copy, which is
/// what makes multi-pass composition exact).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seg {
    pub out: (u32, u32),
    pub src: (u32, u32),
    pub kind: SegKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MacroDiag {
    /// Span in the ORIGINAL text of the pass that raised it.
    pub span: (u32, u32),
    pub msg: String,
}

/// One expansion pass over a bound document.
pub struct PassOut {
    pub text: String,
    /// Tiles `text` exactly; `src` spans refer to the pass INPUT.
    pub segs: Vec<Seg>,
    pub substitutions: u32,
    pub diags: Vec<MacroDiag>,
}

struct MacroDef {
    params: Vec<usize>,
    /// Trimmed body span (absolute, in the pass input).
    body: (u32, u32),
    /// (ref span, parameter position) inside the body, sorted.
    param_refs: Vec<((u32, u32), usize)>,
}

struct Job {
    span: (u32, u32),
    def: usize,
    args: Vec<(u32, u32)>,
    /// Whether the splice DECLARED an args child — a non-macro callee
    /// is silent without one (every C name is a splice site) and a
    /// diagnostic with one (`x!(…)` on a non-macro is an error).
    has_args: bool,
}

fn trim_span(text: &str, span: (u32, u32)) -> (u32, u32) {
    let (mut s, mut e) = (span.0 as usize, span.1 as usize);
    let b = text.as_bytes();
    while s < e && b[s].is_ascii_whitespace() {
        s += 1;
    }
    while e > s && b[e - 1].is_ascii_whitespace() {
        e -= 1;
    }
    (s as u32, e as u32)
}

/// The k-th SYMBOL child's absolute span (and node, when it is one).
fn symbol_child<'a>(
    n: &'a GreenNode,
    base: u32,
    k: usize,
) -> Option<((u32, u32), Option<&'a GreenNode>)> {
    let mut off = base;
    let mut idx = 0usize;
    for c in &n.children {
        let w = c.width();
        let is_symbol = match c {
            GreenChild::Token(t) => !t.trivia && !t.is_missing(),
            GreenChild::Node(m) => m.nt != rantlr_grammar::green::ERROR_NT,
        };
        if is_symbol {
            if idx == k {
                let node = match c {
                    GreenChild::Node(m) => Some(&**m),
                    _ => None,
                };
                return Some(((off, off + w), node));
            }
            idx += 1;
        }
        off += w;
    }
    None
}

/// Token-exact span of a node: first to last real (non-trivia,
/// non-missing) token beneath. Node spans include leading trivia —
/// splicing must not swallow the space (or comment) before a use.
fn content_span(n: &GreenNode, base: u32) -> (u32, u32) {
    fn scan(n: &GreenNode, off: u32, lo: &mut u32, hi: &mut u32) {
        let mut o = off;
        for c in &n.children {
            match c {
                GreenChild::Token(t) if !t.trivia && !t.is_missing() && !t.text.is_empty() => {
                    *lo = (*lo).min(o);
                    *hi = (*hi).max(o + t.text.len() as u32);
                }
                GreenChild::Node(m) => scan(m, o, lo, hi),
                _ => {}
            }
            o += c.width();
        }
    }
    let (mut lo, mut hi) = (u32::MAX, 0u32);
    scan(n, base, &mut lo, &mut hi);
    if lo == u32::MAX { (base, base) } else { (lo, hi) }
}

/// Element spans of a list-valued child: unwrap single-node wrappers
/// down to the LIST node, then take its element nodes token-exactly.
/// A non-list leaf is a single element (empty content = no elements).
fn list_elements(n: &GreenNode, base: u32) -> Vec<(u32, u32)> {
    if n.prod == LIST_PROD {
        let mut out = Vec::new();
        let mut off = base;
        for c in &n.children {
            if let GreenChild::Node(m) = c {
                if m.nt != rantlr_grammar::green::ERROR_NT {
                    out.push(content_span(m, off));
                }
            }
            off += c.width();
        }
        return out;
    }
    // Unwrap a single-node wrapper (a list RULE nonterminal holding
    // the LIST node), tracking the child's base offset.
    let mut nodes = Vec::new();
    let mut off = base;
    for c in &n.children {
        let is_symbol = match c {
            GreenChild::Token(t) => !t.trivia && !t.is_missing(),
            GreenChild::Node(m) => m.nt != rantlr_grammar::green::ERROR_NT,
        };
        if is_symbol {
            nodes.push((off, c));
        }
        off += c.width();
    }
    if let [(o, GreenChild::Node(m))] = nodes.as_slice() {
        return list_elements(m, *o);
    }
    let s = content_span(n, base);
    if s.1 > s.0 { vec![s] } else { Vec::new() }
}

/// Run ONE expansion pass. `tree` must be the tree `db` holds for
/// `uri` (the caller just set it); `text` its exact source.
pub fn expand_pass(
    db: &mut SemDb,
    uri: &str,
    tree: &GreenNode,
    text: &str,
    cfg: &MacroConfig,
) -> PassOut {
    let st = db.symbols(uri);
    let res = db.resolve(uri);
    let mut diags = Vec::new();

    // ---- macro registry: every @macro node, keyed by the def it
    // introduces (the production's own @def, found by containment
    // outside the params/body children) ----
    let mut registry: HashMap<usize, MacroDef> = HashMap::new();
    let mut jobs: Vec<Job> = Vec::new();
    walk(tree, 0, cfg, &st, &res, text, &mut registry, &mut jobs);

    // Uses inside any macro BODY never expand at the definition —
    // they expand after splicing, at use sites (cpp and Rust agree).
    let bodies: Vec<(u32, u32)> = registry.values().map(|m| m.body).collect();
    jobs.retain(|j| !bodies.iter().any(|b| j.span.0 >= b.0 && j.span.1 <= b.1));

    // Outermost jobs only: an inner use travels verbatim inside the
    // outer's argument text and expands next pass.
    jobs.sort_by_key(|j| (j.span.0, std::cmp::Reverse(j.span.1)));
    let mut kept: Vec<Job> = Vec::new();
    for j in jobs {
        if kept.last().is_none_or(|p| j.span.0 >= p.span.1) {
            kept.push(j);
        }
    }

    // ---- emit ----
    let mut out = String::with_capacity(text.len());
    let mut segs: Vec<Seg> = Vec::new();
    let mut cursor = 0u32;
    let push = |out: &mut String, segs: &mut Vec<Seg>, src: (u32, u32), kind: SegKind| {
        if src.1 > src.0 {
            let start = out.len() as u32;
            out.push_str(&text[src.0 as usize..src.1 as usize]);
            segs.push(Seg { out: (start, out.len() as u32), src, kind });
        }
    };
    let mut substitutions = 0u32;
    for j in &kept {
        push(&mut out, &mut segs, (cursor, j.span.0), SegKind::Verbatim);
        let Some(m) = registry.get(&j.def) else {
            // Callee resolves, but not to a macro.
            if j.has_args {
                diags.push(MacroDiag {
                    span: j.span,
                    msg: "spliced name does not resolve to a macro".into(),
                });
            }
            push(&mut out, &mut segs, j.span, SegKind::Verbatim);
            cursor = j.span.1;
            continue;
        };
        if j.args.len() != m.params.len() {
            diags.push(MacroDiag {
                span: j.span,
                msg: format!(
                    "macro takes {} argument(s), {} supplied — left unexpanded",
                    m.params.len(),
                    j.args.len()
                ),
            });
            push(&mut out, &mut segs, j.span, SegKind::Verbatim);
            cursor = j.span.1;
            continue;
        }
        // Body pieces around parameter refs; args at the refs.
        let mut b = m.body.0;
        for &(rspan, pi) in &m.param_refs {
            push(&mut out, &mut segs, (b, rspan.0), SegKind::Body);
            push(&mut out, &mut segs, trim_span(text, j.args[pi]), SegKind::Arg);
            b = rspan.1;
        }
        push(&mut out, &mut segs, (b, m.body.1), SegKind::Body);
        substitutions += 1;
        cursor = j.span.1;
    }
    push(&mut out, &mut segs, (cursor, text.len() as u32), SegKind::Verbatim);

    PassOut { text: out, segs, substitutions, diags }
}

#[allow(clippy::too_many_arguments)]
fn walk(
    n: &GreenNode,
    base: u32,
    cfg: &MacroConfig,
    st: &SymbolTable,
    res: &[Target],
    text: &str,
    registry: &mut HashMap<usize, MacroDef>,
    jobs: &mut Vec<Job>,
) {
    let span = (base, base + n.width);
    if let Some(&(_, _, params_child, body_child)) =
        cfg.defs.iter().find(|&&(nt, p, _, _)| nt == n.nt && p == n.prod)
    {
        if let Some((body_raw, body_node)) = symbol_child(n, base, body_child) {
            let pspan = params_child.and_then(|k| symbol_child(n, base, k)).map(|(s, _)| s);
            let inside = |d: (u32, u32), c: (u32, u32)| d.0 >= c.0 && d.1 <= c.1;
            // The def this production introduces: within the node,
            // outside its params/body children.
            let def_idx = st.defs.iter().position(|d| {
                inside(d.span, span)
                    && !inside(d.span, body_raw)
                    && !pspan.is_some_and(|p| inside(d.span, p))
            });
            if let Some(di) = def_idx {
                let params: Vec<usize> = pspan
                    .map(|p| {
                        st.defs
                            .iter()
                            .enumerate()
                            .filter(|(_, d)| inside(d.span, p))
                            .map(|(i, _)| i)
                            .collect()
                    })
                    .unwrap_or_default();
                // Token-exact body: never splice a body's leading or
                // trailing trivia.
                let body = match body_node {
                    Some(m) => content_span(m, body_raw.0),
                    None => trim_span(text, body_raw),
                };
                let mut param_refs: Vec<((u32, u32), usize)> = st
                    .refs
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| inside(r.span, body))
                    .filter_map(|(ri, r)| match res.get(ri) {
                        Some(&Target::Local { def }) => {
                            params.iter().position(|&p| p == def).map(|pi| (r.span, pi))
                        }
                        _ => None,
                    })
                    .collect();
                param_refs.sort_by_key(|&(s, _)| s.0);
                registry.insert(di, MacroDef { params, body, param_refs });
            }
        }
    }
    if let Some(&(_, _, callee_child, args_child)) =
        cfg.uses.iter().find(|&&(nt, p, _, _)| nt == n.nt && p == n.prod)
    {
        if let Some((cspan, _)) = symbol_child(n, base, callee_child) {
            if let Some(ri) = st.refs.iter().position(|r| r.span == cspan) {
                if let Some(&Target::Local { def }) = res.get(ri) {
                    let args = args_child
                        .and_then(|k| symbol_child(n, base, k))
                        .and_then(|(s, node)| node.map(|m| list_elements(m, s.0)))
                        .unwrap_or_default();
                    // Whether this def is a MACRO is settled at emit
                    // time against the completed registry (a use may
                    // precede its macro's definition textually). The
                    // replaced span is token-exact — leading trivia
                    // stays in the surrounding text.
                    jobs.push(Job {
                        span: content_span(n, base),
                        def,
                        args,
                        has_args: args_child.is_some(),
                    });
                }
            }
        }
    }
    let mut off = base;
    for c in &n.children {
        if let GreenChild::Node(m) = c {
            walk(m, off, cfg, st, res, text, registry, jobs);
        }
        off += c.width();
    }
}

/// Compose provenance across passes: `a` maps original→t1, `b` maps
/// t1→t2; the result maps original→t2. Sound because every segment
/// is a COPY (equal out/src lengths), so offsets split linearly.
pub fn compose(a: &[Seg], b: &[Seg]) -> Vec<Seg> {
    let mut out = Vec::new();
    for bs in b {
        let (mut s, e) = bs.src;
        if s == e {
            continue;
        }
        for asg in a {
            if asg.out.1 <= s || asg.out.0 >= e {
                continue;
            }
            let os = s.max(asg.out.0);
            let oe = e.min(asg.out.1);
            out.push(Seg {
                out: (bs.out.0 + (os - bs.src.0), bs.out.0 + (oe - bs.src.0)),
                src: (asg.src.0 + (os - asg.out.0), asg.src.0 + (oe - asg.out.0)),
                kind: if bs.kind == SegKind::Verbatim { asg.kind } else { bs.kind },
            });
            s = oe;
            if s >= e {
                break;
            }
        }
    }
    out.sort_by_key(|s| s.out.0);
    out
}

/// Do the segments tile [0, len) exactly — full cover, no overlap?
pub fn tiles(segs: &[Seg], len: u32) -> bool {
    let mut at = 0u32;
    for s in segs {
        if s.out.0 != at || s.out.1 < s.out.0 {
            return false;
        }
        if (s.out.1 - s.out.0) != (s.src.1 - s.src.0) {
            return false;
        }
        at = s.out.1;
    }
    at == len
}
