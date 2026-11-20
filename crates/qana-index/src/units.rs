//! The walk that reads a file's units, imports, visibilities, attributes
//! and impl blocks from its tree, as its grammar's binding says (`@unit`,
//! `@root`, `@vis`, `@visibility`, `@import`, `@import_glob`,
//! `@import_prefix`, `@loads`, `@attribute`, `@attributes`, `@impl`), and
//! the unit and path of each definition and reference.
//!
//! What a function's body holds is the body's: its units, imports and
//! impl blocks are not read (as its declarations are not offered), while
//! its references are, in the unit the function is in.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use qana_grammar::green::{LIST_PROD, RUN_PROD};
use qana_grammar::{GreenChild, GreenNode};
use qana_sem::{name_child, symbol_indices, BindingConfig, Path, RefKind, Root};
use qana_services::OutlineConfig;

use crate::{Attribute, Impl, Import, Unit, Visibility, FUNCTION};

/// What the binding says of one production, gathered from the config's
/// tables once, so the walk asks each node once.
#[derive(Default)]
struct Roles {
    /// It opens a scope; `true` when the scope holds members.
    scope: Option<bool>,
    /// The symbol child its `@def` names.
    def: Option<usize>,
    /// Its references: the symbol child each names, and its kind.
    refs: Vec<(usize, RefKind)>,
    /// `@qualify(base, name)`: the base child and the name child.
    qualify: Option<(usize, usize)>,
    /// `@unit(name[, body])`: the body child.
    unit: Option<Option<usize>>,
    /// `@vis`: within, and the path child of `in`.
    vis: Option<(Option<Root>, Option<usize>)>,
    /// `@visibility(v, item)`.
    visibility: Option<(usize, usize)>,
    /// `@import`/`@import_glob`: the path child, and whether a glob.
    import: Option<(Option<usize>, bool)>,
    /// `@loads`: the import loads the library its path names.
    loads: bool,
    /// `@attribute(path)`: the path child.
    attribute: Option<usize>,
    /// `@attributes(attrs[, item])`: the attributes child, and the item
    /// child.
    attribute_list: Option<(usize, Option<usize>)>,
    /// `@import_prefix`: the path child, and the trees child.
    prefix: Option<(Option<usize>, usize)>,
    /// `@impl(type[, trait])`.
    implementation: Option<(usize, Option<usize>)>,
    /// A construct its outline calls a function: what it holds is a body's.
    function: bool,
}

fn roles(cfg: &BindingConfig, outline: &OutlineConfig) -> HashMap<(u16, u16), Roles> {
    let mut map: HashMap<(u16, u16), Roles> = HashMap::new();
    for &(nt, prod, _, _) in &cfg.scopes {
        map.entry((nt, prod)).or_default().scope = Some(cfg.members.contains(&(nt, prod)));
    }
    for &(nt, prod, k) in &cfg.defs {
        map.entry((nt, prod)).or_default().def = Some(k);
    }
    for &(nt, prod, k, kind) in &cfg.refs {
        map.entry((nt, prod)).or_default().refs.push((k, kind));
    }
    for &(nt, prod, base, name) in &cfg.quals {
        map.entry((nt, prod)).or_default().qualify = Some((base, name));
    }
    for &(nt, prod, _, body) in &cfg.units {
        map.entry((nt, prod)).or_default().unit = Some(body);
    }
    for &(nt, prod, within, path) in &cfg.vis_levels {
        map.entry((nt, prod)).or_default().vis = Some((within, path));
    }
    for &(nt, prod, v, item) in &cfg.vis_items {
        map.entry((nt, prod)).or_default().visibility = Some((v, item));
    }
    for &(nt, prod, path, glob) in &cfg.imports {
        map.entry((nt, prod)).or_default().import = Some((path, glob));
    }
    for &(nt, prod, path, trees) in &cfg.prefixes {
        map.entry((nt, prod)).or_default().prefix = Some((path, trees));
    }
    for &(nt, prod) in &cfg.loads {
        map.entry((nt, prod)).or_default().loads = true;
    }
    for &(nt, prod, path) in &cfg.attributes {
        map.entry((nt, prod)).or_default().attribute = Some(path);
    }
    for &(nt, prod, attrs, item) in &cfg.attribute_lists {
        map.entry((nt, prod)).or_default().attribute_list = Some((attrs, item));
    }
    for &(nt, prod, ty, tr) in &cfg.impls {
        map.entry((nt, prod)).or_default().implementation = Some((ty, tr));
    }
    for e in outline.entries.iter().filter(|e| e.kind == FUNCTION) {
        map.entry((e.nt, e.prod)).or_default().function = true;
    }
    map
}

/// Where a definition stands.
pub(crate) struct DefPlace {
    /// Its unit (`None`: the file's own).
    pub unit: Option<u32>,
    pub visibility: Visibility,
    /// The impl block (in walk order) it is a member of.
    pub impl_block: Option<u32>,
}

/// Where a reference stands, and the path it spells.
pub(crate) struct RefPlace {
    /// Its unit (`None`: the file's own).
    pub unit: Option<u32>,
    /// Its path: the name, after the path its qualifiers spell; `None`
    /// when they spell none.
    pub path: Option<Path>,
    /// Whether it is in an import's path or a visibility's, which the
    /// import or the visibility says in full.
    pub imported: bool,
}

/// A byte span of the file.
pub(crate) type Span = (u32, u32);

/// What the walk read, units, imports and impl blocks in document order,
/// each with its span (a unit's: its name's, then its item's).
pub(crate) struct FileUnits {
    /// The attributes said of the file's own unit.
    pub attributes: Vec<Attribute>,
    pub units: Vec<(Unit, Span, Span)>,
    pub imports: Vec<(Import, Span)>,
    pub impls: Vec<(Impl, Span)>,
    /// By the name's span.
    pub defs: HashMap<(u32, u32), DefPlace>,
    /// By the name's span.
    pub refs: HashMap<(u32, u32), RefPlace>,
    /// Each unit with a body here (by its index in `units`), and the
    /// body's span.
    pub bodies: Vec<(u32, (u32, u32))>,
    /// The names imports define (a `use` leaf's, a rename's), by span:
    /// a reference bound to one is bound to what it imports.
    pub import_defs: HashSet<Span>,
}

impl FileUnits {
    /// The unit whose body holds `span` most closely (`None`: the file's
    /// own).
    pub fn unit_at(&self, span: (u32, u32)) -> Option<u32> {
        self.bodies
            .iter()
            .filter(|(_, body)| body.0 <= span.0 && span.1 <= body.1)
            .min_by_key(|(_, body)| body.1 - body.0)
            .map(|&(unit, _)| unit)
    }
}

/// The prefix of the use group the walk is in.
#[derive(Clone)]
enum Prefix {
    /// In no group.
    None,
    /// In a group whose imports begin with this.
    Path(Path),
    /// In a group whose prefix spells no path: its imports name nothing.
    Broken,
}

/// What encloses the node the walk is at.
#[derive(Clone)]
struct Cx {
    unit: Option<u32>,
    /// Inside a function's body.
    in_body: bool,
    /// The visibility what is defined here has, when an enclosing
    /// `@visibility` gave one and no scope opened since.
    visibility: Option<Visibility>,
    /// The attributes what is declared here takes, when an enclosing
    /// `@attributes` gave them and no scope opened since.
    attributes: Option<Rc<Vec<Attribute>>>,
    /// An impl block whose members scope has not opened yet.
    impl_pending: Option<u32>,
    /// The impl block whose members scope this is.
    members_of: Option<u32>,
    prefix: Prefix,
    /// Inside an import's path or a visibility's.
    imported: bool,
    /// The element of a list the walk is in: the item a construct stands
    /// in, with what the grammar puts before it there (attributes,
    /// visibility).
    item: Option<(u32, u32)>,
}

/// Read `tree`'s units, imports, visibilities and impl blocks.
pub(crate) fn read(tree: &GreenNode, cfg: &BindingConfig, outline: &OutlineConfig) -> FileUnits {
    let mut walk = Walk {
        cfg,
        roles: roles(cfg, outline),
        out: FileUnits {
            attributes: Vec::new(),
            units: Vec::new(),
            imports: Vec::new(),
            impls: Vec::new(),
            defs: HashMap::new(),
            refs: HashMap::new(),
            bodies: Vec::new(),
            import_defs: HashSet::new(),
        },
    };
    let cx = Cx {
        unit: None,
        in_body: false,
        visibility: None,
        attributes: None,
        impl_pending: None,
        members_of: None,
        prefix: Prefix::None,
        imported: false,
        item: None,
    };
    walk.node(tree, 0, &cx);
    walk.out
}

/// What is visible within its own unit and that unit's descendants: the
/// default.
pub(crate) fn private() -> Visibility {
    Visibility::Within(Path { root: Some(Root::Unit(0)), segments: Vec::new() })
}

struct Walk<'a> {
    cfg: &'a BindingConfig,
    roles: HashMap<(u16, u16), Roles>,
    out: FileUnits,
}

impl Walk<'_> {
    fn node(&mut self, n: &GreenNode, base: u32, cx: &Cx) {
        let empty = Roles::default();
        let roles = self.roles.get(&(n.nt, n.prod)).unwrap_or(&empty);
        let spans: Vec<Option<(u32, u32)>> = child_spans(n, base);
        let span_of = |k: usize| spans.get(k).copied().flatten();
        let whole = (base, base + n.width);

        // The visibility `@visibility` gives the item child, and what is
        // defined here takes, when its name is in that child.
        let given = roles.visibility.map(|(v, item)| (self.visibility_at(n, v), span_of(item)));
        let visibility_here = |name: (u32, u32)| -> Visibility {
            match &given {
                Some((v, Some(item))) if item.0 <= name.0 && name.1 <= item.1 => v.clone(),
                _ => cx.visibility.clone().unwrap_or_else(private),
            }
        };

        // The attributes `@attributes` gives (an attribute's own it holds
        // under its condition, and says nothing of here): what is declared
        // in the item child takes them, after those an enclosing node gave;
        // without an item child, the unit the node stands in.
        let attributes_given = match (roles.attribute_list, roles.attribute) {
            (Some((attrs, item)), None) => Some((self.attributes_at(n, attrs), item)),
            _ => None,
        };
        let attributes_here = |name: (u32, u32)| -> Vec<Attribute> {
            let mut attributes: Vec<Attribute> =
                cx.attributes.as_deref().cloned().unwrap_or_default();
            if let Some((given, Some(item))) = &attributes_given {
                if span_of(*item).is_some_and(|item| item.0 <= name.0 && name.1 <= item.1) {
                    attributes.extend(given.iter().cloned());
                }
            }
            attributes
        };
        if let (Some((given, None)), false) = (&attributes_given, cx.in_body) {
            match cx.unit {
                None => self.out.attributes.extend(given.iter().cloned()),
                Some(unit) => {
                    self.out.units[unit as usize].0.attributes.extend(given.iter().cloned())
                }
            }
        }

        // This node's definition, in the scope it stands in.
        let mut def_name: Option<(String, (u32, u32))> = None;
        if let Some(k) = roles.def {
            if let Some((name, off)) = name_child(n, k, self.cfg) {
                let span = (base + off, base + off + name.len() as u32);
                self.out.defs.insert(
                    span,
                    DefPlace {
                        unit: cx.unit,
                        visibility: visibility_here(span),
                        impl_block: cx.members_of,
                    },
                );
                def_name = Some((name, span));
            }
        }

        // Its references, each with its path.
        for &(k, kind) in &roles.refs {
            let Some((name, off)) = name_child(n, k, self.cfg) else { continue };
            let span = (base + off, base + off + name.len() as u32);
            let path = match (kind, roles.qualify) {
                (RefKind::Qualified, Some((qbase, qname))) if qname == k => {
                    Path::of_child(n, qbase, self.cfg).map(|mut path| {
                        path.segments.push(name.clone());
                        path
                    })
                }
                _ => Some(Path { root: None, segments: vec![name.clone()] }),
            };
            self.out.refs.insert(span, RefPlace { unit: cx.unit, path, imported: cx.imported });
        }

        // What its children stand in.
        let mut inner = cx.clone();
        if let Some(members) = roles.scope {
            inner.visibility = None;
            inner.attributes = None;
            inner.members_of = if members { cx.impl_pending } else { None };
            inner.impl_pending = None;
        }
        if roles.function {
            inner.in_body = true;
        }
        if let (Some((ty, tr)), false) = (roles.implementation, cx.in_body) {
            let index = self.out.impls.len() as u32;
            self.out.impls.push((
                Impl {
                    unit: cx.unit,
                    self_type: Path::of_child(n, ty, self.cfg),
                    trait_path: tr.and_then(|tr| Path::of_child(n, tr, self.cfg)),
                },
                whole,
            ));
            inner.impl_pending = Some(index);
        }

        // Children whose context differs from `inner`, by symbol index.
        let mut child_cx: Vec<(usize, Cx)> = Vec::new();

        // `pub(in path)`: the path is the visibility's, said in full.
        if let Some((_, Some(k))) = roles.vis {
            let mut path_cx = inner.clone();
            path_cx.imported = true;
            child_cx.push((k, path_cx));
        }

        if let (Some(body), Some((name, span)), false) = (roles.unit, &def_name, cx.in_body) {
            let index = self.out.units.len() as u32;
            self.out.units.push((
                Unit {
                    name: name.clone(),
                    parent: cx.unit,
                    inline: body.is_some(),
                    visibility: visibility_here(*span),
                    attributes: attributes_here(*span),
                },
                *span,
                cx.item.unwrap_or(whole),
            ));
            if let Some(body) = body {
                if let Some(span) = span_of(body) {
                    self.out.bodies.push((index, span));
                }
                let mut body_cx = inner.clone();
                body_cx.unit = Some(index);
                child_cx.push((body, body_cx));
            }
        }

        if let (Some(_), Some((_, span))) = (roles.import, &def_name) {
            self.out.import_defs.insert(*span);
        }
        if let Some((path, glob)) = roles.import {
            let own = match path {
                Some(k) => Path::of_child(n, k, self.cfg),
                None => Path::of_node(n, self.cfg),
            };
            let full = match (&cx.prefix, own) {
                (Prefix::None, own) => own,
                (Prefix::Path(prefix), None) => Some(prefix.clone()),
                (Prefix::Path(prefix), Some(own)) => own.after(prefix),
                (Prefix::Broken, _) => None,
            };
            if let (Some(full), false) = (full, cx.in_body) {
                let name = match (glob, roles.def) {
                    (true, _) | (false, None) => None,
                    (false, Some(_)) => match &def_name {
                        Some((name, _)) => Some(name.clone()),
                        None => full.segments.last().cloned(),
                    },
                };
                let (visibility, attributes) = match &def_name {
                    Some((_, span)) => (visibility_here(*span), attributes_here(*span)),
                    None => (
                        cx.visibility.clone().unwrap_or_else(private),
                        cx.attributes.as_deref().cloned().unwrap_or_default(),
                    ),
                };
                let loads = roles.loads;
                self.out.imports.push((
                    Import { unit: cx.unit, path: full, name, glob, visibility, attributes, loads },
                    whole,
                ));
            }
            if let Some(k) = path {
                let mut path_cx = inner.clone();
                path_cx.imported = true;
                child_cx.push((k, path_cx));
            }
        }

        if let Some((path, trees)) = roles.prefix {
            let own = match path {
                Some(k) => Path::of_child(n, k, self.cfg),
                None => Path::of_node(n, self.cfg),
            };
            let prefix = match (&cx.prefix, own) {
                (Prefix::Broken, _) | (_, None) => Prefix::Broken,
                (Prefix::None, Some(own)) => Prefix::Path(own),
                (Prefix::Path(outer), Some(own)) => match own.after(outer) {
                    Some(joined) => Prefix::Path(joined),
                    None => Prefix::Broken,
                },
            };
            let mut trees_cx = inner.clone();
            trees_cx.prefix = prefix;
            child_cx.push((trees, trees_cx));
            if let Some(k) = path {
                let mut path_cx = inner.clone();
                path_cx.imported = true;
                child_cx.push((k, path_cx));
            }
        }

        if let Some((_, item)) = roles.visibility {
            if let Some((v, _)) = &given {
                // Not in a scope this node opens: there, the item's
                // visibility is not what its contents have.
                if roles.scope.is_none() {
                    cx_of(&mut child_cx, item, &inner).visibility = Some(v.clone());
                }
            }
        }
        if let Some((given, Some(item))) = attributes_given {
            // Nor its attributes.
            if roles.scope.is_none() {
                cx_of(&mut child_cx, item, &inner).attributes = Some(Rc::new(given));
            }
        }

        let is_list = |prod: u16| prod == LIST_PROD || prod == RUN_PROD;
        let mut off = base;
        for (c, index) in n.children.iter().zip(symbol_indices(n)) {
            if let GreenChild::Node(m) = c {
                let cx = index
                    .and_then(|k| child_cx.iter().find(|(c, _)| *c == k))
                    .map_or(&inner, |(_, cx)| cx);
                if is_list(n.prod) && !is_list(m.prod) {
                    // An element of a list: an item, from its first token
                    // (the trivia before it is the space between items).
                    let mut element = cx.clone();
                    element.item = Some((off + leading_trivia(m), off + m.width));
                    self.node(m, off, &element);
                } else {
                    self.node(m, off, cx);
                }
            }
            off += c.width();
        }
    }

    /// The attributes in the subtree of child `k` of `n`, in the file's
    /// order: each attribute's, and those an attribute holds, under its
    /// condition.
    fn attributes_at(&self, n: &GreenNode, k: usize) -> Vec<Attribute> {
        let mut found = Vec::new();
        if let Some(child) = symbol_child(n, k) {
            self.attributes_in(child, false, &mut found);
        }
        found
    }

    fn attributes_in(&self, node: &GreenNode, conditional: bool, found: &mut Vec<Attribute>) {
        let roles = self.roles.get(&(node.nt, node.prod));
        if let Some(k) = roles.and_then(|r| r.attribute) {
            if let Some(path) = Path::of_child(node, k, self.cfg) {
                found.push(Attribute { path, conditional });
            }
            let held =
                roles.and_then(|r| r.attribute_list).and_then(|(a, _)| symbol_child(node, a));
            if let Some(held) = held {
                self.attributes_in(held, true, found);
            }
            return;
        }
        for c in &node.children {
            if let GreenChild::Node(m) = c {
                self.attributes_in(m, conditional, found);
            }
        }
    }

    /// The visibility the `@vis` node at or below child `k` of `n`
    /// declares; the default where there is none (an absent optional
    /// visibility).
    fn visibility_at(&self, n: &GreenNode, k: usize) -> Visibility {
        let Some(mut node) = symbol_child(n, k) else { return private() };
        loop {
            if let Some(&(within, path)) =
                self.roles.get(&(node.nt, node.prod)).and_then(|r| r.vis.as_ref())
            {
                return match (within, path) {
                    (None, None) => Visibility::Public,
                    (Some(root), _) => {
                        Visibility::Within(Path { root: Some(root), segments: Vec::new() })
                    }
                    // `pub(in path)` whose path spells none: the narrowest.
                    (None, Some(k)) => {
                        Path::of_child(node, k, self.cfg).map_or_else(private, Visibility::Within)
                    }
                };
            }
            let mut symbols =
                node.children.iter().zip(symbol_indices(node)).filter(|(_, index)| index.is_some());
            match (symbols.next(), symbols.next()) {
                (Some((GreenChild::Node(only), _)), None) => node = only,
                _ => return private(),
            }
        }
    }
}

/// The context child `k` stands in, among those that differ from
/// `inner`: made from `inner` when there is none yet.
fn cx_of<'c>(child_cx: &'c mut Vec<(usize, Cx)>, k: usize, inner: &Cx) -> &'c mut Cx {
    let at = match child_cx.iter().position(|(c, _)| *c == k) {
        Some(at) => at,
        None => {
            child_cx.push((k, inner.clone()));
            child_cx.len() - 1
        }
    };
    &mut child_cx[at].1
}

/// The width of the trivia before `n`'s first token.
fn leading_trivia(n: &GreenNode) -> u32 {
    let mut width = 0;
    for c in &n.children {
        match c {
            GreenChild::Token(t) if t.trivia => width += c.width(),
            GreenChild::Token(_) => return width,
            GreenChild::Node(m) => {
                let inner = leading_trivia(m);
                if inner < m.width {
                    return width + inner;
                }
                width += m.width;
            }
        }
    }
    width
}

/// The `k`th symbol child of `n`, when it is a node.
fn symbol_child(n: &GreenNode, k: usize) -> Option<&GreenNode> {
    n.children.iter().zip(symbol_indices(n)).find_map(|(c, index)| match c {
        GreenChild::Node(m) if index == Some(k) => Some(m.as_ref()),
        _ => None,
    })
}

/// Each symbol child's span, by its symbol index.
fn child_spans(n: &GreenNode, base: u32) -> Vec<Option<(u32, u32)>> {
    let mut spans = Vec::new();
    let mut off = base;
    for (c, index) in n.children.iter().zip(symbol_indices(n)) {
        let w = c.width();
        if let Some(k) = index {
            if spans.len() <= k {
                spans.resize(k + 1, None);
            }
            spans[k] = Some((off, off + w));
        }
        off += w;
    }
    spans
}
