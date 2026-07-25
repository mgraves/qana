//! The declared type tier (v0): types as grammar-author DATA, checking
//! as one generic engine.
//!
//! The toolchain defines NO types. A grammar declares a type vocabulary
//! and per-production typing rules through `@type(…)` annotations, the
//! compiler lowers them to the [`TypeConfig`] value here, and this
//! module derives type assignment and type diagnostics for ANY language
//! from that data — the same declared-tier pattern as binding
//! (`@def`/`@ref`/`@scope` → [`crate::BindingConfig`] → generic
//! resolution). A grammar that declares nothing gets a type tier of
//! exactly nothing.
//!
//! v0 scope, deliberately "working ugly": atomic types plus
//! per-production signatures with local type variables (bottom-up
//! synthesis, no subtyping, no constructors); types flow through names
//! via the binding tier's resolution; checking is file-granular
//! (recomputed per query, not per-item memoized); cross-file references
//! type as unknown. Unknown NEVER cascades into an error — a diagnostic
//! is emitted only where two KNOWN types disagree.

use crate::SemDb;
use rantlr_grammar::green::ERROR_NT;
use rantlr_grammar::{GreenChild, GreenNode};
use std::collections::HashMap;

/// Index into [`TypeConfig::atoms`].
pub type TypeId = u16;

/// One term of a signature: a grammar-declared atom, or a variable
/// local to that one annotation (unified per node instance).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TyTerm {
    Atom(TypeId),
    Var(u8),
}

/// How one production types, declared in the grammar:
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeRule {
    /// `@type(Atom)` — nodes of this production have this atomic type.
    Const(TypeId),
    /// `@type(of, label)` — the type of the labeled child.
    OfChild(usize),
    /// `@type(sig, p…, R)` — the node's NONTERMINAL children must have
    /// the param types (in order); the node has the result type. Vars
    /// unify across one node instance.
    Sig { params: Vec<TyTerm>, result: TyTerm },
    /// `@type(def, label)` — the name DEFINED here (this production's
    /// `@def` child at `def_child`) carries the type of the child at
    /// `src`. The node itself stays untyped (declarations are not
    /// expressions).
    DefFrom { src: usize, def_child: usize },
    /// `@type(ref)` — the type of whatever definition this production's
    /// `@ref` child resolved to (via the binding tier).
    FromRef { ref_child: usize },
}

/// The whole declared tier: a vocabulary and per-production rules.
/// Empty config ⇒ empty tier ⇒ every query answers "nothing".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TypeConfig {
    /// The grammar's invented type vocabulary, in declaration order.
    pub atoms: Vec<String>,
    /// (nt, prod) → rule. At most one per production.
    pub rules: Vec<(u16, u16, TypeRule)>,
}

impl TypeConfig {
    pub fn intern(&mut self, name: &str) -> TypeId {
        match self.atoms.iter().position(|a| a == name) {
            Some(i) => i as TypeId,
            None => {
                self.atoms.push(name.to_string());
                (self.atoms.len() - 1) as TypeId
            }
        }
    }
    pub fn atom_name(&self, id: TypeId) -> &str {
        self.atoms.get(id as usize).map(|s| s.as_str()).unwrap_or("?")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeDiag {
    /// Absolute byte span of the offending CHILD (not the whole node).
    pub span: (u32, u32),
    pub msg: String,
}

/// One file's derived type facts.
#[derive(Clone, Debug, Default)]
pub struct TypeReport {
    /// (absolute span, type) for every node the rules typed.
    pub types: Vec<((u32, u32), TypeId)>,
    /// (absolute def-name span, type) for every definition that carries
    /// a declared type.
    pub def_types: Vec<((u32, u32), TypeId)>,
    pub diags: Vec<TypeDiag>,
    /// The vocabulary, for display (mirrors the config).
    pub atoms: Vec<String>,
}

impl TypeReport {
    /// The type at a byte offset: innermost typed node containing it.
    pub fn type_at(&self, offset: u32) -> Option<TypeId> {
        self.types
            .iter()
            .filter(|((s, e), _)| *s <= offset && offset < *e)
            .min_by_key(|((s, e), _)| e - s)
            .map(|&(_, t)| t)
    }
}

// ---------------------------------------------------------------------------
// The engine
// ---------------------------------------------------------------------------

struct Walker<'a> {
    rules: HashMap<(u16, u16), &'a TypeRule>,
    cfg: &'a TypeConfig,
    /// abs ref-name start → abs def-name start (same file only, v0).
    ref_res: &'a HashMap<u32, u32>,
    /// abs def-name start → type (from the previous pass).
    def_types: &'a HashMap<u32, TypeId>,
    out_types: Vec<((u32, u32), TypeId)>,
    out_defs: HashMap<u32, ((u32, u32), TypeId)>,
    diags: Vec<TypeDiag>,
}

impl<'a> Walker<'a> {
    /// Post-order: children first, then this node's rule. Returns the
    /// node's type. Error-carrying nodes recurse but never type — the
    /// repaired shape need not match the production's RHS.
    fn node(&mut self, n: &GreenNode, base: u32) -> Option<TypeId> {
        // Symbol children with absolute spans, in symbol order.
        let mut pos_types: Vec<Option<TypeId>> = Vec::new();
        let mut pos_spans: Vec<(u32, u32)> = Vec::new();
        let mut node_types: Vec<(Option<TypeId>, (u32, u32))> = Vec::new();
        let mut off = base;
        for c in &n.children {
            let w = c.width();
            match c {
                GreenChild::Node(m) if m.nt != ERROR_NT => {
                    let t = self.node(m, off);
                    pos_types.push(t);
                    pos_spans.push((off, off + w));
                    node_types.push((t, (off, off + w)));
                }
                GreenChild::Node(m) => {
                    // ERROR node: walk inside for salvageable children,
                    // but it is not a symbol position.
                    self.node(m, off);
                }
                GreenChild::Token(t) if !t.trivia && !t.is_missing() => {
                    pos_types.push(None);
                    pos_spans.push((off, off + w));
                }
                GreenChild::Token(_) => {}
            }
            off += w;
        }

        if n.has_err {
            return None;
        }
        let rule = self.rules.get(&(n.nt, n.prod)).copied()?;
        let span = (base, base + n.width);
        let ty = match rule {
            TypeRule::Const(a) => Some(*a),
            TypeRule::OfChild(k) => pos_types.get(*k).copied().flatten(),
            TypeRule::FromRef { ref_child } => pos_spans
                .get(*ref_child)
                .and_then(|s| self.ref_res.get(&s.0))
                .and_then(|d| self.def_types.get(d))
                .copied(),
            TypeRule::DefFrom { src, def_child } => {
                let t = pos_types.get(*src).copied().flatten();
                if let (Some(t), Some(dspan)) = (t, pos_spans.get(*def_child).copied()) {
                    self.out_defs.insert(dspan.0, (dspan, t));
                }
                None
            }
            TypeRule::Sig { params, result } => {
                // Unify params against nonterminal children.
                let mut bind: [Option<TypeId>; 26] = [None; 26];
                if params.len() == node_types.len() {
                    for (p, (ct, cspan)) in params.iter().zip(&node_types) {
                        let expected = match p {
                            TyTerm::Atom(a) => Some(*a),
                            TyTerm::Var(v) => {
                                if bind[*v as usize].is_none() {
                                    bind[*v as usize] = *ct;
                                }
                                bind[*v as usize]
                            }
                        };
                        if let (Some(e), Some(t)) = (expected, *ct) {
                            if e != t {
                                self.diags.push(TypeDiag {
                                    span: *cspan,
                                    msg: format!(
                                        "type mismatch: expected `{}`, found `{}`",
                                        self.cfg.atom_name(e),
                                        self.cfg.atom_name(t)
                                    ),
                                });
                            }
                        }
                    }
                }
                match result {
                    TyTerm::Atom(a) => Some(*a),
                    TyTerm::Var(v) => bind[*v as usize],
                }
            }
        };
        if let Some(t) = ty {
            self.out_types.push((span, t));
        }
        ty
    }
}

impl SemDb {
    /// Install the declared type tier. An absent or empty config keeps
    /// every type query at "nothing" — the tier's power is exactly what
    /// the grammar declared.
    pub fn set_types(&mut self, cfg: TypeConfig) {
        self.type_cfg = if cfg.rules.is_empty() { None } else { Some(cfg) };
    }

    /// Derive the file's type facts from the declared rules. Types flow
    /// through names via the binding tier's own resolution; def-typing
    /// iterates to a fixed point so chains of definitions (`let b = a;`)
    /// converge regardless of order. File-granular in v0 (recomputed per
    /// call); per-item memoization is the named refinement.
    pub fn types(&mut self, uri: &str) -> TypeReport {
        let Some(cfg) = self.type_cfg.clone() else { return TypeReport::default() };
        if !self.files.contains_key(uri) {
            return TypeReport::default();
        }

        // Resolution pre-pass: every ref's def site, via the binding
        // tier (memoized under the hood). Same-file only in v0.
        let ref_starts: Vec<u32> = {
            let e = &self.files[uri];
            e.items
                .iter()
                .flat_map(|s| s.frag.refs.iter().map(move |r| s.base_off + r.span.0))
                .collect()
        };
        let mut ref_res: HashMap<u32, u32> = HashMap::new();
        for start in ref_starts {
            if let Some((duri, dspan)) = self.definition(uri, start) {
                if duri == uri {
                    ref_res.insert(start, dspan.0);
                }
            }
        }

        let rules: HashMap<(u16, u16), &TypeRule> =
            cfg.rules.iter().map(|(nt, prod, r)| ((*nt, *prod), r)).collect();
        let e = &self.files[uri];
        let items: Vec<(std::sync::Arc<GreenNode>, u32)> =
            e.items.iter().map(|s| (s.node.clone(), s.base_off)).collect();

        // Iterate def-typing to a fixed point (bounded; each pass can
        // only add or update def types along resolution edges).
        let mut def_types: HashMap<u32, TypeId> = HashMap::new();
        let mut last = None;
        for _ in 0..8 {
            let (out_types, out_defs, diags) = {
                let mut w = Walker {
                    rules: rules.clone(),
                    cfg: &cfg,
                    ref_res: &ref_res,
                    def_types: &def_types,
                    out_types: Vec::new(),
                    out_defs: HashMap::new(),
                    diags: Vec::new(),
                };
                for (node, base) in &items {
                    w.node(node, *base);
                }
                (w.out_types, w.out_defs, w.diags)
            };
            let new_defs: HashMap<u32, TypeId> =
                out_defs.iter().map(|(k, (_, t))| (*k, *t)).collect();
            let stable = new_defs == def_types;
            def_types = new_defs;
            let mut defs: Vec<((u32, u32), TypeId)> = out_defs.into_values().collect();
            defs.sort_unstable_by_key(|(s, _)| s.0);
            let mut types = out_types;
            types.sort_unstable_by_key(|((s, e), _)| (*s, u32::MAX - (*e - *s)));
            last = Some(TypeReport { types, def_types: defs, diags, atoms: cfg.atoms.clone() });
            if stable {
                break;
            }
        }
        last.unwrap_or_default()
    }
}

/// Compose two declared tiers across an island boundary: host rules at
/// unchanged ids, guest rules at the product's offsets, vocabularies
/// merged (shared atom NAMES unify — a guest `Num` is the host `Num`,
/// which is the honest v0 reading of one flat vocabulary).
pub fn compose_types(
    host: &TypeConfig,
    guest: &TypeConfig,
    map_nt_offset: u16,
    map_prod_offset: u16,
) -> TypeConfig {
    let mut out = host.clone();
    let remap = |out: &mut TypeConfig, t: &TyTerm| -> TyTerm {
        match t {
            TyTerm::Atom(a) => TyTerm::Atom(out.intern(guest.atom_name(*a))),
            v => *v,
        }
    };
    for (nt, prod, rule) in &guest.rules {
        let rule = match rule {
            TypeRule::Const(a) => TypeRule::Const(out.intern(guest.atom_name(*a))),
            TypeRule::Sig { params, result } => TypeRule::Sig {
                params: params.iter().map(|p| remap(&mut out, p)).collect(),
                result: remap(&mut out, result),
            },
            r => r.clone(),
        };
        out.rules.push((nt + map_nt_offset, prod + map_prod_offset, rule));
    }
    out
}
