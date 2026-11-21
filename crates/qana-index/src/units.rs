//! The walk that reads a file's units, imports, visibilities, attributes
//! and impl blocks from its tree, as its grammar's binding says (`@unit`,
//! `@root`, `@vis`, `@visibility`, `@import`, `@import_glob`,
//! `@import_prefix`, `@loads`, `@attribute`, `@attributes`, `@expands`,
//! `@impl`), and the unit and path of each definition and reference.
//!
//! An invocation whose expansion may declare names (`@expands`) opens the
//! unit it stands among the items of, or else the innermost scope around
//! it (a block): a reference inside a scope so marked may name what the
//! expansion declares. So does an import in a block, which a summary does
//! not read: what it brings is the block's.
//!
//! What a function's body holds is the body's: its units, imports and
//! impl blocks are not read (as its declarations are not offered), while
//! its references are, in the unit the function is in.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use qana_grammar::green::{LIST_PROD, RUN_PROD};
use qana_grammar::{GreenChild, GreenNode};
use qana_sem::{
    name_child, symbol_indices, BindingConfig, MemberForm, Path, RefKind, Root, TypeForm, ValueForm,
};
use qana_services::OutlineConfig;

use crate::{Attribute, Impl, Import, TypeRef, Unit, Visibility, FUNCTION};

/// What the binding says of one production, gathered from the config's
/// tables once, so the walk asks each node once.
#[derive(Default)]
pub(crate) struct Roles {
    /// It opens a scope; `true` when the scope holds members.
    pub(crate) scope: Option<bool>,
    /// The symbol child its `@def` names.
    pub(crate) def: Option<usize>,
    /// Its references: the symbol child each names, and its kind.
    pub(crate) refs: Vec<(usize, RefKind)>,
    /// `@qualify(base, name)`: the base child and the name child.
    pub(crate) qualify: Option<(usize, usize)>,
    /// `@unit(name[, body])`: the body child.
    pub(crate) unit: Option<Option<usize>>,
    /// `@vis`: within, and the path child of `in`.
    pub(crate) vis: Option<(Option<Root>, Option<usize>)>,
    /// `@visibility(v, item)`.
    pub(crate) visibility: Option<(usize, usize)>,
    /// `@import`/`@import_glob`: the path child, and whether a glob.
    pub(crate) import: Option<(Option<usize>, bool)>,
    /// `@loads`: the import loads the library its path names.
    pub(crate) loads: bool,
    /// `@attribute(path[, args])`: the path child.
    pub(crate) attribute: Option<usize>,
    /// `@attribute(path, args)`: the arguments child.
    pub(crate) attribute_args: Option<usize>,
    /// `@attributes(attrs[, item])`: the attributes child, and the item
    /// child.
    pub(crate) attribute_list: Option<(usize, Option<usize>)>,
    /// `@expands`: an invocation whose expansion may declare names.
    pub(crate) expands: bool,
    /// `@type_ref(form, …)`: a type as written.
    pub(crate) type_ref: Option<TypeForm>,
    /// `@typed(name, type)`: the name child and the type child.
    pub(crate) typed: Option<(usize, usize)>,
    /// `@returns(type)`: the type child.
    pub(crate) returns: Option<usize>,
    /// `@derives(path)`: the path child.
    pub(crate) derives: Option<usize>,
    /// `@generic`: what it defines is a generic parameter.
    pub(crate) generic: bool,
    /// `@bounds(b)`: the bounds child.
    pub(crate) bounds: Option<usize>,
    /// `@positional(f)`: the fields child.
    pub(crate) positional: Option<usize>,
    /// `@import_prefix`: the path child, and the trees child.
    pub(crate) prefix: Option<(Option<usize>, usize)>,
    /// `@impl(type[, trait])`.
    pub(crate) implementation: Option<(usize, Option<usize>)>,
    /// A construct its outline calls a function: what it holds is a body's.
    pub(crate) function: bool,
    /// `@value(form, …)`: an expression's value, as written.
    pub(crate) value: Option<ValueForm>,
    /// `@member(form, …)`: what a member access names.
    pub(crate) member_name: Option<MemberForm>,
    /// `@inits(name, value)`: the name child and the value child.
    pub(crate) inits: Option<(usize, usize)>,
    /// `@self_value`: what it defines is the value a method is called on.
    pub(crate) self_value: bool,
    /// `@value(builtin, name)`: a value of the type the language provides
    /// under that name.
    pub(crate) builtin: Option<String>,
    /// `@def(x, textual)`: what it defines is seen in textual order.
    pub(crate) textual: bool,
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
    for &(nt, prod) in &cfg.textual {
        map.entry((nt, prod)).or_default().textual = true;
    }
    for &(nt, prod, path) in &cfg.attributes {
        map.entry((nt, prod)).or_default().attribute = Some(path);
    }
    for &(nt, prod, args) in &cfg.attribute_args {
        map.entry((nt, prod)).or_default().attribute_args = Some(args);
    }
    for &(nt, prod, attrs, item) in &cfg.attribute_lists {
        map.entry((nt, prod)).or_default().attribute_list = Some((attrs, item));
    }
    for &(nt, prod) in &cfg.expands {
        map.entry((nt, prod)).or_default().expands = true;
    }
    for &(nt, prod, form) in &cfg.type_refs {
        map.entry((nt, prod)).or_default().type_ref = Some(form);
    }
    for &(nt, prod, name, ty) in &cfg.typed {
        map.entry((nt, prod)).or_default().typed = Some((name, ty));
    }
    for &(nt, prod, ty) in &cfg.returns {
        map.entry((nt, prod)).or_default().returns = Some(ty);
    }
    for &(nt, prod, path) in &cfg.derives {
        map.entry((nt, prod)).or_default().derives = Some(path);
    }
    for &(nt, prod) in &cfg.generics {
        map.entry((nt, prod)).or_default().generic = true;
    }
    for &(nt, prod, b) in &cfg.bounds {
        map.entry((nt, prod)).or_default().bounds = Some(b);
    }
    for &(nt, prod, f) in &cfg.positional {
        map.entry((nt, prod)).or_default().positional = Some(f);
    }
    for &(nt, prod, form) in &cfg.values {
        map.entry((nt, prod)).or_default().value = Some(form);
    }
    for &(nt, prod, form) in &cfg.member_names {
        map.entry((nt, prod)).or_default().member_name = Some(form);
    }
    for &(nt, prod, name, value) in &cfg.inits {
        map.entry((nt, prod)).or_default().inits = Some((name, value));
    }
    for &(nt, prod) in &cfg.self_values {
        map.entry((nt, prod)).or_default().self_value = true;
    }
    for (nt, prod, name) in &cfg.value_builtins {
        map.entry((*nt, *prod)).or_default().builtin = Some(name.clone());
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
    /// Its type as written (`@typed`), if one is.
    pub ty: Option<TypeRef>,
    /// The type a call of it gives, as written (`@returns`).
    pub returns: Option<TypeRef>,
    /// What it derives (`@derives`, among the attributes said of it),
    /// each path with its span.
    pub derives: Vec<Derive>,
    /// Its generic parameters' names, in order (`@generic`, in the scope
    /// it opens).
    pub generics: Vec<String>,
    /// The traits it is bounded by, as written (`@bounds`).
    pub bounds: Vec<TypeRef>,
    /// Its positional fields' types, in order (`@positional`).
    pub positional: Vec<TypeRef>,
    /// The value it holds, as written (`@inits`): the node, and where it
    /// starts.
    pub init: Option<(Arc<GreenNode>, u32)>,
    /// Whether it is the value a method is called on (`@self_value`).
    pub self_value: bool,
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
    /// The scopes around it below its unit's, outermost first, by their
    /// walk index.
    pub scopes: Rc<Vec<u32>>,
    /// Whether it is a member access's name (`@member`): what it names a
    /// value's type says, not a path.
    pub member: bool,
}

/// A byte span of the file.
pub(crate) type Span = (u32, u32);

/// What rustc's textual scope is made of, in the file's order
/// (`@def(x, textual)`): see [`crate::textual_at`].
#[derive(Clone, Debug)]
pub(crate) enum TextualEvent {
    /// A textual definition: its name, the name's span, the attributes
    /// said of it.
    Def { name: String, span: Span, attributes: Vec<Attribute> },
    /// A module whose body is another file, at its item: its name, the
    /// attributes said of it, its index in `units` where a summary reads
    /// it (not in a body).
    File { name: String, attributes: Vec<Attribute>, at: u32, unit: Option<u32> },
    /// A scope opens (a block: what is defined in it is not seen after
    /// it), or an inline module's body (`module`: what is defined in it is
    /// seen after it where its attributes carry it out).
    Open { module: Option<TextualModule>, span: Span },
    /// What the last `Open` opened closes, at `end`.
    Close { end: u32 },
}

/// An inline module whose body opens: its name, the attributes said of it,
/// its index in `units` where a summary reads it.
#[derive(Clone, Debug)]
pub(crate) struct TextualModule {
    pub name: String,
    pub attributes: Vec<Attribute>,
    pub unit: Option<u32>,
}

/// A derive's path (`@derives`), and its span.
pub(crate) type Derive = (Path, Span);

/// The spans of what an impl block's head names: its type, and its trait.
pub(crate) type ImplHead = (Option<Span>, Option<Span>);

/// What the walk read, units, imports and impl blocks in document order,
/// each with its span (a unit's: its name's, then its item's).
pub(crate) struct FileUnits {
    /// The attributes said of the file's own unit.
    pub attributes: Vec<Attribute>,
    /// Whether the file's own unit is open (an `@expands` construct
    /// stands among its items).
    pub open: bool,
    /// The scopes below a unit's that an `@expands` construct stands in
    /// (not inside a scope of its own), by walk index.
    pub expanding: HashSet<u32>,
    pub units: Vec<(Unit, Span, Span)>,
    pub imports: Vec<(Import, Span)>,
    pub impls: Vec<(Impl, Span, ImplHead)>,
    /// By the name's span.
    pub defs: HashMap<(u32, u32), DefPlace>,
    /// By the name's span.
    pub refs: HashMap<(u32, u32), RefPlace>,
    /// Each unit with a body here (by its index in `units`), and the
    /// body's span.
    pub bodies: Vec<(u32, (u32, u32))>,
    /// Each impl block's generic parameters, a body's too, by the span of
    /// the node that says it is one.
    pub impl_generics: HashMap<Span, Vec<String>>,
    /// The names imports define (a `use` leaf's, a rename's), by span:
    /// a reference bound to one is bound to what it imports.
    pub import_defs: HashSet<Span>,
    /// The textual definitions, modules and scopes, bodies' too, in the
    /// file's order.
    pub textual: Vec<TextualEvent>,
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
    /// `@attributes` gave them and no scope opened since, and what they
    /// derive.
    attributes: Option<Rc<Vec<Attribute>>>,
    derives: Option<Rc<Vec<Derive>>>,
    /// The scopes around it below its unit's, outermost first: empty among
    /// a unit's items.
    scopes: Rc<Vec<u32>>,
    /// The tree's root, whose scope is the file's own unit's.
    root: bool,
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
        g: Grammar::new(cfg, outline),
        out: FileUnits {
            attributes: Vec::new(),
            open: false,
            expanding: HashSet::new(),
            units: Vec::new(),
            imports: Vec::new(),
            impls: Vec::new(),
            defs: HashMap::new(),
            refs: HashMap::new(),
            bodies: Vec::new(),
            impl_generics: HashMap::new(),
            import_defs: HashSet::new(),
            textual: Vec::new(),
        },
        scopes: 0,
        typed_defs: HashMap::new(),
        generics: HashMap::new(),
        init_defs: HashMap::new(),
    };
    let cx = Cx {
        unit: None,
        in_body: false,
        visibility: None,
        attributes: None,
        derives: None,
        scopes: Rc::default(),
        root: true,
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

/// What a grammar's binding says of each production ([`Roles`]), and the
/// readers of what is written that need nothing else: types as written.
pub(crate) struct Grammar<'a> {
    pub cfg: &'a BindingConfig,
    roles: HashMap<(u16, u16), Roles>,
}

impl<'a> Grammar<'a> {
    pub fn new(cfg: &'a BindingConfig, outline: &OutlineConfig) -> Self {
        Self { cfg, roles: roles(cfg, outline) }
    }

    /// What the binding says of `node`'s production.
    pub fn roles_of(&self, node: &GreenNode) -> Option<&Roles> {
        self.roles.get(&(node.nt, node.prod))
    }

    /// The type written at child `k` of `n`.
    pub(crate) fn type_in(&self, n: &GreenNode, k: usize) -> Option<TypeRef> {
        self.type_of(symbol_child(n, k)?)
    }

    /// The type `node` writes: its own, where it is one (`@type_ref`), or
    /// the first its children write. A generic argument is the path's it
    /// stands in, never the type around it.
    fn type_of(&self, node: &GreenNode) -> Option<TypeRef> {
        match self.type_form(node) {
            Some(TypeForm::Arg(_) | TypeForm::Binding(..) | TypeForm::Const) => None,
            None => node.children.iter().find_map(|c| match c {
                GreenChild::Node(m) => self.type_of(m),
                GreenChild::Token(_) => None,
            }),
            Some(form) => Some(self.type_as(node, form)),
        }
    }

    fn type_form(&self, node: &GreenNode) -> Option<TypeForm> {
        self.roles.get(&(node.nt, node.prod)).and_then(|r| r.type_ref)
    }

    /// The type `node`, whose form is `form`, writes.
    fn type_as(&self, node: &GreenNode, form: TypeForm) -> TypeRef {
        let inner = |k: usize| Box::new(self.type_in(node, k).unwrap_or(TypeRef::Other));
        match form {
            TypeForm::Path(k) => match Path::of_child(node, k, self.cfg) {
                Some(path) => {
                    let (mut args, mut bindings) = (Vec::new(), Vec::new());
                    if let Some(child) = symbol_child(node, k) {
                        self.args_in(child, &mut args, &mut bindings);
                    }
                    TypeRef::Path { path, args, bindings }
                }
                None => TypeRef::Other,
            },
            TypeForm::Ref(k) => TypeRef::Ref(inner(k)),
            TypeForm::Slice(k) => TypeRef::Slice(inner(k)),
            TypeForm::Array(k) => TypeRef::Array(inner(k)),
            TypeForm::Tuple => TypeRef::Tuple(self.types_held(node)),
            TypeForm::Dyn => TypeRef::Dyn(self.traits_held(node)),
            TypeForm::Impl => TypeRef::Impl(self.traits_held(node)),
            TypeForm::Arg(_) | TypeForm::Binding(..) | TypeForm::Const | TypeForm::Other => {
                TypeRef::Other
            }
        }
    }

    /// The types of the positional fields listed at child `k` of `n`: one
    /// for each element of the first list there, the type it writes, in
    /// order. No list (none written) is no fields.
    fn positional_in(&self, n: &GreenNode, k: usize) -> Vec<TypeRef> {
        let Some(mut node) = symbol_child(n, k) else { return Vec::new() };
        while node.prod != LIST_PROD && node.prod != RUN_PROD {
            let first = node.children.iter().find_map(|c| match c {
                GreenChild::Node(m) => Some(m),
                GreenChild::Token(_) => None,
            });
            match first {
                Some(m) => node = m,
                None => return Vec::new(),
            }
        }
        node.children
            .iter()
            .filter_map(|c| match c {
                GreenChild::Node(m) => Some(self.type_of(m).unwrap_or(TypeRef::Other)),
                GreenChild::Token(_) => None,
            })
            .collect()
    }

    /// The traits written at child `k` of `n` (bounds): each the
    /// outermost type there, but those that are no trait the type has
    /// (Rust's `?Sized`, `use<…>`).
    fn traits_in(&self, n: &GreenNode, k: usize) -> Vec<TypeRef> {
        symbol_child(n, k).map(|child| self.traits_held(child)).unwrap_or_default()
    }

    /// The traits `node`'s children write: [`Self::types_held`], but those
    /// a bound written there does not say the type has.
    fn traits_held(&self, node: &GreenNode) -> Vec<TypeRef> {
        let mut held = self.types_held(node);
        held.retain(|t| *t != TypeRef::Other);
        held
    }

    /// The types `node`'s children write, each the outermost one; a
    /// generic argument's is the path's it stands in.
    fn types_held(&self, node: &GreenNode) -> Vec<TypeRef> {
        let mut held = Vec::new();
        for c in &node.children {
            if let GreenChild::Node(m) = c {
                match self.type_form(m) {
                    Some(TypeForm::Arg(_) | TypeForm::Binding(..) | TypeForm::Const) => {}
                    None => held.extend(self.types_held(m)),
                    Some(form) => held.push(self.type_as(m, form)),
                }
            }
        }
        held
    }

    /// The type arguments and associated-type bindings written in `node`,
    /// a path's, outside any type of their own.
    fn args_in(
        &self,
        node: &GreenNode,
        args: &mut Vec<TypeRef>,
        bindings: &mut Vec<(String, TypeRef)>,
    ) {
        match self.type_form(node) {
            Some(TypeForm::Arg(t)) => {
                args.push(self.type_in(node, t).unwrap_or(TypeRef::Other));
                return;
            }
            Some(TypeForm::Binding(n, t)) => {
                if let Some((name, _)) = name_child(node, n, self.cfg) {
                    bindings.push((name, self.type_in(node, t).unwrap_or(TypeRef::Other)));
                }
                return;
            }
            Some(TypeForm::Const) => {
                // A constant keeps its place, so that arguments line up
                // with the parameters they are given for.
                args.push(TypeRef::Other);
                return;
            }
            Some(_) => return,
            None => {}
        }
        for c in &node.children {
            if let GreenChild::Node(m) = c {
                self.args_in(m, args, bindings);
            }
        }
    }
}

struct Walk<'a> {
    g: Grammar<'a>,
    out: FileUnits,
    /// How many scopes below a unit's the walk has opened.
    scopes: u32,
    /// The types `@typed` gives definitions made below the node that says
    /// so (a pattern's one binding), by the name's span.
    typed_defs: HashMap<(u32, u32), TypeRef>,
    /// The generic parameters (`@generic`) declared in each scope below a
    /// unit's, by the scope's walk index, in order: what opened the scope
    /// (a definition, or the impl block in it) takes them.
    generics: HashMap<u32, Vec<String>>,
    /// The values `@inits` gives definitions made below the node that says
    /// so (a pattern's one binding), by the name's span.
    init_defs: HashMap<(u32, u32), (Arc<GreenNode>, u32)>,
}

impl Walk<'_> {
    fn node(&mut self, n: &GreenNode, base: u32, cx: &Cx) {
        let empty = Roles::default();
        let roles = self.g.roles.get(&(n.nt, n.prod)).unwrap_or(&empty);
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
            (Some((attrs, item)), None) => {
                let (attributes, derives) = self.attributes_at(n, base, attrs);
                Some((attributes, derives, item))
            }
            _ => None,
        };
        let in_item = |name: (u32, u32)| match &attributes_given {
            Some((_, _, Some(item))) => {
                span_of(*item).is_some_and(|item| item.0 <= name.0 && name.1 <= item.1)
            }
            _ => false,
        };
        let attributes_here = |name: (u32, u32)| -> Vec<Attribute> {
            let mut attributes: Vec<Attribute> =
                cx.attributes.as_deref().cloned().unwrap_or_default();
            if let (Some((given, _, _)), true) = (&attributes_given, in_item(name)) {
                attributes.extend(given.iter().cloned());
            }
            attributes
        };
        let derives_here = |name: (u32, u32)| -> Vec<Derive> {
            let mut derives: Vec<Derive> = cx.derives.as_deref().cloned().unwrap_or_default();
            if let (Some((_, given, _)), true) = (&attributes_given, in_item(name)) {
                derives.extend(given.iter().cloned());
            }
            derives
        };
        if let (Some((given, _, None)), false) = (&attributes_given, cx.in_body) {
            match cx.unit {
                None => self.out.attributes.extend(given.iter().cloned()),
                Some(unit) => {
                    self.out.units[unit as usize].0.attributes.extend(given.iter().cloned())
                }
            }
        }

        // The type `@typed` writes for what its name child defines: this
        // node's own definition, or the binding its name child is (a
        // pattern that destructures binds parts of the value, not it).
        let mut typed_here: Option<TypeRef> = None;
        if let Some((name_k, type_k)) = roles.typed {
            if let Some(ty) = self.g.type_in(n, type_k) {
                if roles.def == Some(name_k) {
                    typed_here = Some(ty);
                } else if let (Some(child), Some(at)) = (symbol_child(n, name_k), span_of(name_k)) {
                    if let Some(def) = self.binding_def(child, at.0) {
                        self.typed_defs.insert(def, ty);
                    }
                }
            }
        }

        // The value `@inits` gives the binding its name child is.
        if let Some((name_k, value_k)) = roles.inits {
            let value = symbol_child_at(n, value_k, base).filter(|(node, _)| node.width > 0);
            if let (Some((node, at)), Some(child), Some(span)) =
                (value, symbol_child(n, name_k), span_of(name_k))
            {
                if let Some(def) = self.binding_def(child, span.0) {
                    self.init_defs.insert(def, (node.clone(), at));
                }
            }
        }

        // This node's definition, in the scope it stands in.
        let mut def_name: Option<(String, (u32, u32))> = None;
        if let Some(k) = roles.def {
            if let Some((name, off)) = name_child(n, k, self.g.cfg) {
                let span = (base + off, base + off + name.len() as u32);
                let ty = typed_here.take().or_else(|| self.typed_defs.remove(&span));
                let init = self.init_defs.remove(&span);
                let returns = roles.returns.and_then(|r| self.g.type_in(n, r));
                let bounds = roles.bounds.map(|b| self.g.traits_in(n, b)).unwrap_or_default();
                let positional =
                    roles.positional.map(|f| self.g.positional_in(n, f)).unwrap_or_default();
                if let (true, Some(&scope)) = (roles.generic, cx.scopes.last()) {
                    self.generics.entry(scope).or_default().push(name.clone());
                }
                self.out.defs.insert(
                    span,
                    DefPlace {
                        unit: cx.unit,
                        visibility: visibility_here(span),
                        impl_block: cx.members_of,
                        ty,
                        returns,
                        derives: derives_here(span),
                        // Its own scope's, once its children are read.
                        generics: Vec::new(),
                        bounds,
                        positional,
                        init,
                        self_value: roles.self_value,
                    },
                );
                def_name = Some((name, span));
            }
        }

        if let (true, Some((name, span))) = (roles.textual, &def_name) {
            self.out.textual.push(TextualEvent::Def {
                name: name.clone(),
                span: *span,
                attributes: attributes_here(*span),
            });
        }

        // Its references, each with its path.
        for &(k, kind) in &roles.refs {
            let Some((name, off)) = name_child(n, k, self.g.cfg) else { continue };
            let span = (base + off, base + off + name.len() as u32);
            let path = match (kind, roles.qualify) {
                (RefKind::Qualified, Some((qbase, qname))) if qname == k => {
                    Path::of_child(n, qbase, self.g.cfg).map(|mut path| {
                        path.segments.push(name.clone());
                        path
                    })
                }
                // A root alone (`Self`, `self`) is that root's path.
                _ => Some(
                    rooted(n, k, self.g.cfg)
                        .unwrap_or_else(|| Path { root: None, segments: vec![name.clone()] }),
                ),
            };
            self.out.refs.insert(
                span,
                RefPlace {
                    unit: cx.unit,
                    path,
                    imported: cx.imported,
                    scopes: cx.scopes.clone(),
                    member: matches!(
                        roles.member_name,
                        Some(MemberForm::Field(m) | MemberForm::Method(m)) if m == k
                    ),
                },
            );
        }

        // An invocation that may declare names opens the unit it stands
        // among the items of, or else the innermost scope around it.
        if roles.expands {
            match (cx.scopes.last(), cx.unit) {
                (Some(&scope), _) => {
                    self.out.expanding.insert(scope);
                }
                (None, None) => self.out.open = true,
                (None, Some(unit)) => self.out.units[unit as usize].0.open = true,
            }
        }

        // What its children stand in.
        let mut inner = cx.clone();
        inner.root = false;
        let mut own_scope: Option<u32> = None;
        if roles.scope.is_some() && !cx.root && roles.unit.is_none() {
            // A scope below its unit's: a function, a type, a block.
            let mut scopes = cx.scopes.as_ref().clone();
            scopes.push(self.scopes);
            own_scope = Some(self.scopes);
            self.scopes += 1;
            inner.scopes = Rc::new(scopes);
        }
        if let Some(members) = roles.scope {
            inner.visibility = None;
            inner.attributes = None;
            inner.derives = None;
            inner.members_of = if members { cx.impl_pending } else { None };
            inner.impl_pending = None;
        }
        if roles.function {
            inner.in_body = true;
        }
        if let Some((_, true)) = roles.implementation.map(|i| (i, cx.in_body)) {
            // A body's impl block, which no summary holds: its generic
            // parameters, for reading what is written in it.
            let generics =
                cx.scopes.last().and_then(|scope| self.generics.remove(scope)).unwrap_or_default();
            self.out.impl_generics.insert(whole, generics);
        }
        if let (Some((ty, tr)), false) = (roles.implementation, cx.in_body) {
            let index = self.out.impls.len() as u32;
            // Its generic parameters were declared before it, in the scope
            // the impl block opens.
            let generics =
                cx.scopes.last().and_then(|scope| self.generics.remove(scope)).unwrap_or_default();
            self.out.impl_generics.insert(whole, generics.clone());
            self.out.impls.push((
                Impl {
                    unit: cx.unit,
                    self_type: self.g.type_in(n, ty).or_else(|| {
                        // A grammar that writes no type forms there: the
                        // path the type spells.
                        Path::of_child(n, ty, self.g.cfg).map(|path| TypeRef::Path {
                            path,
                            args: Vec::new(),
                            bindings: Vec::new(),
                        })
                    }),
                    trait_path: tr.and_then(|tr| Path::of_child(n, tr, self.g.cfg)),
                    generics,
                },
                whole,
                (exact_span(n, base, ty), tr.and_then(|tr| exact_span(n, base, tr))),
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

        // A module, for the textual log, a body's too: a file's at its
        // item, an inline one around its body.
        let mut textual_body: Option<(usize, TextualModule, Span)> = None;
        if let (Some(body), Some((name, span))) = (roles.unit, &def_name) {
            let attributes = attributes_here(*span);
            let unit = (!cx.in_body).then_some(self.out.units.len() as u32);
            match body {
                Some(b) => {
                    if let Some(at) = span_of(b) {
                        let module = TextualModule { name: name.clone(), attributes, unit };
                        textual_body = Some((b, module, at));
                    }
                }
                None => self.out.textual.push(TextualEvent::File {
                    name: name.clone(),
                    attributes,
                    at: cx.item.unwrap_or(whole).0,
                    unit,
                }),
            }
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
                    open: false,
                    textual: Vec::new(),
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
                body_cx.scopes = Rc::default();
                child_cx.push((body, body_cx));
            }
        }

        if let (Some(_), Some((_, span))) = (roles.import, &def_name) {
            self.out.import_defs.insert(*span);
        }
        if let (Some(_), Some(&scope)) = (roles.import, cx.scopes.last()) {
            // An import in a block: what it brings, no summary holds.
            self.out.expanding.insert(scope);
        }
        if let Some((path, glob)) = roles.import {
            let own = match path {
                Some(k) => Path::of_child(n, k, self.g.cfg),
                None => Path::of_node(n, self.g.cfg),
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
                Some(k) => Path::of_child(n, k, self.g.cfg),
                None => Path::of_node(n, self.g.cfg),
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
        if let Some((given, derives, Some(item))) = attributes_given {
            // Nor its attributes.
            if roles.scope.is_none() {
                let item_cx = cx_of(&mut child_cx, item, &inner);
                item_cx.attributes = Some(Rc::new(given));
                item_cx.derives = Some(Rc::new(derives));
            }
        }

        if own_scope.is_some() {
            self.out.textual.push(TextualEvent::Open { module: None, span: whole });
        }
        let is_list = |prod: u16| prod == LIST_PROD || prod == RUN_PROD;
        let mut off = base;
        for (c, index) in n.children.iter().zip(symbol_indices(n)) {
            if let GreenChild::Node(m) = c {
                let cx = index
                    .and_then(|k| child_cx.iter().find(|(c, _)| *c == k))
                    .map_or(&inner, |(_, cx)| cx);
                let body = textual_body.as_ref().filter(|(b, _, _)| index == Some(*b));
                if let Some((_, module, at)) = body {
                    self.out
                        .textual
                        .push(TextualEvent::Open { module: Some(module.clone()), span: *at });
                }
                if is_list(n.prod) && !is_list(m.prod) {
                    // An element of a list: an item, from its first token
                    // (the trivia before it is the space between items).
                    let mut element = cx.clone();
                    element.item = Some((off + leading_trivia(m), off + m.width));
                    self.node(m, off, &element);
                } else {
                    self.node(m, off, cx);
                }
                if let Some((_, _, at)) = body {
                    self.out.textual.push(TextualEvent::Close { end: at.1 });
                }
            }
            off += c.width();
        }
        if own_scope.is_some() {
            self.out.textual.push(TextualEvent::Close { end: whole.1 });
        }

        // The generic parameters declared in the scope this definition
        // opens are its own.
        if let (Some(scope), Some((_, span))) = (own_scope, &def_name) {
            if let (Some(generics), Some(place)) =
                (self.generics.remove(&scope), self.out.defs.get_mut(span))
            {
                place.generics = generics;
            }
        }
    }

    /// The attributes in the subtree of child `k` of `n` (which starts at
    /// `base`), in the file's order: each attribute's, and those an
    /// attribute holds, under its condition; and what they derive
    /// (`@derives`), with where.
    fn attributes_at(&self, n: &GreenNode, base: u32, k: usize) -> (Vec<Attribute>, Vec<Derive>) {
        let (mut found, mut derives) = (Vec::new(), Vec::new());
        if let Some((child, at)) = symbol_child_at(n, k, base) {
            self.attributes_in(child, at, false, &mut found, &mut derives);
        }
        (found, derives)
    }

    fn attributes_in(
        &self,
        node: &GreenNode,
        base: u32,
        conditional: bool,
        found: &mut Vec<Attribute>,
        derives: &mut Vec<Derive>,
    ) {
        let roles = self.g.roles.get(&(node.nt, node.prod));
        if let Some(k) = roles.and_then(|r| r.derives) {
            derives.extend(self.derive_at(node, base, k));
            return;
        }
        if let Some(k) = roles.and_then(|r| r.attribute) {
            if let Some(path) = Path::of_child(node, k, self.g.cfg) {
                let mut args = Vec::new();
                if let Some((held, _)) = roles
                    .and_then(|r| r.attribute_args)
                    .and_then(|a| symbol_child_at(node, a, base))
                {
                    paths_in(held, self.g.cfg, &mut args);
                }
                found.push(Attribute { path, conditional, args });
            }
            let held = roles
                .and_then(|r| r.attribute_list)
                .and_then(|(a, _)| symbol_child_at(node, a, base));
            if let Some((held, at)) = held {
                self.attributes_in(held, at, true, found, derives);
            } else {
                // What an attribute derives is in its arguments.
                for (m, at) in child_nodes(node, base) {
                    self.derives_in(m, at, derives);
                }
            }
            return;
        }
        for (m, at) in child_nodes(node, base) {
            self.attributes_in(m, at, conditional, found, derives);
        }
    }

    /// What the `@derives` nodes in `node`'s subtree (which starts at
    /// `base`) name, with where.
    fn derives_in(&self, node: &GreenNode, base: u32, derives: &mut Vec<Derive>) {
        if let Some(k) = self.g.roles.get(&(node.nt, node.prod)).and_then(|r| r.derives) {
            derives.extend(self.derive_at(node, base, k));
            return;
        }
        for (m, at) in child_nodes(node, base) {
            self.derives_in(m, at, derives);
        }
    }

    /// The path a `@derives` node (`node`, starting at `base`) names at its
    /// child `k`, and that child's span.
    fn derive_at(&self, node: &GreenNode, base: u32, k: usize) -> Option<Derive> {
        let path = Path::of_child(node, k, self.g.cfg)?;
        Some((path, exact_span(node, base, k)?))
    }

    /// The name span of the definition the pattern `node` (starting at
    /// `base`) is, where it binds the whole value (`x`, `mut x`, `x @ p`):
    /// one reached through nodes that only wrap another. `None` for a
    /// pattern that destructures (`Some(x)`, `(a, b)`, `&x`).
    fn binding_def(&self, node: &GreenNode, base: u32) -> Option<(u32, u32)> {
        let (mut node, mut base) = (node, base);
        loop {
            if let Some(k) = self.g.roles.get(&(node.nt, node.prod)).and_then(|r| r.def) {
                let (name, off) = name_child(node, k, self.g.cfg)?;
                return Some((base + off, base + off + name.len() as u32));
            }
            (node, base) = only_node(node, base)?;
        }
    }

    /// The visibility the `@vis` node at or below child `k` of `n`
    /// declares; the default where there is none (an absent optional
    /// visibility).
    fn visibility_at(&self, n: &GreenNode, k: usize) -> Visibility {
        let Some(mut node) = symbol_child(n, k) else { return private() };
        loop {
            if let Some(&(within, path)) =
                self.g.roles.get(&(node.nt, node.prod)).and_then(|r| r.vis.as_ref())
            {
                return match (within, path) {
                    (None, None) => Visibility::Public,
                    (Some(root), _) => {
                        Visibility::Within(Path { root: Some(root), segments: Vec::new() })
                    }
                    // `pub(in path)` whose path spells none: the narrowest.
                    (None, Some(k)) => {
                        Path::of_child(node, k, self.g.cfg).map_or_else(private, Visibility::Within)
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

/// The span of `n`'s `k`th symbol child (`n` starting at `base`), from its
/// first token: a node's leading trivia is not its own.
fn exact_span(n: &GreenNode, base: u32, k: usize) -> Option<Span> {
    let mut off = base;
    for (c, index) in n.children.iter().zip(symbol_indices(n)) {
        if index == Some(k) {
            return Some(match c {
                GreenChild::Node(m) => (off + leading_trivia(m), off + m.width),
                GreenChild::Token(_) => (off, off + c.width()),
            });
        }
        off += c.width();
    }
    None
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
pub(crate) fn symbol_child(n: &GreenNode, k: usize) -> Option<&GreenNode> {
    n.children.iter().zip(symbol_indices(n)).find_map(|(c, index)| match c {
        GreenChild::Node(m) if index == Some(k) => Some(m.as_ref()),
        _ => None,
    })
}

/// The one node among `n`'s children (starting at `base`), where every
/// other child is trivia, and where it starts.
pub(crate) fn only_node(n: &GreenNode, base: u32) -> Option<(&GreenNode, u32)> {
    let mut found = None;
    let mut off = base;
    for c in &n.children {
        match c {
            GreenChild::Node(m) if found.is_none() => found = Some((m.as_ref(), off)),
            GreenChild::Node(_) => return None,
            GreenChild::Token(t) if t.trivia => {}
            GreenChild::Token(_) => return None,
        }
        off += c.width();
    }
    found
}

/// The path a reference's name child spells when it is a root alone (a
/// node that begins a path and names nothing after: `Self`, `self`).
pub(crate) fn rooted(n: &GreenNode, k: usize, cfg: &BindingConfig) -> Option<Path> {
    symbol_child(n, k)?;
    Path::of_child(n, k, cfg).filter(|p| p.root.is_some() && p.segments.is_empty())
}

/// The `k`th symbol child of `n`, a node, and where it starts, `n`
/// starting at `base`.
pub(crate) fn symbol_child_at(
    n: &GreenNode,
    k: usize,
    base: u32,
) -> Option<(&Arc<GreenNode>, u32)> {
    let mut off = base;
    for (c, index) in n.children.iter().zip(symbol_indices(n)) {
        if let (GreenChild::Node(m), true) = (c, index == Some(k)) {
            return Some((m, off));
        }
        off += c.width();
    }
    None
}

/// `n`'s child nodes (`n` starting at `base`), each with where it starts.
/// The paths `node` spells, in order: its own, or, where it spells none,
/// those of its child nodes (a list's elements).
fn paths_in(node: &GreenNode, cfg: &BindingConfig, out: &mut Vec<Path>) {
    if let Some(path) = Path::of_node(node, cfg) {
        out.push(path);
        return;
    }
    for c in &node.children {
        if let GreenChild::Node(m) = c {
            paths_in(m, cfg, out);
        }
    }
}

fn child_nodes(n: &GreenNode, base: u32) -> impl Iterator<Item = (&GreenNode, u32)> {
    let mut off = base;
    n.children.iter().filter_map(move |c| {
        let at = off;
        off += c.width();
        match c {
            GreenChild::Node(m) => Some((m.as_ref(), at)),
            GreenChild::Token(_) => None,
        }
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
