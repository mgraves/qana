//! A member access's value, typed from what declarations write, and the
//! member it names: [`Resolver::member`].
//!
//! A value's type is read, never inferred. A [`Receiver`] says what the
//! file writes of the value; from there each step is a declaration's
//! written type: a binding's or a cast's, a constant's, what a function's
//! call gives (its return type, `Self` read as the type it was found on),
//! a field's. A reference is reached through, as a member access does. A
//! type that is a generic parameter, an associated type (`Self::Item`), a
//! slice, an array or a language's builtin (`str`, `u8`) is not followed
//! further, nor is what calling a closure gives: what follows escapes.
//!
//! A type's members, for a value of it:
//!
//! - a field: those declared inside it;
//! - a method: the functions of its inherent impl blocks; or else, of each
//!   impl block of a trait that is in scope where the access stands, the
//!   block's function of that name, or the trait's own where the block
//!   has none (a provided method); and of each trait it derives that is in
//!   scope, the trait's. Where several blocks of one trait have one (a
//!   trait with parameters: `From<u8>`, `From<char>`), the trait's own is
//!   an answer too. A block of a trait the project does not hold has its
//!   function as an answer: nothing says that trait is out of scope.
//!
//! A path's member of a type (`Cell::new`) is found the same way, an
//! item of any kind. A trait taken as a type (`dyn A + B`, `impl A`, `Self`
//!   in a trait) has its own items and its supertraits'.
//!
//! A trait is in scope in a module when a name the module has binds it (an
//! item, an import, a glob's, a rename's), when its crate's preludes do,
//! or when an import brings it without a name (`use a::T as _;`), the
//! module's own or a glob's or a prelude's.

use std::collections::{HashMap, HashSet};
use std::path::{Path as FilePath, PathBuf};
use std::rc::Rc;

use super::{Binding, Declarations, Module, Resolution, Resolver, Start, Target};
use crate::{Declaration, MemberAccess, MemberKind, Path, Receiver, Root, TypeRef};

/// The outline kinds that say what a declaration is (`qana_lang`'s
/// vocabulary): a function's call gives its return type; a struct is a type
/// (an alias, its written type); a class is a type known by its items (a
/// trait); a constant without a type is a value of the type around it (an
/// enum's variant); a field is read.
const FUNCTION: &str = "function";
const STRUCT: &str = "struct";
const CLASS: &str = "class";
const CONSTANT: &str = "constant";
const FIELD: &str = "field";

/// How far a value's typing follows declarations before it stops: aliases
/// of aliases, bindings of bindings.
const DEPTH: usize = 64;

/// A value's type, as far as what is written says.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Ty {
    /// A type a declaration declares (a struct, an enum, a union; a trait,
    /// taken as a type), by (file, declaration index).
    Declared((u32, u32)),
    /// A type known by its traits (`dyn A + B`, `impl A`): theirs.
    Traits(Vec<(u32, u32)>),
    /// A tuple, of the types written where they were.
    Tuple(Vec<TypeRef>, Rc<Scope>),
}

/// A trait a type's impl blocks implement, in scope: the items of that
/// name its blocks have, and whether one of its blocks has none (the
/// trait provides it).
struct TraitItems {
    of: (u32, u32),
    items: Vec<(u32, u32)>,
    provided: bool,
}

/// Where a type is written: the module its paths are read from, the
/// generic parameters there, and what `Self` is there.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Scope {
    at: Module,
    generics: Vec<String>,
    self_type: Vec<Ty>,
}

impl Resolver {
    /// What the member `access` names, from where it stands in `file`: the
    /// declarations its value's type has under its name. A member is never
    /// said to name nothing: it escapes from segment 0 where its value's
    /// type is not written, and from segment 1 where the type is known and
    /// no summary holds the member (a derive's or a macro's, a builtin's,
    /// one reached through `Deref`). See the [module docs](self).
    pub fn member(&self, file: &FilePath, access: &MemberAccess) -> Resolution {
        let Some(&id) = self.ids.get(file) else {
            return Resolution::Escapes { segment: 0 };
        };
        let mut declarations: Vec<(PathBuf, u32)> = Vec::new();
        let mut typed = false;
        for (krate, _) in self.graph.places(file) {
            let at = Module { krate, file: id, unit: self.unit_named(id, &access.units) };
            let scope =
                Rc::new(Scope { at, generics: access.generics.clone(), self_type: Vec::new() });
            let types = self.value_types(&scope, &access.receiver, 0);
            typed |= !types.is_empty();
            for ty in &types {
                for (file, index) in self.members_named(at, ty, &access.name, access.kind) {
                    declarations.push((self.files[file as usize].clone(), index));
                }
            }
        }
        if declarations.is_empty() {
            return Resolution::Escapes { segment: usize::from(typed) };
        }
        declarations.sort();
        declarations.dedup();
        Resolution::Found { declarations, modules: Vec::new() }
    }

    // ------------------------------------------------------------------
    // Values
    // ------------------------------------------------------------------

    /// The types of the value `receiver`, written where `scope` is.
    fn value_types(&self, scope: &Rc<Scope>, receiver: &Receiver, depth: usize) -> Vec<Ty> {
        if depth > DEPTH {
            return Vec::new();
        }
        let mut types = Vec::new();
        match receiver {
            Receiver::Typed(ty) => types = self.types_of(scope, ty, depth + 1),
            Receiver::Path(path) if !from_parameter(scope, path) => {
                for d in self.declarations_at(scope.at, path) {
                    types.extend(self.value_of(d, depth + 1));
                }
            }
            Receiver::Path(_) => {}
            Receiver::Call(callee) => {
                // Calling a path; what calling any other value gives (a
                // closure's, a field's) is not written, nor what a generic
                // parameter's function gives.
                if let Receiver::Path(path) = callee.as_ref() {
                    if !from_parameter(scope, path) {
                        for d in self.declarations_at(scope.at, path) {
                            types.extend(self.call_of(d, depth + 1));
                        }
                    }
                }
            }
            Receiver::Member { of, name, kind } => {
                for ty in self.value_types(scope, of, depth + 1) {
                    for d in self.members_named(scope.at, &ty, name, *kind) {
                        let written = match kind {
                            MemberKind::Field => self.declaration(d).ty.clone(),
                            MemberKind::Method => self.declaration(d).returns.clone(),
                        };
                        if let Some(written) = written {
                            for at in self.scopes_of(d, vec![ty.clone()]) {
                                types.extend(self.types_of(&at, &written, depth + 1));
                            }
                        }
                    }
                }
            }
            Receiver::Position { of, index } => {
                for ty in self.value_types(scope, of, depth + 1) {
                    match &ty {
                        Ty::Declared(d) => {
                            let Some(written) =
                                self.declaration(*d).positional.get(*index as usize)
                            else {
                                continue;
                            };
                            for at in self.scopes_of(*d, vec![ty.clone()]) {
                                types.extend(self.types_of(&at, written, depth + 1));
                            }
                        }
                        Ty::Tuple(elements, at) => {
                            if let Some(written) = elements.get(*index as usize) {
                                types.extend(self.types_of(at, written, depth + 1));
                            }
                        }
                        Ty::Traits(_) => {}
                    }
                }
            }
            Receiver::Unknown => {}
        }
        dedup(types)
    }

    /// The type of declaration `d` as a value: a constant's or a static's
    /// written type; a unit struct; an enum's unit variant, of the enum.
    fn value_of(&self, d: (u32, u32), depth: usize) -> Vec<Ty> {
        let declaration = self.declaration(d);
        if let Some(written) = &declaration.ty {
            let self_type = self.self_of(d, depth);
            let mut types = Vec::new();
            for at in self.scopes_of(d, self_type) {
                types.extend(self.types_of(&at, written, depth + 1));
            }
            return types;
        }
        match declaration.kind.as_str() {
            STRUCT => vec![Ty::Declared(d)],
            CONSTANT => self.owner_of(d).map(Ty::Declared).into_iter().collect(),
            _ => Vec::new(),
        }
    }

    /// What calling declaration `d` gives: a function's written return
    /// type, `Self` read as the type it is a member of; a tuple struct; a
    /// tuple variant, of its enum.
    fn call_of(&self, d: (u32, u32), depth: usize) -> Vec<Ty> {
        let declaration = self.declaration(d);
        match declaration.kind.as_str() {
            FUNCTION => {
                let Some(written) = &declaration.returns else { return Vec::new() };
                let self_type = self.self_of(d, depth);
                let mut types = Vec::new();
                for at in self.scopes_of(d, self_type) {
                    types.extend(self.types_of(&at, written, depth + 1));
                }
                types
            }
            STRUCT if declaration.ty.is_none() => vec![Ty::Declared(d)],
            CONSTANT if declaration.ty.is_none() => {
                self.owner_of(d).map(Ty::Declared).into_iter().collect()
            }
            _ => Vec::new(),
        }
    }

    /// What `Self` is in declaration `d`, reached by a path: its impl
    /// block's type, or the trait it is an item of.
    fn self_of(&self, d: (u32, u32), depth: usize) -> Vec<Ty> {
        let declaration = self.declaration(d);
        if let Some(block) = declaration.impl_block {
            let summary = &self.summaries[d.0 as usize];
            let Some(of) = summary.offers().impls().get(block as usize) else { return Vec::new() };
            let Some(written) = &of.self_type else { return Vec::new() };
            let mut types = Vec::new();
            for (krate, _) in self.graph.places(&self.files[d.0 as usize]) {
                let at = Rc::new(Scope {
                    at: Module { krate, file: d.0, unit: of.unit },
                    generics: of.generics.clone(),
                    self_type: Vec::new(),
                });
                types.extend(self.types_of(&at, written, depth + 1));
            }
            return types;
        }
        match self.owner_of(d) {
            Some(owner) if self.declaration(owner).kind == CLASS => vec![Ty::Declared(owner)],
            _ => Vec::new(),
        }
    }

    // ------------------------------------------------------------------
    // Types
    // ------------------------------------------------------------------

    /// The types `written` names, where `scope` is.
    fn types_of(&self, scope: &Rc<Scope>, written: &TypeRef, depth: usize) -> Vec<Ty> {
        if depth > DEPTH {
            return Vec::new();
        }
        match written {
            TypeRef::Ref(inner) => self.types_of(scope, inner, depth + 1),
            TypeRef::Path { path, .. } => {
                if path.root == Some(Root::SelfType) {
                    // `Self`; an associated type of it is not followed.
                    return if path.segments.is_empty() {
                        scope.self_type.clone()
                    } else {
                        Vec::new()
                    };
                }
                if path.segments.len() == 1 && from_parameter(scope, path) {
                    // A generic parameter.
                    return Vec::new();
                }
                let mut types = Vec::new();
                for d in self.declarations_at(scope.at, path) {
                    types.extend(self.type_of(d, depth + 1));
                }
                dedup(types)
            }
            TypeRef::Tuple(elements) => vec![Ty::Tuple(elements.clone(), scope.clone())],
            TypeRef::Dyn(traits) | TypeRef::Impl(traits) => {
                let mut found = Vec::new();
                for t in traits.iter().filter_map(TypeRef::path) {
                    for d in self.declarations_at(scope.at, t) {
                        if self.declaration(d).kind == CLASS && !found.contains(&d) {
                            found.push(d);
                        }
                    }
                }
                if found.is_empty() {
                    Vec::new()
                } else {
                    vec![Ty::Traits(found)]
                }
            }
            TypeRef::Slice(_) | TypeRef::Array(_) | TypeRef::Other => Vec::new(),
        }
    }

    /// The type declaration `d` is, named as a type: a struct, an enum, a
    /// union or a trait itself; an alias, the type it is of.
    fn type_of(&self, d: (u32, u32), depth: usize) -> Vec<Ty> {
        let declaration = self.declaration(d);
        match declaration.kind.as_str() {
            STRUCT => match &declaration.ty {
                Some(written) => {
                    let self_type = self.self_of(d, depth);
                    let mut types = Vec::new();
                    for at in self.scopes_of(d, self_type) {
                        types.extend(self.types_of(&at, written, depth + 1));
                    }
                    types
                }
                None => vec![Ty::Declared(d)],
            },
            CLASS => vec![Ty::Declared(d)],
            _ => Vec::new(),
        }
    }

    // ------------------------------------------------------------------
    // Members
    // ------------------------------------------------------------------

    /// The members named `name` of a value of type `ty`, used as `kind`
    /// says, for an access in module `at`.
    fn members_named(&self, at: Module, ty: &Ty, name: &str, kind: MemberKind) -> Vec<(u32, u32)> {
        match (ty, kind) {
            (Ty::Declared(d), MemberKind::Field) => self
                .declared_inside(d.0, d.1, name)
                .into_iter()
                .filter_map(|t| t.declaration)
                .filter(|&f| self.declaration(f).kind == FIELD)
                .collect(),
            (Ty::Declared(d), MemberKind::Method) if self.declaration(*d).kind == CLASS => {
                self.trait_items(*d, name, &mut HashSet::new())
            }
            (Ty::Declared(d), MemberKind::Method) => {
                self.impl_members(at, *d, name, Some(FUNCTION))
            }
            (Ty::Traits(traits), MemberKind::Method) => {
                let mut visited = HashSet::new();
                let mut found = Vec::new();
                for &t in traits {
                    for item in self.trait_items(t, name, &mut visited) {
                        if !found.contains(&item) {
                            found.push(item);
                        }
                    }
                }
                found
            }
            (Ty::Traits(_), MemberKind::Field) | (Ty::Tuple(..), _) => Vec::new(),
        }
    }

    /// The items named `name` of type `d`'s impl blocks, of the outline
    /// kind `only` where given (a method's: a function), for a use in module
    /// `at`: its inherent impl blocks'; or else, of each trait it implements
    /// or derives that is in scope in `at`, the impl block's, or the
    /// trait's own. See the [module docs](self).
    pub(super) fn impl_members(
        &self,
        at: Module,
        d: (u32, u32),
        name: &str,
        only: Option<&str>,
    ) -> Vec<(u32, u32)> {
        let blocks = self.impls_of(d.0, d.1);
        let functions_of = |block: (u32, u32)| -> Vec<(u32, u32)> {
            self.summaries[block.0 as usize]
                .offers()
                .declarations()
                .iter()
                .enumerate()
                .filter(|(_, f)| {
                    f.impl_block == Some(block.1)
                        && f.name == name
                        && only.is_none_or(|kind| f.kind == kind)
                })
                .map(|(i, _)| (block.0, i as u32))
                .collect()
        };
        let mut inherent = Vec::new();
        for &block in blocks.iter() {
            if self.summaries[block.0 as usize].offers().impls()[block.1 as usize]
                .trait_path
                .is_none()
            {
                inherent.extend(functions_of(block));
            }
        }
        if !inherent.is_empty() {
            return inherent;
        }

        // Each trait in scope, with its blocks' items of that name, and
        // whether one of its blocks has none (it provides it).
        let mut by_trait: Vec<TraitItems> = Vec::new();
        let mut note = |t: (u32, u32), items: Vec<(u32, u32)>| {
            let provided = items.is_empty();
            match by_trait.iter_mut().find(|seen| seen.of == t) {
                Some(seen) => {
                    seen.items.extend(items);
                    seen.provided |= provided;
                }
                None => by_trait.push(TraitItems { of: t, items, provided }),
            }
        };
        // The items of blocks of traits the project does not hold: nothing
        // says those traits are out of scope.
        let mut unjudged = Vec::new();
        for &block in blocks.iter() {
            if self.summaries[block.0 as usize].offers().impls()[block.1 as usize]
                .trait_path
                .is_none()
            {
                continue;
            }
            let functions = functions_of(block);
            let traits = self.impl_traits(block);
            if traits.is_empty() {
                unjudged.extend(functions);
                continue;
            }
            for &t in traits.iter() {
                if self.trait_in_scope(at, t) {
                    note(t, functions.clone());
                }
            }
        }
        for t in self.derived(d) {
            if self.trait_in_scope(at, t) {
                note(t, Vec::new());
            }
        }

        let mut found = unjudged;
        for TraitItems { of: t, items: functions, provided } in by_trait {
            if provided || functions.len() > 1 {
                let own = self.trait_items(t, name, &mut HashSet::new());
                found.extend(
                    own.into_iter().filter(|&item| {
                        only.is_none_or(|kind| self.declaration(item).kind == kind)
                    }),
                );
            }
            found.extend(functions);
        }
        found.sort();
        found.dedup();
        found
    }

    /// The items named `name` of trait `t`, and of its supertraits.
    fn trait_items(
        &self,
        t: (u32, u32),
        name: &str,
        visited: &mut HashSet<(u32, u32)>,
    ) -> Vec<(u32, u32)> {
        if !visited.insert(t) {
            return Vec::new();
        }
        let mut found: Vec<(u32, u32)> = self
            .declared_inside(t.0, t.1, name)
            .into_iter()
            .filter_map(|target| target.declaration)
            .collect();
        for written in &self.declaration(t).bounds {
            let Some(path) = written.path() else { continue };
            for at in self.scopes_of(t, Vec::new()) {
                for supertrait in self.declarations_at(at.at, path) {
                    if self.declaration(supertrait).kind == CLASS {
                        found.extend(self.trait_items(supertrait, name, visited));
                    }
                }
            }
        }
        found
    }

    /// The traits impl block `block` implements: its trait's path, read
    /// where the block is.
    fn impl_traits(&self, block: (u32, u32)) -> Declarations {
        if let Some(found) = self.impl_traits.borrow().get(&block) {
            return found.clone();
        }
        let of = &self.summaries[block.0 as usize].offers().impls()[block.1 as usize];
        let mut found = Vec::new();
        if let Some(path) = &of.trait_path {
            for (krate, _) in self.graph.places(&self.files[block.0 as usize]) {
                let at = Module { krate, file: block.0, unit: of.unit };
                for d in self.declarations_at(at, path) {
                    if self.declaration(d).kind == CLASS && !found.contains(&d) {
                        found.push(d);
                    }
                }
            }
        }
        let found = Rc::new(found);
        if self.depth.get() == 0 {
            // Found while no lookup is under way: nothing it met was
            // provisional.
            self.impl_traits.borrow_mut().insert(block, found.clone());
        }
        found
    }

    /// The traits type `d` derives, its derive paths read where it is.
    fn derived(&self, d: (u32, u32)) -> Vec<(u32, u32)> {
        let mut found = Vec::new();
        for path in &self.declaration(d).derives {
            for at in self.scopes_of(d, Vec::new()) {
                for t in self.declarations_at(at.at, path) {
                    if self.declaration(t).kind == CLASS && !found.contains(&t) {
                        found.push(t);
                    }
                }
            }
        }
        found
    }

    /// Whether trait `t` is in scope in module `at`. See the [module
    /// docs](self).
    fn trait_in_scope(&self, at: Module, t: (u32, u32)) -> bool {
        if let Some(&known) = self.in_scope.borrow().get(&(at, t)) {
            return known;
        }
        let names = self.called(&self.declaration(t).name);
        let found = names.iter().any(|name| {
            self.names_of(at, name).iter().any(|b| b.target.declaration == Some(t))
                || (!self.closed(at)
                    && self.prelude_names(at, name).iter().any(|p| p.declaration == Some(t)))
        }) || self.unnamed_traits(at, &mut Vec::new()).0.contains(&t);
        if self.depth.get() == 0 {
            self.in_scope.borrow_mut().insert((at, t), found);
        }
        found
    }

    /// The traits module `module` brings into scope without a name (`use
    /// a::T as _;`): its own imports', and those its globs and its crate's
    /// preludes bring that it may see. `on_path` holds the modules whose
    /// answer is being found, outermost first: one met again (globs in a
    /// cycle, a prelude that is its crate's prelude) adds nothing there,
    /// and the answer of a module below it, which missed what that one
    /// brings, is not kept. Also says how far up the path the answer reached
    /// (`usize::MAX`: nowhere), the lowest index of a module met again.
    fn unnamed_traits(&self, module: Module, on_path: &mut Vec<Module>) -> (Declarations, usize) {
        if let Some(found) = self.unnamed.borrow().get(&module) {
            return (found.clone(), usize::MAX);
        }
        if let Some(at) = on_path.iter().position(|&m| m == module) {
            return (Declarations::default(), at);
        }
        let here = on_path.len();
        on_path.push(module);
        let mut reached = usize::MAX;
        let mut found: Vec<(u32, u32)> = Vec::new();
        let mut globbed: Vec<Module> = Vec::new();
        for import in self.imports_of(module) {
            let Ok(targets) = self.walk_import(module, import) else { continue };
            if import.glob {
                globbed.extend(targets.iter().filter_map(|t| t.module));
            } else if import.name.is_none() {
                found.extend(
                    targets
                        .iter()
                        .filter_map(|t| t.declaration)
                        .filter(|&d| self.declaration(d).kind == CLASS),
                );
            }
        }
        let preludes = &self.graph.crates()[module.krate as usize].preludes;
        if let Some(root) = self.crate_root(module.krate).module {
            for prelude in preludes {
                if let Ok(targets) = self.walk_from(root, prelude, Start::Prelude) {
                    globbed.extend(targets.iter().filter_map(|t| t.module));
                }
            }
        }
        for other in globbed {
            let (brought, low) = self.unnamed_traits(other, on_path);
            reached = reached.min(low);
            for &t in brought.iter() {
                // One a glob brings is an import of the module it is made in.
                let binding = Binding {
                    target: Target { declaration: Some(t), module: None },
                    visibility: self.unnamed_visibility(other, t),
                    from: other,
                };
                if self.visible(&binding, module) {
                    found.push(t);
                }
            }
        }
        found.sort();
        found.dedup();
        on_path.pop();
        let found = Rc::new(found);
        // Whole when nothing below it met a module above it again; and
        // found with no lookup under way.
        if reached >= here && self.depth.get() == 0 {
            self.unnamed.borrow_mut().insert(module, found.clone());
        }
        (found, if reached >= here { usize::MAX } else { reached })
    }

    /// Who may see the import of `module` that brings trait `t` without a
    /// name: the widest of those imports' visibilities.
    fn unnamed_visibility(&self, module: Module, t: (u32, u32)) -> crate::Visibility {
        let mut widest = crate::units::private();
        for import in self.imports_of(module).filter(|i| !i.glob && i.name.is_none()) {
            let Ok(targets) = self.walk_import(module, import) else { continue };
            if targets.iter().any(|target| target.declaration == Some(t)) {
                if import.visibility == crate::Visibility::Public {
                    return crate::Visibility::Public;
                }
                widest = import.visibility.clone();
            }
        }
        widest
    }

    // ------------------------------------------------------------------
    // Declarations
    // ------------------------------------------------------------------

    fn declaration(&self, d: (u32, u32)) -> &Declaration {
        &self.summaries[d.0 as usize].offers().declarations()[d.1 as usize]
    }

    /// The declarations `path` names from module `at`.
    fn declarations_at(&self, at: Module, path: &Path) -> Vec<(u32, u32)> {
        match self.walk(at, path) {
            Ok(targets) => targets.iter().filter_map(|t| t.declaration).collect(),
            Err(_) => Vec::new(),
        }
    }

    /// The declaration declaration `d` is declared inside (a variant's
    /// enum, a field's struct, an item's trait), not an impl block.
    fn owner_of(&self, d: (u32, u32)) -> Option<(u32, u32)> {
        let declarations = self.summaries[d.0 as usize].offers().declarations();
        let declaration = &declarations[d.1 as usize];
        if declaration.impl_block.is_some() {
            return None;
        }
        let (name, container) = declaration.container.split_last()?;
        declarations
            .iter()
            .position(|o| {
                o.impl_block.is_none()
                    && o.name == *name
                    && o.container == container
                    && o.unit == declaration.unit
                    && matches!(o.kind.as_str(), STRUCT | CLASS)
            })
            .map(|i| (d.0, i as u32))
    }

    /// Where what declaration `d` writes is read, in each crate its file is
    /// in: its module, the generic parameters of it and of what it is in
    /// (its impl block, the type or trait it is declared inside), and
    /// `self_type` for `Self`.
    fn scopes_of(&self, d: (u32, u32), self_type: Vec<Ty>) -> Vec<Rc<Scope>> {
        let declaration = self.declaration(d);
        let summary = &self.summaries[d.0 as usize];
        let mut generics = declaration.generics.clone();
        if let Some(block) = declaration.impl_block {
            if let Some(of) = summary.offers().impls().get(block as usize) {
                generics.extend(of.generics.iter().cloned());
            }
        }
        if let Some(owner) = self.owner_of(d) {
            generics.extend(self.declaration(owner).generics.iter().cloned());
        }
        self.graph
            .places(&self.files[d.0 as usize])
            .iter()
            .map(|&(krate, _)| {
                Rc::new(Scope {
                    at: Module { krate, file: d.0, unit: declaration.unit },
                    generics: generics.clone(),
                    self_type: self_type.clone(),
                })
            })
            .collect()
    }
}

/// Whether `path` begins with a generic parameter where `scope` is
/// (`T`, `T::default`).
fn from_parameter(scope: &Scope, path: &Path) -> bool {
    path.root.is_none() && path.segments.first().is_some_and(|first| scope.generics.contains(first))
}

/// `types`, each once, in their first order.
fn dedup(types: Vec<Ty>) -> Vec<Ty> {
    let mut seen: HashMap<Ty, ()> = HashMap::new();
    types.into_iter().filter(|t| seen.insert(t.clone(), ()).is_none()).collect()
}
