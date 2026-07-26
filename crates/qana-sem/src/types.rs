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

use crate::key::NodeKey;
use crate::SemDb;
use qana_grammar::green::ERROR_NT;
use qana_grammar::{GreenChild, GreenNode};
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
    /// `@type(deftype)` / `@type(deftype, body)` — this production's
    /// `@def` child INTRODUCES a document-level type named by that
    /// child's text. The vocabulary opens: grammar atoms + whatever the
    /// DOCUMENT declares. Identity is the def SITE, not the name — two
    /// `struct T`s in different scopes are different types, and which
    /// `T` an annotation denotes is decided by the binding tier's
    /// ordinary scoped resolution. With a `body` child, the defs inside
    /// it are the type's MEMBERS (fields are ordinary typed defs);
    /// without one the type is opaque.
    DefType { def_child: usize, body_child: Option<usize> },
    /// `@type(member, base, name)` — the `name` token is looked up in
    /// the member set of `base`'s type: the node has the member's type,
    /// a missing member on a member-carrying type is diagnosed
    /// ("no member `z` on `Point`"), and a base type with no members at
    /// all ("type `Num` has no members") likewise. An unknown base or a
    /// member whose own type is unknown stays silent.
    Member { base_child: usize, name_child: usize },
    /// `@type(named)` — the type DENOTED by the definition this
    /// production's `@ref` child resolves to (which must be a
    /// `deftype` site — resolving to a non-type def is diagnosed).
    /// Contrast `FromRef`: the type a def CARRIES vs the type it IS.
    Named { ref_child: usize },
    /// `@type(fn, params, rt)` — this node has an ARROW type assembled
    /// from the typed definitions inside the `params` child (in
    /// document order) and the `rt` child's type. Combine with
    /// `@type(def, …)` on the enclosing declaration and the function
    /// NAME carries the arrow — so `@type(ref)` makes functions flow
    /// like any other value. The `rt` child must precede any `returns`
    /// statements in the RHS (it supplies their expectation).
    FnArrow { params_child: usize, rt_child: usize },
    /// `@type(apply, args)` — this production's `@ref` child must
    /// resolve to a def carrying an arrow type; the `args` child's list
    /// items are checked against the arrow's parameters (arity and each
    /// argument), and the node has the arrow's return type. A callee
    /// carrying a non-arrow type is diagnosed; an untyped or unresolved
    /// callee stays silent.
    Apply { ref_child: usize, args_child: usize },
    /// `@type(returns, e)` — the `e` child is checked against the
    /// nearest enclosing `fn` node's declared return type (the tier's
    /// one downward-flowing expectation). Outside any `fn`, silent.
    Returns { expr_child: usize },
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
    /// The RUN vocabulary: the grammar's atoms followed by the types
    /// this document introduced (`deftype`), in document order.
    pub atoms: Vec<String>,
    /// How many leading entries of `atoms` are grammar-declared. The
    /// rest of the table is the GLOBAL vocabulary: document types and
    /// arrows from every file in the SemDb, interned once and stable
    /// for the session (ids are history-dependent; display via `atoms`
    /// is the canonical meaning).
    pub grammar_atoms: usize,
    /// The types THIS document introduces, in declaration order.
    pub local_doc_types: Vec<TypeId>,
    /// (absolute def-name start → introduced type) for this document —
    /// how other files' `named` annotations find a foreign type.
    pub deftypes: Vec<(u32, TypeId)>,
}

/// The SemDb-wide type vocabulary: one TypeId space for every file, so
/// a struct or function type declared in one file MEANS the same thing
/// in another. Document types intern by (uri, name, occurrence) — the
/// pragmatic global identity: within-file shadowing keeps distinct ids
/// (occurrences), and ids survive edits that keep the declaration.
#[derive(Default)]
pub(crate) struct GlobalVocab {
    pub(crate) names: Vec<String>,
    pub(crate) grammar_atoms: usize,
    doc_intern: HashMap<(String, String, u32), TypeId>,
    /// TypeId → declaring file (member-table ownership).
    doc_owner: HashMap<TypeId, String>,
    arrows: HashMap<TypeId, (Vec<TypeId>, TypeId)>,
    arrow_intern: HashMap<(Vec<TypeId>, TypeId), TypeId>,
}

impl GlobalVocab {
    fn intern_doc(&mut self, uri: &str, name: &str, occ: u32) -> TypeId {
        let key = (uri.to_string(), name.to_string(), occ);
        if let Some(&t) = self.doc_intern.get(&key) {
            return t;
        }
        self.names.push(name.to_string());
        let t = (self.names.len() - 1) as TypeId;
        self.doc_intern.insert(key, t);
        self.doc_owner.insert(t, uri.to_string());
        t
    }
    fn owner(&self, t: TypeId) -> Option<&str> {
        self.doc_owner.get(&t).map(|s| s.as_str())
    }
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
    rules: &'a HashMap<(u16, u16), &'a TypeRule>,
    /// (nt, prod) → symbol-child index the walker must NOT type: a
    /// `@macro` body is a TEMPLATE, not an expression — its typing
    /// happens per instantiation, in the materialized output (which
    /// is an ordinary, fully checked document). Rust and cpp make the
    /// same call.
    exempt: &'a HashMap<(u16, u16), usize>,
    /// The run vocabulary (grammar atoms + document types + arrows).
    /// Mutable: arrow types intern on first encounter, and the tables
    /// persist across fixpoint passes so ids stay stable.
    vocab: &'a mut Vec<String>,
    /// arrow TypeId → (parameter types, return type).
    arrows: &'a mut HashMap<TypeId, (Vec<TypeId>, TypeId)>,
    arrow_intern: &'a mut HashMap<(Vec<TypeId>, TypeId), TypeId>,
    /// abs ref-name start → abs def-name start (same file).
    ref_res: &'a HashMap<u32, u32>,
    /// abs ref-name start → the FOREIGN def's type, pre-resolved from
    /// the dependency file's converged report. Global TypeIds, so doc
    /// types and arrows flow as freely as atoms.
    foreign_ref_types: &'a HashMap<u32, TypeId>,
    /// abs ref-name start → what a FOREIGN name denotes in type
    /// position: Some(type) when the target is a deftype, None when it
    /// resolved to a non-type def (diagnosed like the local case).
    foreign_named: &'a HashMap<u32, Option<TypeId>>,
    /// abs def-name start → type (from the previous pass).
    def_types: &'a HashMap<u32, TypeId>,
    /// abs def-name start → the document-level type that def INTRODUCES.
    deftype_ids: &'a HashMap<u32, TypeId>,
    /// EVERY def site in the file (sorted) — arity source for arrows,
    /// independent of whether a def's type is known yet.
    all_defs: &'a [u32],
    /// abs def-name start → the def's name text (member table keys).
    def_names: &'a HashMap<u32, String>,
    /// (owner type, member name) → member's type from the PREVIOUS
    /// pass (None = the member exists but its type is not yet known).
    members: &'a HashMap<(TypeId, String), Option<TypeId>>,
    /// The member sets recorded THIS pass, at `deftype` body nodes.
    out_members: HashMap<(TypeId, String), Option<TypeId>>,
    /// (abs span) → computed type of every node walked THIS pass —
    /// how `apply` reads its argument items without re-walking.
    span_types: HashMap<(u32, u32), Option<TypeId>>,
    /// Declared return types of enclosing `fn` nodes, innermost last.
    ret_stack: Vec<Option<TypeId>>,
    out_types: Vec<((u32, u32), TypeId)>,
    out_defs: HashMap<u32, ((u32, u32), TypeId)>,
    diags: Vec<TypeDiag>,
}

impl<'a> Walker<'a> {
    /// Intern an arrow type into the run vocabulary (stable across
    /// passes: the tables outlive the fixpoint loop).
    fn intern_arrow(&mut self, params: Vec<TypeId>, ret: TypeId) -> TypeId {
        if let Some(&t) = self.arrow_intern.get(&(params.clone(), ret)) {
            return t;
        }
        let show = |v: &Vec<String>, t: TypeId| {
            v.get(t as usize).cloned().unwrap_or_else(|| "?".into())
        };
        let name = format!(
            "fn({}) -> {}",
            params.iter().map(|&p| show(self.vocab, p)).collect::<Vec<_>>().join(", "),
            show(self.vocab, ret)
        );
        self.vocab.push(name);
        let t = (self.vocab.len() - 1) as TypeId;
        self.arrows.insert(t, (params.clone(), ret));
        self.arrow_intern.insert((params, ret), t);
        t
    }

    /// The argument ITEMS of an `apply` args child: descend through
    /// list/run structure and transparent (untyped, rule-less) wrappers;
    /// a node with a type rule is one item. Types come from the pass's
    /// span map — items were walked before their call node.
    fn item_types(&self, n: &GreenNode, base: u32, out: &mut Vec<((u32, u32), Option<TypeId>)>) {
        use qana_grammar::green::{LIST_PROD, RUN_PROD};
        let span = (base, base + n.width);
        if n.prod != LIST_PROD && n.prod != RUN_PROD && self.rules.contains_key(&(n.nt, n.prod)) {
            out.push((span, self.span_types.get(&span).copied().flatten()));
            return;
        }
        let mut off = base;
        for c in &n.children {
            let w = c.width();
            if let GreenChild::Node(m) = c {
                if m.nt != ERROR_NT {
                    self.item_types(m, off, out);
                }
            }
            off += w;
        }
    }

    /// Post-order: children first, then this node's rule. Returns the
    /// node's type. Error-carrying nodes recurse but never type — the
    /// repaired shape need not match the production's RHS.
    fn node(&mut self, n: &GreenNode, base: u32) -> Option<TypeId> {
        let ty = self.node_inner(n, base);
        // Transparent single-child wrappers share their child's span
        // (post-order: the child inserted first). An outer UNTYPED
        // wrapper must not clobber the inner node's type — `apply`
        // reads argument items through this map.
        let slot = self.span_types.entry((base, base + n.width)).or_insert(None);
        if ty.is_some() {
            *slot = ty;
        }
        ty
    }

    fn node_inner<'n>(&mut self, n: &'n GreenNode, base: u32) -> Option<TypeId> {
        // This node's rule, known up front: `fn` nodes push their
        // declared return type once the rt child has been walked, so
        // `returns` statements inside the body can read it.
        let rule_now = self.rules.get(&(n.nt, n.prod)).copied();
        let fn_rt_child = match (n.has_err, rule_now) {
            (false, Some(TypeRule::FnArrow { rt_child, .. })) => Some(*rt_child),
            _ => None,
        };
        let mut pushed_ret = false;

        // Symbol children with absolute spans, in symbol order.
        let mut pos_types: Vec<Option<TypeId>> = Vec::new();
        let mut pos_spans: Vec<(u32, u32)> = Vec::new();
        let mut node_types: Vec<(Option<TypeId>, (u32, u32))> = Vec::new();
        let mut args_node: Option<(&'n GreenNode, u32)> = None;
        let args_child = match rule_now {
            Some(TypeRule::Apply { args_child, .. }) => Some(*args_child),
            _ => None,
        };
        // `member` needs the NAME token's text (lookup key + diagnostic).
        let mut member_name: Option<&'n str> = None;
        let name_child = match rule_now {
            Some(TypeRule::Member { name_child, .. }) => Some(*name_child),
            _ => None,
        };
        let exempt_child = self.exempt.get(&(n.nt, n.prod)).copied();
        let mut off = base;
        for c in &n.children {
            let w = c.width();
            match c {
                GreenChild::Node(m) if m.nt != ERROR_NT => {
                    // A template child stays UNWALKED: positional
                    // alignment is kept, but no types, no diagnostics.
                    let t = if exempt_child == Some(pos_types.len()) {
                        None
                    } else {
                        self.node(m, off)
                    };
                    if fn_rt_child == Some(pos_types.len()) {
                        self.ret_stack.push(t);
                        pushed_ret = true;
                    }
                    if args_child == Some(pos_types.len()) {
                        args_node = Some((&**m, off));
                    }
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
                    if name_child == Some(pos_types.len()) {
                        member_name = Some(&t.text);
                    }
                    pos_types.push(None);
                    pos_spans.push((off, off + w));
                }
                GreenChild::Token(_) => {}
            }
            off += w;
        }
        let popped_ret = if pushed_ret { self.ret_stack.pop() } else { None };
        let _ = popped_ret;

        if n.has_err {
            return None;
        }
        let rule = self.rules.get(&(n.nt, n.prod)).copied()?;
        let span = (base, base + n.width);
        let ty = match rule {
            TypeRule::Const(a) => Some(*a),
            TypeRule::OfChild(k) => pos_types.get(*k).copied().flatten(),
            TypeRule::FromRef { ref_child } => {
                let start = pos_spans.get(*ref_child).map(|s| s.0);
                start
                    .and_then(|s| self.ref_res.get(&s))
                    .and_then(|d| self.def_types.get(d))
                    .copied()
                    .or_else(|| start.and_then(|s| self.foreign_ref_types.get(&s)).copied())
            }
            // The introduction itself was recorded in the pre-pass; the
            // declaring node is not an expression and stays untyped.
            // With a body child, the def sites inside it become the
            // type's MEMBER SET — name from the binding tier, type from
            // this pass's def records (None = exists, not yet known).
            TypeRule::DefType { def_child, body_child } => {
                if let (Some(&tid), Some(k)) = (
                    pos_spans.get(*def_child).and_then(|s| self.deftype_ids.get(&s.0)),
                    *body_child,
                ) {
                    if let Some((lo, hi)) = pos_spans.get(k).copied() {
                        let sites = &self.all_defs[self.all_defs.partition_point(|&d| d < lo)
                            ..self.all_defs.partition_point(|&d| d < hi)];
                        for site in sites {
                            if let Some(name) = self.def_names.get(site) {
                                let mt = self.out_defs.get(site).map(|&(_, t)| t);
                                self.out_members.insert((tid, name.clone()), mt);
                            }
                        }
                    }
                }
                None
            }
            TypeRule::Member { base_child, name_child: _ } => {
                let base_ty = pos_types.get(*base_child).copied().flatten();
                match (base_ty, member_name) {
                    (Some(t), Some(name)) => {
                        match self.members.get(&(t, name.to_string())) {
                            Some(Some(mt)) => Some(*mt),
                            Some(None) => None, // member exists, type unknown
                            None => {
                                // Does this type carry ANY member set?
                                let owner = self
                                    .vocab
                                    .get(t as usize)
                                    .map(|s| s.as_str())
                                    .unwrap_or("?");
                                let has_members =
                                    self.members.keys().any(|(o, _)| *o == t);
                                let nspan = name_child
                                    .and_then(|k| pos_spans.get(k).copied())
                                    .unwrap_or(span);
                                self.diags.push(TypeDiag {
                                    span: nspan,
                                    msg: if has_members {
                                        format!("no member `{name}` on `{owner}`")
                                    } else {
                                        format!("type `{owner}` has no members")
                                    },
                                });
                                None
                            }
                        }
                    }
                    _ => None, // unknown base or repaired name: silent
                }
            }
            TypeRule::Named { ref_child } => {
                let start = pos_spans.get(*ref_child).map(|s| s.0);
                match start.and_then(|s| self.ref_res.get(&s)) {
                    Some(d) => match self.deftype_ids.get(d) {
                        Some(&t) => Some(t),
                        None => {
                            self.diags.push(TypeDiag {
                                span: pos_spans[*ref_child],
                                msg: "this name does not denote a type".into(),
                            });
                            None
                        }
                    },
                    // Foreign resolution: the dependency's report says
                    // whether the target introduces a type.
                    None => match start.and_then(|s| self.foreign_named.get(&s)) {
                        Some(Some(t)) => Some(*t),
                        Some(None) => {
                            self.diags.push(TypeDiag {
                                span: pos_spans[*ref_child],
                                msg: "this name does not denote a type".into(),
                            });
                            None
                        }
                        None => None, // unresolved: the binding tier reports it
                    },
                }
            }
            TypeRule::FnArrow { params_child, rt_child } => {
                // Parameter types: the typed defs recorded (this pass)
                // inside the params child's span, in document order.
                // Arity comes from ALL def sites in that span, so a
                // still-unknown param keeps the whole arrow unknown
                // rather than silently shortening it.
                let pspan = pos_spans.get(*params_child).copied();
                let rt = pos_types.get(*rt_child).copied().flatten();
                match (pspan, rt) {
                    (Some((lo, hi)), Some(rt)) => {
                        let sites =
                            &self.all_defs[self.all_defs.partition_point(|&d| d < lo)
                                ..self.all_defs.partition_point(|&d| d < hi)];
                        let params: Vec<TypeId> = sites
                            .iter()
                            .filter_map(|s| self.out_defs.get(s).map(|&(_, t)| t))
                            .collect();
                        if params.len() == sites.len() {
                            Some(self.intern_arrow(params, rt))
                        } else {
                            None // some param's type is not yet known
                        }
                    }
                    _ => None,
                }
            }
            TypeRule::Apply { ref_child, args_child: _ } => {
                let start = pos_spans.get(*ref_child).map(|s| s.0);
                let callee = start
                    .and_then(|s| self.ref_res.get(&s))
                    .and_then(|d| self.def_types.get(d))
                    .copied()
                    .or_else(|| start.and_then(|s| self.foreign_ref_types.get(&s)).copied());
                match callee {
                    None => None, // untyped or unresolved callee: silent
                    Some(t) => match self.arrows.get(&t).cloned() {
                        None => {
                            self.diags.push(TypeDiag {
                                span: pos_spans[*ref_child],
                                msg: format!(
                                    "not callable: this name has type `{}`",
                                    self.vocab.get(t as usize).map(|s| s.as_str()).unwrap_or("?")
                                ),
                            });
                            None
                        }
                        Some((params, ret)) => {
                            let mut items: Vec<((u32, u32), Option<TypeId>)> = Vec::new();
                            if let Some((an, abase)) = args_node {
                                self.item_types(an, abase, &mut items);
                            }
                            if items.len() != params.len() {
                                self.diags.push(TypeDiag {
                                    span,
                                    msg: format!(
                                        "expected {} argument(s), found {}",
                                        params.len(),
                                        items.len()
                                    ),
                                });
                            } else {
                                for (&p, (ispan, it)) in params.iter().zip(&items) {
                                    if let Some(it) = it {
                                        if *it != p {
                                            self.diags.push(TypeDiag {
                                                span: *ispan,
                                                msg: format!(
                                                    "type mismatch: expected `{}`, found `{}`",
                                                    self.vocab.get(p as usize).map(|s| s.as_str()).unwrap_or("?"),
                                                    self.vocab.get(*it as usize).map(|s| s.as_str()).unwrap_or("?"),
                                                ),
                                            });
                                        }
                                    }
                                }
                            }
                            Some(ret)
                        }
                    },
                }
            }
            TypeRule::Returns { expr_child } => {
                let expected = self.ret_stack.last().copied().flatten();
                let actual = pos_types.get(*expr_child).copied().flatten();
                if let (Some(e), Some(a)) = (expected, actual) {
                    if e != a {
                        self.diags.push(TypeDiag {
                            span: pos_spans[*expr_child],
                            msg: format!(
                                "return type mismatch: expected `{}`, found `{}`",
                                self.vocab.get(e as usize).map(|s| s.as_str()).unwrap_or("?"),
                                self.vocab.get(a as usize).map(|s| s.as_str()).unwrap_or("?"),
                            ),
                        });
                    }
                }
                None
            }
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
                                        self.vocab.get(e as usize).map(|s| s.as_str()).unwrap_or("?"),
                                        self.vocab.get(t as usize).map(|s| s.as_str()).unwrap_or("?"),
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

/// Pre-pass: every `deftype` introduction site in document order —
/// (absolute def-name start, name text). Static per tree (independent
/// of any computed types), so it runs once, before the fixpoint.
fn collect_deftypes(
    n: &GreenNode,
    base: u32,
    rules: &HashMap<(u16, u16), &TypeRule>,
    out: &mut Vec<(u32, String)>,
) {
    let mut pos = 0usize;
    let mut off = base;
    let target = match rules.get(&(n.nt, n.prod)) {
        Some(TypeRule::DefType { def_child, .. }) if !n.has_err => Some(*def_child),
        _ => None,
    };
    for c in &n.children {
        let w = c.width();
        match c {
            GreenChild::Node(m) if m.nt != ERROR_NT => {
                collect_deftypes(m, off, rules, out);
                pos += 1;
            }
            GreenChild::Node(m) => collect_deftypes(m, off, rules, out),
            GreenChild::Token(t) if !t.trivia && !t.is_missing() => {
                if target == Some(pos) {
                    out.push((off, t.text.clone()));
                }
                pos += 1;
            }
            GreenChild::Token(_) => {}
        }
        off += w;
    }
}

impl SemDb {
    /// Install the declared type tier. An absent or empty config keeps
    /// every type query at "nothing" — the tier's power is exactly what
    /// the grammar declared. The grammar's atoms seed the GLOBAL
    /// vocabulary; installing a different config resets it (and every
    /// derived cache) — TypeIds are only meaningful within one config.
    pub fn set_types(&mut self, cfg: TypeConfig) {
        let atoms_changed = self.gvocab.grammar_atoms != cfg.atoms.len()
            || self.gvocab.names[..self.gvocab.grammar_atoms] != cfg.atoms[..];
        if atoms_changed {
            self.gvocab = GlobalVocab {
                names: cfg.atoms.clone(),
                grammar_atoms: cfg.atoms.len(),
                ..GlobalVocab::default()
            };
            self.type_caches.clear();
            self.global_members.clear();
        }
        self.type_cfg = if cfg.rules.is_empty() { None } else { Some(cfg) };
    }

    /// Tell the type tier where MACRO BODIES live, so it exempts them:
    /// a template is not an expression — its typing happens per
    /// instantiation, in the materialized output.
    pub fn set_macro_bodies(&mut self, macros: &crate::macros::MacroConfig) {
        let map: HashMap<(u16, u16), usize> =
            macros.defs.iter().map(|&(nt, p, _, body)| ((nt, p), body)).collect();
        if map != self.macro_body_exempt {
            self.macro_body_exempt = map;
            self.type_caches.clear();
        }
    }

    /// Derive the file's type facts from the declared rules. Types flow
    /// through names via the binding tier's own resolution; def-typing
    /// and member tables iterate to a fixed point so chains converge
    /// regardless of order.
    ///
    /// MEMOIZED per item: outputs are cached (relative spans) keyed by
    /// subtree identity, and an edit whose item keeps the same def-type
    /// sequence and member contribution — a BODY edit — replays every
    /// other item and re-walks only the changed one, in one pass. An
    /// edit that changes a def's type — a SIGNATURE edit — ripples: the
    /// whole file re-derives (the P6 firewall philosophy, applied to
    /// types). `SemStats::{type_item_walks, type_passes}` are the proof.
    ///
    /// CROSS-FILE: a reference resolving to another file (the binding
    /// tier's foreign resolution) is typed from that file's own
    /// converged report — grammar-atom types only, since atom ids are
    /// file-independent; foreign document types and arrows stay unknown
    /// until a global vocabulary exists. Dependency cycles resolve with
    /// what is known and never hang.
    pub fn types(&mut self, uri: &str) -> TypeReport {
        let Some(cfg) = self.type_cfg.clone() else { return TypeReport::default() };
        if !self.files.contains_key(uri) || self.type_visiting.contains(uri) {
            return TypeReport::default();
        }
        self.type_visiting.insert(uri.to_string());
        let report = self.types_inner(uri, &cfg);
        self.type_visiting.remove(uri);
        report
    }

    fn types_inner(&mut self, uri: &str, cfg: &TypeConfig) -> TypeReport {
        // ---- gather: refs, defs, items (cheap clones of metadata) ----
        let (ref_meta, item_meta, all_defs, def_names) = {
            let e = &self.files[uri];
            let ref_meta: Vec<u32> = e
                .items
                .iter()
                .flat_map(|s| s.frag.refs.iter().map(move |r| s.base_off + r.span.0))
                .collect();
            let item_meta: Vec<(NodeKey, u32)> =
                e.items.iter().map(|s| (s.key.clone(), s.base_off)).collect();
            let mut v: Vec<u32> = Vec::new();
            let mut names: HashMap<u32, String> = HashMap::new();
            for s in &e.items {
                for d in &s.frag.defs {
                    let start = s.base_off + d.span.0;
                    v.push(start);
                    names.insert(start, d.name.clone());
                }
            }
            v.sort_unstable();
            (ref_meta, item_meta, v, names)
        };

        // ---- resolution: local map + foreign targets ----
        let mut ref_res: HashMap<u32, u32> = HashMap::new();
        let mut foreign_targets: Vec<(u32, String, u32)> = Vec::new();
        for start in ref_meta {
            if let Some((duri, dspan)) = self.definition(uri, start) {
                if duri == uri {
                    ref_res.insert(start, dspan.0);
                } else {
                    foreign_targets.push((start, duri, dspan.0));
                }
            }
        }
        // Foreign types come from each dependency's OWN report (its
        // memoization makes this cheap when the dependency is
        // unchanged). Global ids: values, document types, and function
        // types all flow.
        let mut foreign_ref_types: HashMap<u32, TypeId> = HashMap::new();
        let mut foreign_named: HashMap<u32, Option<TypeId>> = HashMap::new();
        {
            let mut dep_uris: Vec<String> =
                foreign_targets.iter().map(|(_, u, _)| u.clone()).collect();
            dep_uris.sort();
            dep_uris.dedup();
            let mut dep_reports: HashMap<String, TypeReport> = HashMap::new();
            for dep in dep_uris {
                let r = self.types(&dep);
                dep_reports.insert(dep, r);
            }
            for (ref_start, dep, dstart) in &foreign_targets {
                if let Some(r) = dep_reports.get(dep) {
                    if let Some(&(_, t)) =
                        r.def_types.iter().find(|((s, _), _)| s == dstart)
                    {
                        foreign_ref_types.insert(*ref_start, t);
                    }
                    foreign_named.insert(
                        *ref_start,
                        r.deftypes.iter().find(|(s, _)| s == dstart).map(|&(_, t)| t),
                    );
                }
            }
        }
        // The foreign member environment: every file's member tables
        // except this one's own contributions.
        let foreign_members: HashMap<(TypeId, String), Option<TypeId>> = self
            .global_members
            .iter()
            .filter(|((t, _), _)| self.gvocab.owner(*t) != Some(uri))
            .map(|(k, v)| (k.clone(), *v))
            .collect();
        // Position-independent snapshot of ALL foreign inputs — the
        // cache-validity comparison (a dependency changing any value a
        // replay depends on must ripple here). Members close
        // transitively: a chain like `p.i.y` depends on Inner's table
        // even though only P appears in the ref values.
        let foreign_snapshot: FSnap = {
            let rel_of = |s: u32| {
                item_meta
                    .iter()
                    .rev()
                    .find(|(_, b)| *b <= s)
                    .map(|(_, b)| s - b)
                    .unwrap_or(s)
            };
            let mut refs: Vec<(u32, TypeId)> =
                foreign_ref_types.iter().map(|(&s, &t)| (rel_of(s), t)).collect();
            refs.sort_unstable();
            let mut named: Vec<(u32, Option<TypeId>)> =
                foreign_named.iter().map(|(&s, &t)| (rel_of(s), t)).collect();
            named.sort_unstable();
            let mut tids: std::collections::HashSet<TypeId> = foreign_ref_types
                .values()
                .copied()
                .chain(foreign_named.values().filter_map(|t| *t))
                .collect();
            loop {
                let more: Vec<TypeId> = foreign_members
                    .iter()
                    .filter(|((t, _), _)| tids.contains(t))
                    .filter_map(|(_, v)| *v)
                    .filter(|t| !tids.contains(t))
                    .collect();
                if more.is_empty() {
                    break;
                }
                tids.extend(more);
            }
            let mut members: Vec<(TypeId, String, Option<TypeId>)> = foreign_members
                .iter()
                .filter(|((t, _), _)| tids.contains(t))
                .map(|((t, n), v)| (*t, n.clone(), *v))
                .collect();
            members.sort();
            FSnap { refs, named, members }
        };

        let rules: HashMap<(u16, u16), &TypeRule> =
            cfg.rules.iter().map(|(nt, prod, r)| ((*nt, *prod), r)).collect();

        // ---- document types, interned into the GLOBAL vocabulary ----
        let mut cache = self.type_caches.remove(uri).unwrap_or_default();
        let mut gv = std::mem::take(&mut self.gvocab);
        let mut gen_current: Vec<(usize, u32, String)> = Vec::new();
        for (idx, (key, base)) in item_meta.iter().enumerate() {
            if let Some(it) = cache.per_item.get(key) {
                for (rel, name) in &it.rel_deftypes {
                    gen_current.push((idx, *rel, name.clone()));
                }
            } else {
                let mut sites: Vec<(u32, String)> = Vec::new();
                collect_deftypes(key.node(), *base, &rules, &mut sites);
                for (abs, name) in sites {
                    gen_current.push((idx, abs - base, name));
                }
            }
        }
        // Cached outputs stay valid only while the file's type
        // introductions are unchanged (same names, same order — the
        // interned ids are then identical). The global vocabulary
        // itself always persists.
        let vocab_reusable = cache.primed && cache.deftype_gen == gen_current;
        if !vocab_reusable {
            cache.per_item.clear();
        }
        let warm_members = cache.member_types.clone();
        let mut deftype_ids: HashMap<u32, TypeId> = HashMap::new();
        let mut local_doc_types: Vec<TypeId> = Vec::new();
        {
            let mut occ: HashMap<&str, u32> = HashMap::new();
            for (idx, rel, name) in &gen_current {
                let n = occ.entry(name.as_str()).or_insert(0);
                let t = gv.intern_doc(uri, name, *n);
                *n += 1;
                let base = item_meta[*idx].1;
                deftype_ids.insert(base + rel, t);
                local_doc_types.push(t);
            }
        }
        let GlobalVocab {
            names: mut vocab,
            grammar_atoms,
            doc_intern,
            doc_owner,
            arrows: mut g_arrows,
            arrow_intern: mut g_arrow_intern,
        } = gv;

        // ---- warm start: cached converged values at CURRENT offsets.
        // The replaced item (same index, dead ptr) seeds its old values
        // too, so self-recursive items resolve in the first pass. ----
        let mut warm_defs: HashMap<u32, TypeId> = HashMap::new();
        for (idx, (key, base)) in item_meta.iter().enumerate() {
            let it = cache.per_item.get(key).or_else(|| {
                cache
                    .items_order
                    .get(idx)
                    .and_then(|old| cache.per_item.get(old))
            });
            if let Some(it) = it {
                for (rel, _, t) in &it.rel_defs {
                    warm_defs.insert(base + rel, *t);
                }
            }
        }

        // One walk of one item, capturing its outputs relative to base.
        let exempt = self.macro_body_exempt.clone();
        let walk_item = |vocab: &mut Vec<String>,
                         arrows: &mut HashMap<TypeId, (Vec<TypeId>, TypeId)>,
                         arrow_intern: &mut HashMap<(Vec<TypeId>, TypeId), TypeId>,
                         def_types: &HashMap<u32, TypeId>,
                         members: &HashMap<(TypeId, String), Option<TypeId>>,
                         node: &GreenNode,
                         base: u32|
         -> ItemOut {
            let mut w = Walker {
                rules: &rules,
                exempt: &exempt,
                vocab,
                arrows,
                arrow_intern,
                ref_res: &ref_res,
                foreign_ref_types: &foreign_ref_types,
                foreign_named: &foreign_named,
                def_types,
                deftype_ids: &deftype_ids,
                all_defs: &all_defs,
                def_names: &def_names,
                members,
                out_members: HashMap::new(),
                span_types: HashMap::new(),
                ret_stack: Vec::new(),
                out_types: Vec::new(),
                out_defs: HashMap::new(),
                diags: Vec::new(),
            };
            w.node(node, base);
            let mut rel_defs: Vec<(u32, (u32, u32), TypeId)> = w
                .out_defs
                .into_iter()
                .map(|(s, ((a, b), t))| (s - base, (a - base, b - base), t))
                .collect();
            rel_defs.sort_unstable_by_key(|(s, _, _)| *s);
            let mut members_out: Vec<((TypeId, String), Option<TypeId>)> =
                w.out_members.into_iter().collect();
            members_out.sort();
            let mut rel_deftypes: Vec<(u32, String)> = Vec::new();
            let mut sites: Vec<(u32, String)> = Vec::new();
            collect_deftypes(node, base, &rules, &mut sites);
            for (abs, name) in sites {
                rel_deftypes.push((abs - base, name));
            }
            ItemOut {
                rel_types: w
                    .out_types
                    .into_iter()
                    .map(|((a, b), t)| ((a - base, b - base), t))
                    .collect(),
                rel_defs,
                members: members_out,
                rel_diags: w
                    .diags
                    .into_iter()
                    .map(|d| ((d.span.0 - base, d.span.1 - base), d.msg))
                    .collect(),
                rel_deftypes,
            }
        };

        // Members visible to a walk: every other file's tables plus
        // this file's evolving local table.
        let merge_members = |local: &HashMap<(TypeId, String), Option<TypeId>>| {
            let mut m = foreign_members.clone();
            for (k, v) in local {
                m.insert(k.clone(), *v);
            }
            m
        };

        // ---- fast path: walk only FRESH items against the warm env;
        // if their def sequences and member contributions match what
        // the cache converged to, every cached item replays. ----
        let mut outs: Vec<Option<ItemOut>> = vec![None; item_meta.len()];
        let mut fast_ok = vocab_reusable || cache.per_item.is_empty();
        let mut fresh_walked = 0u64;
        if vocab_reusable {
            let warm_merged = merge_members(&warm_members);
            for (idx, (key, base)) in item_meta.iter().enumerate() {
                if cache.per_item.contains_key(key) {
                    continue;
                }
                let out = walk_item(
                    &mut vocab,
                    &mut g_arrows,
                    &mut g_arrow_intern,
                    &warm_defs,
                    &warm_merged,
                    key.node(),
                    *base,
                );
                fresh_walked += 1;
                // Same def-type sequence + member contribution as the
                // item this one replaced (same index)?
                let old = cache
                    .items_order
                    .get(idx)
                    .and_then(|old| cache.per_item.get(old));
                let seq_ok = match old {
                    Some(o) => {
                        o.rel_defs.iter().map(|(_, _, t)| *t).collect::<Vec<_>>()
                            == out.rel_defs.iter().map(|(_, _, t)| *t).collect::<Vec<_>>()
                            && o.members == out.members
                            && o.rel_deftypes == out.rel_deftypes
                    }
                    None => false,
                };
                if !seq_ok {
                    fast_ok = false;
                }
                outs[idx] = Some(out);
            }
            fast_ok = fast_ok
                && item_meta.len() == cache.items_order.len()
                && foreign_snapshot == cache.foreign_snapshot;
        } else {
            fast_ok = false;
        }

        self.stats.type_item_walks += fresh_walked;

        let (final_outs, member_final) = if fast_ok && !cache.per_item.is_empty() {
            // Replay every cached item; fresh outputs slot in.
            self.stats.type_passes += 1;
            for (idx, (key, _)) in item_meta.iter().enumerate() {
                if outs[idx].is_none() {
                    outs[idx] = cache.per_item.get(key).cloned();
                }
            }
            (outs, warm_members)
        } else {
            // Ripple (or cold): full fixpoint from the warm seed. The
            // global vocabulary persists — ids are stable for the
            // session, and equivalence with a fresh computation is
            // display-canonical, not id-numeric.
            cache.per_item.clear();
            let mut def_types = warm_defs;
            let mut member_types = warm_members;
            let mut result: Vec<Option<ItemOut>> = vec![None; item_meta.len()];
            for _ in 0..8 {
                self.stats.type_passes += 1;
                let merged = merge_members(&member_types);
                let mut pass_outs: Vec<Option<ItemOut>> = Vec::with_capacity(item_meta.len());
                for (key, base) in &item_meta {
                    pass_outs.push(Some(walk_item(
                        &mut vocab,
                        &mut g_arrows,
                        &mut g_arrow_intern,
                        &def_types,
                        &merged,
                        key.node(),
                        *base,
                    )));
                    self.stats.type_item_walks += 1;
                }
                let mut new_defs: HashMap<u32, TypeId> = HashMap::new();
                let mut new_members: HashMap<(TypeId, String), Option<TypeId>> = HashMap::new();
                for (out, (_, base)) in pass_outs.iter().zip(&item_meta) {
                    let out = out.as_ref().unwrap();
                    for (rel, _, t) in &out.rel_defs {
                        new_defs.insert(base + rel, *t);
                    }
                    for (k, v) in &out.members {
                        new_members.insert(k.clone(), *v);
                    }
                }
                let stable = new_defs == def_types && new_members == member_types;
                def_types = new_defs;
                member_types = new_members;
                result = pass_outs;
                if stable {
                    break;
                }
            }
            (result, member_types)
        };

        // ---- store: cache, global vocabulary, global members ----
        cache.primed = true;
        cache.deftype_gen = gen_current;
        cache.member_types = member_final.clone();
        cache.foreign_snapshot = foreign_snapshot;
        let prev_order = std::mem::replace(
            &mut cache.items_order,
            item_meta.iter().map(|(k, _)| k.clone()).collect(),
        );
        self.gvocab = GlobalVocab {
            names: vocab,
            grammar_atoms,
            doc_intern,
            doc_owner,
            arrows: g_arrows,
            arrow_intern: g_arrow_intern,
        };
        // This file's member contributions replace its previous ones.
        self.global_members
            .retain(|(t, _), _| self.gvocab.owner(*t) != Some(uri));
        for (k, v) in member_final {
            self.global_members.insert(k, v);
        }

        let mut report = TypeReport {
            types: Vec::new(),
            def_types: Vec::new(),
            diags: Vec::new(),
            atoms: self.gvocab.names.clone(),
            grammar_atoms: self.gvocab.grammar_atoms,
            local_doc_types,
            deftypes: {
                let mut v: Vec<(u32, TypeId)> =
                    deftype_ids.iter().map(|(&s, &t)| (s, t)).collect();
                v.sort_unstable();
                v
            },
        };
        for (out, (key, base)) in final_outs.into_iter().zip(&item_meta) {
            let Some(out) = out else { continue };
            for &((a, b), t) in &out.rel_types {
                report.types.push(((base + a, base + b), t));
            }
            for &(_, (a, b), t) in &out.rel_defs {
                report.def_types.push(((base + a, base + b), t));
            }
            for ((a, b), msg) in &out.rel_diags {
                report
                    .diags
                    .push(TypeDiag { span: (base + a, base + b), msg: msg.clone() });
            }
            cache.per_item.insert(key.clone(), out);
        }
        // Keep exactly two generations: the current items, and the
        // ones they replaced (a replaced item finds its predecessor by
        // index, which is how a BODY edit is told from a SIGNATURE
        // edit). Anything older would pin subtrees forever.
        let live: std::collections::HashSet<NodeKey> = cache
            .items_order
            .iter()
            .cloned()
            .chain(prev_order.iter().cloned())
            .collect();
        cache.per_item.retain(|k, _| live.contains(k));
        report.types.sort_unstable_by_key(|((s, e), _)| (*s, u32::MAX - (*e - *s)));
        report.def_types.sort_unstable_by_key(|(s, _)| s.0);
        report.diags.sort_unstable_by_key(|d| d.span);
        self.type_caches.insert(uri.to_string(), cache);
        report
    }
}

/// Foreign inputs a file's cached outputs depend on — item-relative and
/// value-based, so a dependency changing anything a replay used forces
/// the honest ripple.
#[derive(Default, PartialEq)]
pub(crate) struct FSnap {
    refs: Vec<(u32, TypeId)>,
    named: Vec<(u32, Option<TypeId>)>,
    members: Vec<(TypeId, String, Option<TypeId>)>,
}

/// Per-file memoization of the type pass: the converged environment and
/// every item's outputs, spans relative to the item so position shifts
/// never invalidate.
#[derive(Default)]
pub(crate) struct TypeCache {
    /// Set once the cache holds a converged result.
    primed: bool,
    /// (item index, rel site, name) of every deftype — cached outputs
    /// are valid only while this is unchanged (the interned ids are
    /// then identical); the global vocabulary itself always persists.
    deftype_gen: Vec<(usize, u32, String)>,
    member_types: HashMap<(TypeId, String), Option<TypeId>>,
    foreign_snapshot: FSnap,
    /// Item subtree pointers at cache time, in order — how a replaced
    /// item finds its predecessor for the body/signature comparison.
    items_order: Vec<NodeKey>,
    /// Item identity → outputs. The key OWNS its subtree (see
    /// [`crate::key`]): that is what makes identity comparison sound
    /// across edits, and it is enforced by the type rather than by
    /// remembering to store a handle alongside.
    per_item: HashMap<NodeKey, ItemOut>,
}

#[derive(Clone, PartialEq)]
pub(crate) struct ItemOut {
    rel_types: Vec<((u32, u32), TypeId)>,
    /// (rel def-name start, rel def-name span, type), sorted by start.
    rel_defs: Vec<(u32, (u32, u32), TypeId)>,
    members: Vec<((TypeId, String), Option<TypeId>)>,
    rel_diags: Vec<((u32, u32), String)>,
    rel_deftypes: Vec<(u32, String)>,
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
