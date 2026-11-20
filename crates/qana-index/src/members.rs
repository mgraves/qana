//! A file's member accesses (`c.width()`, `self.cells`, `p.0.x`): each
//! member's name, and what it is a member of as the file writes it, for a
//! project index to type ([`Resolver::member`](crate::Resolver::member)).
//!
//! What a value is, a file says only as far as it is written, and that is
//! what is read here, from the grammar's `@value`, `@member`, `@inits`,
//! `@self_value` and `@typed` annotations:
//!
//! - a binding whose type is written (a parameter, an annotated `let`), and
//!   the value a method is called on (`self`: of the type of the impl
//!   block or trait it stands in);
//! - a binding without a written type, by the value it holds (`let c =
//!   Cell::new();`), read the same way;
//! - what a path names (a constant, a unit struct), and what calling it
//!   gives (a function, a tuple struct);
//! - a member of a value, called or read, and a positional field (`.0`);
//! - a cast's type, a struct literal's;
//! - a value through a reference to it, or what it refers to (`&x`, `*x`,
//!   `(x)`): a member access reaches through references.
//!
//! Anything else is not read: a closure's parameter, a `for` or `match`
//! binding, `x?`, `a[i]`, an arithmetic operator's result, a block's
//! value. Those take inference, which a project index does not do.
//!
//! `Self` (a path whose root is [`Root::SelfType`]) is what the file says
//! where it stands: the type of the impl block around it, or the trait
//! around it, by its name. [`path_at`] says a path that begins with it so.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use qana_grammar::{GreenChild, GreenNode};
use qana_lang::live::LiveDoc;
use qana_lang::EmbeddedLang;
use qana_sem::ValueForm;
use qana_sem::{name_child, symbol_indices, MemberForm, Path, Root, SemDb, SymbolTable, Target};
use serde::{Deserialize, Serialize};

use crate::units::{self, only_node, symbol_child_at, FileUnits, Grammar};
use crate::{outside_bodies, TypeRef, URI};

/// A member of a value, named after it (`c.width()`, `self.cells`). See
/// the [module docs](self).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberAccess {
    /// The member's name.
    pub name: String,
    /// Its name's byte span.
    pub span: (u32, u32),
    /// Whether it is read (a field) or called (a method).
    pub kind: MemberKind,
    /// The value it is a member of.
    pub receiver: Receiver,
    /// The names of the units with a body around it, outermost first: where
    /// what is written about the receiver is read from.
    pub units: Vec<String>,
    /// The generic parameters of the declarations and impl blocks around
    /// it: a type written there that is one of them is that parameter, not
    /// a type a path names.
    pub generics: Vec<String>,
}

/// How a member is used. See [`MemberAccess::kind`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MemberKind {
    /// Read: a field.
    Field,
    /// Called: a method.
    Method,
}

/// A value, as the file writes it. See the [module docs](self).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Receiver {
    /// A value of a type the file writes (a binding's, a parameter's,
    /// `self`'s, a cast's, a struct literal's), read where the access
    /// stands. `Self` is already what it is there.
    Typed(TypeRef),
    /// What a path names, as a value: a constant, a static, a unit struct,
    /// an enum's unit variant. Called, a function, a tuple struct or a
    /// tuple variant.
    Path(Path),
    /// What calling a value gives.
    Call(Box<Receiver>),
    /// A member of a value.
    Member { of: Box<Receiver>, name: String, kind: MemberKind },
    /// A positional field of a value (`.0`).
    Position { of: Box<Receiver>, index: u32 },
    /// A value whose type the file does not write.
    Unknown,
}

/// The member accesses in `text`, a file of `lang`'s language, in the
/// text's order.
pub fn members(lang: &EmbeddedLang, text: &str) -> Vec<MemberAccess> {
    let session = lang.session(text);
    let tree = session.tree().expect("parsing is total").clone();
    let mut db = SemDb::new(lang.binding.clone());
    db.set_types(lang.types.clone());
    db.set_macro_bodies(&lang.macros);
    db.set_tree(URI, tree.clone());
    let symbols = db.symbols(URI);
    let resolutions = db.resolve(URI);
    let reader = Reader::new(lang, &tree, &symbols, &resolutions);
    let mut found = Vec::new();
    reader.collect(&tree, 0, &Cx::default(), &mut found);
    found.sort_by_key(|a| a.span);
    found
}

/// The member access whose name holds byte `offset` of `doc`, if one does.
pub fn member_at(doc: &mut LiveDoc, offset: u32) -> Option<MemberAccess> {
    // Whether a member's name is there, read on the way down to it, before
    // the file and its binding are read as a whole.
    let (lang, tree) = doc.tree()?;
    if !names_a_member(&Grammar::new(&lang.binding, &lang.outline), tree, offset) {
        return None;
    }
    let (lang, tree, symbols, resolutions) = doc.bound()?;
    let reader = Reader::new(lang, tree, &symbols, &resolutions);
    let mut cx = Cx::default();
    let mut node: &GreenNode = tree;
    let mut base = 0;
    loop {
        cx = reader.enter(node, base, &cx);
        if let Some(access) = reader.access(node, base, &cx) {
            if access.span.0 <= offset && offset < access.span.1 {
                return Some(access);
            }
        }
        let (child, at) = child_holding(node, base, offset)?;
        node = child;
        base = at;
    }
}

/// Where the reference at byte `offset` of `doc` stands and the path it
/// spells, as [`LiveDoc::path_at`] says, with a path that begins with
/// `Self` said from the type that is there: the impl block's around it, or
/// the trait's. `None` where no reference's name is at `offset`, and for a
/// path that begins with `Self` where the type is no path (an impl block of
/// `[T]`).
pub fn path_at(doc: &mut LiveDoc, offset: u32) -> Option<(Vec<String>, Path, bool)> {
    let (units, path, imported) = doc.path_at(offset)?;
    if path.root != Some(Root::SelfType) {
        return Some((units, path, imported));
    }
    let (lang, tree, symbols, resolutions) = doc.bound()?;
    let reader = Reader::new(lang, tree, &symbols, &resolutions);
    let path = reader.said_here(tree, offset, &path)?;
    Some((units, path, imported))
}

/// Says a path that begins with `Self` from the type it stands in, for
/// a file's references ([`references`](crate::references)).
pub(crate) struct SelfPaths<'a>(Reader<'a>);

impl<'a> SelfPaths<'a> {
    pub(crate) fn new(
        lang: &'a EmbeddedLang,
        tree: &GreenNode,
        symbols: &SymbolTable,
        resolutions: &[Target],
    ) -> Self {
        Self(Reader::new(lang, tree, symbols, resolutions))
    }

    /// `path`, which stands at byte `offset` of `tree`, said from the type
    /// `Self` is there; `None` where that type is no path.
    pub(crate) fn path(&self, tree: &GreenNode, offset: u32, path: &Path) -> Option<Path> {
        self.0.said_here(tree, offset, path)
    }
}

/// Whether byte `offset` of `tree` is in the name of a member access's
/// member (`@member(field|method, n)`).
fn names_a_member(g: &Grammar, tree: &GreenNode, offset: u32) -> bool {
    let (mut node, mut base) = (tree, 0);
    loop {
        if let Some(MemberForm::Field(k) | MemberForm::Method(k)) =
            g.roles_of(node).and_then(|r| r.member_name)
        {
            if let Some((name, off)) = name_child(node, k, g.cfg) {
                let start = base + off;
                if start <= offset && offset < start + name.len() as u32 {
                    return true;
                }
            }
        }
        match child_holding(node, base, offset) {
            Some((child, at)) => (node, base) = (child, at),
            None => return false,
        }
    }
}

/// What encloses the node a reading is at.
#[derive(Clone, Default)]
struct Cx {
    /// The generic parameters of the declarations and impl blocks around.
    generics: Rc<Vec<String>>,
    /// The type of the impl block around, for the members scope it opens.
    impl_type: Option<Rc<TypeRef>>,
    /// The name of the definition around, for a members scope it opens (a
    /// trait's).
    owner: Option<Rc<str>>,
    /// What `Self` is here.
    self_type: Option<Rc<TypeRef>>,
}

/// Reads a file's member accesses.
struct Reader<'a> {
    g: Grammar<'a>,
    read: FileUnits,
    /// The name spans of the declarations the file offers: a value that is
    /// one is what its path names, across the project (as an import's name
    /// is).
    declared: HashSet<(u32, u32)>,
    /// What each reference binds within the file, by its name's span: a
    /// definition's name span, or nothing.
    bound: HashMap<(u32, u32), Option<(u32, u32)>>,
    /// The references' name spans, sorted.
    refs: Vec<(u32, u32)>,
}

impl<'a> Reader<'a> {
    fn new(
        lang: &'a EmbeddedLang,
        tree: &GreenNode,
        symbols: &SymbolTable,
        resolutions: &[Target],
    ) -> Self {
        let read = units::read(tree, &lang.binding, &lang.outline);
        let declared = outside_bodies(tree, &lang.outline)
            .into_iter()
            .map(|(symbol, _)| symbol.selection)
            .collect();
        let bound = symbols
            .refs
            .iter()
            .zip(resolutions)
            .map(|(r, target)| {
                let def = match target {
                    Target::Local { def } => symbols.defs.get(*def).map(|d| d.span),
                    _ => None,
                };
                (r.span, def)
            })
            .collect();
        let mut refs: Vec<(u32, u32)> = read.refs.keys().copied().collect();
        refs.sort();
        Self { g: Grammar::new(&lang.binding, &lang.outline), read, declared, bound, refs }
    }

    /// The context `n`'s children stand in.
    fn enter(&self, n: &GreenNode, base: u32, cx: &Cx) -> Cx {
        let mut inner = cx.clone();
        let Some(roles) = self.g.roles_of(n) else { return inner };
        if let Some(k) = roles.def {
            if let Some((name, off)) = name_child(n, k, self.g.cfg) {
                let span = (base + off, base + off + name.len() as u32);
                if let Some(place) = self.read.defs.get(&span).filter(|p| !p.generics.is_empty()) {
                    Rc::make_mut(&mut inner.generics).extend(place.generics.iter().cloned());
                }
                inner.owner = Some(Rc::from(name));
            }
        }
        if let Some((ty, _)) = roles.implementation {
            if let Some(generics) =
                self.read.impl_generics.get(&(base, base + n.width)).filter(|g| !g.is_empty())
            {
                Rc::make_mut(&mut inner.generics).extend(generics.iter().cloned());
            }
            inner.impl_type = self
                .g
                .type_in(n, ty)
                .or_else(|| {
                    Path::of_child(n, ty, self.g.cfg).map(|path| TypeRef::Path {
                        path,
                        args: Vec::new(),
                        bindings: Vec::new(),
                    })
                })
                .map(Rc::new);
        }
        if roles.scope == Some(true) {
            // A members scope: an impl block's, whose type `Self` is; or a
            // definition's (a trait's), which `Self` is had by.
            inner.self_type = match inner.impl_type.take() {
                Some(ty) => Some(ty),
                None => inner.owner.as_ref().map(|name| {
                    Rc::new(TypeRef::Path {
                        path: Path { root: None, segments: vec![name.to_string()] },
                        args: Vec::new(),
                        bindings: Vec::new(),
                    })
                }),
            };
        }
        inner
    }

    /// `path`, which stands at byte `offset` of `tree`, said from the type
    /// `Self` is there.
    fn said_here(&self, tree: &GreenNode, offset: u32, path: &Path) -> Option<Path> {
        let mut cx = Cx::default();
        let mut node: &GreenNode = tree;
        let mut base = 0;
        while let Some((child, at)) = child_holding(node, base, offset) {
            cx = self.enter(node, base, &cx);
            node = child;
            base = at;
        }
        self_path(path, cx.self_type.as_deref())
    }

    /// Every member access in `n`'s subtree, `n` starting at `base`.
    fn collect(&self, n: &GreenNode, base: u32, cx: &Cx, out: &mut Vec<MemberAccess>) {
        let cx = self.enter(n, base, cx);
        out.extend(self.access(n, base, &cx));
        let mut off = base;
        for c in &n.children {
            if let GreenChild::Node(m) = c {
                self.collect(m, off, &cx, out);
            }
            off += c.width();
        }
    }

    /// The member access `n` is, if it is one whose member is named (a
    /// field read or a method called).
    fn access(&self, n: &GreenNode, base: u32, cx: &Cx) -> Option<MemberAccess> {
        let Some(ValueForm::Member { base: of, position, member }) =
            self.g.roles_of(n).and_then(|r| r.value)
        else {
            return None;
        };
        let (member_node, member_at) = symbol_child_at(n, member, base)?;
        let (kind, k) = match self.g.roles_of(member_node).and_then(|r| r.member_name)? {
            MemberForm::Field(k) => (MemberKind::Field, k),
            MemberForm::Method(k) => (MemberKind::Method, k),
            MemberForm::Position(_) => return None,
        };
        let (name, off) = name_child(member_node, k, self.g.cfg)?;
        let span = (member_at + off, member_at + off + name.len() as u32);
        let mut receiver = match symbol_child_at(n, of, base) {
            Some((value, at)) => self.value(value, at, cx, &mut Vec::new()),
            None => Receiver::Unknown,
        };
        if let Some(i) = position {
            receiver = match token_text(n, i).and_then(positions) {
                Some(indexes) => indexes
                    .into_iter()
                    .fold(receiver, |of, index| Receiver::Position { of: Box::new(of), index }),
                None => Receiver::Unknown,
            };
        }
        Some(MemberAccess {
            name,
            span,
            kind,
            receiver,
            units: self.units_around(span),
            generics: cx.generics.as_ref().clone(),
        })
    }

    /// The value `n`, starting at `base`, is, as written. `visiting` holds
    /// the bindings whose values are being read, so that none is read
    /// inside its own.
    fn value(&self, n: &GreenNode, base: u32, cx: &Cx, visiting: &mut Vec<(u32, u32)>) -> Receiver {
        let child = |k: usize| symbol_child_at(n, k, base);
        let Some(form) = self.g.roles_of(n).and_then(|r| r.value) else {
            // A node that only wraps another is the value it wraps.
            return match only_node(n, base) {
                Some((m, at)) => self.value(m, at, cx, visiting),
                None => Receiver::Unknown,
            };
        };
        match form {
            ValueForm::Path(p) => match child(p) {
                Some((m, at)) => self.path_value(m, at, cx, visiting),
                None => Receiver::Unknown,
            },
            ValueForm::Call(c) => match child(c) {
                Some((m, at)) => Receiver::Call(Box::new(self.value(m, at, cx, visiting))),
                None => Receiver::Unknown,
            },
            ValueForm::Member { base: of, position, member } => {
                let mut receiver = match child(of) {
                    Some((m, at)) => self.value(m, at, cx, visiting),
                    None => return Receiver::Unknown,
                };
                if let Some(i) = position {
                    let Some(indexes) = token_text(n, i).and_then(positions) else {
                        return Receiver::Unknown;
                    };
                    for index in indexes {
                        receiver = Receiver::Position { of: Box::new(receiver), index };
                    }
                }
                let Some((member_node, _)) = child(member) else { return Receiver::Unknown };
                let named = |k: usize, kind: MemberKind, receiver: Receiver| match name_child(
                    member_node,
                    k,
                    self.g.cfg,
                ) {
                    Some((name, _)) => Receiver::Member { of: Box::new(receiver), name, kind },
                    None => Receiver::Unknown,
                };
                match self.g.roles_of(member_node).and_then(|r| r.member_name) {
                    Some(MemberForm::Field(k)) => named(k, MemberKind::Field, receiver),
                    Some(MemberForm::Method(k)) => named(k, MemberKind::Method, receiver),
                    Some(MemberForm::Position(i)) => {
                        match token_text(member_node, i).and_then(positions) {
                            Some(indexes) => indexes.into_iter().fold(receiver, |of, index| {
                                Receiver::Position { of: Box::new(of), index }
                            }),
                            None => Receiver::Unknown,
                        }
                    }
                    None => Receiver::Unknown,
                }
            }
            ValueForm::Same(e) => match child(e) {
                Some((m, at)) => self.value(m, at, cx, visiting),
                None => Receiver::Unknown,
            },
            ValueForm::Typed(t) => match self.g.type_in(n, t) {
                Some(ty) => typed(&ty, cx),
                None => Receiver::Unknown,
            },
            ValueForm::Struct(p) => match Path::of_child(n, p, self.g.cfg) {
                Some(path) => {
                    typed(&TypeRef::Path { path, args: Vec::new(), bindings: Vec::new() }, cx)
                }
                None => Receiver::Unknown,
            },
        }
    }

    /// The value the path node `n`, starting at `base`, names: what its
    /// last name binds within the file (a binding, by what is written of
    /// it), or else the path, for the project to resolve.
    fn path_value(
        &self,
        n: &GreenNode,
        base: u32,
        cx: &Cx,
        visiting: &mut Vec<(u32, u32)>,
    ) -> Receiver {
        let end = base + n.width;
        let last = self.refs.partition_point(|r| r.0 < end);
        let Some(&span) = self.refs[..last].iter().rev().find(|r| r.0 >= base && r.1 <= end) else {
            return Receiver::Unknown;
        };
        let Some(place) = self.read.refs.get(&span) else { return Receiver::Unknown };
        // A binding within the file, by what is written of it; a
        // declaration the file offers, or an import's name, is what its path
        // names across the project.
        if let Some(Some(def)) = self.bound.get(&span) {
            if !self.declared.contains(def) && !self.read.import_defs.contains(def) {
                return self.binding_value(*def, cx, visiting);
            }
        }
        match place.path.as_ref().and_then(|path| self_path(path, cx.self_type.as_deref())) {
            Some(path) => Receiver::Path(path),
            None => Receiver::Unknown,
        }
    }

    /// The value the binding defined at `def` holds, as written: its type,
    /// or the value it was given.
    fn binding_value(&self, def: (u32, u32), cx: &Cx, visiting: &mut Vec<(u32, u32)>) -> Receiver {
        let Some(place) = self.read.defs.get(&def) else { return Receiver::Unknown };
        if let Some(ty) = &place.ty {
            return typed(ty, cx);
        }
        if place.self_value {
            return match &cx.self_type {
                Some(ty) => Receiver::Typed(ty.as_ref().clone()),
                None => Receiver::Unknown,
            };
        }
        match &place.init {
            Some((node, at)) if !visiting.contains(&def) => {
                visiting.push(def);
                let value = self.value(node, *at, cx, visiting);
                visiting.pop();
                value
            }
            _ => Receiver::Unknown,
        }
    }

    /// The names of the units with a body around `span`, outermost first.
    fn units_around(&self, span: (u32, u32)) -> Vec<String> {
        let mut names = Vec::new();
        let mut at = self.read.unit_at(span);
        while let Some(unit) = at {
            let (u, _, _) = &self.read.units[unit as usize];
            names.push(u.name.clone());
            at = u.parent;
        }
        names.reverse();
        names
    }
}

/// A value of type `ty`, written where `cx` is: `Self` said as what it is.
fn typed(ty: &TypeRef, cx: &Cx) -> Receiver {
    match with_self(ty, cx.self_type.as_deref()) {
        Some(ty) => Receiver::Typed(ty),
        None => Receiver::Unknown,
    }
}

/// `ty`, with each type that is `Self`, or begins with it, said from
/// `self_type`; `None` where one is and `self_type` cannot say it.
fn with_self(ty: &TypeRef, self_type: Option<&TypeRef>) -> Option<TypeRef> {
    let each = |types: &[TypeRef]| -> Option<Vec<TypeRef>> {
        types.iter().map(|t| with_self(t, self_type)).collect()
    };
    Some(match ty {
        TypeRef::Path { path, args, bindings } => {
            if path.root == Some(Root::SelfType) && path.segments.is_empty() {
                return self_type.cloned();
            }
            let bindings = bindings
                .iter()
                .map(|(name, t)| Some((name.clone(), with_self(t, self_type)?)))
                .collect::<Option<Vec<_>>>()?;
            TypeRef::Path { path: self_path(path, self_type)?, args: each(args)?, bindings }
        }
        TypeRef::Ref(t) => TypeRef::Ref(Box::new(with_self(t, self_type)?)),
        TypeRef::Slice(t) => TypeRef::Slice(Box::new(with_self(t, self_type)?)),
        TypeRef::Array(t) => TypeRef::Array(Box::new(with_self(t, self_type)?)),
        TypeRef::Tuple(types) => TypeRef::Tuple(each(types)?),
        TypeRef::Dyn(types) => TypeRef::Dyn(each(types)?),
        TypeRef::Impl(types) => TypeRef::Impl(each(types)?),
        TypeRef::Other => TypeRef::Other,
    })
}

/// `path`, where it begins with `Self`, said from `self_type`'s path: `Self::new`
/// in `impl Grid<T>` is `Grid::new`. `None` where `self_type` is no path.
fn self_path(path: &Path, self_type: Option<&TypeRef>) -> Option<Path> {
    if path.root != Some(Root::SelfType) {
        return Some(path.clone());
    }
    let own = self_type?.path()?;
    let mut segments = own.segments.clone();
    segments.extend(path.segments.iter().cloned());
    Some(Path { root: own.root, segments })
}

/// The child node of `n` (starting at `base`) that holds byte `offset`,
/// and where it starts.
fn child_holding(n: &GreenNode, base: u32, offset: u32) -> Option<(&GreenNode, u32)> {
    let mut off = base;
    for c in &n.children {
        let end = off + c.width();
        if let GreenChild::Node(m) = c {
            if off <= offset && offset < end {
                return Some((m.as_ref(), off));
            }
        }
        off = end;
    }
    None
}

/// The text of `n`'s `k`th symbol child, a token.
fn token_text(n: &GreenNode, k: usize) -> Option<&str> {
    n.children.iter().zip(symbol_indices(n)).find_map(|(c, index)| match c {
        GreenChild::Token(t) if index == Some(k) => Some(t.text.as_str()),
        _ => None,
    })
}

/// The positional fields a token names, one inside the other: `0` is the
/// first; `0.1`, which a lexer reads as one number, the first's second;
/// `0.`, the first.
fn positions(text: &str) -> Option<Vec<u32>> {
    let indexes: Option<Vec<u32>> =
        text.split('.').filter(|part| !part.is_empty()).map(|part| part.parse().ok()).collect();
    indexes.filter(|indexes| !indexes.is_empty())
}

#[cfg(test)]
mod tests {
    use super::positions;

    #[test]
    fn a_tokens_positions_are_its_numbers() {
        assert_eq!(positions("0"), Some(vec![0]));
        assert_eq!(positions("0."), Some(vec![0]));
        assert_eq!(positions("1.2"), Some(vec![1, 2]));
        assert_eq!(positions("1e3"), None);
        assert_eq!(positions(""), None);
    }
}
