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

use crate::types::{TypeConfig, TypeRule};
use crate::{SemDb, SymbolTable, Target};
use rantlr_grammar::green::{GreenChild, GreenNode, LIST_PROD};
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

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
    /// (nt, prod, type-name child index, separator) — `@reflect(ty[,
    /// sep])` on a `@splice` production: the macro's arguments come
    /// from ITERATION over the resolved type's declared members, one
    /// substitution per member — parameter 1 gets the member's name,
    /// parameter 2 its declared type text — joined by `sep`. The
    /// member map is the TYPE tier's own declared forms (`deftype`
    /// bodies and their typed defs): reflection reads the language's
    /// declarations, not an engine-side schema.
    pub reflects: Vec<(u16, u16, usize, String)>,
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
    /// Copied from a use site's argument — or, for a reflection
    /// splice, from the reflected member's declaration (name or type
    /// annotation): generated fields point at the fields.
    Arg,
    /// Synthesized separator bytes from a `@reflect` join — declared
    /// in the GRAMMAR, so they have no document source (the one
    /// exception to the everything-is-a-copy rule; src is empty).
    Sep,
}

/// One provenance segment: output bytes `out` are a copy of source
/// bytes `src` (equal lengths — every segment is a copy, which is
/// what makes multi-pass composition exact). `src_uri` is None for
/// the document being expanded and Some(uri) when the bytes came from
/// ANOTHER file — a cross-file macro's body splices text whose home
/// is the defining file, and provenance says so.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seg {
    pub out: (u32, u32),
    pub src: (u32, u32),
    pub kind: SegKind,
    pub src_uri: Option<String>,
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
    /// Token-exact body span in the DEFINING file's text.
    body: (u32, u32),
    /// (ref span, parameter position) inside the body, sorted.
    param_refs: Vec<((u32, u32), usize)>,
}

struct Job {
    span: (u32, u32),
    /// None: this file. Some(uri): the macro lives in a sibling.
    def_file: Option<String>,
    def: usize,
    args: Vec<(u32, u32)>,
    /// Whether the splice DECLARED an args child — a non-macro callee
    /// is silent without one (every C name is a splice site) and a
    /// diagnostic with one (`x!(…)` on a non-macro is an error).
    has_args: bool,
    /// `@reflect`: (type-name ref index into st.refs, separator).
    reflect: Option<(usize, String)>,
    /// Resolved reflection: the members to iterate, with the join.
    members: Option<(Vec<RMember>, String)>,
}

/// One reflected member: spans of its declared name and its type
/// annotation — both substitution source AND provenance (generated
/// code points at the field that generated it) — plus the file those
/// spans index (None = the expanding document, Some(uri) = the
/// sibling that declared the type).
struct RMember {
    name_span: (u32, u32),
    ty_span: (u32, u32),
    home: Option<String>,
}

/// Everything one pass needs from a SIBLING document: its text (for
/// splicing), its macro registry, and — for reflection — its symbols
/// and the type tier's declared shapes.
struct ForeignFile {
    text: String,
    registry: HashMap<usize, MacroDef>,
    st: Arc<SymbolTable>,
    tmap: TypeMap,
}

/// What one tree walk learns from the TYPE tier's declared forms:
/// where deftypes and their bodies are, where typed defs (fields)
/// are, and which tokens sit in member-name positions.
#[derive(Default)]
struct TypeMap {
    /// (introducing def-name span, body span) per `deftype` node.
    deftypes: Vec<((u32, u32), (u32, u32))>,
    /// (name span, name text, type-annotation span, its text) per
    /// `@type(def, …)` node.
    fields: Vec<((u32, u32), String, (u32, u32), String)>,
    /// (span, text) of every member-NAME position token.
    member_positions: Vec<((u32, u32), String)>,
}

fn type_map(tree: &GreenNode, text: &str, tcfg: &TypeConfig) -> TypeMap {
    let mut map = TypeMap::default();
    go(tree, 0, tcfg, text, &mut map);
    return map;

    fn slice(text: &str, s: (u32, u32)) -> String {
        text[s.0 as usize..s.1 as usize].to_string()
    }
    fn go(n: &GreenNode, base: u32, tcfg: &TypeConfig, text: &str, map: &mut TypeMap) {
        if let Some((_, _, rule)) =
            tcfg.rules.iter().find(|&&(nt, p, _)| nt == n.nt && p == n.prod)
        {
            match rule {
                TypeRule::DefType { def_child, body_child: Some(body) } => {
                    if let (Some((ds, _)), Some((bs, _))) =
                        (symbol_child(n, base, *def_child), symbol_child(n, base, *body))
                    {
                        map.deftypes.push((ds, bs));
                    }
                }
                TypeRule::DefFrom { src, def_child } => {
                    if let (Some((ns, _)), Some((ts, tn))) =
                        (symbol_child(n, base, *def_child), symbol_child(n, base, *src))
                    {
                        let ts = match tn {
                            Some(m) => content_span(m, ts.0),
                            None => ts,
                        };
                        map.fields.push((ns, slice(text, ns), ts, slice(text, ts)));
                    }
                }
                TypeRule::Member { name_child, .. } => {
                    if let Some((ms, _)) = symbol_child(n, base, *name_child) {
                        map.member_positions.push((ms, slice(text, ms)));
                    }
                }
                _ => {}
            }
        }
        let mut off = base;
        for c in &n.children {
            if let GreenChild::Node(m) = c {
                go(m, off, tcfg, text, map);
            }
            off += c.width();
        }
    }
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

/// Build a file's macro registry: every @macro node in `tree`, keyed
/// by the ABSOLUTE index of the def it introduces (the production's
/// own @def, found by containment outside the params/body children).
/// `member_positions` lets a parameter substitute at MEMBER-name
/// positions too (`origin.f` where `f` is the parameter): those
/// tokens are not binding refs — the TYPE tier's `member` form is
/// what identifies them, and v0 matches them by TEXT within the body.
fn build_registry(
    tree: &GreenNode,
    text: &str,
    st: &SymbolTable,
    res: &[Target],
    cfg: &MacroConfig,
    member_positions: &[((u32, u32), String)],
) -> HashMap<usize, MacroDef> {
    let mut registry = HashMap::new();
    go(tree, 0, cfg, st, res, text, &mut registry);
    for m in registry.values_mut() {
        let names: Vec<&str> =
            m.params.iter().map(|&p| st.defs[p].name.as_str()).collect();
        for (span, mtext) in member_positions {
            if span.0 >= m.body.0 && span.1 <= m.body.1 {
                if let Some(pi) = names.iter().position(|n| *n == mtext.as_str()) {
                    m.param_refs.push((*span, pi));
                }
            }
        }
        m.param_refs.sort_by_key(|&(s, _)| s.0);
        m.param_refs.dedup();
    }
    return registry;

    fn go(
        n: &GreenNode,
        base: u32,
        cfg: &MacroConfig,
        st: &SymbolTable,
        res: &[Target],
        text: &str,
        registry: &mut HashMap<usize, MacroDef>,
    ) {
        let span = (base, base + n.width);
        if let Some(&(_, _, params_child, body_child)) =
            cfg.defs.iter().find(|&&(nt, p, _, _)| nt == n.nt && p == n.prod)
        {
            if let Some((body_raw, body_node)) = symbol_child(n, base, body_child) {
                let pspan = params_child.and_then(|k| symbol_child(n, base, k)).map(|(s, _)| s);
                let inside = |d: (u32, u32), c: (u32, u32)| d.0 >= c.0 && d.1 <= c.1;
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
                    // Token-exact body: never splice leading trivia.
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
        let mut off = base;
        for c in &n.children {
            if let GreenChild::Node(m) = c {
                go(m, off, cfg, st, res, text, registry);
            }
            off += c.width();
        }
    }
}

/// Run ONE expansion pass over `uri`. `tree`/`text` must be what `db`
/// holds for it. Uses resolving to macros in SIBLING files (the db's
/// other set_tree'd documents) expand too: their bodies splice from
/// the sibling's text, and the segments say so (`src_uri`). With a
/// TYPE config, `@reflect` splices iterate the resolved type's
/// declared members.
pub fn expand_pass(
    db: &mut SemDb,
    uri: &str,
    tree: &GreenNode,
    text: &str,
    cfg: &MacroConfig,
    tcfg: Option<&TypeConfig>,
) -> PassOut {
    let st = db.symbols(uri);
    let res = db.resolve(uri);
    let mut diags = Vec::new();

    // The TYPE tier's declared forms, mapped once: deftype bodies,
    // typed defs (fields), member-name positions.
    let tmap = tcfg.map(|t| type_map(tree, text, t)).unwrap_or_default();

    let registry = build_registry(tree, text, &st, &res, cfg, &tmap.member_positions);
    let mut jobs: Vec<Job> = Vec::new();
    walk(tree, 0, cfg, &st, &res, &mut jobs);

    // Sibling documents this pass needs: the homes of foreign macro
    // DEFINITIONS and of foreign reflected TYPES. Sorted, so a pass is
    // deterministic regardless of map iteration order.
    let mut wanted: BTreeSet<String> = BTreeSet::new();
    for j in &jobs {
        if let Some(fu) = &j.def_file {
            wanted.insert(fu.clone());
        }
        if let Some((tri, _)) = &j.reflect {
            if let Some(Target::Foreign { uri, .. }) = res.get(*tri) {
                wanted.insert(uri.clone());
            }
        }
    }
    let mut foreign: HashMap<String, ForeignFile> = HashMap::new();
    for fu in wanted {
        let Some(ftree) = db.tree(&fu) else { continue };
        // Lossless trees mean the sibling's text needs no second read.
        let ftext = ftree.text();
        let fst = db.symbols(&fu);
        let fres = db.resolve(&fu);
        let tmap = tcfg.map(|t| type_map(&ftree, &ftext, t)).unwrap_or_default();
        let registry = build_registry(&ftree, &ftext, &fst, &fres, cfg, &tmap.member_positions);
        foreign.insert(fu, ForeignFile { text: ftext, registry, st: fst, tmap });
    }

    // Resolve @reflect jobs to member lists. The type-name ref may
    // resolve HERE or into a sibling — a foreign type's members are
    // read from ITS declarations, and the spans carry that file as
    // their home so substitution and provenance both stay honest.
    for j in jobs.iter_mut() {
        let Some((tri, sep)) = &j.reflect else { continue };
        let target = match res.get(*tri) {
            Some(&Target::Local { def }) => Some((None, def)),
            Some(Target::Foreign { uri, def }) => Some((Some(uri.clone()), *def)),
            _ => None,
        };
        let members = target.and_then(|(home, def)| {
            let (fst, ftmap): (&SymbolTable, &TypeMap) = match &home {
                None => (&st, &tmap),
                Some(u) => {
                    let f = foreign.get(u)?;
                    (&f.st, &f.tmap)
                }
            };
            let dspan = fst.defs.get(def)?.span;
            let &(_, body) = ftmap.deftypes.iter().find(|(ds, _)| *ds == dspan)?;
            Some(
                ftmap
                    .fields
                    .iter()
                    .filter(|(ns, ..)| ns.0 >= body.0 && ns.1 <= body.1)
                    .map(|(ns, _, ts, _)| RMember {
                        name_span: *ns,
                        ty_span: *ts,
                        home: home.clone(),
                    })
                    .collect::<Vec<_>>(),
            )
        });
        match members {
            Some(ms) => j.members = Some((ms, sep.clone())),
            None => diags.push(MacroDiag {
                span: j.span,
                msg: "reflected name does not resolve to a declared type".into(),
            }),
        }
    }

    // Uses inside any LOCAL macro BODY never expand at the definition
    // — they expand after splicing, at use sites (cpp and Rust agree).
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
    // A copy from SOME source text into the output. `from` is the
    // source string the span indexes; `src_uri` names it (None = the
    // pass input).
    let push = |out: &mut String,
                segs: &mut Vec<Seg>,
                from: &str,
                src_uri: &Option<String>,
                src: (u32, u32),
                kind: SegKind| {
        if src.1 > src.0 {
            let start = out.len() as u32;
            out.push_str(&from[src.0 as usize..src.1 as usize]);
            segs.push(Seg { out: (start, out.len() as u32), src, kind, src_uri: src_uri.clone() });
        }
    };
    let mut substitutions = 0u32;
    for j in &kept {
        push(&mut out, &mut segs, text, &None, (cursor, j.span.0), SegKind::Verbatim);
        // The macro's home: this file or a sibling.
        let (m, body_text) = match &j.def_file {
            None => (registry.get(&j.def), text),
            Some(fu) => match foreign.get(fu) {
                Some(f) => (f.registry.get(&j.def), f.text.as_str()),
                None => (None, text),
            },
        };
        let body_uri = j.def_file.clone();
        let body_uri = &body_uri;
        let Some(m) = m else {
            // Callee resolves, but not to a macro.
            if j.has_args || j.reflect.is_some() {
                diags.push(MacroDiag {
                    span: j.span,
                    msg: "spliced name does not resolve to a macro".into(),
                });
            }
            push(&mut out, &mut segs, text, &None, j.span, SegKind::Verbatim);
            cursor = j.span.1;
            continue;
        };
        // REFLECTION: substitute the body once per declared member —
        // parameter 1 takes the member's name, parameter 2 its type
        // annotation, both spliced from the STRUCT's own declaration
        // (provenance points at the fields) — joined by the declared
        // separator (synthesized bytes, marked Sep).
        if let Some((members, sep)) = &j.members {
            if m.params.len() != 2 {
                diags.push(MacroDiag {
                    span: j.span,
                    msg: format!(
                        "a reflection macro takes (member, type) — this one has {} parameter(s)",
                        m.params.len()
                    ),
                });
                push(&mut out, &mut segs, text, &None, j.span, SegKind::Verbatim);
                cursor = j.span.1;
                continue;
            }
            for (mi, mem) in members.iter().enumerate() {
                if mi > 0 && !sep.is_empty() {
                    let start = out.len() as u32;
                    out.push_str(sep);
                    segs.push(Seg {
                        out: (start, out.len() as u32),
                        src: (0, 0),
                        kind: SegKind::Sep,
                        src_uri: None,
                    });
                }
                // Member spans index the file that DECLARED the type.
                let mem_text = match &mem.home {
                    None => text,
                    Some(u) => foreign.get(u).map_or(text, |f| f.text.as_str()),
                };
                let mut b = m.body.0;
                for &(rspan, pi) in &m.param_refs {
                    push(&mut out, &mut segs, body_text, body_uri, (b, rspan.0), SegKind::Body);
                    let src = if pi == 0 { mem.name_span } else { mem.ty_span };
                    push(&mut out, &mut segs, mem_text, &mem.home, src, SegKind::Arg);
                    b = rspan.1;
                }
                push(&mut out, &mut segs, body_text, body_uri, (b, m.body.1), SegKind::Body);
            }
            substitutions += 1;
            cursor = j.span.1;
            continue;
        }
        if j.reflect.is_some() {
            // Reflection declared but the type never resolved (the
            // diagnostic is already recorded): leave the use intact.
            push(&mut out, &mut segs, text, &None, j.span, SegKind::Verbatim);
            cursor = j.span.1;
            continue;
        }
        if j.args.len() != m.params.len() {
            diags.push(MacroDiag {
                span: j.span,
                msg: format!(
                    "macro takes {} argument(s), {} supplied — left unexpanded",
                    m.params.len(),
                    j.args.len()
                ),
            });
            push(&mut out, &mut segs, text, &None, j.span, SegKind::Verbatim);
            cursor = j.span.1;
            continue;
        }
        // Body pieces (from the DEFINING file) around parameter refs;
        // args (from THIS file's use site) at the refs.
        let mut b = m.body.0;
        for &(rspan, pi) in &m.param_refs {
            push(&mut out, &mut segs, body_text, body_uri, (b, rspan.0), SegKind::Body);
            push(&mut out, &mut segs, text, &None, trim_span(text, j.args[pi]), SegKind::Arg);
            b = rspan.1;
        }
        push(&mut out, &mut segs, body_text, body_uri, (b, m.body.1), SegKind::Body);
        substitutions += 1;
        cursor = j.span.1;
    }
    push(&mut out, &mut segs, text, &None, (cursor, text.len() as u32), SegKind::Verbatim);

    PassOut { text: out, segs, substitutions, diags }
}

/// Collect splice JOBS: every @splice node whose callee resolves —
/// locally or into a sibling file. Whether the target is a MACRO is
/// settled at emit time against the registries (a use may precede
/// its macro's definition textually).
fn walk(
    n: &GreenNode,
    base: u32,
    cfg: &MacroConfig,
    st: &SymbolTable,
    res: &[Target],
    jobs: &mut Vec<Job>,
) {
    if let Some(&(_, _, callee_child, args_child)) =
        cfg.uses.iter().find(|&&(nt, p, _, _)| nt == n.nt && p == n.prod)
    {
        if let Some((cspan, _)) = symbol_child(n, base, callee_child) {
            if let Some(ri) = st.refs.iter().position(|r| r.span == cspan) {
                let target = match res.get(ri) {
                    Some(&Target::Local { def }) => Some((None, def)),
                    Some(Target::Foreign { uri, def }) => Some((Some(uri.clone()), *def)),
                    _ => None,
                };
                if let Some((def_file, def)) = target {
                    let args = args_child
                        .and_then(|k| symbol_child(n, base, k))
                        .and_then(|(s, node)| node.map(|m| list_elements(m, s.0)))
                        .unwrap_or_default();
                    // `@reflect` on the same production: iteration
                    // will supply the "arguments" per member.
                    let reflect = cfg
                        .reflects
                        .iter()
                        .find(|&&(nt, p, _, _)| nt == n.nt && p == n.prod)
                        .and_then(|(_, _, ty_child, sep)| {
                            let (tspan, _) = symbol_child(n, base, *ty_child)?;
                            let tri = st.refs.iter().position(|r| r.span == tspan)?;
                            Some((tri, sep.clone()))
                        });
                    // The replaced span is token-exact — leading
                    // trivia stays in the surrounding text.
                    jobs.push(Job {
                        span: content_span(n, base),
                        def_file,
                        def,
                        args,
                        has_args: args_child.is_some(),
                        reflect,
                        members: None,
                    });
                }
            }
        }
    }
    let mut off = base;
    for c in &n.children {
        if let GreenChild::Node(m) = c {
            walk(m, off, cfg, st, res, jobs);
        }
        off += c.width();
    }
}

/// Compose provenance across passes: `a` maps original→t1, `b` maps
/// t1→t2; the result maps original→t2. Sound because every segment
/// is a COPY (equal out/src lengths), so offsets split linearly. A
/// b-segment whose src lives in a FOREIGN file does not reference t1
/// at all — it passes through unchanged.
pub fn compose(a: &[Seg], b: &[Seg]) -> Vec<Seg> {
    let mut out = Vec::new();
    for bs in b {
        if bs.src_uri.is_some() || bs.kind == SegKind::Sep {
            // Foreign or synthesized: src never references t1.
            out.push(bs.clone());
            continue;
        }
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
            let piece_out = (bs.out.0 + (os - bs.src.0), bs.out.0 + (oe - bs.src.0));
            if asg.kind == SegKind::Sep {
                // Copied separator bytes stay separators (no source).
                out.push(Seg { out: piece_out, src: (0, 0), kind: SegKind::Sep, src_uri: None });
            } else {
                out.push(Seg {
                    out: piece_out,
                    src: (asg.src.0 + (os - asg.out.0), asg.src.0 + (oe - asg.out.0)),
                    kind: if bs.kind == SegKind::Verbatim { asg.kind } else { bs.kind },
                    src_uri: asg.src_uri.clone(),
                });
            }
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
/// (Sep segments are synthesized and exempt from the copy-length
/// rule; every other segment is a copy.)
pub fn tiles(segs: &[Seg], len: u32) -> bool {
    let mut at = 0u32;
    for s in segs {
        if s.out.0 != at || s.out.1 < s.out.0 {
            return false;
        }
        if s.kind != SegKind::Sep && (s.out.1 - s.out.0) != (s.src.1 - s.src.0) {
            return false;
        }
        at = s.out.1;
    }
    at == len
}
