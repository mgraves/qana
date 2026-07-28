//! The semantic layer — envelope commitments L8 (binding as data) and
//! L9 (signature/body firewalls) — with PER-ITEM memoization (P6).
//!
//! The granularity story: a file's top-level items (the elements of its
//! root list) are the memo units. The incremental parser reuses
//! untouched subtrees by `Arc` identity, so an item's pointer is a free,
//! exact cache key. Two memoized layers per item:
//!
//! 1. **Fragment** (keyed by item pointer alone): the item's defs/refs
//!    with ITEM-RELATIVE spans, fragment-local scopes and order, plus
//!    env-independent local resolution — a ref either resolves inside
//!    the fragment (inner scopes shadow everything outside) or ESCAPES.
//! 2. **Item resolution** (keyed by pointer + environment fingerprint +
//!    foreign fingerprint): each escaped ref is CLASSIFIED — top-level
//!    (some earlier item defines the name), foreign (another file
//!    exports it), or unresolved. Positions are never cached: they are
//!    recovered at query time through a per-revision name index, so
//!    caches survive item insertion and body edits untouched.
//!
//! The environment fingerprint chains over the SEQUENCE of top-level
//! definition names of preceding items — exactly the information
//! escaped classification depends on. A body edit changes no top-level
//! name, so every other item (and every other file, via signature
//! backdating) answers from cache: the firewall now holds WITHIN files,
//! proven by recompute counters, at item granularity.
//!
//! One bounded adjacency: the newline ENDING an item is trivia attached
//! inside the NEXT item's leading spine (losslessness), so an edit that
//! re-lexes it re-anchors the right neighbor's Arc — at most one extra
//! fragment walk per edit, and the recomputed fragment is value-equal.

use crate::key::NodeKey;
use qana_grammar::green::{ERROR_NT, LIST_PROD, RUN_PROD};
use qana_grammar::{GreenChild, GreenNode, SynGrammar};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

pub mod key;
pub mod macros;
pub mod types;
pub use types::{compose_types, TypeConfig, TypeDiag, TypeId, TypeReport, TypeRule, TyTerm};

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
    /// An IMPORT position (`@import`): resolves against other files'
    /// EXPORTS only — never against local scopes (so `use x;` with
    /// `@def` on the same token cannot bind to itself). Unresolved
    /// imports are diagnosed; imports of existing-but-private names are
    /// diagnosed separately ("not exported").
    Import,
    /// A QUALIFIED position (`@qualify`): the name resolves among the
    /// MEMBERS of whatever its base resolves to — member access on
    /// namespaces, the binding-level twin of the type tier's member
    /// form. Resolved eagerly at query time, not in the env fold.
    Qualified,
}

#[derive(Clone, Debug, Default)]
pub struct BindingConfig {
    /// (nt, prod, symbol-child index of the name token).
    pub defs: Vec<(u16, u16, usize)>,
    /// (nt, prod, symbol-child index of the name token, kind).
    pub refs: Vec<(u16, u16, usize, RefKind)>,
    /// (nt, prod, unordered, barrier) — productions that open a lexical
    /// scope. `unordered` scopes resolve declaration-language style
    /// (forward references legal — ordering is a PER-SCOPE property).
    /// `barrier` scopes seal their namespace: references inside cannot
    /// escape past the barrier (an island's names are island-local, and
    /// a guest reference never silently resolves to a host binding).
    pub scopes: Vec<(u16, u16, bool, bool)>,
    /// Ordering of the ROOT (file) scope: unordered namespaces
    /// (declaration languages) resolve forward anywhere; the default
    /// (false) is sequential definition-before-use with shadowing.
    pub unordered: bool,
    /// Productions whose definitions are EXPORTED from their file
    /// (`@export`). Declaring any export or import activates the module
    /// tier: cross-file resolution then goes through imports only, and
    /// only exported names are importable. A grammar that declares
    /// neither keeps the open world (every top-level name ambient).
    pub exports: Vec<(u16, u16)>,
    /// (nt, prod, body child) — the def INTRODUCES a namespace whose
    /// members are the definitions inside the body child (`@module`,
    /// the binding twin of `@type(deftype, body)`).
    pub modules: Vec<(u16, u16, usize)>,
    /// (nt, prod, base child, name child) — the name token resolves
    /// among the members of what the base resolves to (`@qualify`).
    pub quals: Vec<(u16, u16, usize, usize)>,
    /// (nt, prod, namespace) — this production's def AND refs live in a
    /// NAMED namespace (`@ns`). Namespaces partition resolution: a ref
    /// only ever binds a def in its own namespace, so `struct point`
    /// (tag) and a variable `point` coexist. Named namespaces resolve
    /// HOISTED — order-free within every scope they appear in — while
    /// the default namespace keeps each scope's declared ordering.
    /// That split is per-NAMESPACE ordering: C struct tags are
    /// forward-declarable while its values stay define-before-use.
    pub namespaces: Vec<(u16, u16, String)>,
}

impl BindingConfig {
    /// Is the module tier declared? (Any `@export` or `@import`.)
    pub fn module_tier(&self) -> bool {
        !self.exports.is_empty() || self.refs.iter().any(|r| r.3 == RefKind::Import)
    }
}

/// Compose a guest language's binding into a host's over a composed
/// grammar (offsets from the [`qana_grammar::ComposeMap`]): host
/// entries apply unchanged (host ids are preserved by composition),
/// guest entries shift by the offsets, and every island production
/// becomes a BARRIER scope carrying the guest's root ordering — the
/// guest's namespace, island-local and sealed in both directions.
pub fn compose_binding(
    host: &BindingConfig,
    guest: &BindingConfig,
    sg: &SynGrammar,
    map: &qana_grammar::ComposeMap,
) -> BindingConfig {
    let mut out = host.clone();
    for &(_, _, prod) in &map.islands {
        out.scopes.push((sg.prods[prod as usize].lhs, prod, guest.unordered, true));
    }
    for &(nt, prod, k) in &guest.defs {
        out.defs.push((nt + map.guest_nt_offset, prod + map.guest_prod_offset, k));
    }
    for &(nt, prod, k, kind) in &guest.refs {
        out.refs.push((nt + map.guest_nt_offset, prod + map.guest_prod_offset, k, kind));
    }
    for &(nt, prod, unordered, barrier) in &guest.scopes {
        out.scopes.push((
            nt + map.guest_nt_offset,
            prod + map.guest_prod_offset,
            unordered,
            barrier,
        ));
    }
    for &(nt, prod) in &guest.exports {
        out.exports.push((nt + map.guest_nt_offset, prod + map.guest_prod_offset));
    }
    for &(nt, prod, body) in &guest.modules {
        out.modules.push((nt + map.guest_nt_offset, prod + map.guest_prod_offset, body));
    }
    for &(nt, prod, b, k) in &guest.quals {
        out.quals.push((nt + map.guest_nt_offset, prod + map.guest_prod_offset, b, k));
    }
    for (nt, prod, ns) in &guest.namespaces {
        out.namespaces.push((
            nt + map.guest_nt_offset,
            prod + map.guest_prod_offset,
            ns.clone(),
        ));
    }
    out.unordered = host.unordered;
    out
}

/// The cross-item key for a (namespace, name) pair. `\u{1}` cannot
/// appear in an identifier, so qualified keys never collide with plain
/// names — and the default namespace keys stay the bare name (every
/// pre-namespace map keeps its meaning).
fn tkey(ns: &str, name: &str) -> String {
    if ns.is_empty() { name.to_string() } else { format!("{ns}\u{1}{name}") }
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
            "Block" => cfg.scopes.push((nt, prod, false, false)),
            _ => {}
        }
    }
    cfg
}

// ---------------------------------------------------------------------------
// Fragments (per item, position-independent)
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
    /// Declared `@export` (meaningful when the module tier is on).
    pub exported: bool,
    /// Declared namespace (`@ns`); "" is the default namespace. Named
    /// namespaces resolve hoisted (order-free) in every scope.
    pub ns: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ref {
    pub name: String,
    pub span: (u32, u32),
    pub scope: ScopeId,
    pub order: u32,
    pub kind: RefKind,
    /// Declared namespace (`@ns`); refs only bind same-namespace defs.
    pub ns: String,
}

/// Composed whole-file symbol view (absolute spans/orders) — a derived
/// convenience for tests and tools; queries run on fragments.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct SymbolTable {
    /// scope → parent (root = 0, parent self).
    pub scope_parents: Vec<ScopeId>,
    pub defs: Vec<Def>,
    pub refs: Vec<Ref>,
}

/// Env-independent resolution of a ref within its own fragment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LocalRes {
    /// Resolved to the fragment's def with this index.
    Def(u32),
    /// No fragment-local binding — classify against the environment.
    Escape,
    /// No fragment-local binding AND a barrier scope seals the ref in:
    /// unresolved, definitively (island names never leak).
    Contained,
}

#[derive(Debug)]
struct Fragment {
    /// Process-unique identity, never reused: equal uids mean the SAME
    /// fragment value (Arc reuse can recycle addresses; uids cannot).
    /// This is what lets a painter cache per-item work by identity.
    uid: u64,
    /// Spans item-relative; scopes fragment-local (0 = the file's root
    /// scope); order fragment-local (1..).
    defs: Vec<Def>,
    refs: Vec<Ref>,
    scope_parents: Vec<ScopeId>,
    /// Per-scope ordering: unordered scopes resolve forward (index 0 =
    /// the config's root ordering).
    scope_unordered: Vec<bool>,
    /// Per-scope barrier: visibility and escape stop here.
    scope_barrier: Vec<bool>,
    /// Indices into `defs` of scope-0 definitions, in order — the
    /// item's contribution to the environment (and the signature).
    top_defs: Vec<u32>,
    local: Vec<LocalRes>,
    /// (def index, body span rel) — defs that introduce a NAMESPACE
    /// whose members are the defs inside the span (`@module`).
    modules: Vec<(u32, (u32, u32))>,
    /// (base child span rel, index into `refs` of the Qualified name).
    quals: Vec<((u32, u32), u32)>,
}

impl Fragment {
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
    /// Is a def in `anc` visible from a ref in `s`? Ancestor-or-self,
    /// but the walk STOPS at barrier scopes: defs AT a barrier are
    /// visible from inside it; defs above it are not.
    fn visible(&self, anc: ScopeId, mut s: ScopeId) -> bool {
        loop {
            if s == anc {
                return true;
            }
            if self.scope_barrier[s as usize] {
                return false;
            }
            let p = self.scope_parents[s as usize];
            if p == s {
                return false;
            }
            s = p;
        }
    }
    /// May a ref in scope `s` escape to the file environment (no
    /// barrier between it and the root)?
    fn escapes(&self, mut s: ScopeId) -> bool {
        loop {
            if self.scope_barrier[s as usize] {
                return false;
            }
            let p = self.scope_parents[s as usize];
            if p == s {
                return true;
            }
            s = p;
        }
    }
}

/// Process-unique ids for paint-identity (fragments and resolutions).
/// Monotone, never reused — see `Fragment::uid`.
fn next_paint_uid() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn build_fragment(item: &GreenNode, cfg: &BindingConfig) -> Fragment {
    let mut f = Fragment {
        uid: next_paint_uid(),
        defs: Vec::new(),
        refs: Vec::new(),
        scope_parents: vec![0],
        scope_unordered: vec![cfg.unordered],
        scope_barrier: vec![false],
        top_defs: Vec::new(),
        local: Vec::new(),
        modules: Vec::new(),
        quals: Vec::new(),
    };
    let mut order = 0u32;
    walk(item, 0, 0, cfg, &mut f, &mut order);
    for (i, d) in f.defs.iter().enumerate() {
        if d.scope == 0 {
            f.top_defs.push(i as u32);
        }
    }
    // Env-independent local resolution: candidates are fragment defs in
    // VISIBLE scopes (barriers stop the walk). Ordering is a per-scope
    // property: unordered scopes resolve forward — and NAMED namespaces
    // resolve forward in EVERY scope (per-namespace ordering: hoisted).
    // Root-scope candidates defer to the environment when the root is
    // unordered OR the def is namespaced (a later ITEM may hold the
    // namespace's def); barrier scopes are fragment-complete, so their
    // resolution is final here.
    let mut by_name: HashMap<(&str, &str), Vec<u32>> = HashMap::new();
    for (i, d) in f.defs.iter().enumerate() {
        by_name.entry((d.ns.as_str(), d.name.as_str())).or_default().push(i as u32);
    }
    let depths: Vec<u32> = (0..f.scope_parents.len() as u32).map(|s| f.scope_depth(s)).collect();
    for r in &f.refs {
        let mut best: Option<(u32, u32, u32)> = None; // (depth, order, idx)
        // Imports resolve against other files' exports ONLY; qualified
        // names resolve among their base's members at query time.
        if r.kind != RefKind::Import && r.kind != RefKind::Qualified {
        if let Some(cands) = by_name.get(&(r.ns.as_str(), r.name.as_str())) {
            for &i in cands {
                let d = &f.defs[i as usize];
                if (f.scope_unordered[0] || !d.ns.is_empty()) && d.scope == 0 {
                    continue; // top-level forward world: environment decides
                }
                if (f.scope_unordered[d.scope as usize] || !d.ns.is_empty() || d.order < r.order)
                    && f.visible(d.scope, r.scope)
                {
                    let key = (depths[d.scope as usize], d.order, i);
                    if best.map_or(true, |b| (key.0, key.1) > (b.0, b.1)) {
                        best = Some(key);
                    }
                }
            }
        }
        }
        f.local.push(match best {
            Some((_, _, i)) => LocalRes::Def(i),
            None if f.escapes(r.scope) => LocalRes::Escape,
            None => LocalRes::Contained,
        });
    }
    return f;

    /// Span of the k-th SYMBOL child (token or rule), item-relative.
    fn child_span(n: &GreenNode, base: u32, k: usize) -> Option<(u32, u32)> {
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
                    return Some((base + off, base + off + w));
                }
                idx += 1;
            }
            off += w;
        }
        None
    }

    fn name_child(n: &GreenNode, k: usize) -> Option<(String, u32)> {
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
        f: &mut Fragment,
        order: &mut u32,
    ) {
        let mut scope = scope;
        // When a production BOTH defines and opens a scope, the def
        // belongs to the ENCLOSING scope — function-name semantics:
        // `macro m(x) => …` puts `m` outside and `x` inside. (Nothing
        // else is sensible: a def inside its own scope is invisible.)
        let outer = scope;
        if n.prod != RUN_PROD {
            if let Some(&(_, _, unordered, barrier)) =
                cfg.scopes.iter().find(|&&(nt, p, _, _)| nt == n.nt && p == n.prod)
            {
                let id = f.scope_parents.len() as ScopeId;
                f.scope_parents.push(scope);
                f.scope_unordered.push(unordered);
                f.scope_barrier.push(barrier);
                scope = id;
            }
        }
        // A production's declared namespace covers its def and refs.
        let node_ns = cfg
            .namespaces
            .iter()
            .find(|&&(nt2, p2, _)| nt2 == n.nt && p2 == n.prod)
            .map(|t| t.2.as_str())
            .unwrap_or("");
        if let Some(&(_, _, k)) = cfg.defs.iter().find(|&&(nt, p, _)| nt == n.nt && p == n.prod) {
            if let Some((name, off)) = name_child(n, k) {
                *order += 1;
                f.defs.push(Def {
                    span: (base + off, base + off + name.len() as u32),
                    name,
                    scope: outer,
                    order: *order,
                    top_level: outer == 0,
                    exported: cfg.exports.iter().any(|&(nt2, p2)| nt2 == n.nt && p2 == n.prod),
                    ns: node_ns.to_string(),
                });
            }
        }
        // A production may carry SEVERAL references (a qualified path
        // has a base @ref and a @qualify name), so collect all entries.
        for &(nt2, p2, k, kind) in cfg.refs.iter() {
            if nt2 != n.nt || p2 != n.prod {
                continue;
            }
            if let Some((name, off)) = name_child(n, k) {
                *order += 1;
                f.refs.push(Ref {
                    span: (base + off, base + off + name.len() as u32),
                    name,
                    scope,
                    order: *order,
                    kind,
                    ns: node_ns.to_string(),
                });
            }
        }
        if let Some(&(_, _, body)) =
            cfg.modules.iter().find(|&&(nt2, p2, _)| nt2 == n.nt && p2 == n.prod)
        {
            if let (false, Some(span)) = (f.defs.is_empty(), child_span(n, base, body)) {
                // The def this production just pushed introduces the
                // namespace; members are the defs inside `span`.
                f.modules.push(((f.defs.len() - 1) as u32, span));
            }
        }
        if let Some(&(_, _, base_child, _)) =
            cfg.quals.iter().find(|&&(nt2, p2, _, _)| nt2 == n.nt && p2 == n.prod)
        {
            if let Some(bspan) = child_span(n, base, base_child) {
                // The Qualified ref for this node was pushed just above.
                if let Some(ri) = f
                    .refs
                    .iter()
                    .rposition(|r| r.kind == RefKind::Qualified && r.span.0 >= base && r.span.0 < base + n.width)
                {
                    f.quals.push((bspan, ri as u32));
                }
            }
        }
        let mut off = base;
        for c in &n.children {
            if let GreenChild::Node(m) = c {
                walk(m, off, scope, cfg, f, order);
            }
            off += c.width();
        }
    }
}

// ---------------------------------------------------------------------------
// Item resolutions (classification only — positions recovered at query
// time, so caches survive unrelated edits)
// ---------------------------------------------------------------------------

/// Cached classification of one ref.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Classified {
    /// Fragment-local def (index into the fragment's defs).
    Local(u32),
    /// Bound by an earlier item's top-level def (ordered) or any item's
    /// (unordered) — looked up by name at query time.
    Top,
    /// Exported by this other file.
    Foreign(String),
    /// A qualified name (`@qualify`): resolved eagerly at query time
    /// against its base's member set, never in the env fold.
    Qualified,
    Unresolved,
}

#[derive(Debug, PartialEq, Eq)]
struct ItemRes {
    targets: Vec<Classified>,
    /// Indices of unresolved VARIABLE/IMPORT refs (precomputed so the
    /// per-keystroke diagnostics pass touches nothing else).
    unresolved: Vec<u32>,
    /// Refs naming something that EXISTS in another file but is not
    /// exported — the module tier's access diagnostic.
    private: Vec<u32>,
}

/// A file's exported signature: sorted top-level definition names. The
/// firewall key — body edits cannot change it.
pub type Signature = Vec<String>;

// ---------------------------------------------------------------------------
// The query database
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SemStats {
    /// Fragment (symbol) walks actually performed.
    pub fragments_computed: u64,
    /// Item resolutions actually computed (cache misses).
    pub item_resolves_computed: u64,
    /// Type-tier item walks actually performed (cache misses) — the
    /// memoization proof: a body edit costs O(1) of these.
    pub type_item_walks: u64,
    /// Type-tier fixpoint passes run (a warm, unchanged file converges
    /// in one).
    pub type_passes: u64,
}

struct ItemSlot {
    /// Identity AND keepalive in one value: a [`NodeKey`] cannot exist
    /// without owning its subtree, so the address it compares by can
    /// never be recycled underneath this slot.
    key: NodeKey,
    base_off: u32,
    frag: Arc<Fragment>,
    /// (env fp, foreign fp, resolutions) — carried ACROSS revisions by
    /// the positional diff in set_tree, validated by fingerprint
    /// compare (two u64s, no hashing) in ensure_resolved.
    res: Option<(u64, u64, u64, Arc<ItemRes>)>,
}

struct FileEntry {
    tree: Arc<GreenNode>,
    tree_rev: u64,
    items: Vec<ItemSlot>,
    /// item identity → fragment — survives edits (moved items keep
    /// their fragments); swept exactly, from the positional diff. The
    /// key owns the subtree, so a swept entry's address cannot come
    /// back as someone else's.
    frag_cache: HashMap<NodeKey, Arc<Fragment>>,
    /// Top-level name → occurrence count, maintained EXACTLY from the
    /// per-edit fragment delta — membership answers in O(1), and an
    /// unchanged signature is proven in O(edit).
    top_counts: HashMap<String, (i64, i64)>,
    sig_dirty: bool,
    /// name → top-level def sites [(item idx, def idx in fragment)], in
    /// item order — the query-time POSITION index, built lazily per
    /// revision (keystrokes never pay for it; navigation builds once).
    top_index: Option<HashMap<String, Vec<(u32, u32)>>>,
    /// (signature value, revision at which the VALUE last changed).
    signature: Option<(Arc<Signature>, u64)>,
    /// Env fingerprint per item (fp BEFORE the item) — valid for
    /// (tree_rev); rebuilt lazily.
    env_fps: Option<Vec<u64>>,
    resolved_under: Option<u64 /* foreign fp */>,
}

pub struct SemDb {
    cfg: BindingConfig,
    /// The declared type tier, when the grammar declared one.
    type_cfg: Option<types::TypeConfig>,
    /// (nt, prod) → symbol-child index of a MACRO BODY: subtrees the
    /// type walker exempts (templates type per instantiation).
    pub(crate) macro_body_exempt: HashMap<(u16, u16), usize>,
    /// Per-file memoization of the type pass (converged results +
    /// per-item outputs keyed by subtree identity).
    type_caches: HashMap<String, types::TypeCache>,
    /// The GLOBAL type vocabulary: one TypeId space per SemDb, so
    /// document types and function types mean the same thing in every
    /// file. Ids are interned once and stable for the session.
    gvocab: types::GlobalVocab,
    /// (owner type, member name) → member type, across ALL files —
    /// what makes `p.x` work when `p`'s struct lives elsewhere.
    /// Entries are owned by the type's declaring file and replaced
    /// whenever that file re-derives.
    global_members: HashMap<(types::TypeId, String), Option<types::TypeId>>,
    /// Re-entrancy guard for cross-file type queries (a dependency
    /// cycle resolves with whatever is already known, never hangs).
    type_visiting: HashSet<String>,
    revision: u64,
    files: HashMap<String, FileEntry>,
    pub stats: SemStats,
}

fn fnv(h: u64, bytes: &[u8]) -> u64 {
    let mut h = h;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}
const FNV_SEED: u64 = 0xcbf29ce484222325;

impl SemDb {
    pub fn new(cfg: BindingConfig) -> Self {
        SemDb {
            cfg,
            type_cfg: None,
            macro_body_exempt: HashMap::new(),
            type_caches: HashMap::new(),
            gvocab: types::GlobalVocab::default(),
            global_members: HashMap::new(),
            type_visiting: HashSet::new(),
            revision: 0,
            files: HashMap::new(),
            stats: SemStats::default(),
        }
    }

    /// Input: a file's current tree (from the incremental session).
    /// Cost tracks the EDIT: cached fragments are reused by Arc
    /// identity; the top-name count map updates from the fragment delta
    /// alone, so an unchanged signature is proven without ever sorting
    /// names; the position index and cache sweeps are deferred off the
    /// keystroke path.
    pub fn set_tree(&mut self, uri: &str, tree: Arc<GreenNode>) {
        self.revision += 1;
        let rev = self.revision;
        let cfg = &self.cfg;
        let mut fragments_computed = 0u64;

        let e = self.files.entry(uri.to_string()).or_insert_with(|| FileEntry {
            tree: tree.clone(),
            tree_rev: rev,
            items: Vec::new(),
            frag_cache: HashMap::new(),
            top_counts: HashMap::new(),
            sig_dirty: true,
            top_index: None,
            signature: None,
            env_fps: None,
            resolved_under: None,
        });
        e.tree = tree.clone();
        e.tree_rev = rev;
        e.env_fps = None;
        e.resolved_under = None;
        e.top_index = None;

        // Items = the elements of the root's list child(ren); any other
        // node child of the root is an item of its own. Files without
        // list structure degrade to one whole-file item.
        let mut items: Vec<(NodeKey, u32)> = Vec::new();
        {
            let mut off = 0u32;
            for c in &tree.children {
                if let GreenChild::Node(n) = c {
                    if n.prod == LIST_PROD {
                        collect_list_items(n, off, &mut items);
                    } else if n.nt != ERROR_NT {
                        items.push((NodeKey::new(n.clone()), off));
                    }
                }
                off += c.width();
            }
            if items.is_empty() {
                items.push((NodeKey::new(tree.clone()), 0));
            }
        }

        // Positional diff by pointer identity: the common prefix and
        // suffix carry fragments AND resolutions over by position (no
        // map traffic); only the edit-sized middle window consults the
        // fragment cache. Dropped middle fragments are swept EXACTLY
        // (their top names counted out — what keeps `top_counts` and
        // the signature-dirty decision O(edit), not O(file)).
        let mut old = std::mem::take(&mut e.items);
        let mut lo = 0usize;
        while lo < old.len() && lo < items.len() && old[lo].key == items[lo].0 {
            lo += 1;
        }
        let mut suffix = 0usize;
        while suffix < old.len() - lo
            && suffix < items.len() - lo
            && old[old.len() - 1 - suffix].key == items[items.len() - 1 - suffix].0
        {
            suffix += 1;
        }
        let n_new = items.len();
        let old_len = old.len();
        let mut new_items: Vec<ItemSlot> = Vec::with_capacity(n_new);
        for (i, (key, base_off)) in items.into_iter().enumerate() {
            if i < lo || i >= n_new - suffix {
                // Carried by position: same subtree, new base offset.
                let oi = if i < lo { i } else { old_len - (n_new - i) };
                let o = &mut old[oi];
                debug_assert!(o.key == key);
                new_items.push(ItemSlot {
                    key,
                    base_off,
                    frag: o.frag.clone(),
                    res: o.res.take(),
                });
                continue;
            }
            let frag = match e.frag_cache.get(&key) {
                Some(f) => f.clone(),
                None => {
                    let f = Arc::new(build_fragment(key.node(), cfg));
                    fragments_computed += 1;
                    for &di in &f.top_defs {
                        let d = &f.defs[di as usize];
                        let c = e.top_counts.entry(tkey(&d.ns, &d.name)).or_insert((0, 0));
                        c.0 += 1;
                        c.1 += d.exported as i64;
                        e.sig_dirty = true;
                    }
                    e.frag_cache.insert(key.clone(), f.clone());
                    f
                }
            };
            new_items.push(ItemSlot { key, base_off, frag, res: None });
        }
        let new_mid: HashSet<NodeKey> =
            new_items[lo..n_new - suffix].iter().map(|s| s.key.clone()).collect();
        for slot in &old[lo..old_len - suffix] {
            if !new_mid.contains(&slot.key) {
                if let Some(f) = e.frag_cache.remove(&slot.key) {
                    for &di in &f.top_defs {
                        let d = &f.defs[di as usize];
                        let k = tkey(&d.ns, &d.name);
                        if let Some(c) = e.top_counts.get_mut(&k) {
                            c.0 -= 1;
                            c.1 -= d.exported as i64;
                            e.sig_dirty = true;
                            if c.0 <= 0 {
                                e.top_counts.remove(&k);
                            }
                        }
                    }
                }
            }
        }
        e.items = new_items;

        self.stats.fragments_computed += fragments_computed;
    }

    /// Signature with EARLY CUTOFF, computed on demand from the exact
    /// top-name counts: if the value is unchanged, it keeps its old
    /// change-revision (backdating), so other files' fingerprints don't
    /// move on body edits — and body edits never even mark it dirty.
    fn ensure_signature(&mut self, uri: &str) {
        let rev = self.files[uri].tree_rev;
        {
            let e = &self.files[uri];
            if !e.sig_dirty && e.signature.is_some() {
                return;
            }
        }
        // Module tier on: the signature is the EXPORT surface — a
        // private def cannot appear in it, so editing one can never
        // invalidate another file's resolutions. `pub` is an
        // incrementality contract.
        let tier = self.cfg.module_tier();
        let e = self.files.get_mut(uri).unwrap();
        let mut names: Vec<String> = e
            .top_counts
            .iter()
            .filter(|(_, c)| if tier { c.1 > 0 } else { c.0 > 0 })
            .map(|(n, _)| n.clone())
            .collect();
        names.sort();
        let sig = Arc::new(names);
        match &mut e.signature {
            Some((old, _)) if **old == *sig => {}
            slot => *slot = Some((sig, rev)),
        }
        e.sig_dirty = false;
    }

    pub fn remove(&mut self, uri: &str) {
        self.revision += 1;
        self.files.remove(uri);
        self.type_caches.remove(uri);
    }

    /// The tree currently held for `uri` (lossless — its text() is the
    /// document). The macro engine reads sibling trees through this.
    pub fn tree(&self, uri: &str) -> Option<Arc<GreenNode>> {
        self.files.get(uri).map(|e| e.tree.clone())
    }

    pub fn signature(&mut self, uri: &str) -> Arc<Signature> {
        self.ensure_signature(uri);
        self.files[uri].signature.as_ref().unwrap().0.clone()
    }

    /// Dependency fingerprint over OTHER files' signatures (their
    /// last-CHANGE revisions — backdated, so body edits don't move it).
    fn foreign_fp(&mut self, uri: &str) -> u64 {
        let mut others: Vec<String> =
            self.files.keys().filter(|u| *u != uri).cloned().collect();
        others.sort();
        let mut h = FNV_SEED;
        for u in others {
            self.ensure_signature(&u);
            let (_, rev) = self.files[&u].signature.as_ref().unwrap();
            h = fnv(h, u.as_bytes());
            h = fnv(h, &rev.to_le_bytes());
        }
        h
    }

    /// Ensure every item of `uri` has valid resolutions (memoized per
    /// item under env + foreign fingerprints).
    fn ensure_resolved(&mut self, uri: &str) {
        let foreign_fp = self.foreign_fp(uri);
        if self.files[uri].resolved_under == Some(foreign_fp)
            && self.files[uri].env_fps.is_some()
        {
            return;
        }

        // Environment fingerprints: chained over preceding items'
        // top-level NAME sequences (ordered), or the whole file's
        // (unordered — every item sees the same environment). NAMED
        // namespaces are hoisted, so their defs contribute whole-file
        // in BOTH modes: fold them into one hash mixed into every
        // item's fingerprint.
        let unordered = self.cfg.unordered;
        let tier = self.cfg.module_tier();
        let e = &self.files[uri];
        let n_items = e.items.len();
        let mut named_h = FNV_SEED;
        for slot in &e.items {
            for &di in &slot.frag.top_defs {
                let d = &slot.frag.defs[di as usize];
                if !d.ns.is_empty() {
                    named_h = fnv(named_h, d.ns.as_bytes());
                    named_h = fnv(named_h, b"\0");
                    named_h = fnv(named_h, d.name.as_bytes());
                    named_h = fnv(named_h, b"\0");
                }
            }
        }
        let mut env_fps = Vec::with_capacity(n_items);
        if unordered {
            let mut h = named_h;
            for slot in &e.items {
                for &di in &slot.frag.top_defs {
                    let d = &slot.frag.defs[di as usize];
                    if d.ns.is_empty() {
                        h = fnv(h, d.name.as_bytes());
                        h = fnv(h, b"\0");
                    }
                }
            }
            env_fps.resize(n_items, h);
        } else {
            let mut h = FNV_SEED;
            for slot in &e.items {
                env_fps.push(fnv(h, &named_h.to_le_bytes()));
                for &di in &slot.frag.top_defs {
                    let d = &slot.frag.defs[di as usize];
                    if d.ns.is_empty() {
                        h = fnv(h, d.name.as_bytes());
                        h = fnv(h, b"\0");
                    }
                }
            }
        }

        // Foreign exports, in deterministic smallest-uri-first order.
        let mut other_uris: Vec<String> =
            self.files.keys().filter(|u| *u != uri).cloned().collect();
        other_uris.sort();

        // Classify escaped refs per item. Slots carry their resolutions
        // across revisions (moved over by the positional diff); a
        // two-u64 fingerprint compare validates each — no hashing, no
        // map lookups on the keystroke path.
        let mut env: HashSet<(&str, &str)> = HashSet::new();
        let e = &self.files[uri];
        let mut computed: Vec<(usize, Arc<ItemRes>)> = Vec::new(); // (item idx, res)
        let mut new_resolves = 0u64;
        // Hoisted contributions first: every named-namespace top def is
        // visible to every item, whatever the root's ordering says.
        for slot in &e.items {
            for &di in &slot.frag.top_defs {
                let d = &slot.frag.defs[di as usize];
                if unordered || !d.ns.is_empty() {
                    env.insert((d.ns.as_str(), d.name.as_str()));
                }
            }
        }
        for (k, slot) in e.items.iter().enumerate() {
            let env_fp = env_fps[k];
            let hit = matches!(
                &slot.res,
                Some((ef, ff, _, _)) if *ef == env_fp && *ff == foreign_fp
            );
            if !hit {
                let mut targets = Vec::with_capacity(slot.frag.refs.len());
                let mut unresolved = Vec::new();
                let mut private = Vec::new();
                for (ri, r) in slot.frag.refs.iter().enumerate() {
                    let t = if r.kind == RefKind::Qualified {
                        Classified::Qualified
                    } else {
                    match slot.frag.local[ri] {
                        LocalRes::Def(d) => Classified::Local(d),
                        LocalRes::Contained => Classified::Unresolved,
                        LocalRes::Escape => {
                            let rk = tkey(&r.ns, &r.name);
                            if r.kind != RefKind::Import
                                && env.contains(&(r.ns.as_str(), r.name.as_str()))
                            {
                                Classified::Top
                            } else if r.kind == RefKind::Import || !tier {
                                // Cross-file: imports always may; plain
                                // refs only in the open world (tier off).
                                // Tier on ⇒ only EXPORTED names import.
                                let visible = other_uris.iter().find(|u| {
                                    self.files[u.as_str()]
                                        .top_counts
                                        .get(rk.as_str())
                                        .is_some_and(|c| if tier { c.1 > 0 } else { c.0 > 0 })
                                });
                                match visible {
                                    Some(u) => Classified::Foreign(u.clone()),
                                    None => {
                                        // Exists somewhere but private?
                                        let hidden = tier
                                            && other_uris.iter().any(|u| {
                                                self.files[u.as_str()]
                                                    .top_counts
                                                    .get(rk.as_str())
                                                    .is_some_and(|c| c.0 > 0)
                                            });
                                        if hidden {
                                            private.push(ri as u32);
                                        }
                                        Classified::Unresolved
                                    }
                                }
                            } else {
                                Classified::Unresolved
                            }
                        }
                    }
                    };
                    if matches!(r.kind, RefKind::Var | RefKind::Import)
                        && t == Classified::Unresolved
                        && !private.last().is_some_and(|&p| p == ri as u32)
                    {
                        unresolved.push(ri as u32);
                    }
                    targets.push(t);
                }
                computed.push((k, Arc::new(ItemRes { targets, unresolved, private })));
                new_resolves += 1;
            }
            if !unordered {
                for &di in &slot.frag.top_defs {
                    let d = &slot.frag.defs[di as usize];
                    if d.ns.is_empty() {
                        env.insert(("", d.name.as_str()));
                    }
                }
            }
        }
        drop(env);
        let e = self.files.get_mut(uri).unwrap();
        for (k, res) in computed {
            let env_fp = env_fps[k];
            e.items[k].res = Some((env_fp, foreign_fp, next_paint_uid(), res));
        }
        e.env_fps = Some(env_fps);
        e.resolved_under = Some(foreign_fp);
        self.stats.item_resolves_computed += new_resolves;
    }

    /// The lazy position index over top-level defs — built at most once
    /// per revision, and only when a NAVIGATION query needs positions.
    fn ensure_top_index(&mut self, uri: &str) {
        if self.files[uri].top_index.is_some() {
            return;
        }
        let e = self.files.get_mut(uri).unwrap();
        let mut idx: HashMap<String, Vec<(u32, u32)>> = HashMap::new();
        for (i, slot) in e.items.iter().enumerate() {
            for &di in &slot.frag.top_defs {
                let d = &slot.frag.defs[di as usize];
                idx.entry(tkey(&d.ns, &d.name)).or_default().push((i as u32, di));
            }
        }
        e.top_index = Some(idx);
    }

    /// Top-level def site for a (ns, name) visible to item `k`
    /// (ordered: latest strictly-earlier item; unordered OR named
    /// namespace: latest anywhere — per-namespace ordering).
    fn top_site(&mut self, uri: &str, ns: &str, name: &str, k: u32) -> Option<(u32, u32)> {
        self.ensure_top_index(uri);
        let sites = self.files[uri].top_index.as_ref().unwrap().get(&tkey(ns, name))?;
        if self.cfg.unordered || !ns.is_empty() {
            sites.last().copied()
        } else {
            sites.iter().rev().find(|(i, _)| *i < k).copied()
        }
    }

    /// A foreign file's export site for a (ns, name) (its own last
    /// binding; module tier on ⇒ exported bindings only).
    fn export_site(&mut self, uri: &str, ns: &str, name: &str) -> Option<(u32, u32)> {
        let tier = self.cfg.module_tier();
        self.ensure_top_index(uri);
        let e = &self.files[uri];
        let sites = e.top_index.as_ref().unwrap().get(&tkey(ns, name))?;
        sites
            .iter()
            .rev()
            .find(|&&(i, di)| {
                !tier || e.items[i as usize].frag.defs[di as usize].exported
            })
            .copied()
    }

    fn abs_def_span(&self, uri: &str, item: u32, def: u32) -> (u32, u32) {
        let slot = &self.files[uri].items[item as usize];
        let d = &slot.frag.defs[def as usize];
        (slot.base_off + d.span.0, slot.base_off + d.span.1)
    }

    // ---------------- derived services ----------------

    /// The item containing `offset`, if any.
    fn item_at(&self, uri: &str, offset: u32) -> Option<u32> {
        let items = &self.files.get(uri)?.items;
        let k = items.partition_point(|s| s.base_off <= offset).checked_sub(1)?;
        (offset < items[k].base_off + items[k].key.node().width).then_some(k as u32)
    }

    /// Go-to-definition from an offset (on a ref or a def).
    pub fn definition(&mut self, uri: &str, offset: u32) -> Option<(String, (u32, u32))> {
        self.ensure_resolved(uri);
        let k = self.item_at(uri, offset)?;
        let slot = &self.files[uri].items[k as usize];
        let rel = offset - slot.base_off;
        // `use scale;` makes one token BOTH the local binding (@def) and
        // the import reference (@import). Navigation jumps THROUGH: the
        // import ref wins over the def-at-cursor, so go-to-definition
        // lands on the foreign export (Rust's `use` behavior).
        let import_here = slot
            .frag
            .refs
            .iter()
            .any(|r| r.kind == RefKind::Import && r.span.0 <= rel && rel < r.span.1);
        if !import_here {
            if let Some(d) = slot.frag.defs.iter().find(|d| d.span.0 <= rel && rel < d.span.1) {
                let span = (slot.base_off + d.span.0, slot.base_off + d.span.1);
                return Some((uri.to_string(), span));
            }
        }
        let ri = slot.frag.refs.iter().position(|r| r.span.0 <= rel && rel < r.span.1)?;
        let name = slot.frag.refs[ri].name.clone();
        let ns = slot.frag.refs[ri].ns.clone();
        let (_, _, _, res) = slot.res.as_ref()?;
        match res.targets[ri].clone() {
            Classified::Qualified => self.resolve_qualified(uri, k, ri as u32).ok(),
            Classified::Local(d) => Some((uri.to_string(), self.abs_def_span(uri, k, d))),
            Classified::Top => {
                let (i, d) = self.top_site(uri, &ns, &name, k)?;
                Some((uri.to_string(), self.abs_def_span(uri, i, d)))
            }
            Classified::Foreign(fu) => {
                let (i, d) = self.export_site(&fu, &ns, &name)?;
                Some((fu.clone(), self.abs_def_span(&fu, i, d)))
            }
            Classified::Unresolved => None,
        }
    }

    /// All references (across files) to the definition at/under
    /// `offset`, plus the definition site itself.
    pub fn references(
        &mut self,
        uri: &str,
        offset: u32,
    ) -> Option<(Vec<(String, (u32, u32))>, (String, (u32, u32)))> {
        let (def_uri, def_span) = self.definition(uri, offset)?;
        // Canonical def identity: (uri, item, def idx).
        let (def_item, def_idx, def_name, def_is_top, def_ns) = {
            let k = self.item_at(&def_uri, def_span.0)?;
            let slot = &self.files[&def_uri].items[k as usize];
            let rel = def_span.0 - slot.base_off;
            let di = slot.frag.defs.iter().position(|d| d.span.0 == rel)?;
            let d = &slot.frag.defs[di];
            (k, di as u32, d.name.clone(), d.top_level, d.ns.clone())
        };
        let uris: Vec<String> = {
            let mut v: Vec<String> = self.files.keys().cloned().collect();
            v.sort();
            v
        };
        let mut out = Vec::new();
        for u in &uris {
            self.ensure_resolved(u);
            let n_items = self.files[u].items.len();
            for k in 0..n_items as u32 {
                let (base_off, n_refs) = {
                    let slot = &self.files[u].items[k as usize];
                    (slot.base_off, slot.frag.refs.len())
                };
                for ri in 0..n_refs {
                    let (name, ns, span, target) = {
                        let slot = &self.files[u].items[k as usize];
                        let r = &slot.frag.refs[ri];
                        let res = &slot.res.as_ref().unwrap().3;
                        (
                            r.name.clone(),
                            r.ns.clone(),
                            (base_off + r.span.0, base_off + r.span.1),
                            res.targets[ri].clone(),
                        )
                    };
                    if name != def_name || ns != def_ns {
                        continue;
                    }
                    let hit = match target {
                        Classified::Qualified => {
                            self.resolve_qualified(u, k, ri as u32).ok().is_some_and(|(qu, span)| {
                                qu == def_uri
                                    && span == self.abs_def_span(&def_uri, def_item, def_idx)
                            })
                        }
                        Classified::Local(d) => u == &def_uri && k == def_item && d == def_idx,
                        Classified::Top => {
                            def_is_top
                                && u == &def_uri
                                && self.top_site(u, &ns, &name, k) == Some((def_item, def_idx))
                        }
                        Classified::Foreign(fu) => {
                            def_is_top
                                && fu == def_uri
                                && self.export_site(&fu, &ns, &name) == Some((def_item, def_idx))
                        }
                        Classified::Unresolved => false,
                    };
                    if hit {
                        out.push((u.clone(), span));
                    }
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

    /// Unresolved VARIABLE reads (diagnostic substrate) — a concat of
    /// per-item cached classifications: O(items) after a body edit.
    pub fn unresolved(&mut self, uri: &str) -> Vec<(String, (u32, u32))> {
        self.ensure_resolved(uri);
        let e = &self.files[uri];
        let mut out = Vec::new();
        for slot in &e.items {
            let res = &slot.res.as_ref().unwrap().3;
            for &ri in &res.unresolved {
                let r = &slot.frag.refs[ri as usize];
                out.push((
                    r.name.clone(),
                    (slot.base_off + r.span.0, slot.base_off + r.span.1),
                ));
            }
        }
        out
    }

    /// Resolve one qualified reference: the base's target (chased
    /// through imports by `definition`'s import-wins rule, so
    /// `use math; math::pi` lands in the exporting file for free) must
    /// be a `@module` def; the name then resolves among the defs inside
    /// its body span. Same-file access sees all members; CROSSING a
    /// file requires the member to be `@export`ed.
    fn resolve_qualified(
        &mut self,
        uri: &str,
        item: u32,
        ri: u32,
    ) -> Result<(String, (u32, u32)), String> {
        let (name, name_span, base_span) = {
            let slot = &self.files[uri].items[item as usize];
            let r = &slot.frag.refs[ri as usize];
            let q = slot
                .frag
                .quals
                .iter()
                .find(|(_, qri)| *qri == ri)
                .ok_or_else(|| "malformed qualified reference".to_string())?;
            (
                r.name.clone(),
                (slot.base_off + r.span.0, slot.base_off + r.span.1),
                (slot.base_off + q.0 .0, slot.base_off + q.0 .1),
            )
        };
        let _ = name_span;
        // The base's resolution = the resolution of the LAST reference
        // inside the base child (leftward recursion for nested paths).
        let base_ref_start = {
            let slot = &self.files[uri].items[item as usize];
            slot.frag
                .refs
                .iter()
                .filter(|r| {
                    let s = slot.base_off + r.span.0;
                    s >= base_span.0 && s < base_span.1
                })
                .map(|r| slot.base_off + r.span.0)
                .max()
                .ok_or_else(|| "path base carries no reference".to_string())?
        };
        let (mut turi, mut tspan) = self
            .definition(uri, base_ref_start)
            .ok_or_else(|| "path base does not resolve".to_string())?;
        // The base may land on an IMPORT binding (`use math;` then
        // `math::pi`): chase through it — definition() at the import
        // token jumps to the foreign export (bounded, re-export chains
        // included).
        for _ in 0..8 {
            self.ensure_resolved(&turi);
            let is_import_binding = {
                let e = &self.files[&turi];
                e.items
                    .partition_point(|s| s.base_off <= tspan.0)
                    .checked_sub(1)
                    .map(|k| {
                        let slot = &e.items[k];
                        let rel = tspan.0 - slot.base_off;
                        slot.frag
                            .refs
                            .iter()
                            .any(|r| r.kind == RefKind::Import && r.span.0 == rel)
                    })
                    .unwrap_or(false)
            };
            if !is_import_binding {
                break;
            }
            match self.definition(&turi, tspan.0) {
                Some((nu, ns)) if (nu.as_str(), ns) != (turi.as_str(), tspan) => {
                    turi = nu;
                    tspan = ns;
                }
                _ => break,
            }
        }
        // Locate the target def and its module body.
        self.ensure_resolved(&turi);
        let (body_abs, titem) = {
            let e = &self.files[&turi];
            let k = e
                .items
                .partition_point(|s| s.base_off <= tspan.0)
                .checked_sub(1)
                .ok_or_else(|| "target out of range".to_string())?;
            let slot = &e.items[k];
            let rel = tspan.0 - slot.base_off;
            let di = slot
                .frag
                .defs
                .iter()
                .position(|d| d.span.0 == rel)
                .ok_or_else(|| "target is not a definition".to_string())?;
            let m = slot
                .frag
                .modules
                .iter()
                .find(|(mdi, _)| *mdi == di as u32)
                .ok_or_else(|| format!("`{}` is not a module", &self.files[&turi].items[k].frag.defs[di].name))?;
            ((slot.base_off + m.1 .0, slot.base_off + m.1 .1), k)
        };
        // Members: defs of the target item inside the body span.
        let crossing = turi != uri;
        let e = &self.files[&turi];
        let slot = &e.items[titem];
        let mut found_private = false;
        let mut hit: Option<(u32, u32)> = None;
        for d in &slot.frag.defs {
            let s = slot.base_off + d.span.0;
            if s >= body_abs.0 && s < body_abs.1 && d.name == name {
                if crossing && !d.exported {
                    found_private = true;
                } else {
                    hit = Some((s, slot.base_off + d.span.1));
                }
            }
        }
        match (hit, found_private) {
            (Some(span), _) => Ok((turi, span)),
            (None, true) => Err(format!("`{name}` exists in the module but is not exported")),
            (None, false) => Err(format!("no member `{name}` in this module")),
        }
    }

    /// Path errors for the module tier: qualified names whose base is
    /// not a module, whose member is missing, or whose member is
    /// private across a file boundary. Message + span, query-time.
    pub fn qualified_errors(&mut self, uri: &str) -> Vec<(String, (u32, u32))> {
        self.ensure_resolved(uri);
        let quals: Vec<(u32, u32, (u32, u32))> = {
            let e = &self.files[uri];
            e.items
                .iter()
                .enumerate()
                .flat_map(|(k, slot)| {
                    slot.frag.quals.iter().map(move |&(_, ri)| {
                        let r = &slot.frag.refs[ri as usize];
                        (
                            k as u32,
                            ri,
                            (slot.base_off + r.span.0, slot.base_off + r.span.1),
                        )
                    })
                })
                .collect()
        };
        let mut out = Vec::new();
        for (k, ri, span) in quals {
            if let Err(msg) = self.resolve_qualified(uri, k, ri) {
                // An unresolved BASE is already the base ref's own
                // diagnostic; do not double-report.
                if msg != "path base does not resolve" {
                    out.push((msg, span));
                }
            }
        }
        out
    }

    /// The module tier's access errors: references (imports, usually)
    /// naming something that exists in another file but is NOT
    /// exported. Empty when the grammar declares no module tier.
    pub fn not_exported(&mut self, uri: &str) -> Vec<(String, (u32, u32))> {
        self.ensure_resolved(uri);
        let e = &self.files[uri];
        let mut out = Vec::new();
        for slot in &e.items {
            let res = &slot.res.as_ref().unwrap().3;
            for &ri in &res.private {
                let r = &slot.frag.refs[ri as usize];
                out.push((
                    r.name.clone(),
                    (slot.base_off + r.span.0, slot.base_off + r.span.1),
                ));
            }
        }
        out
    }

    /// Names visible at an offset (innermost scopes first, then exports
    /// of other files) — the binding-aware completion source.
    pub fn names_in_scope(&mut self, uri: &str, offset: u32) -> Vec<String> {
        self.ensure_resolved(uri);
        let unordered = self.cfg.unordered;
        let e = &self.files[uri];
        let mut cands: Vec<(String, u32, u32)> = Vec::new(); // (name, depth, abs order)
        let mut base_order = 0u32;
        for slot in &e.items {
            for d in &slot.frag.defs {
                if unordered || slot.base_off + d.span.0 < offset {
                    cands.push((
                        d.name.clone(),
                        slot.frag.scope_depth(d.scope),
                        base_order + d.order,
                    ));
                }
            }
            base_order += (slot.frag.defs.len() + slot.frag.refs.len()) as u32;
        }
        cands.sort_by(|a, b| (b.1, b.2).cmp(&(a.1, a.2)));
        let mut names: Vec<String> = cands.into_iter().map(|(n, _, _)| n).collect();
        let mut other_uris: Vec<String> =
            self.files.keys().filter(|u| *u != uri).cloned().collect();
        other_uris.sort();
        for u in other_uris {
            let sig = self.signature(&u);
            names.extend(sig.iter().cloned());
        }
        let mut seen = HashSet::new();
        names.retain(|n| seen.insert(n.clone()));
        names
    }

    // ---------------- composed views (tests, tools) ----------------

    /// Whole-file symbol table with absolute spans/orders — composed
    /// from fragments on demand.
    pub fn symbols(&mut self, uri: &str) -> Arc<SymbolTable> {
        let e = &self.files[uri];
        let mut st = SymbolTable { scope_parents: vec![0], ..Default::default() };
        let mut base_order = 0u32;
        for slot in &e.items {
            // Fragment scopes 1.. remap to fresh global ids.
            let scope_base = st.scope_parents.len() as u32;
            let remap = |s: ScopeId| if s == 0 { 0 } else { scope_base + s - 1 };
            for (si, &p) in slot.frag.scope_parents.iter().enumerate().skip(1) {
                let _ = si;
                st.scope_parents.push(remap(p));
            }
            for d in &slot.frag.defs {
                st.defs.push(Def {
                    name: d.name.clone(),
                    span: (slot.base_off + d.span.0, slot.base_off + d.span.1),
                    scope: remap(d.scope),
                    order: base_order + d.order,
                    top_level: d.top_level,
                    exported: d.exported,
                    ns: d.ns.clone(),
                });
            }
            for r in &slot.frag.refs {
                st.refs.push(Ref {
                    name: r.name.clone(),
                    span: (slot.base_off + r.span.0, slot.base_off + r.span.1),
                    scope: remap(r.scope),
                    order: base_order + r.order,
                    kind: r.kind,
                    ns: r.ns.clone(),
                });
            }
            base_order += (slot.frag.defs.len() + slot.frag.refs.len()) as u32;
        }
        Arc::new(st)
    }

    /// Whole-file resolutions with absolute def indices — composed from
    /// per-item classifications on demand.
    pub fn resolve(&mut self, uri: &str) -> Arc<Resolutions> {
        self.ensure_resolved(uri);
        // Absolute def index bases per item, here and (lazily) foreign.
        let def_base = |db: &Self, u: &str| -> Vec<u32> {
            let mut v = Vec::new();
            let mut acc = 0u32;
            for slot in &db.files[u].items {
                v.push(acc);
                acc += slot.frag.defs.len() as u32;
            }
            v
        };
        let bases = def_base(self, uri);
        let n_items = self.files[uri].items.len();
        let mut out = Vec::new();
        let mut foreign_bases: HashMap<String, Vec<u32>> = HashMap::new();
        for k in 0..n_items as u32 {
            let n_refs = self.files[uri].items[k as usize].frag.refs.len();
            for ri in 0..n_refs {
                let (name, ns, target) = {
                    let slot = &self.files[uri].items[k as usize];
                    let res = &slot.res.as_ref().unwrap().3;
                    (
                        slot.frag.refs[ri].name.clone(),
                        slot.frag.refs[ri].ns.clone(),
                        res.targets[ri].clone(),
                    )
                };
                let t = match target {
                    Classified::Local(d) => {
                        Target::Local { def: (bases[k as usize] + d) as usize }
                    }
                    Classified::Top => match self.top_site(uri, &ns, &name, k) {
                        Some((i, d)) => {
                            Target::Local { def: (bases[i as usize] + d) as usize }
                        }
                        None => Target::Unresolved,
                    },
                    Classified::Foreign(fu) => {
                        self.ensure_resolved(&fu);
                        let fb = foreign_bases
                            .entry(fu.clone())
                            .or_insert_with(|| def_base(self, &fu))
                            .clone();
                        match self.export_site(&fu, &ns, &name) {
                            Some((i, d)) => Target::Foreign {
                                uri: fu.clone(),
                                def: (fb[i as usize] + d) as usize,
                            },
                            None => Target::Unresolved,
                        }
                    }
                    Classified::Qualified => match self.resolve_qualified(uri, k, ri as u32) {
                        Ok((qu, span)) => {
                            let same = qu == uri;
                            let fb = if same {
                                bases.clone()
                            } else {
                                self.ensure_resolved(&qu);
                                foreign_bases
                                    .entry(qu.clone())
                                    .or_insert_with(|| def_base(self, &qu))
                                    .clone()
                            };
                            // span → (item, def idx) in the target file.
                            let e = &self.files[&qu];
                            let ti = e.items.partition_point(|s| s.base_off <= span.0) - 1;
                            let slot = &e.items[ti];
                            match slot
                                .frag
                                .defs
                                .iter()
                                .position(|d| slot.base_off + d.span.0 == span.0)
                            {
                                Some(di) => {
                                    let def = (fb[ti] + di as u32) as usize;
                                    if same {
                                        Target::Local { def }
                                    } else {
                                        Target::Foreign { uri: qu, def }
                                    }
                                }
                                None => Target::Unresolved,
                            }
                        }
                        Err(_) => Target::Unresolved,
                    },
                    Classified::Unresolved => Target::Unresolved,
                };
                out.push(t);
            }
        }
        Arc::new(out)
    }

    /// Is a type tier declared? (Painters use the answer to pick the
    /// identity-cached overlay path — the type report has no per-item
    /// identity yet, so typed grammars keep the composed-view path.)
    pub fn has_types(&self) -> bool {
        self.type_cfg.is_some()
    }

    /// The identity-keyed paint view: one key per item, in document
    /// order, after ensuring resolution — O(items), no allocation
    /// beyond the vec. Equal `(frag_uid, res_uid)` pairs mean the
    /// item's paint-relevant facts are IDENTICAL (uids are process-
    /// unique and never reused), so a painter caches derived marks per
    /// pair and re-derives only misses via [`item_paint`](Self::item_paint).
    /// An item's span runs from its `start` to the next item's (or the
    /// document's end): the gap trivia carries no marks, so the
    /// over-coverage is harmless.
    pub fn paint_sync(&mut self, uri: &str) -> Vec<PaintKey> {
        self.ensure_resolved(uri);
        self.files[uri]
            .items
            .iter()
            .map(|s| PaintKey {
                start: s.base_off,
                frag_uid: s.frag.uid,
                res_uid: s.res.as_ref().map(|r| r.2).unwrap_or(0),
            })
            .collect()
    }

    /// Paint facts for ONE item, spans ITEM-RELATIVE. Mirrors the
    /// classification arms of [`resolve`](Self::resolve) exactly —
    /// the paint differential gate holds it there — but stops at the
    /// bits level (no def indices), so it allocates only this item's
    /// two vectors.
    pub fn item_paint(&mut self, uri: &str, item: u32) -> ItemPaint {
        self.ensure_resolved(uri);
        let (frag, res) = {
            let slot = &self.files[uri].items[item as usize];
            (slot.frag.clone(), slot.res.as_ref().expect("resolved").3.clone())
        };
        let defs = frag.defs.iter().map(|d| (d.span, d.exported)).collect();
        let mut refs = Vec::with_capacity(frag.refs.len());
        for (ri, r) in frag.refs.iter().enumerate() {
            let quiet_gate = |rp: RefPaint| {
                // The unresolved BIT is kind-gated, exactly as the
                // composed-view overlay gates it.
                if rp == RefPaint::Unresolved
                    && !matches!(
                        r.kind,
                        RefKind::Qualified | RefKind::Var | RefKind::Call | RefKind::Import
                    )
                {
                    RefPaint::Quiet
                } else {
                    rp
                }
            };
            let rp = match &res.targets[ri] {
                Classified::Local(_) => RefPaint::Ref,
                Classified::Top => match self.top_site(uri, &r.ns, &r.name, item) {
                    Some(_) => RefPaint::Ref,
                    None => RefPaint::Unresolved,
                },
                Classified::Foreign(fu) => {
                    let fu = fu.clone();
                    self.ensure_resolved(&fu);
                    match self.export_site(&fu, &r.ns, &r.name) {
                        Some(_) => RefPaint::RefForeign,
                        None => RefPaint::Unresolved,
                    }
                }
                Classified::Qualified => match self.resolve_qualified(uri, item, ri as u32) {
                    Ok((qu, span)) => {
                        let e = &self.files[&qu];
                        let ti = e.items.partition_point(|s| s.base_off <= span.0) - 1;
                        let slot = &e.items[ti];
                        match slot
                            .frag
                            .defs
                            .iter()
                            .position(|d| slot.base_off + d.span.0 == span.0)
                        {
                            Some(_) if qu == uri => RefPaint::Ref,
                            Some(_) => RefPaint::RefForeign,
                            None => RefPaint::Unresolved,
                        }
                    }
                    Err(_) => RefPaint::Unresolved,
                },
                Classified::Unresolved => RefPaint::Unresolved,
            };
            refs.push((r.span, quiet_gate(rp)));
        }
        ItemPaint { defs, refs }
    }
}

/// One item's identity + position in the paint view — see
/// [`SemDb::paint_sync`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaintKey {
    /// Absolute char offset of the item's subtree.
    pub start: u32,
    /// Identity of the item's fragment (defs/refs/spans).
    pub frag_uid: u64,
    /// Identity of the item's resolution (classifications).
    pub res_uid: u64,
}

/// One item's paint facts, spans ITEM-RELATIVE — see
/// [`SemDb::item_paint`].
#[derive(Clone, Debug)]
pub struct ItemPaint {
    /// (span, exported).
    pub defs: Vec<((u32, u32), bool)>,
    /// (span, resolution class).
    pub refs: Vec<((u32, u32), RefPaint)>,
}

/// A ref's resolution, at the granularity paint needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefPaint {
    /// Resolves in this file.
    Ref,
    /// Resolves in another file.
    RefForeign,
    /// Does not resolve (and its kind is diagnosed).
    Unresolved,
    /// Does not resolve, but its kind stays quiet.
    Quiet,
}

fn collect_list_items(n: &GreenNode, base: u32, out: &mut Vec<(NodeKey, u32)>) {
    let mut off = base;
    for c in &n.children {
        match c {
            GreenChild::Node(m) if m.prod == RUN_PROD => {
                collect_list_items(m, off, out);
            }
            GreenChild::Node(m) => {
                out.push((NodeKey::new(m.clone()), off));
            }
            _ => {}
        }
        off += c.width();
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Local { def: usize },
    Foreign { uri: String, def: usize },
    Unresolved,
}

pub type Resolutions = Vec<Target>;
