//! P4: the semantic layer — envelope commitments L8 (binding as data)
//! and L9 (signature/body firewalls), running.
//!
//! Architecture is salsa's — revisions, memoized queries, EARLY CUTOFF by
//! comparing recomputed outputs (a query whose value didn't change keeps
//! its old change-revision, so dependents stay valid: "backdating") —
//! implemented as a minimal transparent engine because the demo's query
//! DAG is three levels deep (tree → symbols → signature → resolution)
//! and the inputs already arrive incrementally from our own sessions.
//! Swapping in the salsa crate is the documented refinement when the
//! workspace model grows.
//!
//! Binding is DECLARATIVE: a [`BindingConfig`] names which (nonterminal,
//! production) pairs define names, reference them, and open scopes —
//! the `@def/@ref/@scope` annotations of the design, as data. Scoping is
//! sequential (definition-before-use) with block shadowing; top-level
//! definitions are a file's exported SIGNATURE, and cross-file
//! resolution goes only through signatures — which is exactly what makes
//! the firewall theorem testable: a body edit cannot change a signature,
//! so it can never invalidate another file's resolution.

use rantlr_grammar::green::{ERROR_NT, RUN_PROD};
use rantlr_grammar::{GreenChild, GreenNode, SynGrammar};
use std::collections::HashMap;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Binding configuration (the @def/@ref/@scope annotations, as data)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefKind {
    /// A variable read — unresolved ones are diagnosed.
    Var,
    /// A callee position — resolves for navigation, never diagnosed
    /// (the demo host provides functions).
    Call,
}

#[derive(Clone, Debug, Default)]
pub struct BindingConfig {
    /// (nt, prod, symbol-child index of the name token).
    pub defs: Vec<(u16, u16, usize)>,
    /// (nt, prod, symbol-child index of the name token, kind).
    pub refs: Vec<(u16, u16, usize, RefKind)>,
    /// (nt, prod) pairs that open a lexical scope.
    pub scopes: Vec<(u16, u16)>,
}

/// The demo grammar's annotations: `let` defines (child 1), `NameRef`
/// reads (child 0), `CallExpr` calls (child 0), `Block` scopes.
pub fn demo_binding_config(sg: &SynGrammar) -> BindingConfig {
    let mut cfg = BindingConfig::default();
    for i in 0..sg.prods.len() {
        let (nt, prod) = (sg.prods[i].lhs, i as u16);
        match sg.prod_name(i).as_str() {
            "LetStmt" => cfg.defs.push((nt, prod, 1)),
            "NameRef" => cfg.refs.push((nt, prod, 0, RefKind::Var)),
            "CallExpr" => cfg.refs.push((nt, prod, 0, RefKind::Call)),
            "Block" => cfg.scopes.push((nt, prod)),
            _ => {}
        }
    }
    cfg
}

// ---------------------------------------------------------------------------
// Symbol tables (per file, one tree walk)
// ---------------------------------------------------------------------------

pub type ScopeId = u32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Def {
    pub name: String,
    pub span: (u32, u32),
    pub scope: ScopeId,
    /// Walk order — definition-before-use.
    pub order: u32,
    pub top_level: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ref {
    pub name: String,
    pub span: (u32, u32),
    pub scope: ScopeId,
    pub order: u32,
    pub kind: RefKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct SymbolTable {
    /// scope → parent (root = 0, parent self).
    pub scope_parents: Vec<ScopeId>,
    pub defs: Vec<Def>,
    pub refs: Vec<Ref>,
}

impl SymbolTable {
    fn is_ancestor_or_self(&self, anc: ScopeId, mut s: ScopeId) -> bool {
        loop {
            if s == anc {
                return true;
            }
            let p = self.scope_parents[s as usize];
            if p == s {
                return false;
            }
            s = p;
        }
    }
    fn scope_depth(&self, mut s: ScopeId) -> u32 {
        let mut d = 0;
        loop {
            let p = self.scope_parents[s as usize];
            if p == s {
                return d;
            }
            d += 1;
            s = p;
        }
    }
}

pub fn build_symbols(tree: &GreenNode, cfg: &BindingConfig) -> SymbolTable {
    let mut st = SymbolTable { scope_parents: vec![0], ..Default::default() };
    let mut order = 0u32;
    walk(tree, 0, 0, cfg, &mut st, &mut order);
    return st;

    fn name_child(n: &GreenNode, k: usize) -> Option<(String, u32)> {
        // k-th symbol child, with its byte offset within n.
        let mut off = 0u32;
        let mut idx = 0usize;
        for c in &n.children {
            let w = c.width();
            let is_symbol = match c {
                GreenChild::Token(t) => !t.trivia && !t.is_missing(),
                GreenChild::Node(m) => m.nt != ERROR_NT,
            };
            if is_symbol {
                if idx == k {
                    if let GreenChild::Token(t) = c {
                        return Some((t.text.clone(), off));
                    }
                    return None;
                }
                idx += 1;
            }
            off += w;
        }
        None
    }

    fn walk(
        n: &GreenNode,
        base: u32,
        scope: ScopeId,
        cfg: &BindingConfig,
        st: &mut SymbolTable,
        order: &mut u32,
    ) {
        let mut scope = scope;
        if n.prod != RUN_PROD && cfg.scopes.iter().any(|&(nt, p)| nt == n.nt && p == n.prod) {
            let id = st.scope_parents.len() as ScopeId;
            st.scope_parents.push(scope);
            scope = id;
        }
        if let Some(&(_, _, k)) =
            cfg.defs.iter().find(|&&(nt, p, _)| nt == n.nt && p == n.prod)
        {
            if let Some((name, off)) = name_child(n, k) {
                *order += 1;
                st.defs.push(Def {
                    span: (base + off, base + off + name.len() as u32),
                    name,
                    scope,
                    order: *order,
                    top_level: scope == 0,
                });
            }
        }
        if let Some(&(_, _, k, kind)) =
            cfg.refs.iter().find(|&&(nt, p, _, _)| nt == n.nt && p == n.prod)
        {
            if let Some((name, off)) = name_child(n, k) {
                *order += 1;
                st.refs.push(Ref {
                    span: (base + off, base + off + name.len() as u32),
                    name,
                    scope,
                    order: *order,
                    kind,
                });
            }
        }
        let mut off = base;
        for c in &n.children {
            if let GreenChild::Node(m) = c {
                walk(m, off, scope, cfg, st, order);
            }
            off += c.width();
        }
    }
}

/// A file's exported signature: sorted top-level definition names. The
/// firewall key — body edits cannot change it.
pub type Signature = Vec<String>;

pub fn signature_of(st: &SymbolTable) -> Signature {
    let mut names: Vec<String> =
        st.defs.iter().filter(|d| d.top_level).map(|d| d.name.clone()).collect();
    names.sort();
    names.dedup();
    names
}

// ---------------------------------------------------------------------------
// Resolution
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Local { def: usize },
    Foreign { uri: String, def: usize },
    Unresolved,
}

pub type Resolutions = Vec<Target>;

/// Resolve one file's refs: nearest preceding definition in the innermost
/// visible scope; otherwise another file's exported top-level def
/// (deterministic: lexicographically smallest uri). Definitions are
/// indexed by name so cost is O(refs × same-name candidates), not
/// O(refs × defs).
fn resolve_file(
    st: &SymbolTable,
    foreign: &[(String, Arc<SymbolTable>)], // other files, sorted by uri
) -> Resolutions {
    let mut by_name: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, d) in st.defs.iter().enumerate() {
        by_name.entry(d.name.as_str()).or_default().push(i);
    }
    let depths: Vec<u32> = (0..st.scope_parents.len() as u32)
        .map(|s| st.scope_depth(s))
        .collect();
    let mut foreign_by_name: HashMap<&str, (usize, usize)> = HashMap::new(); // name → (file idx, def idx)
    for (fi, (_, ft)) in foreign.iter().enumerate() {
        for (di, d) in ft.defs.iter().enumerate() {
            if d.top_level {
                foreign_by_name.entry(d.name.as_str()).or_insert((fi, di));
            }
        }
    }
    st.refs
        .iter()
        .map(|r| {
            let mut best: Option<(u32, u32, usize)> = None; // (depth, order, idx)
            if let Some(cands) = by_name.get(r.name.as_str()) {
                for &i in cands {
                    let d = &st.defs[i];
                    if d.order < r.order && st.is_ancestor_or_self(d.scope, r.scope) {
                        let key = (depths[d.scope as usize], d.order, i);
                        if best.map_or(true, |b| (key.0, key.1) > (b.0, b.1)) {
                            best = Some(key);
                        }
                    }
                }
            }
            if let Some((_, _, i)) = best {
                return Target::Local { def: i };
            }
            if let Some(&(fi, di)) = foreign_by_name.get(r.name.as_str()) {
                return Target::Foreign { uri: foreign[fi].0.clone(), def: di };
            }
            Target::Unresolved
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The query database (salsa architecture, minimal engine)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SemStats {
    pub symbols_computed: u64,
    pub resolve_computed: u64,
}

struct FileEntry {
    tree: Arc<GreenNode>,
    /// Revision at which the tree input last changed.
    tree_rev: u64,
    symbols: Option<(u64, Arc<SymbolTable>)>, // (input rev it was computed from)
    /// (value, revision at which the VALUE last changed) — early cutoff.
    signature: Option<(Arc<Signature>, u64)>,
    /// (dependency fingerprint, value).
    resolve: Option<(Vec<(String, u64)>, Arc<Resolutions>)>,
}

pub struct SemDb {
    cfg: BindingConfig,
    revision: u64,
    files: HashMap<String, FileEntry>,
    pub stats: SemStats,
}

impl SemDb {
    pub fn new(cfg: BindingConfig) -> Self {
        SemDb { cfg, revision: 0, files: HashMap::new(), stats: SemStats::default() }
    }

    /// Input: a file's current tree (from the incremental session).
    pub fn set_tree(&mut self, uri: &str, tree: Arc<GreenNode>) {
        self.revision += 1;
        let rev = self.revision;
        let e = self.files.entry(uri.to_string()).or_insert(FileEntry {
            tree: tree.clone(),
            tree_rev: rev,
            symbols: None,
            signature: None,
            resolve: None,
        });
        e.tree = tree;
        e.tree_rev = rev;
    }

    pub fn remove(&mut self, uri: &str) {
        self.revision += 1;
        self.files.remove(uri);
    }

    pub fn symbols(&mut self, uri: &str) -> Arc<SymbolTable> {
        let e = self.files.get(uri).expect("known file");
        if let Some((rev, st)) = &e.symbols {
            if *rev == e.tree_rev {
                return st.clone();
            }
        }
        let st = Arc::new(build_symbols(&self.files[uri].tree, &self.cfg));
        self.stats.symbols_computed += 1;
        let tree_rev = self.files[uri].tree_rev;
        let e = self.files.get_mut(uri).unwrap();
        e.symbols = Some((tree_rev, st.clone()));
        // Signature with EARLY CUTOFF: if unchanged, keep its old
        // change-revision so dependents stay valid (backdating).
        let sig = Arc::new(signature_of(&st));
        match &mut e.signature {
            Some((old, _)) if **old == *sig => {}
            slot => *slot = Some((sig, tree_rev)),
        }
        st
    }

    fn signature_rev(&mut self, uri: &str) -> (Arc<Signature>, u64) {
        self.symbols(uri); // ensure fresh
        let (sig, rev) = self.files[uri].signature.as_ref().unwrap();
        (sig.clone(), *rev)
    }

    /// Resolutions for one file. Depends on: this file's symbols and
    /// every OTHER file's signature — the firewall boundary.
    pub fn resolve(&mut self, uri: &str) -> Arc<Resolutions> {
        let mut others: Vec<String> =
            self.files.keys().filter(|u| *u != uri).cloned().collect();
        others.sort();
        // Dependency fingerprint: own tree rev + others' signature revs.
        let own_symbols = self.symbols(uri);
        let mut fp: Vec<(String, u64)> = vec![(uri.to_string(), self.files[uri].tree_rev)];
        let mut foreign: Vec<(String, Arc<SymbolTable>)> = Vec::new();
        for u in &others {
            let (_, rev) = self.signature_rev(u);
            fp.push((u.clone(), rev));
            foreign.push((u.clone(), self.symbols(u)));
        }
        if let Some((old_fp, res)) = &self.files[uri].resolve {
            if *old_fp == fp {
                return res.clone();
            }
        }
        let res = Arc::new(resolve_file(&own_symbols, &foreign));
        self.stats.resolve_computed += 1;
        self.files.get_mut(uri).unwrap().resolve = Some((fp, res.clone()));
        res
    }

    // ---------------- derived services ----------------

    /// The ref (index) whose span contains `offset`, if any.
    pub fn ref_at(&mut self, uri: &str, offset: u32) -> Option<usize> {
        let st = self.symbols(uri);
        st.refs.iter().position(|r| r.span.0 <= offset && offset < r.span.1)
    }

    /// The def (index) whose span contains `offset`, if any.
    pub fn def_at(&mut self, uri: &str, offset: u32) -> Option<usize> {
        let st = self.symbols(uri);
        st.defs.iter().position(|d| d.span.0 <= offset && offset < d.span.1)
    }

    /// Go-to-definition from an offset (on a ref or a def).
    pub fn definition(&mut self, uri: &str, offset: u32) -> Option<(String, (u32, u32))> {
        if let Some(d) = self.def_at(uri, offset) {
            let st = self.symbols(uri);
            return Some((uri.to_string(), st.defs[d].span));
        }
        let r = self.ref_at(uri, offset)?;
        match &self.resolve(uri)[r] {
            Target::Local { def } => {
                let st = self.symbols(uri);
                Some((uri.to_string(), st.defs[*def].span))
            }
            Target::Foreign { uri: fu, def } => {
                let st = self.symbols(fu);
                Some((fu.clone(), st.defs[*def].span))
            }
            Target::Unresolved => None,
        }
    }

    /// All references (across files) to the definition at/under `offset`,
    /// plus the definition site itself.
    pub fn references(
        &mut self,
        uri: &str,
        offset: u32,
    ) -> Option<(Vec<(String, (u32, u32))>, (String, (u32, u32)))> {
        let (def_uri, def_span) = self.definition(uri, offset)?;
        let def_idx = {
            let st = self.symbols(&def_uri);
            st.defs.iter().position(|d| d.span == def_span)?
        };
        let uris: Vec<String> = self.files.keys().cloned().collect();
        let mut out = Vec::new();
        for u in uris {
            let res = self.resolve(&u);
            let st = self.symbols(&u);
            for (i, t) in res.iter().enumerate() {
                let hit = match t {
                    Target::Local { def } => u == def_uri && *def == def_idx,
                    Target::Foreign { uri: fu, def } => *fu == def_uri && *def == def_idx,
                    Target::Unresolved => false,
                };
                if hit {
                    out.push((u.clone(), st.refs[i].span));
                }
            }
        }
        out.sort();
        Some((out, (def_uri, def_span)))
    }

    /// Rename: all text edits (def site + references), per uri.
    pub fn rename_edits(
        &mut self,
        uri: &str,
        offset: u32,
    ) -> Option<HashMap<String, Vec<(u32, u32)>>> {
        let (refs, (def_uri, def_span)) = self.references(uri, offset)?;
        let mut out: HashMap<String, Vec<(u32, u32)>> = HashMap::new();
        out.entry(def_uri).or_default().push(def_span);
        for (u, span) in refs {
            out.entry(u).or_default().push(span);
        }
        for spans in out.values_mut() {
            spans.sort();
            spans.dedup();
        }
        Some(out)
    }

    /// Unresolved VARIABLE reads (diagnostic substrate).
    pub fn unresolved(&mut self, uri: &str) -> Vec<(String, (u32, u32))> {
        let res = self.resolve(uri);
        let st = self.symbols(uri);
        st.refs
            .iter()
            .zip(res.iter())
            .filter(|(r, t)| r.kind == RefKind::Var && **t == Target::Unresolved)
            .map(|(r, _)| (r.name.clone(), r.span))
            .collect()
    }

    /// Names visible at an offset (innermost scopes first, then exports
    /// of other files) — the binding-aware completion source.
    pub fn names_in_scope(&mut self, uri: &str, offset: u32) -> Vec<String> {
        let st = self.symbols(uri);
        // Innermost scope containing offset: deepest scope whose defs/refs
        // neighborhood... scopes carry no spans here; approximate by the
        // defs visible: defined before offset in any scope that is an
        // ancestor of the scope of the nearest following item, falling
        // back to root. Demo-grade: all defs with span.start < offset,
        // innermost-first by scope depth then recency.
        let mut cands: Vec<(&Def, u32)> = st
            .defs
            .iter()
            .filter(|d| d.span.0 < offset)
            .map(|d| (d, st.scope_depth(d.scope)))
            .collect();
        cands.sort_by(|a, b| (b.1, b.0.order).cmp(&(a.1, a.0.order)));
        let mut names: Vec<String> = cands.into_iter().map(|(d, _)| d.name.clone()).collect();
        let uris: Vec<String> = self.files.keys().filter(|u| *u != uri).cloned().collect();
        for u in uris {
            let (sig, _) = self.signature_rev(&u);
            names.extend(sig.iter().cloned());
        }
        let mut seen = std::collections::HashSet::new();
        names.retain(|n| seen.insert(n.clone()));
        names
    }
}
