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
//!    Below the item, what each subtree declares is kept by the
//!    subtree's pointer too, at every depth (a PART): an item built
//!    again walks only the subtrees the edit rebuilt, so typing in one
//!    function of a module that is a single 100,000-line item walks a
//!    piece of it, and a grammar that declares no binding walks nothing.
//!    A fragment built again that declares exactly what the one it
//!    replaces declared IS that fragment still: same identity, same
//!    resolution, nothing repainted.
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
//! fragment built per edit, and the rebuilt fragment is value-equal, so
//! it keeps its identity.

use crate::key::NodeKey;
use qana_grammar::green::{ERROR_NT, LIST_PROD, RUN_PROD};
use qana_grammar::{GreenChild, GreenNode, SynGrammar};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

pub mod key;
pub mod macros;
pub mod paths;
pub mod types;
pub use paths::{path_at, Path, Root};
pub use types::{compose_types, TypeConfig, TypeDiag, TypeId, TypeReport, TypeRule, TyTerm};

// ---------------------------------------------------------------------------
// Binding configuration (the @def/@ref/@scope annotations, as data)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
    /// A name that may be defined outside what this file can see
    /// (`@ref(x, escaping)`): a prelude, another crate, a glob import, a
    /// macro's expansion. It resolves as a variable read does when the
    /// file defines the name; when it does not, it ESCAPES: it is never
    /// diagnosed and never painted unresolved, and a project index (which
    /// sees further) is left to place it. A qualified name whose base
    /// escapes escapes too (see [`Target::Escapes`]).
    Escaping,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
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
    /// (nt, prod, child, namespace) — the defs and refs inside the
    /// subtree of `child` live in `namespace` (`@ns(name, label…)`),
    /// unless a production inside declares its own. What a path names
    /// depends on where the path stands: a macro's path names a macro,
    /// an attribute's an attribute, and the path rules themselves are
    /// shared.
    pub child_namespaces: Vec<(u16, u16, usize, String)>,
    /// (nt, prod) — this production's def is HOISTED (`@def(x,
    /// hoisted)`): visible throughout its scope whatever the order,
    /// while the scope's other defs keep its ordering (Rust's items
    /// beside its `let`s).
    pub hoisted: Vec<(u16, u16)>,
    /// (nt, prod) — the namespace this production's def introduces is
    /// OPEN (`@def(x, open)`): it has members the binding cannot see
    /// (Rust's `impl` blocks, a macro's expansion, a glob import), so a
    /// qualified name under it that is not among the members it can see
    /// escapes rather than being an error. Without `@module` it has no
    /// members the binding sees.
    pub open: Vec<(u16, u16)>,
    /// (nt, prod) — the scope this production opens holds MEMBERS
    /// (`@scope(members)`): what is defined directly in it is reached
    /// only through a qualified name (`E::A`, `Self::new`), never by a
    /// plain one — a type's fields, an enum's variants, an impl's or a
    /// trait's items. What is defined in scopes opened inside it is not
    /// a member, and binds as anything does.
    pub members: Vec<(u16, u16)>,
    /// (nt, prod, pattern child, after child) — the defs inside the
    /// `pattern` child take effect after the node, or after the `after`
    /// child when given (`@defer(pat[, after])`): `let x = x + 1;` reads
    /// the earlier `x`, and `for x in xs` does not let `xs` see `x`.
    pub defers: Vec<(u16, u16, usize, Option<usize>)>,
    /// (nt, prod, child) — a def or ref labelled on a RULE reaches its
    /// name token through these: where a production's node is given a
    /// name, the one child it passes it to (the grammar's
    /// `@style(inherit, …)` chains, where they pass to one child).
    pub passes: Vec<(u16, u16, usize)>,
    /// (nt, prod, name child, body child) — the definition this production
    /// makes (its `@def`, at `name child`) is a UNIT (`@unit(name[,
    /// body])`): a scope for imports and visibility, whose members are the
    /// definitions made in it. With a body, the body is that child, and
    /// the definition is a module of it too (`modules` holds it); without
    /// one, the body is elsewhere (Rust's `mod foo;`), where a project
    /// model says.
    pub units: Vec<(u16, u16, usize, Option<usize>)>,
    /// (nt, prod, root, first child) — the node begins a path at `root`
    /// (`@root(kind[, first])`), then the name its `first child` holds;
    /// `super` on a path whose head is `self` or `super` goes one unit
    /// further up. See [`paths`].
    pub roots: Vec<(u16, u16, Root, Option<usize>)>,
    /// (nt, prod, within, path child) — a visibility (`@vis(level)`):
    /// what is declared with it is seen everywhere (`public`: both
    /// `None`), within the unit `within` places from its own (`crate`,
    /// `super`, `private`), or within the unit the path at `path child`
    /// names (`in`).
    pub vis_levels: Vec<(u16, u16, Option<Root>, Option<usize>)>,
    /// (nt, prod, visibility child, item child) — the definitions and
    /// imports the `item child` makes where this node stands (not in a
    /// scope it opens) have the visibility the `@vis` node at the
    /// `visibility child` declares (`@visibility(v, item)`); with none
    /// there, the default: within their own unit.
    pub vis_items: Vec<(u16, u16, usize, usize)>,
    /// (nt, prod, path child, glob) — the production imports what a path
    /// names (`@import(path)`, `@import`, `@import_glob(path)`,
    /// `@import_glob`): the path at `path child`, or, without one, the
    /// path the node itself spells, after the prefix of the use group it
    /// stands in. A glob
    /// imports every name of the unit it names; any other import, one
    /// declaration, under the name the alternative's `@def` gives it, or
    /// under its path's last name when that `@def` reaches no name (`self`
    /// in a group); without a `@def` it names nothing (`as _`).
    pub imports: Vec<(u16, u16, Option<usize>, bool)>,
    /// (nt, prod, path child, trees child) — the imports in the `trees
    /// child` begin with a path (`@import_prefix([path,] trees)`): the one
    /// at `path child`, or, without one, the one the node itself spells.
    pub prefixes: Vec<(u16, u16, Option<usize>, usize)>,
    /// (nt, prod, type child, trait child) — the production is an
    /// implementation block (`@impl(type[, trait])`): of the type the path
    /// at `type child` names, for the trait at `trait child`. Its members
    /// are the definitions made directly in the members scope it opens.
    pub impls: Vec<(u16, u16, usize, Option<usize>)>,
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
    for (nt, prod, k, ns) in &guest.child_namespaces {
        out.child_namespaces.push((
            nt + map.guest_nt_offset,
            prod + map.guest_prod_offset,
            *k,
            ns.clone(),
        ));
    }
    for &(nt, prod) in &guest.hoisted {
        out.hoisted.push((nt + map.guest_nt_offset, prod + map.guest_prod_offset));
    }
    for &(nt, prod) in &guest.open {
        out.open.push((nt + map.guest_nt_offset, prod + map.guest_prod_offset));
    }
    for &(nt, prod) in &guest.members {
        out.members.push((nt + map.guest_nt_offset, prod + map.guest_prod_offset));
    }
    for &(nt, prod, pat, after) in &guest.defers {
        out.defers.push((nt + map.guest_nt_offset, prod + map.guest_prod_offset, pat, after));
    }
    for &(nt, prod, k) in &guest.passes {
        out.passes.push((nt + map.guest_nt_offset, prod + map.guest_prod_offset, k));
    }
    let (nts, prods) = (map.guest_nt_offset, map.guest_prod_offset);
    for &(nt, prod, name, body) in &guest.units {
        out.units.push((nt + nts, prod + prods, name, body));
    }
    for &(nt, prod, root, first) in &guest.roots {
        out.roots.push((nt + nts, prod + prods, root, first));
    }
    for &(nt, prod, within, path) in &guest.vis_levels {
        out.vis_levels.push((nt + nts, prod + prods, within, path));
    }
    for &(nt, prod, v, item) in &guest.vis_items {
        out.vis_items.push((nt + nts, prod + prods, v, item));
    }
    for &(nt, prod, path, glob) in &guest.imports {
        out.imports.push((nt + nts, prod + prods, path, glob));
    }
    for &(nt, prod, path, trees) in &guest.prefixes {
        out.prefixes.push((nt + nts, prod + prods, path, trees));
    }
    for &(nt, prod, ty, tr) in &guest.impls {
        out.impls.push((nt + nts, prod + prods, ty, tr));
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
    /// Declared hoisted (`@def(x, hoisted)`): visible throughout its
    /// scope whatever the order.
    pub hoisted: bool,
    /// Declared open (`@def(x, open)`): the namespace it introduces has
    /// members the binding cannot see.
    pub open: bool,
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
    /// Per-scope: its own defs are members, reached only by qualified
    /// names (`@scope(members)`).
    scope_members: Vec<bool>,
    /// Indices into `defs` of scope-0 definitions, in order — the
    /// item's contribution to the environment (and the signature).
    top_defs: Vec<u32>,
    /// The [`name_sig`] of each of `top_defs`, in the same order.
    top_sigs: Vec<u64>,
    local: Vec<LocalRes>,
    /// (def index, body span rel) — defs that introduce a NAMESPACE
    /// whose members are the defs inside the span (`@module`).
    modules: Vec<(u32, (u32, u32))>,
    /// (base span rel, index into `refs` of the Qualified name): the
    /// span of the name the base child passes on, or of the whole child
    /// when it passes none (see [`base_span`]).
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

// ---------------------------------------------------------------------------
// Top-level signatures: asking the file "who defines this name?" without
// building anything
// ---------------------------------------------------------------------------
//
// Resolving one item asks, for each name that escapes it, whether some
// item of the file defines that name at the top level (an EARLIER item,
// in an ordered root scope). The answer used to come from a hash set of
// every top-level name in the file, built again for every keystroke:
// O(file) of hashing to answer a handful of questions about one item.
//
// Instead, every top-level definition has a signature, one word hashed
// from its name when its fragment is built, and the file keeps them all
// in one flat array, item after item (`FileEntry::top_sigs`, with
// `top_ends` marking where each item's run stops). A question is a pass
// over that array comparing words. Only a definition whose word equals a
// question's is looked at, to confirm the name. Nothing is hashed but the
// names being asked about, and nothing is allocated per item.

/// Bit 0 of a signature: the definition is HOISTED, visible to every item
/// whatever the root's ordering: it is in a named namespace, or declared
/// hoisted. The environment fingerprints fold hoisted definitions apart
/// from the plain ones, which follow the root's ordering, so the fold has
/// to tell them apart without the name.
const SIG_HOISTED: u64 = 1;

/// Bit 1 of a signature: the definition is declared hoisted
/// (`@def(x, hoisted)`), in any namespace. A plain name in an ordered root
/// can then be answered by an item after the asking one.
const SIG_DECLARED: u64 = 2;

/// The bits of a signature that are not the name's: what a question
/// compares without.
const SIG_FLAGS: u64 = SIG_HOISTED | SIG_DECLARED;

/// The signature of one top-level name: a hash of `(ns, name)`, with
/// [`SIG_HOISTED`] set when `ns` is a named namespace. Equal names have
/// equal signatures, but for [`SIG_FLAGS`]. A scan always confirms a match
/// against the names, so a collision there costs a comparison and nothing
/// else.
fn name_sig(ns: &str, name: &str) -> u64 {
    let mut h = fnv(FNV_SEED, ns.as_bytes());
    h = fnv(h, &[1]);
    h = fnv(h, name.as_bytes());
    // FNV leaves its high bits weak on short keys, and the scan reads the
    // high bits first.
    h ^= h >> 32;
    h = h.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h ^= h >> 29;
    let h = h & !SIG_FLAGS;
    if ns.is_empty() {
        h
    } else {
        h | SIG_HOISTED
    }
}

/// One step of an order-sensitive hash over a sequence of signatures:
/// what an environment fingerprint is made of. Each step is a bijection
/// of `h`, so two sequences that part at one name and agree after it
/// never meet again.
fn fold_sig(h: u64, sig: u64) -> u64 {
    (h.rotate_left(5) ^ sig).wrapping_mul(0x517C_C1B7_2722_0A95)
}

/// One question for [`scan_top_defs`]: is `(ns, name)` defined at the top
/// level, in an item before the asking one, or in any item when
/// `anywhere`?
struct Wanted<'a> {
    ns: &'a str,
    name: &'a str,
    sig: u64,
    anywhere: bool,
    found: bool,
}

impl<'a> Wanted<'a> {
    /// The question a ref in namespace `ns` asks. An unordered root scope
    /// sees the whole file, and a named namespace is hoisted, so both look
    /// anywhere; a plain name in an ordered root looks only behind it.
    fn new(ns: &'a str, name: &'a str, unordered: bool) -> Self {
        Wanted {
            ns,
            name,
            sig: name_sig(ns, name) & !SIG_FLAGS,
            anywhere: unordered || !ns.is_empty(),
            found: false,
        }
    }
}

/// Answer every question in `wanted` in one pass over the file's
/// signatures. `k` is the asking item: a question that is not `anywhere`
/// is answered by the items before `k`, and by hoisted definitions after
/// it.
///
/// The pass stops as soon as nothing is left to ask. Its cost is one word
/// compared per top-level definition, plus the names of the definitions
/// whose word equalled a question still open.
fn scan_top_defs(e: &FileEntry, k: usize, wanted: &mut [Wanted<'_>]) {
    debug_assert_eq!(e.items.len(), e.top_ends.len());
    if wanted.is_empty() {
        return;
    }
    // The questions by signature, so that a definition's word finds its
    // questions by search and not by trying each.
    let mut by_sig: Vec<(u64, u32)> =
        wanted.iter().enumerate().map(|(i, w)| (w.sig, i as u32)).collect();
    by_sig.sort_unstable();
    // A word can only equal a question's if it shares its top six bits:
    // one AND turns most definitions away before the search.
    let bit = |sig: u64| 1u64 << (sig >> 58);
    let (mut mask_before, mut mask_anywhere) = (0u64, 0u64);
    let (mut left_before, mut left_anywhere) = (0usize, 0usize);
    for w in wanted.iter() {
        if w.anywhere {
            mask_anywhere |= bit(w.sig);
            left_anywhere += 1;
        } else {
            mask_before |= bit(w.sig);
            left_before += 1;
        }
    }

    // Only a declared-hoisted definition answers a question that is not
    // `anywhere` from after the asking item; without one in the file, the
    // pass stops at the asking item as it always did.
    let late = e.declared_hoisted > 0;
    let mut start = 0usize;
    for (j, &end) in e.top_ends.iter().enumerate() {
        let before = j < k;
        if left_anywhere == 0 && (left_before == 0 || !(before || late)) {
            break;
        }
        let end = end as usize;
        for (nth, &sig) in e.top_sigs[start..end].iter().enumerate() {
            // A hoisted definition answers from anywhere.
            let reach = before || sig & SIG_DECLARED != 0;
            let key = sig & !SIG_FLAGS;
            let mask = if reach { mask_before | mask_anywhere } else { mask_anywhere };
            if mask & bit(key) == 0 {
                continue;
            }
            let first = by_sig.partition_point(|&(s, _)| s < key);
            let same = |&&(s, _): &&(u64, u32)| s == key;
            // Open the fragment only for a question this item can still
            // answer: a name defined a thousand times is confirmed once.
            let open = by_sig[first..].iter().take_while(same).any(|&(_, wi)| {
                let w = &wanted[wi as usize];
                !w.found && (w.anywhere || reach)
            });
            if !open {
                continue;
            }
            let frag = &e.items[j].frag;
            let d = &frag.defs[frag.top_defs[nth] as usize];
            for &(_, wi) in by_sig[first..].iter().take_while(same) {
                let w = &mut wanted[wi as usize];
                if !w.found && (w.anywhere || reach) && d.ns == w.ns && d.name == w.name {
                    w.found = true;
                    if w.anywhere {
                        left_anywhere -= 1;
                    } else {
                        left_before -= 1;
                    }
                }
            }
        }
        start = end;
    }
}

/// A file re-resolves this many items by asking [`scan_top_defs`] for
/// each; more than that, and it builds the set of every top-level name
/// once and asks that. Both give the same answers. A scan costs a word
/// per top-level definition of the file; the set costs a hash and an
/// insertion per name. Measured on a 250,000-line C file (38,644 items):
/// under 0.1 ms for a scan of the whole file, about 5 ms for the set. So
/// a few scans are far cheaper than one set, and thousands of them are
/// not. Typing changes one item; opening a file, or renaming something
/// every later item can see, changes them all.
const SCAN_LIMIT: usize = 8;

/// Process-unique ids for paint-identity (fragments and resolutions).
/// Monotone, never reused — see `Fragment::uid`.
fn next_paint_uid() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// Parts: what a subtree declares, kept by the subtree, at every depth
// ---------------------------------------------------------------------------
//
// An item's fragment used to come from walking every node of the item
// again whenever the item changed. A keystroke changes one item, and an
// item is usually small; but an item is a top-level construct, and one of
// those can be a module of 100,000 lines. Typing in it walked all of it.
//
// What a subtree declares depends on nothing outside it, once it is said
// relative to the subtree: spans from where it starts, scopes from the
// one it is entered in, orders from its first declaration. So it is kept
// by the subtree, as a PART, and the incremental parser hands back the
// same subtrees, by pointer, for everything an edit did not rebuild. A
// subtree up to `PIECE_BYTES` is walked whole; a larger one is read child
// by child, each child a part of its own. Building an item's fragment
// again walks the chain of subtrees the edit rebuilt and the pieces at
// its end, and copies what every other part declares into place.
//
// A configuration that annotates nothing declares nothing anywhere, and
// walks nothing.

/// A subtree up to this many bytes is walked whole; a larger one is read
/// child by child. Small enough that walking one again is cheap next to a
/// keystroke, large enough that a document of megabytes is a few thousand
/// of them.
pub const PIECE_BYTES: u32 = 4096;

/// What a run of nodes declares, RELATIVE to the subtree it was read
/// from: spans from the subtree's start; scope 0 is the scope the subtree
/// is entered in, and scope `k ≥ 1` the k-th it opens; orders count from
/// 1 within it; a module's def index counts from its first def, `-1`
/// being the last def declared before it. Nothing here depends on where
/// the subtree stands or what is around it.
#[derive(Clone, Debug, Default)]
struct Facts {
    /// `top_level` is false here: what is top level depends on the scope
    /// the subtree is entered in, and is decided when it is put in place.
    defs: Vec<Def>,
    refs: Vec<Ref>,
    /// The scopes opened, in order (scope `k` is entry `k - 1`): parent,
    /// unordered, barrier, members.
    scopes: Vec<(ScopeId, bool, bool, bool)>,
    modules: Vec<(i64, (u32, u32))>,
    quals: Vec<((u32, u32), u32)>,
}

impl Facts {
    /// How many facts there are, of every kind.
    fn len(&self) -> usize {
        self.defs.len() + self.refs.len() + self.scopes.len() + self.modules.len() + self.quals.len()
    }
}

/// What one subtree declares: see [`Facts`].
#[derive(Debug, Default)]
struct Part {
    /// For a subtree walked whole, everything in it. For one read child by
    /// child, what its own node declares (the scope it opens, its def, its
    /// refs, its module and its qualified name).
    facts: Facts,
    /// The scope (relative) the node's children are entered in: the one
    /// it opens, or the one it was entered in.
    inner: ScopeId,
    /// For a subtree read child by child, its node children in order: each
    /// one's offset within the subtree and its own part.
    kids: Vec<Kid>,
    /// For a subtree read child by child whose node defers the defs of a
    /// child (`@defer`): that child's span (relative), and how many of
    /// `kids` are put in place before they take effect.
    defer: Option<((u32, u32), usize)>,
    /// How many facts the whole subtree holds. A part holding none puts
    /// nothing in place, and is passed over.
    total: usize,
}

/// A child subtree of a part read child by child.
#[derive(Debug)]
struct Kid {
    off: u32,
    /// The subtree: its identity, and what keeps that identity from being
    /// handed to another subtree while the part is kept.
    key: NodeKey,
    part: Arc<Part>,
    /// The namespace the node gives this child's subtree (`@ns(name,
    /// label)`), if it gives one; otherwise it stands in the node's.
    ns: Option<String>,
}

/// Span of the k-th SYMBOL child (token or rule), relative to `base`.
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

/// The text and offset of the name the k-th SYMBOL child holds: the child
/// itself when it is a token, and when it is a rule, the token its node
/// passes a name to (`cfg.passes`), however deep.
pub fn name_child(n: &GreenNode, k: usize, cfg: &BindingConfig) -> Option<(String, u32)> {
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
                return match c {
                    GreenChild::Token(t) => Some((t.text.clone(), off)),
                    GreenChild::Node(m) => {
                        let &(_, _, k2) =
                            cfg.passes.iter().find(|&&(nt, p, _)| nt == m.nt && p == m.prod)?;
                        let (name, inner) = name_child(m, k2, cfg)?;
                        Some((name, off + inner))
                    }
                };
            }
            idx += 1;
        }
        off += w;
    }
    None
}

/// Where the base of a qualified name names what it names: the name its
/// child `k` passes on (`Vec` of `Vec::<T>`, the last segment of `a::b`),
/// or, when it passes none, the whole child. The base resolves as the
/// last ref inside this span.
fn base_span(n: &GreenNode, base: u32, k: usize, cfg: &BindingConfig) -> Option<(u32, u32)> {
    match name_child(n, k, cfg) {
        Some((name, off)) => Some((base + off, base + off + name.len() as u32)),
        None => child_span(n, base, k),
    }
}

/// The namespace a def or ref takes from where it stands, until it is put
/// in place: what encloses the subtree it was read from decides (see
/// [`ItemFacts::put`]). Never left in a fragment.
const CONTEXT_NS: &str = "\u{0}context";

/// The namespace the subtree of child `k` of `n` stands in, when `n`
/// declares one for it (`@ns(name, label…)`).
fn child_ns<'c>(n: &GreenNode, k: usize, cfg: &'c BindingConfig) -> Option<&'c str> {
    cfg.child_namespaces
        .iter()
        .find(|&&(nt, p, c, _)| nt == n.nt && p == n.prod && c == k)
        .map(|t| t.3.as_str())
}

/// The symbol index of each child of `n` (`None` for trivia, a missing
/// token, or an error node): the numbering labels use.
pub fn symbol_indices(n: &GreenNode) -> impl Iterator<Item = Option<usize>> + '_ {
    let mut idx = 0usize;
    n.children.iter().map(move |c| {
        let is_symbol = match c {
            GreenChild::Token(t) => !t.trivia && !t.is_missing(),
            GreenChild::Node(m) => m.nt != ERROR_NT,
        };
        if is_symbol {
            idx += 1;
            Some(idx - 1)
        } else {
            None
        }
    })
}

/// How `n` defers the defs of one of its children (`@defer`): the span of
/// the pattern child (relative to `n`), and the symbol index of the child
/// after which they take effect (`None`: after `n` itself).
fn defer_of(n: &GreenNode, base: u32, cfg: &BindingConfig) -> Option<((u32, u32), Option<usize>)> {
    let &(_, _, pat, after) =
        cfg.defers.iter().find(|&&(nt, p, _, _)| nt == n.nt && p == n.prod)?;
    let span = child_span(n, base, pat)?;
    Some((span, after))
}

/// Give the defs from `first` on whose spans lie inside `span` the order
/// of the last fact read so far: they take effect after it.
fn defer_defs(defs: &mut [Def], first: usize, span: (u32, u32), order: u32) {
    for d in &mut defs[first..] {
        if d.span.0 >= span.0 && d.span.1 <= span.1 {
            d.order = order;
        }
    }
}

/// What node `n` itself declares, appended to `facts`: `n` stands at
/// `base` and is entered in `scope`. Returns the scope its children are
/// entered in. Its children are not read.
fn node_facts(
    n: &GreenNode,
    base: u32,
    scope: ScopeId,
    ctx: Option<&str>,
    cfg: &BindingConfig,
    facts: &mut Facts,
) -> ScopeId {
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
            let members = cfg.members.contains(&(n.nt, n.prod));
            facts.scopes.push((scope, unordered, barrier, members));
            scope = facts.scopes.len() as ScopeId;
        }
    }
    // A production's declared namespace covers its def and refs; without
    // one, they stand in the namespace of where the node stands.
    let node_ns = cfg
        .namespaces
        .iter()
        .find(|&&(nt2, p2, _)| nt2 == n.nt && p2 == n.prod)
        .map(|t| t.2.as_str())
        .or(ctx)
        .unwrap_or(CONTEXT_NS);
    // Every def and ref takes the next order: the count of those before
    // it, plus one.
    let next_order = |facts: &Facts| (facts.defs.len() + facts.refs.len()) as u32 + 1;
    if let Some(&(_, _, k)) = cfg.defs.iter().find(|&&(nt, p, _)| nt == n.nt && p == n.prod) {
        if let Some((name, off)) = name_child(n, k, cfg) {
            let order = next_order(facts);
            facts.defs.push(Def {
                span: (base + off, base + off + name.len() as u32),
                name,
                scope: outer,
                order,
                top_level: false,
                exported: cfg.exports.iter().any(|&(nt2, p2)| nt2 == n.nt && p2 == n.prod),
                ns: node_ns.to_string(),
                hoisted: cfg.hoisted.iter().any(|&(nt2, p2)| nt2 == n.nt && p2 == n.prod),
                open: cfg.open.iter().any(|&(nt2, p2)| nt2 == n.nt && p2 == n.prod),
            });
        }
    }
    // A production may carry SEVERAL references (a qualified path
    // has a base @ref and a @qualify name), so collect all entries.
    for &(nt2, p2, k, kind) in cfg.refs.iter() {
        if nt2 != n.nt || p2 != n.prod {
            continue;
        }
        if let Some((name, off)) = name_child(n, k, cfg) {
            let order = next_order(facts);
            facts.refs.push(Ref {
                span: (base + off, base + off + name.len() as u32),
                name,
                scope,
                order,
                kind,
                ns: node_ns.to_string(),
            });
        }
    }
    if let Some(&(_, _, body)) = cfg.modules.iter().find(|&&(nt2, p2, _)| nt2 == n.nt && p2 == n.prod) {
        if let Some(span) = child_span(n, base, body) {
            // The last def declared so far introduces the namespace (the
            // def this production just declared, when it declared one);
            // members are the defs inside `span`. `-1` here is the last
            // def before the subtree, if there is one: decided when the
            // subtree is put in place.
            facts.modules.push((facts.defs.len() as i64 - 1, span));
        }
    }
    if let Some(&(_, _, base_child, _)) =
        cfg.quals.iter().find(|&&(nt2, p2, _, _)| nt2 == n.nt && p2 == n.prod)
    {
        if let Some(bspan) = base_span(n, base, base_child, cfg) {
            // The Qualified ref for this node was pushed just above: no
            // ref declared before this node has its span inside the node.
            if let Some(ri) = facts
                .refs
                .iter()
                .rposition(|r| r.kind == RefKind::Qualified && r.span.0 >= base && r.span.0 < base + n.width)
            {
                facts.quals.push((bspan, ri as u32));
            }
        }
    }
    scope
}

/// Everything subtree `n` declares, appended to `facts`: every node, in
/// document order, each before its children. `ctx` is the namespace the
/// subtree stands in, when something within the walk declared one.
fn walk_whole(
    n: &GreenNode,
    base: u32,
    scope: ScopeId,
    ctx: Option<&str>,
    cfg: &BindingConfig,
    facts: &mut Facts,
) {
    let first_def = facts.defs.len();
    let inner = node_facts(n, base, scope, ctx, cfg, facts);
    let defer = defer_of(n, base, cfg);
    let mut deferred = false;
    let mut off = base;
    for (c, sym) in n.children.iter().zip(symbol_indices(n)) {
        if let GreenChild::Node(m) = c {
            let kid_ctx = sym.and_then(|k| child_ns(n, k, cfg)).or(ctx);
            walk_whole(m, off, inner, kid_ctx, cfg, facts);
        }
        off += c.width();
        if let (Some((span, Some(after))), Some(k)) = (defer, sym) {
            if k == after {
                let order = (facts.defs.len() + facts.refs.len()) as u32;
                defer_defs(&mut facts.defs, first_def, span, order);
                deferred = true;
            }
        }
    }
    // After the node: where it says so, or where the child it names is
    // missing (a node recovery left short).
    if let (Some((span, _)), false) = (defer, deferred) {
        let order = (facts.defs.len() + facts.refs.len()) as u32;
        defer_defs(&mut facts.defs, first_def, span, order);
    }
}

/// Does `cfg` annotate anything? Defs, refs and scopes are what a walk
/// can find; modules, qualified names, namespaces and exports only ever
/// add to a def or a ref found on the same node.
fn annotates_anything(cfg: &BindingConfig) -> bool {
    !cfg.defs.is_empty() || !cfg.refs.is_empty() || !cfg.scopes.is_empty()
}

/// A subtree the last revision read: its identity and its part.
type Read<'a> = (&'a NodeKey, &'a Arc<Part>);

/// Reads parts, reusing the last revision's where the subtree is the same.
struct Parts<'a> {
    cfg: &'a BindingConfig,
    /// The size up to which a subtree is walked whole: see
    /// `SemDb::piece_bytes`.
    piece: u32,
    /// The part of a subtree that declares nothing: one, shared.
    empty: &'a Arc<Part>,
    /// Bytes of subtrees walked whole.
    walked: u64,
}

impl Parts<'_> {
    /// Is `node` walked whole?
    fn whole(&self, node: &GreenNode) -> bool {
        node.width <= self.piece || !node.children.iter().any(|c| matches!(c, GreenChild::Node(_)))
    }

    /// The part of `node`. The last revision's subtrees that may be among
    /// its own, or among their children, are `old`.
    fn part(&mut self, node: &GreenNode, old: &[Read<'_>]) -> Arc<Part> {
        if self.whole(node) {
            self.walked += node.width as u64;
            let mut facts = Facts::default();
            walk_whole(node, 0, 0, None, self.cfg, &mut facts);
            let total = facts.len();
            if total == 0 {
                return self.empty.clone();
            }
            return Arc::new(Part { facts, inner: 0, kids: Vec::new(), defer: None, total });
        }
        let mut facts = Facts::default();
        let inner = node_facts(node, 0, 0, None, self.cfg, &mut facts);

        // Last revision's subtrees, and their children, by identity: the
        // edit rebuilt the chain of subtrees above it, and the subtrees on
        // either side of that chain are the ones it held before, one level
        // down or up at most where a list regrouped its runs.
        let mut known: HashMap<&NodeKey, &Arc<Part>> = HashMap::new();
        for &(key, part) in old {
            known.insert(key, part);
            for kid in &part.kids {
                known.insert(&kid.key, &kid.part);
            }
        }
        let mut kids: Vec<Kid> = Vec::new();
        let mut fresh: Vec<usize> = Vec::new();
        let defer = defer_of(node, 0, self.cfg);
        // How many kids come before the child after which the deferred
        // defs take effect, that child among them: error nodes, which no
        // label counts, included.
        let mut defer_before = None;
        let mut off = 0u32;
        for (c, sym) in node.children.iter().zip(symbol_indices(node)) {
            if let GreenChild::Node(m) = c {
                let key = NodeKey::new(m.clone());
                let part = match known.remove(&key) {
                    Some(part) => part.clone(),
                    None => {
                        fresh.push(kids.len());
                        self.empty.clone()
                    }
                };
                let ns = sym.and_then(|k| child_ns(node, k, self.cfg)).map(str::to_string);
                kids.push(Kid { off, key, part, ns });
            }
            off += c.width();
            if let (Some((_, Some(after))), Some(k)) = (defer, sym) {
                if k == after {
                    defer_before = Some(kids.len());
                }
            }
        }
        // Where the child it names is missing, or it names none: after
        // the node.
        let defer = defer.map(|(span, _)| (span, defer_before.unwrap_or(kids.len())));
        if !fresh.is_empty() {
            // What is left of the last revision, read child by child, may
            // hold the subtrees of the children read again.
            let left: Vec<Read<'_>> =
                known.into_iter().filter(|(_, part)| !part.kids.is_empty()).collect();
            for i in fresh {
                let part = self.part(kids[i].key.node(), &left);
                kids[i].part = part;
            }
        }
        // Kept even when nothing beneath declares anything: the children
        // are what the next edit finds its subtrees among.
        let total = facts.len() + kids.iter().map(|k| k.part.total).sum::<usize>();
        Arc::new(Part { facts, inner, kids, defer, total })
    }
}

/// An item's facts in place: spans item-relative, scopes the fragment's
/// (0 = the file's root scope), orders from 1 — the fragment, without
/// what is derived from it.
#[derive(Debug, PartialEq)]
struct ItemFacts {
    defs: Vec<Def>,
    refs: Vec<Ref>,
    scope_parents: Vec<ScopeId>,
    scope_unordered: Vec<bool>,
    scope_barrier: Vec<bool>,
    scope_members: Vec<bool>,
    modules: Vec<(u32, (u32, u32))>,
    quals: Vec<((u32, u32), u32)>,
}

impl ItemFacts {
    fn new(cfg: &BindingConfig) -> ItemFacts {
        ItemFacts {
            defs: Vec::new(),
            refs: Vec::new(),
            scope_parents: vec![0],
            scope_unordered: vec![cfg.unordered],
            scope_barrier: vec![false],
            scope_members: vec![false],
            modules: Vec::new(),
            quals: Vec::new(),
        }
    }

    /// Put `facts`, read from a subtree that stands at `at` and is entered
    /// in scope `enter`, in place after everything put so far. Returns the
    /// scope in place of relative scope `inner`.
    fn put(
        &mut self,
        facts: std::borrow::Cow<'_, Facts>,
        at: u32,
        enter: ScopeId,
        inner: ScopeId,
        ctx: Option<&str>,
    ) -> ScopeId {
        let scope_base = self.scope_parents.len() as ScopeId;
        let def_base = self.defs.len();
        let ref_base = self.refs.len();
        let order_base = (def_base + ref_base) as u32;
        let place = |rel: ScopeId| if rel == 0 { enter } else { scope_base + rel - 1 };
        let Facts { defs, refs, scopes, modules, quals } = facts.into_owned();
        for (parent, unordered, barrier, members) in scopes {
            self.scope_parents.push(place(parent));
            self.scope_unordered.push(unordered);
            self.scope_barrier.push(barrier);
            self.scope_members.push(members);
        }
        // What stands in the namespace of where the subtree stands takes
        // it now: the subtree's own facts do not know it.
        let context = ctx.unwrap_or("");
        self.defs.extend(defs.into_iter().map(|mut d| {
            d.span = (d.span.0 + at, d.span.1 + at);
            d.scope = place(d.scope);
            d.order += order_base;
            d.top_level = d.scope == 0;
            if d.ns == CONTEXT_NS {
                d.ns = context.to_string();
            }
            d
        }));
        self.refs.extend(refs.into_iter().map(|mut r| {
            r.span = (r.span.0 + at, r.span.1 + at);
            r.scope = place(r.scope);
            r.order += order_base;
            if r.ns == CONTEXT_NS {
                r.ns = context.to_string();
            }
            r
        }));
        for (def, span) in modules {
            // The last def before the subtree, when there is none, is
            // no def: the module is not declared.
            let def = def_base as i64 + def;
            if def >= 0 {
                self.modules.push((def as u32, (span.0 + at, span.1 + at)));
            }
        }
        self.quals.extend(quals.into_iter().map(|(span, ri)| ((span.0 + at, span.1 + at), ri + ref_base as u32)));
        place(inner)
    }

    /// Put a part, and everything beneath it, in place, the subtree
    /// standing in namespace `ctx` (`None`: the default).
    fn put_part(&mut self, part: &Part, at: u32, enter: ScopeId, ctx: Option<&str>) {
        if part.total == 0 {
            return;
        }
        let first_def = self.defs.len();
        let inner = self.put(std::borrow::Cow::Borrowed(&part.facts), at, enter, part.inner, ctx);
        for i in 0..=part.kids.len() {
            if let Some((span, before)) = part.defer {
                if i == before {
                    let order = (self.defs.len() + self.refs.len()) as u32;
                    defer_defs(&mut self.defs, first_def, (span.0 + at, span.1 + at), order);
                }
            }
            if let Some(kid) = part.kids.get(i) {
                self.put_part(&kid.part, at + kid.off, inner, kid.ns.as_deref().or(ctx));
            }
        }
    }

    /// Does `frag` hold exactly these facts?
    fn same_as(&self, frag: &Fragment) -> bool {
        self.defs == frag.defs
            && self.refs == frag.refs
            && self.scope_parents == frag.scope_parents
            && self.scope_unordered == frag.scope_unordered
            && self.scope_barrier == frag.scope_barrier
            && self.scope_members == frag.scope_members
            && self.modules == frag.modules
            && self.quals == frag.quals
    }

    /// The fragment of these facts: what is derived from them, and a new
    /// identity.
    fn finish(self) -> Fragment {
        let ItemFacts {
            defs,
            refs,
            scope_parents,
            scope_unordered,
            scope_barrier,
            scope_members,
            modules,
            quals,
        } = self;
        let mut f = Fragment {
            uid: next_paint_uid(),
            defs,
            refs,
            scope_parents,
            scope_unordered,
            scope_barrier,
            scope_members,
            top_defs: Vec::new(),
            top_sigs: Vec::new(),
            local: Vec::new(),
            modules,
            quals,
        };
        for (i, d) in f.defs.iter().enumerate() {
            if d.scope == 0 {
                f.top_defs.push(i as u32);
            }
        }
        f.top_sigs = f
            .top_defs
            .iter()
            .map(|&di| {
                let d = &f.defs[di as usize];
                let sig = name_sig(&d.ns, &d.name);
                if d.hoisted {
                    sig | SIG_HOISTED | SIG_DECLARED
                } else {
                    sig
                }
            })
            .collect();
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
        let mut local = Vec::with_capacity(f.refs.len());
        for r in &f.refs {
            let mut best: Option<(u32, u32, u32)> = None; // (depth, order, idx)
            // Imports resolve against other files' exports ONLY; qualified
            // names resolve among their base's members at query time.
            if r.kind != RefKind::Import && r.kind != RefKind::Qualified {
                if let Some(cands) = by_name.get(&(r.ns.as_str(), r.name.as_str())) {
                    for &i in cands {
                        let d = &f.defs[i as usize];
                        if f.scope_members[d.scope as usize] {
                            continue; // a member: reached only by a qualified name
                        }
                        if (f.scope_unordered[0] || !d.ns.is_empty() || d.hoisted) && d.scope == 0 {
                            continue; // top-level forward world: environment decides
                        }
                        if (f.scope_unordered[d.scope as usize]
                            || !d.ns.is_empty()
                            || d.hoisted
                            || d.order < r.order)
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
            local.push(match best {
                Some((_, _, i)) => LocalRes::Def(i),
                None if f.escapes(r.scope) => LocalRes::Escape,
                None => LocalRes::Contained,
            });
        }
        drop(by_name);
        f.local = local;
        f
    }
}

/// An item's facts, read through `parts`: walked whole when the item is
/// small, and then no part is kept; otherwise put together from its part,
/// which is returned to be kept with the item. `old` are the last
/// revision's items this one may hold subtrees of.
fn item_facts(
    item: &GreenNode,
    parts: &mut Parts<'_>,
    old: &[Read<'_>],
) -> (ItemFacts, Option<Arc<Part>>) {
    let mut out = ItemFacts::new(parts.cfg);
    if !annotates_anything(parts.cfg) {
        return (out, None);
    }
    if parts.whole(item) {
        parts.walked += item.width as u64;
        let mut facts = Facts::default();
        walk_whole(item, 0, 0, None, parts.cfg, &mut facts);
        out.put(std::borrow::Cow::Owned(facts), 0, 0, 0, None);
        return (out, None);
    }
    let part = parts.part(item, old);
    out.put_part(&part, 0, 0, None);
    (out, Some(part))
}

// ---------------------------------------------------------------------------
// Item resolutions (classification only — positions recovered at query
// time, so caches survive unrelated edits)
// ---------------------------------------------------------------------------

/// Why a qualified name names nothing.
enum QualifiedMiss {
    /// Its base names nothing the binding sees (it holds no reference,
    /// or its reference escapes), or a namespace open to members the
    /// binding cannot see: not a problem.
    Escapes,
    /// What to report.
    Error(String),
}

/// The miss of a qualified name whose base does not resolve: the base's
/// own diagnostic, never reported a second time for the name.
const BASE_UNRESOLVED: &str = "path base does not resolve";

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
    /// Times the set of EVERY top-level name of a file was built to
    /// resolve it. That is O(file): right when most of the file is being
    /// resolved (an open, a rename every later item sees), and never on a
    /// keystroke that changes one item.
    pub name_set_builds: u64,
    /// Times the position index over a file's top-level names was built.
    /// O(file) as well: navigation and whole-file views build it once per
    /// revision; a keystroke's repaint does not.
    pub top_index_builds: u64,
    /// Passes over a file's top-level signatures (see `scan_top_defs`):
    /// what answers "is this name defined?" for the few items a keystroke
    /// re-resolves or repaints.
    pub top_scans: u64,
    /// Bytes of subtrees walked node by node to build fragments. Building
    /// an item again walks what the edit rebuilt and puts every other part
    /// of it in place without a walk: a keystroke walks a piece or two,
    /// whatever the size of the item.
    pub bytes_walked: u64,
    /// Fragments built again that declare exactly what the fragment they
    /// replace declared, and so are that fragment still: its identity, and
    /// its resolution, carry over, and nothing downstream sees a change.
    pub fragments_kept: u64,
}

struct ItemSlot {
    /// Identity AND keepalive in one value: a [`NodeKey`] cannot exist
    /// without owning its subtree, so the address it compares by can
    /// never be recycled underneath this slot.
    key: NodeKey,
    base_off: u32,
    /// `frag.uid`, beside the slot so that listing every item's identity
    /// (`paint_sync`, once per keystroke) opens no fragment.
    frag_uid: u64,
    frag: Arc<Fragment>,
    /// How the fragment was put together, for the next revision to reuse:
    /// see [`Part`]. `None` for an item walked whole.
    part: Option<Arc<Part>>,
    /// (env fp, foreign fp, resolutions) — carried ACROSS revisions by
    /// the positional diff in set_tree, validated by fingerprint
    /// compare (two u64s, no hashing) in ensure_resolved.
    res: Option<(u64, u64, u64, Arc<ItemRes>)>,
}

struct FileEntry {
    tree: Arc<GreenNode>,
    tree_rev: u64,
    items: Vec<ItemSlot>,
    /// The signature of every top-level definition in the file, item
    /// after item (each item's `frag.top_sigs`, concatenated): the flat
    /// array a name question is scanned over, so that asking does not
    /// open every fragment. Carried across revisions with the items.
    top_sigs: Vec<u64>,
    /// Per item, where its run of `top_sigs` ends (it starts where the
    /// item before it ended).
    top_ends: Vec<u32>,
    /// How many of `top_sigs` are declared hoisted ([`SIG_DECLARED`]).
    declared_hoisted: usize,
    /// How many times this revision's repaint asked a name question by
    /// scan. Past [`SCAN_LIMIT`] the position index is cheaper, and is
    /// built.
    paint_scans: usize,
    /// item identity → fragment — survives edits (moved items keep
    /// their fragments); swept exactly, from the positional diff. The
    /// key owns the subtree, so a swept entry's address cannot come
    /// back as someone else's.
    frag_cache: HashMap<NodeKey, Cached>,
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

/// An item's fragment, kept by the item's identity, and its part.
struct Cached {
    frag: Arc<Fragment>,
    part: Option<Arc<Part>>,
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
    /// The part of every subtree that declares nothing.
    empty_part: Arc<Part>,
    /// The size up to which a subtree is walked whole: [`PIECE_BYTES`].
    /// (The tests read finer, down to every node a part, to put every
    /// fragment together from every shape of part.)
    piece_bytes: u32,
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
            empty_part: Arc::new(Part::default()),
            piece_bytes: PIECE_BYTES,
        }
    }

    /// Input: a file's current tree (from the incremental session).
    /// Cost tracks the EDIT: cached fragments are reused by Arc
    /// identity; the top-name count map updates from the fragment delta
    /// alone, so an unchanged signature is proven without ever sorting
    /// names; the position index and cache sweeps are deferred off the
    /// keystroke path.
    ///
    /// What still grows with the file is one walk of the root's list to
    /// find the items, a pointer compared per item to see which ones the
    /// edit left alone, and an offset stored per item. An item left
    /// alone keeps its slot: nothing is allocated, cloned or hashed for
    /// it.
    ///
    /// An item the edit rebuilt is put together from the parts of the
    /// items it replaced (see `Part`): what is walked is the chain of
    /// subtrees the edit rebuilt and the pieces at its end. What grows
    /// with the ITEM is copying its facts into place; that is nothing
    /// for an item that declares nothing.
    pub fn set_tree(&mut self, uri: &str, tree: Arc<GreenNode>) {
        self.revision += 1;
        let rev = self.revision;
        let cfg = &self.cfg;
        let mut fragments_computed = 0u64;

        let e = self.files.entry(uri.to_string()).or_insert_with(|| FileEntry {
            tree: tree.clone(),
            tree_rev: rev,
            items: Vec::new(),
            top_sigs: Vec::new(),
            top_ends: Vec::new(),
            declared_hoisted: 0,
            paint_scans: 0,
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
        e.paint_scans = 0;

        // Items = the elements of the root's list child(ren); any other
        // node child of the root is an item of its own. Files without
        // list structure degrade to one whole-file item.
        //
        // They are borrowed from the tree, not cloned: an item an edit
        // left alone is recognised where it stands, and its slot stays.
        let mut items: Vec<(&Arc<GreenNode>, u32)> = Vec::new();
        {
            let mut off = 0u32;
            for c in &tree.children {
                if let GreenChild::Node(n) = c {
                    if n.prod == LIST_PROD {
                        collect_list_items(n, off, &mut items);
                    } else if n.nt != ERROR_NT {
                        items.push((n, off));
                    }
                }
                off += c.width();
            }
            if items.is_empty() {
                items.push((&tree, 0));
            }
        }

        // Positional diff by pointer identity: the common prefix and
        // suffix keep their slots, fragments AND resolutions, where they
        // are (no map traffic, no reference counted, nothing allocated);
        // only the edit-sized window between them consults the fragment
        // cache. Dropped window fragments are swept EXACTLY (their top
        // names counted out — what keeps `top_counts` and the
        // signature-dirty decision O(edit), not O(file)).
        let mut slots = std::mem::take(&mut e.items);
        let (old_len, n_new) = (slots.len(), items.len());
        let mut lo = 0usize;
        while lo < old_len && lo < n_new && slots[lo].key.is(items[lo].0) {
            lo += 1;
        }
        let mut suffix = 0usize;
        while suffix < old_len - lo
            && suffix < n_new - lo
            && slots[old_len - 1 - suffix].key.is(items[n_new - 1 - suffix].0)
        {
            suffix += 1;
        }
        let (old_hi, new_hi) = (old_len - suffix, n_new - suffix);

        // The window: each new item there is one the last revision had too
        // (it moved, and keeps its fragment), or one built again, from the
        // parts of the items it replaced wherever it holds their subtrees.
        let new_keys: Vec<NodeKey> =
            items[lo..new_hi].iter().map(|&(node, _)| NodeKey::new(node.clone())).collect();
        let new_mid: HashSet<NodeKey> = new_keys.iter().cloned().collect();
        let replaced: Vec<usize> = (lo..old_hi).filter(|&i| !new_mid.contains(&slots[i].key)).collect();
        // Where the edit put one item in the place of one it replaced, the
        // new one that declares exactly what the old one did IS the old
        // one's fragment still: same identity, same resolution.
        let one_for_one = old_hi - lo == new_hi - lo;
        let mut parts = Parts { cfg, piece: self.piece_bytes, empty: &self.empty_part, walked: 0 };
        let mut fragments_kept = 0u64;
        let mut window: Vec<ItemSlot> = Vec::with_capacity(new_hi - lo);
        for (j, (key, &(_, base_off))) in new_keys.into_iter().zip(&items[lo..new_hi]).enumerate() {
            if let Some(c) = e.frag_cache.get(&key) {
                let (frag, part) = (c.frag.clone(), c.part.clone());
                window.push(ItemSlot { key, base_off, frag_uid: frag.uid, frag, part, res: None });
                continue;
            }
            let (facts, part) = {
                let old: Vec<Read<'_>> = replaced
                    .iter()
                    .filter_map(|&i| slots[i].part.as_ref().map(|part| (&slots[i].key, part)))
                    .collect();
                item_facts(key.node(), &mut parts, &old)
            };
            fragments_computed += 1;
            let at = lo + j;
            let (frag, res) = if one_for_one
                && !new_mid.contains(&slots[at].key)
                && facts.same_as(&slots[at].frag)
            {
                // The replaced item's fragment passes to this one, its top
                // names counted once still: the sweep below finds nothing
                // of it to count out.
                fragments_kept += 1;
                e.frag_cache.remove(&slots[at].key);
                (slots[at].frag.clone(), slots[at].res.take())
            } else {
                let f = Arc::new(facts.finish());
                for &di in &f.top_defs {
                    let d = &f.defs[di as usize];
                    let c = e.top_counts.entry(tkey(&d.ns, &d.name)).or_insert((0, 0));
                    c.0 += 1;
                    c.1 += d.exported as i64;
                    e.sig_dirty = true;
                }
                (f, None)
            };
            e.frag_cache.insert(key.clone(), Cached { frag: frag.clone(), part: part.clone() });
            window.push(ItemSlot { key, base_off, frag_uid: frag.uid, frag, part, res });
        }

        // The signatures follow the items: the runs of the items kept
        // before and after the edit stay, and only the window between
        // them is read from its fragments.
        {
            let run_start = |ends: &[u32], i: usize| if i == 0 { 0 } else { ends[i - 1] as usize };
            let (old_lo_at, old_hi_at) =
                (run_start(&e.top_ends, lo), run_start(&e.top_ends, old_hi));
            let window_sigs: Vec<u64> =
                window.iter().flat_map(|slot| slot.frag.top_sigs.iter().copied()).collect();
            let mut at = old_lo_at;
            let window_ends: Vec<u32> = window
                .iter()
                .map(|slot| {
                    at += slot.frag.top_sigs.len();
                    at as u32
                })
                .collect();
            // The runs after the window keep their lengths and move by
            // the window's change in length.
            let shift = at as i64 - old_hi_at as i64;
            if shift != 0 {
                for end in &mut e.top_ends[old_hi..] {
                    *end = (*end as i64 + shift) as u32;
                }
            }
            let declared = |sigs: &[u64]| sigs.iter().filter(|&&s| s & SIG_DECLARED != 0).count();
            e.declared_hoisted = e.declared_hoisted + declared(&window_sigs)
                - declared(&e.top_sigs[old_lo_at..old_hi_at]);
            e.top_sigs.splice(old_lo_at..old_hi_at, window_sigs);
            e.top_ends.splice(lo..old_hi, window_ends);
        }

        let removed: Vec<ItemSlot> = slots.splice(lo..old_hi, window).collect();
        for slot in &removed {
            if !new_mid.contains(&slot.key) {
                if let Some(Cached { frag: f, .. }) = e.frag_cache.remove(&slot.key) {
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
        // A kept item is the same subtree; where it starts may have moved.
        debug_assert_eq!(slots.len(), n_new);
        for (slot, &(node, base_off)) in slots.iter_mut().zip(&items) {
            debug_assert!(slot.key.is(node));
            slot.base_off = base_off;
        }
        debug_assert_eq!(e.top_ends.len(), slots.len());
        debug_assert!({
            let mut start = 0usize;
            slots.iter().zip(&e.top_ends).all(|(slot, &end)| {
                let run = &e.top_sigs[start..end as usize];
                start = end as usize;
                run == slot.frag.top_sigs.as_slice()
            })
        });
        e.items = slots;

        self.stats.fragments_computed += fragments_computed;
        self.stats.fragments_kept += fragments_kept;
        self.stats.bytes_walked += parts.walked;
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
        //
        // The sequences are folded from the names' signatures, which
        // were hashed once when each fragment was built: these passes
        // read one flat array and open no fragment. (They used to hash
        // every top-level name of the file again for every keystroke.)
        let unordered = self.cfg.unordered;
        let tier = self.cfg.module_tier();
        let e = &self.files[uri];
        let n_items = e.items.len();
        let mut named_h = FNV_SEED;
        for &sig in &e.top_sigs {
            if sig & SIG_HOISTED != 0 {
                named_h = fold_sig(named_h, sig);
            }
        }
        let mut env_fps = Vec::with_capacity(n_items);
        if unordered {
            let mut h = named_h;
            for &sig in &e.top_sigs {
                if sig & SIG_HOISTED == 0 {
                    h = fold_sig(h, sig);
                }
            }
            env_fps.resize(n_items, h);
        } else {
            let mut h = FNV_SEED;
            let mut start = 0usize;
            for &end in &e.top_ends {
                env_fps.push(fold_sig(h, named_h));
                for &sig in &e.top_sigs[start..end as usize] {
                    if sig & SIG_HOISTED == 0 {
                        h = fold_sig(h, sig);
                    }
                }
                start = end as usize;
            }
        }
        debug_assert_eq!(env_fps.len(), n_items);

        // Foreign exports, in deterministic smallest-uri-first order.
        let mut other_uris: Vec<String> =
            self.files.keys().filter(|u| *u != uri).cloned().collect();
        other_uris.sort();

        // Classify escaped refs per item. Slots carry their resolutions
        // across revisions (moved over by the positional diff); a
        // two-u64 fingerprint compare validates each. Only the items
        // that fail it are resolved, and how they ask the file about a
        // name depends on how many of them there are (see SCAN_LIMIT).
        let e = &self.files[uri];
        let is_hit = |k: usize, slot: &ItemSlot| {
            matches!(&slot.res, Some((ef, ff, _, _)) if *ef == env_fps[k] && *ff == foreign_fp)
        };
        let misses: Vec<usize> = e
            .items
            .iter()
            .enumerate()
            .filter(|(k, slot)| !is_hit(*k, slot))
            .map(|(k, _)| k)
            .collect();
        let mut computed: Vec<(usize, Arc<ItemRes>)> = Vec::with_capacity(misses.len()); // (item idx, res)
        let new_resolves = misses.len() as u64;
        let (mut scans, mut set_builds) = (0u64, 0u64);
        if misses.len() <= SCAN_LIMIT {
            // A few items (a keystroke changes one): ask the file's
            // signatures, one pass per item, for every name that item's
            // refs let escape.
            for &k in &misses {
                let frag = &e.items[k].frag;
                // ref index → its question, for the refs that ask one:
                // exactly those the classification consults the
                // environment for.
                let mut question = vec![u32::MAX; frag.refs.len()];
                let mut wanted: Vec<Wanted<'_>> = Vec::new();
                for (ri, r) in frag.refs.iter().enumerate() {
                    if r.kind != RefKind::Qualified
                        && r.kind != RefKind::Import
                        && matches!(frag.local[ri], LocalRes::Escape)
                    {
                        question[ri] = wanted.len() as u32;
                        wanted.push(Wanted::new(&r.ns, &r.name, unordered));
                    }
                }
                if !wanted.is_empty() {
                    scan_top_defs(e, k, &mut wanted);
                    scans += 1;
                }
                let res = self.classify_item(frag, &other_uris, tier, |ri| {
                    wanted[question[ri] as usize].found
                });
                computed.push((k, Arc::new(res)));
            }
        } else {
            // Most of the file: build the environment once and walk it
            // forward, as an ordered root scope reads.
            set_builds += 1;
            let mut env: HashSet<(&str, &str)> = HashSet::new();
            // Hoisted contributions first: every named-namespace or
            // declared-hoisted top def is visible to every item, whatever
            // the root's ordering says.
            for slot in &e.items {
                for &di in &slot.frag.top_defs {
                    let d = &slot.frag.defs[di as usize];
                    if unordered || !d.ns.is_empty() || d.hoisted {
                        env.insert((d.ns.as_str(), d.name.as_str()));
                    }
                }
            }
            for (k, slot) in e.items.iter().enumerate() {
                if !is_hit(k, slot) {
                    let frag = &slot.frag;
                    let res = self.classify_item(frag, &other_uris, tier, |ri| {
                        let r = &frag.refs[ri];
                        env.contains(&(r.ns.as_str(), r.name.as_str()))
                    });
                    computed.push((k, Arc::new(res)));
                }
                if !unordered {
                    for &di in &slot.frag.top_defs {
                        let d = &slot.frag.defs[di as usize];
                        if d.ns.is_empty() && !d.hoisted {
                            env.insert(("", d.name.as_str()));
                        }
                    }
                }
            }
        }
        let e = self.files.get_mut(uri).unwrap();
        for (k, res) in computed {
            let env_fp = env_fps[k];
            e.items[k].res = Some((env_fp, foreign_fp, next_paint_uid(), res));
        }
        e.env_fps = Some(env_fps);
        e.resolved_under = Some(foreign_fp);
        self.stats.item_resolves_computed += new_resolves;
        self.stats.top_scans += scans;
        self.stats.name_set_builds += set_builds;
    }

    /// Classify the refs of one item. `in_env(ri)` says whether the
    /// file's own top level binds the name of ref `ri` where the item can
    /// see it; it is asked only for a ref that escapes its fragment and
    /// is not an import.
    fn classify_item(
        &self,
        frag: &Fragment,
        other_uris: &[String],
        tier: bool,
        in_env: impl Fn(usize) -> bool,
    ) -> ItemRes {
        let mut targets = Vec::with_capacity(frag.refs.len());
        let mut unresolved = Vec::new();
        let mut private = Vec::new();
        for (ri, r) in frag.refs.iter().enumerate() {
            let t = if r.kind == RefKind::Qualified {
                Classified::Qualified
            } else {
                match frag.local[ri] {
                    LocalRes::Def(d) => Classified::Local(d),
                    LocalRes::Contained => Classified::Unresolved,
                    LocalRes::Escape => {
                        let rk = tkey(&r.ns, &r.name);
                        if r.kind != RefKind::Import && in_env(ri) {
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
        ItemRes { targets, unresolved, private }
    }

    /// The lazy position index over top-level defs — built at most once
    /// per revision, and only when a query needs POSITIONS (navigation,
    /// the composed whole-file view) or asks about more items than a scan
    /// is worth (see [`tops_bound`](Self::tops_bound)).
    fn ensure_top_index(&mut self, uri: &str) {
        if self.files[uri].top_index.is_some() {
            return;
        }
        self.stats.top_index_builds += 1;
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
    /// (ordered: latest strictly-earlier item, or latest hoisted def
    /// anywhere; unordered OR named namespace: latest anywhere —
    /// per-namespace ordering).
    fn top_site(&mut self, uri: &str, ns: &str, name: &str, k: u32) -> Option<(u32, u32)> {
        self.ensure_top_index(uri);
        let e = &self.files[uri];
        let sites = e.top_index.as_ref().unwrap().get(&tkey(ns, name))?;
        if self.cfg.unordered || !ns.is_empty() {
            sites.last().copied()
        } else {
            sites
                .iter()
                .rev()
                .find(|&&(i, di)| i < k || e.items[i as usize].frag.defs[di as usize].hoisted)
                .copied()
        }
    }

    /// For item `k`, which of these `(ns, name)` does the file's top
    /// level bind where `k` can see it: exactly
    /// `top_site(uri, ns, name, k).is_some()` for each, without needing
    /// the position index.
    ///
    /// A repaint asks this for the few items a keystroke changed, and
    /// only wants yes or no. Building the index for that is O(file) for
    /// every keystroke; one pass over the signatures answers all of an
    /// item's names. When the index already exists it is used, and when a
    /// revision has asked more than [`SCAN_LIMIT`] times (a whole-file
    /// repaint: an open) the index is built and used from there.
    fn tops_bound(&mut self, uri: &str, k: u32, names: &[(&str, &str)]) -> Vec<bool> {
        if names.is_empty() {
            return Vec::new();
        }
        let e = &self.files[uri];
        if e.top_index.is_none() && e.paint_scans < SCAN_LIMIT {
            let unordered = self.cfg.unordered;
            let mut wanted: Vec<Wanted<'_>> =
                names.iter().map(|&(ns, name)| Wanted::new(ns, name, unordered)).collect();
            scan_top_defs(e, k as usize, &mut wanted);
            let bound = wanted.iter().map(|w| w.found).collect();
            self.files.get_mut(uri).unwrap().paint_scans += 1;
            self.stats.top_scans += 1;
            return bound;
        }
        names.iter().map(|&(ns, name)| self.top_site(uri, ns, name, k).is_some()).collect()
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

    /// Does the reference at `offset` ESCAPE: bind to nothing the
    /// binding sees, without that being a problem? (See
    /// [`Target::Escapes`].) False where there is no reference, and for
    /// one that resolves or is unresolved.
    pub fn escapes(&mut self, uri: &str, offset: u32) -> bool {
        self.ensure_resolved(uri);
        let Some(k) = self.item_at(uri, offset) else { return false };
        let slot = &self.files[uri].items[k as usize];
        let rel = offset - slot.base_off;
        let Some(ri) = slot.frag.refs.iter().position(|r| r.span.0 <= rel && rel < r.span.1) else {
            return false;
        };
        let escaping = slot.frag.refs[ri].kind == RefKind::Escaping;
        let qualified =
            matches!(slot.res.as_ref().map(|r| &r.3.targets[ri]), Some(Classified::Qualified));
        if qualified {
            matches!(self.resolve_qualified(uri, k, ri as u32), Err(QualifiedMiss::Escapes))
        } else {
            escaping && self.definition(uri, offset).is_none()
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
    ) -> Result<(String, (u32, u32)), QualifiedMiss> {
        let (name, name_span, base_span) = {
            let slot = &self.files[uri].items[item as usize];
            let r = &slot.frag.refs[ri as usize];
            let q =
                slot.frag.quals.iter().find(|(_, qri)| *qri == ri).ok_or_else(|| {
                    QualifiedMiss::Error("malformed qualified reference".to_string())
                })?;
            (
                r.name.clone(),
                (slot.base_off + r.span.0, slot.base_off + r.span.1),
                (slot.base_off + q.0 .0, slot.base_off + q.0 .1),
            )
        };
        let _ = name_span;
        // The base's resolution = the resolution of the LAST reference
        // inside the base's span (leftward recursion for nested paths).
        // A base holding none names nothing the binding sees.
        let (base_ri, base_ref_start, base_kind, base_qualified) = {
            let slot = &self.files[uri].items[item as usize];
            let (bi, b) = slot
                .frag
                .refs
                .iter()
                .enumerate()
                .filter(|(_, r)| {
                    let s = slot.base_off + r.span.0;
                    s >= base_span.0 && s < base_span.1
                })
                .max_by_key(|(_, r)| r.span.0)
                .ok_or(QualifiedMiss::Escapes)?;
            // `definition` takes a def on the token before a ref on it.
            let def_on_it =
                slot.frag.defs.iter().any(|d| d.span.0 <= b.span.0 && b.span.0 < d.span.1);
            let qualified = !def_on_it
                && matches!(
                    slot.res.as_ref().map(|r| &r.3.targets[bi]),
                    Some(Classified::Qualified)
                );
            (bi as u32, slot.base_off + b.span.0, b.kind, qualified)
        };
        // A base that does not resolve: the name escapes when the base
        // does, and is otherwise the base's own error. A qualified base is
        // asked once, not through `definition` and again on a miss: a
        // path is resolved in time linear in its segments.
        let base = if base_qualified {
            self.resolve_qualified(uri, item, base_ri).map_err(|miss| match miss {
                QualifiedMiss::Escapes => QualifiedMiss::Escapes,
                QualifiedMiss::Error(_) => QualifiedMiss::Error(BASE_UNRESOLVED.to_string()),
            })
        } else {
            self.definition(uri, base_ref_start).ok_or_else(|| {
                if base_kind == RefKind::Escaping {
                    QualifiedMiss::Escapes
                } else {
                    QualifiedMiss::Error(BASE_UNRESOLVED.to_string())
                }
            })
        };
        let (mut turi, mut tspan) = base?;
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
        // Locate the target def and its module body. An OPEN namespace
        // has members the binding cannot see: a name missing from it
        // escapes.
        let open: bool;
        self.ensure_resolved(&turi);
        let (body_abs, titem) = {
            let e = &self.files[&turi];
            let k = e
                .items
                .partition_point(|s| s.base_off <= tspan.0)
                .checked_sub(1)
                .ok_or_else(|| QualifiedMiss::Error("target out of range".to_string()))?;
            let slot = &e.items[k];
            let rel = tspan.0 - slot.base_off;
            let di =
                slot.frag.defs.iter().position(|d| d.span.0 == rel).ok_or_else(|| {
                    QualifiedMiss::Error("target is not a definition".to_string())
                })?;
            let target = &slot.frag.defs[di];
            open = target.open;
            let m =
                slot.frag.modules.iter().find(|(mdi, _)| *mdi == di as u32).ok_or_else(|| {
                    if open {
                        QualifiedMiss::Escapes
                    } else {
                        QualifiedMiss::Error(format!("`{}` is not a module", target.name))
                    }
                })?;
            ((slot.base_off + m.1 .0, slot.base_off + m.1 .1), k)
        };
        // Members: defs of the target item inside the body span.
        let crossing = turi != uri;
        let e = &self.files[&turi];
        let slot = &e.items[titem];
        // The members are the body's own defs: the shallowest inside it.
        // A `let` in one of a module's functions is no member of it.
        let inside = |d: &Def| {
            let s = slot.base_off + d.span.0;
            s >= body_abs.0 && s < body_abs.1
        };
        let depth = slot
            .frag
            .defs
            .iter()
            .filter(|d| inside(d))
            .map(|d| slot.frag.scope_depth(d.scope))
            .min();
        let mut found_private = false;
        let mut hit: Option<(u32, u32)> = None;
        for d in &slot.frag.defs {
            let s = slot.base_off + d.span.0;
            if inside(d) && Some(slot.frag.scope_depth(d.scope)) == depth && d.name == name {
                if crossing && !d.exported {
                    found_private = true;
                } else {
                    hit = Some((s, slot.base_off + d.span.1));
                }
            }
        }
        match (hit, found_private) {
            (Some(span), _) => Ok((turi, span)),
            (None, true) => Err(QualifiedMiss::Error(format!(
                "`{name}` exists in the module but is not exported"
            ))),
            (None, false) if open => Err(QualifiedMiss::Escapes),
            (None, false) => {
                Err(QualifiedMiss::Error(format!("no member `{name}` in this module")))
            }
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
                        (k as u32, ri, (slot.base_off + r.span.0, slot.base_off + r.span.1))
                    })
                })
                .collect()
        };
        let mut out = Vec::new();
        for (k, ri, span) in quals {
            // A name that escapes is not reported: what it names may be
            // where this file cannot see. An unresolved BASE is already
            // the base ref's own diagnostic; it is not reported twice.
            if let Err(QualifiedMiss::Error(msg)) = self.resolve_qualified(uri, k, ri) {
                if msg != BASE_UNRESOLVED {
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
                // A member is no name in scope: it is reached by a
                // qualified name.
                if slot.frag.scope_members[d.scope as usize] {
                    continue;
                }
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
                    hoisted: d.hoisted,
                    open: d.open,
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
                        Err(QualifiedMiss::Escapes) => Target::Escapes,
                        Err(QualifiedMiss::Error(_)) => Target::Unresolved,
                    },
                    Classified::Unresolved => Target::Unresolved,
                };
                // An escaping ref the file does not define escapes; a
                // qualified name's miss was told above.
                let escaping =
                    self.files[uri].items[k as usize].frag.refs[ri].kind == RefKind::Escaping;
                out.push(if t == Target::Unresolved && escaping { Target::Escapes } else { t });
            }
        }
        Arc::new(out)
    }

    /// Does this database declare nothing a document could hold: no
    /// binding annotation and no type tier? Then every file's fragments
    /// are empty and its type report is too, whatever its text: there is
    /// nothing to resolve, and nothing to paint over the lexical runs.
    pub fn declares_nothing(&self) -> bool {
        !annotates_anything(&self.cfg) && self.type_cfg.is_none()
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
                frag_uid: s.frag_uid,
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
        // The refs the file's own top level binds, all asked at once.
        let top_refs: Vec<usize> =
            (0..frag.refs.len()).filter(|&ri| res.targets[ri] == Classified::Top).collect();
        let top_names: Vec<(&str, &str)> = top_refs
            .iter()
            .map(|&ri| (frag.refs[ri].ns.as_str(), frag.refs[ri].name.as_str()))
            .collect();
        let top_bound = self.tops_bound(uri, item, &top_names);
        let mut next_top = 0usize;
        let mut refs = Vec::with_capacity(frag.refs.len());
        for (ri, r) in frag.refs.iter().enumerate() {
            let quiet_gate = |rp: RefPaint| {
                // The unresolved BIT is kind-gated, exactly as the
                // composed-view overlay gates it: an escaping ref is never
                // painted unresolved. (A qualified name that escapes is
                // told quiet where it is resolved, below.)
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
                Classified::Top => {
                    debug_assert_eq!(top_refs[next_top], ri);
                    let bound = top_bound[next_top];
                    next_top += 1;
                    if bound {
                        RefPaint::Ref
                    } else {
                        RefPaint::Unresolved
                    }
                }
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
                    Err(QualifiedMiss::Escapes) => RefPaint::Quiet,
                    Err(QualifiedMiss::Error(_)) => RefPaint::Unresolved,
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

fn collect_list_items<'t>(n: &'t GreenNode, base: u32, out: &mut Vec<(&'t Arc<GreenNode>, u32)>) {
    let mut off = base;
    for c in &n.children {
        match c {
            GreenChild::Node(m) if m.prod == RUN_PROD => {
                collect_list_items(m, off, out);
            }
            GreenChild::Node(m) => {
                out.push((m, off));
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
    /// Bound to nothing the binding sees, and not a problem: an escaping
    /// reference, or a qualified name whose base escapes, holds no
    /// reference, or names an open namespace (`@def(x, open)`). What it
    /// names may lie beyond the files the database holds.
    Escapes,
    Unresolved,
}

pub type Resolutions = Vec<Target>;

#[cfg(test)]
mod part_tests {
    //! Parts against the walk they replace. After every step of an edit
    //! script, every item's fragment must declare exactly what the walk
    //! the parts replaced finds in the item — the walk as it was, kept
    //! here verbatim as the oracle — however finely the item is read, and
    //! whatever the last revision's parts it was put together from.

    use super::*;
    use qana_engine::{IncSession, Line, LineEdit, LineTerm};
    use qana_grammar::demo::{demo_grammar, demo_syn_grammar};
    use qana_grammar::{build_lr, CompiledLexer, LrTables};

    struct Pipe {
        lexer: CompiledLexer,
        sg: SynGrammar,
        tables: LrTables,
    }

    fn pipe() -> Pipe {
        let (g, ids) = demo_grammar();
        let lexer = CompiledLexer::build(&g).unwrap();
        let sg = demo_syn_grammar(&ids, &lexer.vocab);
        let tables = build_lr(&sg);
        assert!(tables.conflicts.is_empty());
        Pipe { lexer, sg, tables }
    }

    /// Every kind of annotation, on the demo grammar's productions. What
    /// it means as a language does not matter here: a walk is mechanical
    /// over (production, child), and this config makes it do all of it —
    /// nested and barrier scopes, ordered and not, exported defs, two refs
    /// on one node, a qualified name, a namespace, a module whose def is
    /// its own, and a module whose def is the last one declared before it
    /// (in another part, as often as not).
    fn stress_config(sg: &SynGrammar) -> BindingConfig {
        let mut cfg = BindingConfig::default();
        for i in 0..sg.prods.len() {
            let (nt, prod) = (sg.prods[i].lhs, i as u16);
            match sg.prod_name(i).as_str() {
                "LetStmt" => {
                    cfg.defs.push((nt, prod, 1));
                    cfg.exports.push((nt, prod));
                    cfg.modules.push((nt, prod, 3));
                }
                "NameRef" => cfg.refs.push((nt, prod, 0, RefKind::Var)),
                "CallExpr" => {
                    cfg.refs.push((nt, prod, 0, RefKind::Call));
                    cfg.refs.push((nt, prod, 0, RefKind::Qualified));
                    cfg.quals.push((nt, prod, 2, 0));
                    cfg.namespaces.push((nt, prod, "fn".to_string()));
                }
                "Block" => cfg.scopes.push((nt, prod, false, false)),
                "IfStmt" => cfg.scopes.push((nt, prod, true, true)),
                "BlockStmt" => cfg.modules.push((nt, prod, 0)),
                _ => {}
            }
        }
        cfg
    }

    /// What binding came to need after the walk's first annotations, on
    /// the demo grammar's productions, mechanical as [`stress_config`] is:
    /// a pattern whose defs take effect after a later child (`let`'s,
    /// after its value), after a sibling subtree (the first block of an
    /// `if … else`, after the second) and after the node itself; hoisted
    /// defs; escaping refs, and a qualified name that escapes with its
    /// base; a namespace a node gives one child's subtree, nested in
    /// another's; a scope of members; and a def whose name is passed to
    /// it through rules, two deep for `((a));`.
    fn features_config(sg: &SynGrammar) -> BindingConfig {
        let mut cfg = BindingConfig::default();
        for i in 0..sg.prods.len() {
            let (nt, prod) = (sg.prods[i].lhs, i as u16);
            match sg.prod_name(i).as_str() {
                "LetStmt" => {
                    cfg.defs.push((nt, prod, 1));
                    cfg.defers.push((nt, prod, 1, Some(3)));
                }
                "ExprStmt" => {
                    cfg.defs.push((nt, prod, 0));
                    cfg.hoisted.push((nt, prod));
                }
                "NameRef" => {
                    cfg.refs.push((nt, prod, 0, RefKind::Escaping));
                    cfg.passes.push((nt, prod, 0));
                }
                "ParenExpr" => cfg.passes.push((nt, prod, 1)),
                "CallExpr" => {
                    cfg.refs.push((nt, prod, 0, RefKind::Call));
                    cfg.refs.push((nt, prod, 0, RefKind::Qualified));
                    cfg.quals.push((nt, prod, 2, 0));
                    cfg.namespaces.push((nt, prod, "fn".to_string()));
                }
                "Block" => cfg.scopes.push((nt, prod, false, false)),
                "IfStmt" => {
                    cfg.scopes.push((nt, prod, true, false));
                    cfg.members.push((nt, prod));
                    cfg.child_namespaces.push((nt, prod, 2, "cond".to_string()));
                }
                "IfElseStmt" => {
                    cfg.defers.push((nt, prod, 4, Some(6)));
                    cfg.child_namespaces.push((nt, prod, 6, "else".to_string()));
                }
                "BlockStmt" => {
                    cfg.modules.push((nt, prod, 0));
                    cfg.defers.push((nt, prod, 0, None));
                }
                _ => {}
            }
        }
        cfg
    }

    fn rng(mut seed: u64) -> impl FnMut(usize) -> usize {
        move |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n.max(1) as u64) as usize
        }
    }

    /// One line of the demo language, not counting braces that open or
    /// close a block over lines.
    fn statement(r: &mut dyn FnMut(usize) -> usize) -> String {
        let n = r(40);
        match r(8) {
            0 => format!("let a{n} = a{} + {};", r(40), r(9)),
            1 => format!("a{n};"),
            2 => format!("emit(a{n}, {});", r(9)),
            3 => format!("if (a{n}) {{ let b{n} = a{n}; }}"),
            4 => format!("if (a{n}) {{ f(b{n}); }} else {{ let c{n} = 2; }}"),
            5 => format!("{{ let c{n} = {}; c{n}; }}", r(9)),
            6 => "// a note".to_string(),
            _ => format!("let a{n} = {};", r(9)),
        }
    }

    /// One line for [`features_config`]: the demo's lines, and those that
    /// reach what it adds — a name passed through two rules, a namespace
    /// given inside another, and calls whose base escapes or does not.
    fn feature_statement(r: &mut dyn FnMut(usize) -> usize) -> String {
        let n = r(40);
        match r(6) {
            0 => format!("((a{n}));"),
            1 => format!("if (a{n}) {{ if (b{n}) {{ g(a{n}); }} else {{ (c{n}); }} }} else {{ h(c{n}, {}); }}", r(9)),
            2 => format!("if (f(a{n})) {{ let a{n} = a{n}; a{n}; }}"),
            _ => statement(r),
        }
    }

    /// A file of a few small items and one block item of `lines` lines,
    /// with blocks nested in it over several lines, each line made by
    /// `statement`.
    fn program_of(
        r: &mut dyn FnMut(usize) -> usize,
        lines: usize,
        statement: fn(&mut dyn FnMut(usize) -> usize) -> String,
    ) -> String {
        let mut out = String::new();
        for _ in 0..4 {
            out.push_str(&statement(r));
            out.push('\n');
        }
        out.push_str("{\n");
        let mut depth = 1usize;
        for _ in 0..lines {
            match r(10) {
                0 if depth < 6 => {
                    out.push_str("{\n");
                    depth += 1;
                }
                1 if depth > 1 => {
                    out.push_str("}\n");
                    depth -= 1;
                }
                _ => {
                    out.push_str(&statement(r));
                    out.push('\n');
                }
            }
        }
        for _ in 0..depth {
            out.push_str("}\n");
        }
        for _ in 0..4 {
            out.push_str(&statement(r));
            out.push('\n');
        }
        out
    }

    /// What the walk the parts replaced finds in `item`.
    fn walked(item: &GreenNode, cfg: &BindingConfig) -> ItemFacts {
        let mut f = ItemFacts::new(cfg);
        let mut order = 0u32;
        walk(item, 0, 0, "", cfg, &mut f, &mut order);
        for d in &mut f.defs {
            // The walk said so itself, per def.
            debug_assert_eq!(d.top_level, d.scope == 0);
        }
        f
    }

    /// Every item of `uri` declares what the walk finds in it.
    fn hold_to_the_walk(db: &SemDb, uri: &str, what: &str) {
        let e = &db.files[uri];
        for (k, slot) in e.items.iter().enumerate() {
            let expected = walked(slot.key.node(), &db.cfg);
            assert!(expected.same_as(&slot.frag), "{what}: item {k} declares what the walk finds");
            // And what is derived from it is derived from that.
            let derived = expected.finish();
            assert_eq!(derived.local, slot.frag.local, "{what}: item {k} resolves as the walk's");
            assert_eq!(derived.top_defs, slot.frag.top_defs);
            assert_eq!(derived.top_sigs, slot.frag.top_sigs);
        }
    }

    /// The line generator for each config: what reaches its annotations.
    type Statement = fn(&mut dyn FnMut(usize) -> usize) -> String;

    fn configs(sg: &SynGrammar) -> [(&'static str, BindingConfig, Statement); 3] {
        [
            ("stress", stress_config(sg), statement),
            ("demo", demo_binding_config(sg), statement),
            ("features", features_config(sg), feature_statement),
        ]
    }

    /// Opened, read at every size from every node a part to the real
    /// one: the same fragments as the walk.
    #[test]
    fn parts_put_together_what_the_walk_finds() {
        let p = pipe();
        for (name, cfg, statement) in configs(&p.sg) {
            let mut r = rng(0x9A27_5EED);
            let text = program_of(&mut r, 400, statement);
            let s = IncSession::new(&p.lexer, &p.sg, &p.tables, &text).unwrap();
            for piece in [0, 1, 24, 200, 1000, PIECE_BYTES] {
                let mut db = SemDb::new(cfg.clone());
                db.piece_bytes = piece;
                db.set_tree("a", s.tree().unwrap().clone());
                hold_to_the_walk(&db, "a", &format!("{name} piece {piece}"));
                assert!(db.files["a"].items.iter().any(|slot| slot.part.is_some()), "the block item is read in parts");
            }
        }
    }

    /// Through an edit script — lines retyped, inserted and deleted
    /// inside the block item and around it, braces among them, so that
    /// lists regroup and parses break and recover — each revision put
    /// together from the last one's parts: the same fragments as the
    /// walk, and the same answers as a database that never had a last
    /// revision.
    #[test]
    fn parts_kept_across_edits_put_together_what_the_walk_finds() {
        let p = pipe();
        for (name, cfg, statement) in configs(&p.sg) {
            if name == "demo" {
                // Its annotations are a subset of the stress config's.
                continue;
            }
            for (seed, piece) in [
                (0x0001_C0DE, 0),
                (0x0002_C0DE, 48),
                (0x0003_C0DE, 300),
                (0x0004_C0DE, PIECE_BYTES),
            ] {
                let mut r = rng(seed);
                let text = program_of(&mut r, 600, statement);
                let mut s = IncSession::new(&p.lexer, &p.sg, &p.tables, &text).unwrap();
                let mut db = SemDb::new(cfg.clone());
                db.piece_bytes = piece;
                db.set_tree("a", s.tree().unwrap().clone());
                hold_to_the_walk(&db, "a", &format!("{name} opened"));
                for step in 0..150 {
                    let n = s.buf.lines.len();
                    let start = r(n.saturating_sub(1).max(1));
                    let line = match r(6) {
                        0 => "{".to_string(),
                        1 => "}".to_string(),
                        _ => statement(&mut r),
                    };
                    let (end, replacement) = match r(4) {
                        0 => (start, vec![Line::new(line, LineTerm::Lf)]),
                        1 => (start + 1, vec![]),
                        2 => {
                            // The same line, a digit changed for another: the
                            // facts do not move, and the fragment is kept.
                            let old = s.buf.lines[start].text.clone();
                            let swapped: String = old
                                .chars()
                                .map(|c| {
                                    if c.is_ascii_digit() {
                                        char::from(b'0' + ((c as u8 - b'0' + 1) % 10))
                                    } else {
                                        c
                                    }
                                })
                                .collect();
                            (start + 1, vec![Line::new(swapped, LineTerm::Lf)])
                        }
                        _ => (start + 1, vec![Line::new(line, LineTerm::Lf)]),
                    };
                    s.edit(&p.sg, &p.tables, &[LineEdit { start, end, replacement }]).unwrap();
                    db.set_tree("a", s.tree().unwrap().clone());
                    let what = format!("{name} seed {seed:#x} piece {piece} step {step}");
                    hold_to_the_walk(&db, "a", &what);
                    if step % 10 == 0 {
                        let mut fresh = SemDb::new(cfg.clone());
                        fresh.set_tree("a", s.tree().unwrap().clone());
                        assert_eq!(*db.symbols("a"), *fresh.symbols("a"), "{what}: symbols");
                        assert_eq!(db.resolve("a"), fresh.resolve("a"), "{what}: resolutions");
                        assert_eq!(db.unresolved("a"), fresh.unresolved("a"), "{what}: unresolved");
                    }
                }
                assert!(
                    db.stats.fragments_kept > 0,
                    "{name} seed {seed:#x}: some edit left an item's facts as they were"
                );
            }
        }
    }

    /// An item retyped without changing what it declares keeps its
    /// fragment, though a qualified name in it escapes with its base:
    /// that a name escapes is told by resolving it, and nothing of it is
    /// kept with what the tree declares.
    #[test]
    fn a_fragment_whose_qualified_name_escapes_is_kept_when_its_facts_are() {
        let p = pipe();
        let cfg = features_config(&p.sg);
        let mut s = IncSession::new(&p.lexer, &p.sg, &p.tables, "emit(a1, 2);\n").unwrap();
        let mut db = SemDb::new(cfg);
        db.set_tree("a", s.tree().unwrap().clone());
        let syms = db.symbols("a");
        let emit = syms
            .refs
            .iter()
            .position(|r| r.name == "emit" && r.kind == RefKind::Qualified)
            .unwrap();
        assert_eq!(db.resolve("a")[emit], Target::Escapes, "`emit` escapes with `a1`, its base");
        let replacement = vec![Line::new("emit(a1, 3);".to_string(), LineTerm::Lf)];
        s.edit(&p.sg, &p.tables, &[LineEdit { start: 0, end: 1, replacement }]).unwrap();
        db.set_tree("a", s.tree().unwrap().clone());
        assert_eq!(db.stats.fragments_kept, 1, "the retyped item's fragment is kept");
        assert!(db.unresolved("a").is_empty() && db.qualified_errors("a").is_empty());
    }

    /// The walk the parts replaced, verbatim, with what was added since
    /// written out its own way: the namespace a subtree stands in passed
    /// down (`ns`), and a node that defers its pattern's defs re-ordering
    /// them after the child it names (or after itself).
    fn walk(
        n: &GreenNode,
        base: u32,
        scope: ScopeId,
        ns: &str,
        cfg: &BindingConfig,
        f: &mut ItemFacts,
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
                f.scope_members.push(cfg.members.contains(&(n.nt, n.prod)));
                scope = id;
            }
        }
        // A production's declared namespace covers its def and refs; without
        // one they stand in `ns`.
        let node_ns = cfg
            .namespaces
            .iter()
            .find(|&&(nt2, p2, _)| nt2 == n.nt && p2 == n.prod)
            .map(|t| t.2.as_str())
            .unwrap_or(ns);
        let first_def = f.defs.len();
        if let Some(&(_, _, k)) = cfg.defs.iter().find(|&&(nt, p, _)| nt == n.nt && p == n.prod) {
            if let Some((name, off)) = name_child(n, k, cfg) {
                *order += 1;
                f.defs.push(Def {
                    span: (base + off, base + off + name.len() as u32),
                    name,
                    scope: outer,
                    order: *order,
                    top_level: outer == 0,
                    exported: cfg.exports.iter().any(|&(nt2, p2)| nt2 == n.nt && p2 == n.prod),
                    ns: node_ns.to_string(),
                    hoisted: cfg.hoisted.contains(&(n.nt, n.prod)),
                    open: cfg.open.contains(&(n.nt, n.prod)),
                });
            }
        }
        // A production may carry SEVERAL references (a qualified path
        // has a base @ref and a @qualify name), so collect all entries.
        for &(nt2, p2, k, kind) in cfg.refs.iter() {
            if nt2 != n.nt || p2 != n.prod {
                continue;
            }
            if let Some((name, off)) = name_child(n, k, cfg) {
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
            if let Some(bspan) = base_span(n, base, base_child, cfg) {
                // The Qualified ref for this node was pushed just above.
                if let Some(ri) = f.refs.iter().rposition(|r| {
                    r.kind == RefKind::Qualified && r.span.0 >= base && r.span.0 < base + n.width
                }) {
                    f.quals.push((bspan, ri as u32));
                }
            }
        }
        let defer = cfg
            .defers
            .iter()
            .find(|&&(nt2, p2, _, _)| nt2 == n.nt && p2 == n.prod)
            .and_then(|&(_, _, pat, after)| child_span(n, base, pat).map(|span| (span, after)));
        let mut deferred = false;
        // The pattern's defs take the order of the last fact read so far.
        let later = |f: &mut ItemFacts, span: (u32, u32), last: u32| {
            for d in &mut f.defs[first_def..] {
                if d.span.0 >= span.0 && d.span.1 <= span.1 {
                    d.order = last;
                }
            }
        };
        let mut off = base;
        let mut sym = 0usize;
        for c in &n.children {
            let is_symbol = match c {
                GreenChild::Token(t) => !t.trivia && !t.is_missing(),
                GreenChild::Node(m) => m.nt != ERROR_NT,
            };
            if let GreenChild::Node(m) = c {
                let given = is_symbol
                    .then(|| {
                        cfg.child_namespaces
                            .iter()
                            .find(|&&(nt2, p2, k, _)| nt2 == n.nt && p2 == n.prod && k == sym)
                            .map(|t| t.3.as_str())
                    })
                    .flatten();
                walk(m, off, scope, given.unwrap_or(ns), cfg, f, order);
            }
            off += c.width();
            if is_symbol {
                if let Some((span, Some(after))) = defer {
                    if after == sym {
                        later(f, span, *order);
                        deferred = true;
                    }
                }
                sym += 1;
            }
        }
        if let (Some((span, _)), false) = (defer, deferred) {
            later(f, span, *order);
        }
    }
}
