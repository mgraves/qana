//! A member access's value, typed from what declarations write, and the
//! member it names: [`Resolver::member`].
//!
//! A value's type is read, never inferred. A [`Receiver`] says what the
//! file writes of the value; from there each step is a declaration's
//! written type:
//!
//! - a binding's, a cast's or a constant's type, a literal's builtin;
//! - what a function's call gives (its return type);
//! - a field's type, a positional field's;
//! - what a value gives up (`x?`: its type's `Try` output, where the
//!   project model names the trait).
//!
//! A type keeps the types its generic parameters take ([`Ty::Declared`]'s
//! arguments: `Vec<Cell>`'s `T` is `Cell`), and what a declaration of it
//! writes is read with them: a field of a `Grid<Cell>`, `Grid<T>`'s `T` read
//! as `Cell`; a method of an impl block, the block's parameters read from
//! matching its type to the value's (`impl<T> Vec<T>`); `Self` as the type
//! the member was found on, `Self::Item` as that type's associated type. A
//! parameter nothing gives a type to (a function's own, one a call would
//! infer) stops there, as do a closure's call and a qualified path.
//!
//! A type's members, for a value of it, are looked for as a method call
//! probes: on the type, then on what it dereferences to (its `Deref`
//! impl's `Target`, where the project model names the trait; an array's
//! slice), one step at a time, until a step has the name. A reference is
//! reached through. At each step:
//!
//! - a field: those declared inside the type;
//! - a method: the functions of its inherent impl blocks; or else, of each
//!   trait in scope where the access stands, the function of an impl
//!   block of the trait for the type (its own, a type the language
//!   provides', a blanket impl's for a trait the type does not implement
//!   itself), or the trait's own where the block has none (a provided
//!   method), and the trait's own for a trait the type derives. Where
//!   several blocks of one trait have one (`From<u8>`, `From<char>`), the
//!   trait's own is an answer too. A block of a trait the project does not
//!   hold has its function as an answer: nothing says that trait is out of
//!   scope. A trait taken as a type (`dyn A + B`, `impl A`, `Self` in a
//!   trait) has its own items and its supertraits'.
//!
//! A path's member of a type (`Cell::new`) is found the same way, an
//! item of any kind, without dereferencing.
//!
//! A trait is in scope in a module when a name the module has binds it (an
//! item, an import, a glob's, a rename's), when its crate's preludes do,
//! or when an import brings it without a name (`use a::T as _;`), the
//! module's own or a glob's or a prelude's.

use std::collections::{HashMap, HashSet};
use std::path::{Path as FilePath, PathBuf};
use std::rc::Rc;

use super::{
    Binding, Blanket, BlanketsByName, Declarations, ImplBlocks, Module, Names, Resolution,
    Resolver, Start, Target,
};
use crate::{Declaration, MemberAccess, MemberKind, Path, Receiver, Root, TypeRef};

/// The outline kinds that say what a declaration is (`qana_lang`'s
/// vocabulary): a function's call gives its return type; a struct is a type
/// (an alias, its written type); a class is a type known by its items (a
/// trait); a constant without a type is a value of the type around it (an
/// enum's variant); a field is read.
const FUNCTION: &str = "function";
pub(super) const STRUCT: &str = "struct";
pub(super) const CLASS: &str = "class";
const CONSTANT: &str = "constant";
const FIELD: &str = "field";

/// How far a value's typing follows declarations before it stops: aliases
/// of aliases, bindings of bindings, types inside types.
const DEPTH: usize = 32;

/// How many times a member access dereferences before it stops.
const DEREFS: usize = 8;

/// How deeply typing may nest, whatever starts it: a path walked while
/// typing may look for a type's members, which types again.
const NESTING: usize = 48;

/// What each type argument of a type is: the types it may be (none where
/// not known), by its parameter's place.
type Args = Rc<Vec<Vec<Ty>>>;

/// A value's type, as far as what is written says.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Ty {
    /// A type a declaration declares (a struct, an enum, a union; a trait,
    /// taken as a type), by (file, declaration index), with what its
    /// generic parameters are, by [`Declaration::generics`]' order.
    Declared((u32, u32), Args),
    /// A type the language provides, by its name (`str`, `u8`).
    Builtin(Rc<str>),
    /// A slice, and an array, of the types each element may be.
    Slice(Rc<Vec<Ty>>),
    Array(Rc<Vec<Ty>>),
    /// A tuple, of what each element may be.
    Tuple(Args),
    /// A type known by its traits (`dyn A + B`, `impl A`): theirs, and the
    /// associated types written of them (`impl Iterator<Item = u8>`).
    Traits(Rc<Vec<(u32, u32)>>, Rc<Vec<(String, Vec<Ty>)>>),
}

/// A trait a type's impl blocks implement, in scope: the members of that
/// name its blocks have, and whether one of its blocks has none (the trait
/// provides it).
struct TraitItems {
    of: (u32, u32),
    items: Vec<Found>,
    provided: bool,
}

/// Where a type is written: the module its paths are read from, the
/// generic parameters there with the types each is known to be, and what
/// `Self` is there.
#[derive(Clone, Debug)]
struct Scope {
    at: Module,
    generics: Vec<(String, Vec<Ty>)>,
    self_type: Vec<Ty>,
}

impl Scope {
    /// The types the generic parameter `name` is known to be here, if it is
    /// one.
    fn parameter(&self, name: &str) -> Option<&Vec<Ty>> {
        self.generics.iter().find(|(p, _)| p == name).map(|(_, types)| types)
    }
}

/// A member found on a type: its declaration, and where what it writes is
/// read (in each crate its file is in).
#[derive(Clone)]
struct Found {
    declaration: (u32, u32),
    scopes: Vec<Rc<Scope>>,
}

/// An operator whose result a trait names (see [`crate::CrateInfo`]).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Operator {
    Deref = 0,
    Try = 1,
}

/// One level of typing's nesting, given back when dropped.
struct Nested<'r>(&'r std::cell::Cell<usize>);

impl Drop for Nested<'_> {
    fn drop(&mut self) {
        self.0.set(self.0.get() - 1);
    }
}

impl Resolver {
    /// One more level of typing's nesting; `None` past [`NESTING`].
    fn nest(&self) -> Option<Nested<'_>> {
        let now = self.typing.get();
        if now >= NESTING {
            return None;
        }
        self.typing.set(now + 1);
        Some(Nested(&self.typing))
    }

    /// What the member `access` names, from where it stands in `file`: the
    /// declarations its value's type has under its name. A member is never
    /// said to name nothing: it escapes from segment 0 where its value's
    /// type is not written, and from segment 1 where the type is known and
    /// no summary holds the member (a derive's or a macro's, one through a
    /// trait the project does not hold). See the [module docs](self).
    pub fn member(&self, file: &FilePath, access: &MemberAccess) -> Resolution {
        let Some(&id) = self.ids.get(file) else {
            return Resolution::Escapes { segment: 0 };
        };
        let mut declarations: Vec<(PathBuf, u32)> = Vec::new();
        // Whether the receiver is typed, and every type it is, and every
        // step down from each, known: then no summary holds the member.
        let (mut typed, mut known) = (false, true);
        for (krate, _) in self.graph.places(file) {
            let at = Module { krate, file: id, unit: self.unit_named(id, &access.units) };
            let generics = access.generics.iter().map(|g| (g.clone(), Vec::new())).collect();
            let scope = Rc::new(Scope { at, generics, self_type: Vec::new() });
            let types = self.value_types(&scope, &access.receiver, 0);
            typed |= !types.is_empty();
            for ty in &types {
                let (found, unknown) =
                    self.members_through_known(at, ty, &access.name, access.kind, 0);
                known &= !unknown;
                for found in found {
                    let (file, index) = found.declaration;
                    declarations.push((self.files[file as usize].clone(), index));
                }
            }
        }
        if declarations.is_empty() {
            return Resolution::Escapes { segment: usize::from(typed && known) };
        }
        declarations.sort();
        declarations.dedup();
        Resolution::Found { declarations, modules: Vec::new() }
    }

    /// Whether a member access can name the `declaration`th declaration of
    /// `file`, which [`member`](Self::member) would then find: a field, or
    /// a function of an impl block or of a trait.
    pub fn is_member(&self, file: &FilePath, declaration: u32) -> bool {
        let Some(&id) = self.ids.get(file) else {
            return false;
        };
        let declarations = self.summaries[id as usize].offers().declarations();
        let Some(declared) = declarations.get(declaration as usize) else {
            return false;
        };
        match declared.kind.as_str() {
            FIELD => true,
            FUNCTION => {
                declared.impl_block.is_some()
                    || self
                        .owner_of((id, declaration))
                        .is_some_and(|owner| self.declaration(owner).kind == CLASS)
            }
            _ => false,
        }
    }

    // ------------------------------------------------------------------
    // Values
    // ------------------------------------------------------------------

    /// The types of the value `receiver`, written where `scope` is.
    fn value_types(&self, scope: &Rc<Scope>, receiver: &Receiver, depth: usize) -> Vec<Ty> {
        if depth > DEPTH {
            return Vec::new();
        }
        let Some(_nested) = self.nest() else { return Vec::new() };
        let mut types = Vec::new();
        match receiver {
            Receiver::Typed(written) => types = self.types_of(scope, written, depth + 1),
            Receiver::Builtin(name) => types.push(Ty::Builtin(Rc::from(name.as_str()))),
            Receiver::Path(path) if !from_parameter(scope, path) => {
                for d in self.declarations_at(scope.at, path) {
                    let self_type = self.path_self(scope, path, d, depth + 1);
                    types.extend(self.value_of(d, self_type, depth + 1));
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
                            let self_type = self.path_self(scope, path, d, depth + 1);
                            types.extend(self.call_of(d, self_type, depth + 1));
                        }
                    }
                }
            }
            Receiver::Member { of, name, kind } => {
                for ty in self.value_types(scope, of, depth + 1) {
                    for found in self.members_through(scope.at, &ty, name, *kind, depth + 1) {
                        let declaration = self.declaration(found.declaration);
                        let written = match kind {
                            MemberKind::Field => declaration.ty.as_ref(),
                            MemberKind::Method => declaration.returns.as_ref(),
                        };
                        if let Some(written) = written {
                            for at in &found.scopes {
                                types.extend(self.types_of(at, written, depth + 1));
                            }
                        }
                    }
                }
            }
            Receiver::Position { of, index } => {
                for ty in self.value_types(scope, of, depth + 1) {
                    types.extend(self.position_through(scope.at, &ty, *index, depth + 1));
                }
            }
            Receiver::Try(of) => {
                for ty in self.value_types(scope, of, depth + 1) {
                    types.extend(self.output_of(scope.at, &ty, Operator::Try, depth + 1));
                }
            }
            Receiver::Unknown => {}
        }
        dedup(types)
    }

    /// The type of declaration `d` as a value: a constant's or a static's
    /// written type; a unit struct; an enum's unit variant, of the enum.
    fn value_of(&self, d: (u32, u32), self_type: Vec<Ty>, depth: usize) -> Vec<Ty> {
        let declaration = self.declaration(d);
        if let Some(written) = &declaration.ty {
            let mut types = Vec::new();
            for at in self.scopes_of(d, self_type, &HashMap::new()) {
                types.extend(self.types_of(&at, written, depth + 1));
            }
            return types;
        }
        match declaration.kind.as_str() {
            STRUCT => vec![Ty::Declared(d, Args::default())],
            CONSTANT => {
                self.owner_of(d).map(|o| Ty::Declared(o, Args::default())).into_iter().collect()
            }
            _ => Vec::new(),
        }
    }

    /// What calling declaration `d` gives: a function's written return
    /// type, `Self` read as `self_type`; a tuple struct; a tuple variant,
    /// of its enum.
    fn call_of(&self, d: (u32, u32), self_type: Vec<Ty>, depth: usize) -> Vec<Ty> {
        let declaration = self.declaration(d);
        match declaration.kind.as_str() {
            FUNCTION => {
                let Some(written) = &declaration.returns else { return Vec::new() };
                let mut types = Vec::new();
                for at in self.scopes_of(d, self_type, &HashMap::new()) {
                    types.extend(self.types_of(&at, written, depth + 1));
                }
                types
            }
            STRUCT if declaration.ty.is_none() => vec![Ty::Declared(d, Args::default())],
            CONSTANT if declaration.ty.is_none() => {
                self.owner_of(d).map(|o| Ty::Declared(o, Args::default())).into_iter().collect()
            }
            _ => Vec::new(),
        }
    }

    /// What `Self` is in declaration `d`, which `path`, written where
    /// `scope` is, names: the type the path names before it, as rustc
    /// reads it (`Cell::default`, whose `Self` is `Cell` though `default`
    /// is `Default`'s; `Splitter::new`, an alias's type with its
    /// arguments; `Self::new`). Through a trait (`Default::default`), it is
    /// what the call infers: unknown. A path of one name, or one through a
    /// module, says nothing more than `d` does ([`Self::self_of`]).
    fn path_self(&self, scope: &Rc<Scope>, path: &Path, d: (u32, u32), depth: usize) -> Vec<Ty> {
        let Some((_, before)) = path.segments.split_last() else { return Vec::new() };
        if before.is_empty() && path.root != Some(Root::SelfType) {
            return self.self_of(d, depth);
        }
        let prefix = Path { root: path.root, segments: before.to_vec() };
        let written = TypeRef::Path { path: prefix, args: Vec::new(), bindings: Vec::new() };
        let types = self.types_of(scope, &written, depth + 1);
        let trait_named = types
            .iter()
            .any(|t| matches!(t, Ty::Declared(t, _) if self.declaration(*t).kind == CLASS));
        if trait_named {
            return Vec::new();
        }
        if types.is_empty() {
            return self.self_of(d, depth);
        }
        types
    }

    /// What `Self` is in declaration `d`, reached by a path: its impl
    /// block's type, its parameters unknown; or the trait it is an item of.
    fn self_of(&self, d: (u32, u32), depth: usize) -> Vec<Ty> {
        let declaration = self.declaration(d);
        if let Some(block) = declaration.impl_block {
            let summary = &self.summaries[d.0 as usize];
            let Some(of) = summary.offers().impls().get(block as usize) else { return Vec::new() };
            let Some(written) = &of.self_type else { return Vec::new() };
            let mut types = Vec::new();
            for at in self.block_scopes((d.0, block), &HashMap::new(), Vec::new()) {
                types.extend(self.types_of(&at, written, depth + 1));
            }
            return types;
        }
        match self.owner_of(d) {
            Some(owner) if self.declaration(owner).kind == CLASS => {
                vec![Ty::Declared(owner, Args::default())]
            }
            _ => Vec::new(),
        }
    }

    /// The types of the `index`th positional field of a value of type `ty`,
    /// dereferencing as a member access does.
    fn position_through(&self, at: Module, ty: &Ty, index: u32, depth: usize) -> Vec<Ty> {
        let mut step = vec![ty.clone()];
        let mut seen: Vec<Ty> = Vec::new();
        for _ in 0..DEREFS {
            let mut types = Vec::new();
            for t in &step {
                match t {
                    Ty::Declared(d, args) => {
                        let declaration = self.declaration(*d);
                        if let Some(written) = declaration.positional.get(index as usize) {
                            let bound = bind(&declaration.generics, args);
                            for scope in self.scopes_of(*d, vec![t.clone()], &bound) {
                                types.extend(self.types_of(&scope, written, depth + 1));
                            }
                        }
                    }
                    Ty::Tuple(elements) => {
                        types.extend(elements.get(index as usize).cloned().unwrap_or_default())
                    }
                    _ => {}
                }
            }
            if !types.is_empty() {
                return dedup(types);
            }
            seen.extend(step.iter().cloned());
            step = self.next_step(at, &step, &seen, depth);
            if step.is_empty() {
                break;
            }
        }
        Vec::new()
    }

    // ------------------------------------------------------------------
    // Types
    // ------------------------------------------------------------------

    /// The types `written` names, where `scope` is.
    fn types_of(&self, scope: &Rc<Scope>, written: &TypeRef, depth: usize) -> Vec<Ty> {
        if depth > DEPTH {
            return Vec::new();
        }
        let Some(_nested) = self.nest() else { return Vec::new() };
        let each = |types: &[TypeRef]| -> Vec<Vec<Ty>> {
            types.iter().map(|t| self.types_of(scope, t, depth + 1)).collect()
        };
        match written {
            TypeRef::Ref(inner) => self.types_of(scope, inner, depth + 1),
            TypeRef::Path { path, args, .. } => {
                if path.root == Some(Root::SelfType) {
                    // `Self`, or an associated type of it.
                    return match path.segments.as_slice() {
                        [] => scope.self_type.clone(),
                        [name] => dedup(
                            scope
                                .self_type
                                .iter()
                                .flat_map(|t| self.associated(t, name, depth + 1))
                                .collect(),
                        ),
                        _ => Vec::new(),
                    };
                }
                if from_parameter(scope, path) {
                    // A generic parameter, as far as it is known; or an
                    // associated type of it (`T::Output`).
                    let known = scope.parameter(&path.segments[0]).cloned().unwrap_or_default();
                    return match &path.segments[1..] {
                        [] => known,
                        [name] => dedup(
                            known
                                .iter()
                                .flat_map(|t| self.associated(t, name, depth + 1))
                                .collect(),
                        ),
                        _ => Vec::new(),
                    };
                }
                let declarations = self.type_declarations_at(scope.at, path);
                if declarations.is_empty() {
                    // One the language provides: a name of one that names a
                    // module alone (`use crate::str::{self, …}`) or nothing
                    // names it, as rustc reads a type.
                    return match path.segments.as_slice() {
                        [name] if path.root.is_none() && self.is_builtin(scope.at, name) => {
                            vec![Ty::Builtin(Rc::from(name.as_str()))]
                        }
                        _ => Vec::new(),
                    };
                }
                let args = each(args);
                let mut types = Vec::new();
                for d in declarations {
                    types.extend(self.type_of(d, &args, depth + 1));
                }
                dedup(types)
            }
            TypeRef::Slice(element) => {
                vec![Ty::Slice(Rc::new(self.types_of(scope, element, depth + 1)))]
            }
            TypeRef::Array(element) => {
                vec![Ty::Array(Rc::new(self.types_of(scope, element, depth + 1)))]
            }
            TypeRef::Tuple(elements) => vec![Ty::Tuple(Rc::new(each(elements)))],
            TypeRef::Dyn(traits) | TypeRef::Impl(traits) => {
                let mut found = Vec::new();
                let mut bindings = Vec::new();
                for t in traits {
                    let TypeRef::Path { path, bindings: written, .. } = t else { continue };
                    for d in self.declarations_at(scope.at, path) {
                        if self.declaration(d).kind == CLASS && !found.contains(&d) {
                            found.push(d);
                        }
                    }
                    for (name, ty) in written {
                        bindings.push((name.clone(), self.types_of(scope, ty, depth + 1)));
                    }
                }
                if found.is_empty() {
                    Vec::new()
                } else {
                    vec![Ty::Traits(Rc::new(found), Rc::new(bindings))]
                }
            }
            TypeRef::Other => Vec::new(),
        }
    }

    /// The type declaration `d` is, named as a type with the arguments
    /// `args`: a struct, an enum, a union or a trait itself; an alias, the
    /// type it is of, its parameters the arguments.
    fn type_of(&self, d: (u32, u32), args: &[Vec<Ty>], depth: usize) -> Vec<Ty> {
        let declaration = self.declaration(d);
        match declaration.kind.as_str() {
            STRUCT => match &declaration.ty {
                Some(written) => {
                    let self_type = self.self_of(d, depth);
                    let bound = bind(&declaration.generics, args);
                    let mut types = Vec::new();
                    for at in self.scopes_of(d, self_type, &bound) {
                        types.extend(self.types_of(&at, written, depth + 1));
                    }
                    types
                }
                None => vec![Ty::Declared(d, Rc::new(args.to_vec()))],
            },
            CLASS => vec![Ty::Declared(d, Rc::new(args.to_vec()))],
            _ => Vec::new(),
        }
    }

    /// Whether `name` is a type the language provides, in the crate of
    /// module `at`.
    fn is_builtin(&self, at: Module, name: &str) -> bool {
        self.graph.crates()[at.krate as usize].builtins.iter().any(|b| b == name)
    }

    // ------------------------------------------------------------------
    // Members
    // ------------------------------------------------------------------

    /// The members named `name` of a value of type `ty`, used as `kind`
    /// says, for an access in module `at`: on the type, or else on what it
    /// dereferences to, one step at a time.
    fn members_through(
        &self,
        at: Module,
        ty: &Ty,
        name: &str,
        kind: MemberKind,
        depth: usize,
    ) -> Vec<Found> {
        self.members_through_known(at, ty, name, kind, depth).0
    }

    /// [`Self::members_through`]'s members, and, where there are none,
    /// whether the walk down stopped at a type it does not know: a step's
    /// `Deref` impl whose target types to nothing (an `Rc<T>` whose `T` a
    /// call infers), or more steps than it takes.
    fn members_through_known(
        &self,
        at: Module,
        ty: &Ty,
        name: &str,
        kind: MemberKind,
        depth: usize,
    ) -> (Vec<Found>, bool) {
        let Some(_nested) = self.nest() else { return (Vec::new(), true) };
        let mut step = vec![ty.clone()];
        let mut seen: Vec<Ty> = Vec::new();
        for _ in 0..DEREFS {
            let found: Vec<Found> =
                step.iter().flat_map(|t| self.members_on(at, t, name, kind, depth)).collect();
            if !found.is_empty() {
                return (found, false);
            }
            seen.extend(step.iter().cloned());
            let (next, unknown) = self.next_step_known(at, &step, &seen, depth);
            if unknown {
                return (Vec::new(), true);
            }
            step = next;
            if step.is_empty() {
                return (Vec::new(), false);
            }
        }
        (Vec::new(), true)
    }

    /// What the types of `step` dereference to, but those already `seen`.
    fn next_step(&self, at: Module, step: &[Ty], seen: &[Ty], depth: usize) -> Vec<Ty> {
        self.next_step_known(at, step, seen, depth).0
    }

    /// [`Self::next_step`]'s types, and whether one of `step`'s types
    /// dereferences to a type not known.
    fn next_step_known(
        &self,
        at: Module,
        step: &[Ty],
        seen: &[Ty],
        depth: usize,
    ) -> (Vec<Ty>, bool) {
        let mut next = Vec::new();
        let mut unknown = false;
        for t in step {
            let (types, held) = self.derefs_known(at, t, depth + 1);
            unknown |= held && types.is_empty();
            next.extend(types.into_iter().filter(|t| !seen.contains(t)));
        }
        (dedup(next), unknown)
    }

    /// The members named `name` of a value of type `ty` itself.
    fn members_on(
        &self,
        at: Module,
        ty: &Ty,
        name: &str,
        kind: MemberKind,
        depth: usize,
    ) -> Vec<Found> {
        let as_found = |items: Vec<(u32, u32)>, bound: &HashMap<String, Vec<Ty>>| -> Vec<Found> {
            items
                .into_iter()
                .map(|item| Found {
                    declaration: item,
                    scopes: self.scopes_of(item, vec![ty.clone()], bound),
                })
                .collect()
        };
        match (ty, kind) {
            (Ty::Declared(d, args), MemberKind::Field) => {
                let fields: Vec<(u32, u32)> = self
                    .declared_inside(d.0, d.1, name)
                    .into_iter()
                    .filter_map(|t| t.declaration)
                    .filter(|&f| self.declaration(f).kind == FIELD)
                    .collect();
                as_found(fields, &bind(&self.declaration(*d).generics, args))
            }
            (Ty::Declared(d, _), MemberKind::Method) if self.declaration(*d).kind == CLASS => {
                as_found(self.trait_items(*d, name, &mut HashSet::new()), &HashMap::new())
            }
            (Ty::Traits(traits, _), MemberKind::Method) => {
                // A trait object's own: its traits' items, and those of the
                // impl blocks of the object itself (`impl dyn Any`), as an
                // inherent impl block's are; then, as any type's, those of
                // the traits in scope it implements.
                let mut visited = HashSet::new();
                let mut items = Vec::new();
                for &t in traits.iter() {
                    for item in self.trait_items(t, name, &mut visited) {
                        if !items.contains(&item) {
                            items.push(item);
                        }
                    }
                }
                let mut found = as_found(items, &HashMap::new());
                for &block in self.blocks_of(ty).iter().filter(|&&b| self.is_inherent(b)) {
                    found.extend(self.block_members(block, ty, name, Some(FUNCTION), depth));
                }
                if found.is_empty() {
                    return self.impl_members(at, ty, name, Some(FUNCTION), depth);
                }
                found
            }
            (_, MemberKind::Method) => self.impl_members(at, ty, name, Some(FUNCTION), depth),
            (_, MemberKind::Field) => Vec::new(),
        }
    }

    /// The items named `name` of type `d`, as a path's next segment names
    /// one: [`Self::impl_members`]' for a value of `d`, of any kind. A
    /// declaration that is no type (a function that shares a module's
    /// name) has none.
    pub(super) fn items_named(&self, at: Module, d: (u32, u32), name: &str) -> Vec<(u32, u32)> {
        // Only a type has items: a function, a constant, a field has none.
        if !matches!(self.declaration(d).kind.as_str(), STRUCT | CLASS) {
            return Vec::new();
        }
        self.impl_members(at, &Ty::Declared(d, Args::default()), name, None, 0)
            .into_iter()
            .map(|found| found.declaration)
            .collect()
    }

    /// The items named `name` of the impl blocks of type `ty`, of the
    /// outline kind `only` where given (a method's: a function), for a use
    /// in module `at`: its inherent impl blocks'; or else, of each trait in
    /// scope in `at`, an impl block's, or the trait's own. See the [module
    /// docs](self).
    fn impl_members(
        &self,
        at: Module,
        ty: &Ty,
        name: &str,
        only: Option<&str>,
        depth: usize,
    ) -> Vec<Found> {
        let blocks = self.blocks_of(ty);
        let items_of = |block: (u32, u32)| self.block_members(block, ty, name, only, depth);
        let is_inherent = |block: (u32, u32)| self.is_inherent(block);
        let inherent: Vec<Found> =
            blocks.iter().filter(|&&b| is_inherent(b)).flat_map(|&b| items_of(b)).collect();
        if !inherent.is_empty() {
            return inherent;
        }
        if self.depth.get() > 0 || self.finding_impls.get() {
            // Inside a name lookup (an import's path, an impl block's
            // type): judging which traits are in scope would start lookups
            // inside it, in other modules, without end. Each trait impl
            // block's item is an answer there.
            let mut found: Vec<Found> =
                blocks.iter().filter(|&&b| !is_inherent(b)).flat_map(|&b| items_of(b)).collect();
            found.sort_by_key(|f| f.declaration);
            found.dedup_by_key(|f| f.declaration);
            return found;
        }

        // Each trait in scope, with its blocks' items of that name, and
        // whether one of its blocks has none (it provides it).
        let mut by_trait: Vec<TraitItems> = Vec::new();
        let mut note = |t: (u32, u32), items: Vec<Found>| {
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
        // Whether trait `t` has an item named `name`, or a supertrait of it
        // does: asked before whether it is in scope, which is dearer.
        let has = |t: (u32, u32)| self.trait_names(t).contains(name);
        let mut found: Vec<Found> = Vec::new();
        let mut implemented: HashSet<(u32, u32)> = HashSet::new();
        for &block in blocks.iter().filter(|&&b| !is_inherent(b)) {
            let traits = self.impl_traits(block);
            let items = items_of(block);
            if traits.is_empty() {
                found.extend(items);
                continue;
            }
            for &t in traits.iter() {
                implemented.insert(t);
                if (!items.is_empty() || has(t)) && self.trait_in_scope(at, t) {
                    note(t, items.clone());
                }
            }
        }
        if let Ty::Declared(d, _) = ty {
            for t in self.derived(*d) {
                implemented.insert(t);
                if has(t) && self.trait_in_scope(at, t) {
                    note(t, Vec::new());
                }
            }
        }
        // A blanket impl's, for a trait the type does not implement itself.
        let blankets = self.blankets_named();
        for &(block, t) in blankets.get(name).into_iter().flatten() {
            if !implemented.contains(&t) && self.trait_in_scope(at, t) {
                note(t, items_of(block));
            }
        }

        for TraitItems { of: t, items, provided } in by_trait {
            if provided || items.len() > 1 {
                let own: Vec<(u32, u32)> = self
                    .trait_items(t, name, &mut HashSet::new())
                    .into_iter()
                    .filter(|&item| only.is_none_or(|kind| self.declaration(item).kind == kind))
                    .collect();
                for item in own {
                    found.push(Found {
                        declaration: item,
                        scopes: self.scopes_of(item, vec![ty.clone()], &HashMap::new()),
                    });
                }
            }
            found.extend(items);
        }
        found.sort_by_key(|f| f.declaration);
        found.dedup_by_key(|f| f.declaration);
        found
    }

    /// The items named `name` of impl block `block`, of the outline kind
    /// `only` where given, for a value of type `ty`.
    fn block_members(
        &self,
        block: (u32, u32),
        ty: &Ty,
        name: &str,
        only: Option<&str>,
        depth: usize,
    ) -> Vec<Found> {
        let declarations = self.summaries[block.0 as usize].offers().declarations();
        let items: Vec<(u32, u32)> = self
            .items_of_block(block)
            .iter()
            .filter(|&&i| {
                let f = &declarations[i as usize];
                f.name == name && only.is_none_or(|kind| f.kind == kind)
            })
            .map(|&i| (block.0, i))
            .collect();
        if items.is_empty() {
            return Vec::new();
        }
        let bound = self.bindings_of(block, ty, depth);
        items
            .into_iter()
            .map(|item| Found {
                declaration: item,
                scopes: self.scopes_of(item, vec![ty.clone()], &bound),
            })
            .collect()
    }

    /// Whether impl block `block` is inherent: it implements no trait.
    pub(super) fn is_inherent(&self, block: (u32, u32)) -> bool {
        self.summaries[block.0 as usize].offers().impls()[block.1 as usize].trait_path.is_none()
    }

    /// The items of impl block `block`, as indexes into its file's
    /// declarations.
    pub(super) fn items_of_block(&self, block: (u32, u32)) -> Rc<[u32]> {
        let file = self.block_items.borrow().get(&block.0).cloned();
        let file = file.unwrap_or_else(|| {
            let mut by_block: HashMap<u32, Vec<u32>> = HashMap::new();
            let declarations = self.summaries[block.0 as usize].offers().declarations();
            for (i, d) in declarations.iter().enumerate() {
                if let Some(b) = d.impl_block {
                    by_block.entry(b).or_default().push(i as u32);
                }
            }
            let by_block: HashMap<u32, Rc<[u32]>> =
                by_block.into_iter().map(|(b, items)| (b, Rc::from(items))).collect();
            let by_block = Rc::new(by_block);
            self.block_items.borrow_mut().insert(block.0, by_block.clone());
            by_block
        });
        file.get(&block.1).cloned().unwrap_or_else(|| Rc::from(Vec::new()))
    }

    /// The blanket impl blocks, each with a trait it implements, by each
    /// name the block has an item of or the trait (or a supertrait)
    /// declares: those that may answer for a member of that name. Found
    /// once, with no lookup under way.
    fn blankets_named(&self) -> BlanketsByName {
        if let Some(found) = self.blankets_named.borrow().as_ref() {
            return found.clone();
        }
        let mut by_name: HashMap<String, Vec<Blanket>> = HashMap::new();
        let names = self.impl_names();
        for &block in &names.blankets {
            let declarations = self.summaries[block.0 as usize].offers().declarations();
            for &t in self.impl_traits(block).iter() {
                let mut named: HashSet<String> = self
                    .items_of_block(block)
                    .iter()
                    .map(|&i| declarations[i as usize].name.clone())
                    .collect();
                named.extend(self.trait_names(t).iter().cloned());
                for name in named {
                    by_name.entry(name).or_default().push((block, t));
                }
            }
        }
        let by_name = Rc::new(by_name);
        if self.depth.get() == 0 {
            *self.blankets_named.borrow_mut() = Some(by_name.clone());
        }
        by_name
    }

    /// The impl blocks of type `ty` itself: a declared type's, a builtin's,
    /// a slice's or an array's.
    fn blocks_of(&self, ty: &Ty) -> ImplBlocks {
        match ty {
            Ty::Declared(d, _) => self.impls_of(d.0, d.1),
            Ty::Builtin(name) => self.builtin_blocks(name),
            Ty::Slice(_) => Rc::new(self.impl_names().slices.clone()),
            Ty::Array(_) => Rc::new(self.impl_names().arrays.clone()),
            Ty::Traits(traits, _) => self.object_blocks(traits),
            Ty::Tuple(_) => ImplBlocks::default(),
        }
    }

    /// The impl blocks of the trait object of exactly `traits` (`impl dyn
    /// Any + Send` for a `dyn Any + Send`, not `impl dyn Any`, whose type is
    /// another).
    fn object_blocks(&self, traits: &[(u32, u32)]) -> ImplBlocks {
        let names = self.impl_names();
        let mut candidates: Vec<(u32, u32)> = traits
            .iter()
            .filter_map(|&t| names.objects.get(&self.declaration(t).name))
            .flatten()
            .copied()
            .collect();
        candidates.sort_unstable();
        candidates.dedup();
        let wanted = sorted(traits.to_vec());
        let found = candidates
            .into_iter()
            .filter(|&block| {
                let of = &self.summaries[block.0 as usize].offers().impls()[block.1 as usize];
                let Some(TypeRef::Dyn(written)) = &of.self_type else { return false };
                self.graph.places(&self.files[block.0 as usize]).iter().any(|&(krate, _)| {
                    let at = Module { krate, file: block.0, unit: of.unit };
                    let mut held: Vec<(u32, u32)> = Vec::new();
                    for path in written.iter().filter_map(TypeRef::path) {
                        for d in self.declarations_at(at, path) {
                            if self.declaration(d).kind == CLASS {
                                held.push(d);
                            }
                        }
                    }
                    sorted(held) == wanted
                })
            })
            .collect();
        Rc::new(found)
    }

    /// The impl blocks of the type the language provides under `name`
    /// (`impl str`, `impl u8`): those whose type is that name alone, where
    /// it names nothing the project declares.
    fn builtin_blocks(&self, name: &str) -> ImplBlocks {
        if let Some(found) = self.builtin_blocks.borrow().get(name) {
            return found.clone();
        }
        let names = self.impl_names();
        let mut found = Vec::new();
        for &block in names.by_name.get(name).into_iter().flatten() {
            let of = &self.summaries[block.0 as usize].offers().impls()[block.1 as usize];
            let alone = of
                .self_type
                .as_ref()
                .and_then(TypeRef::path)
                .is_some_and(|p| p.root.is_none() && p.segments == [name]);
            if alone && self.impl_type(block).0.is_empty() {
                found.push(block);
            }
        }
        let found = Rc::new(found);
        if self.depth.get() == 0 {
            self.builtin_blocks.borrow_mut().insert(name.to_string(), found.clone());
        }
        found
    }

    // ------------------------------------------------------------------
    // What a type dereferences to, and what it gives up
    // ------------------------------------------------------------------

    /// The types a value of type `ty` dereferences to, for a member access
    /// (its `Deref` impl's target; an array's slice), and whether a `Deref`
    /// impl of `ty` holds (where none of its targets is known, what it
    /// dereferences to is not).
    fn derefs_known(&self, at: Module, ty: &Ty, depth: usize) -> (Vec<Ty>, bool) {
        let mut types = Vec::new();
        if let Ty::Array(elements) = ty {
            types.push(Ty::Slice(elements.clone()));
        }
        let (output, held) = self.output_held(at, ty, Operator::Deref, depth);
        types.extend(output);
        (types, held)
    }

    /// What `operator` gives of a value of type `ty`: the associated type
    /// its impl block of the trait the crate of `at` names for it writes
    /// (`impl Deref for String { type Target = str; }`).
    fn output_of(&self, at: Module, ty: &Ty, operator: Operator, depth: usize) -> Vec<Ty> {
        self.output_held(at, ty, operator, depth).0
    }

    /// [`Self::output_of`]'s types, and whether an impl block of `ty` of
    /// the operator's trait holds, whatever its output types to.
    fn output_held(
        &self,
        at: Module,
        ty: &Ty,
        operator: Operator,
        depth: usize,
    ) -> (Vec<Ty>, bool) {
        if depth > DEPTH {
            return (Vec::new(), false);
        }
        let Some(_nested) = self.nest() else { return (Vec::new(), false) };
        let info = &self.graph.crates()[at.krate as usize];
        let Some((_, output)) = (match operator {
            Operator::Deref => &info.deref_trait,
            Operator::Try => &info.try_trait,
        }) else {
            return (Vec::new(), false);
        };
        let traits = self.operator_traits(at.krate, operator);
        if traits.is_empty() {
            return (Vec::new(), false);
        }
        let mut types = Vec::new();
        let mut held = false;
        for &block in self.blocks_of(ty).iter() {
            if self.impl_traits(block).iter().any(|t| traits.contains(t)) {
                held = true;
                types.extend(self.block_associated(block, ty, output, depth + 1));
            }
        }
        (dedup(types), held)
    }

    /// The traits the crate `krate` names for `operator`, its path read
    /// from the crate's root.
    fn operator_traits(&self, krate: u32, operator: Operator) -> Declarations {
        let key = (krate, operator as u8);
        if let Some(found) = self.operators.borrow().get(&key) {
            return found.clone();
        }
        let info = &self.graph.crates()[krate as usize];
        let named = match operator {
            Operator::Deref => &info.deref_trait,
            Operator::Try => &info.try_trait,
        };
        let mut found = Vec::new();
        let declared = match named {
            Some((path, _)) => self.lang_declarations(krate, path),
            None => Vec::new(),
        };
        for d in declared {
            if self.declaration(d).kind == CLASS && !found.contains(&d) {
                found.push(d);
            }
        }
        let found = Rc::new(found);
        if self.depth.get() == 0 {
            self.operators.borrow_mut().insert(key, found.clone());
        }
        found
    }

    /// What the path of a trait the crate `krate` names for an operator
    /// declares ([`CrateInfo::deref_trait`]): read from the crate's root;
    /// from the crates themselves (`::core::ops::Deref`), its first segment
    /// a crate the crate names, not a name its root declares.
    ///
    /// [`CrateInfo::deref_trait`]: crate::CrateInfo::deref_trait
    fn lang_declarations(&self, krate: u32, path: &Path) -> Vec<(u32, u32)> {
        if path.root != Some(Root::Global) {
            return match self.crate_root(krate).module {
                Some(root) => self.declarations_at(root, path),
                None => Vec::new(),
            };
        }
        let Some((first, rest)) = path.segments.split_first() else { return Vec::new() };
        if rest.is_empty() {
            return Vec::new();
        }
        let rest = Path { root: None, segments: rest.to_vec() };
        let mut found = Vec::new();
        for named in self.named_crates(krate, first, false) {
            if let Some(root) = named.module {
                found.extend(self.declarations_at(root, &rest));
            }
        }
        found
    }

    /// The associated type `name` of type `ty` (`Self::Item`): what an
    /// impl block of it writes (`type Item = &'a T;`), or what its traits'
    /// written bindings say (`impl Iterator<Item = u8>`).
    fn associated(&self, ty: &Ty, name: &str, depth: usize) -> Vec<Ty> {
        if depth > DEPTH {
            return Vec::new();
        }
        let Some(_nested) = self.nest() else { return Vec::new() };
        if let Ty::Traits(_, bindings) = ty {
            return dedup(
                bindings.iter().filter(|(n, _)| n == name).flat_map(|(_, t)| t.clone()).collect(),
            );
        }
        let mut types = Vec::new();
        for &block in self.blocks_of(ty).iter() {
            types.extend(self.block_associated(block, ty, name, depth + 1));
        }
        dedup(types)
    }

    /// The associated type `name` impl block `block` writes, for a value of
    /// type `ty`.
    fn block_associated(&self, block: (u32, u32), ty: &Ty, name: &str, depth: usize) -> Vec<Ty> {
        let declarations = self.summaries[block.0 as usize].offers().declarations();
        let mut types = Vec::new();
        let mut bound: Option<HashMap<String, Vec<Ty>>> = None;
        for &i in self.items_of_block(block).iter() {
            let item = &declarations[i as usize];
            if item.name != name || item.kind != STRUCT {
                continue;
            }
            let Some(written) = &item.ty else { continue };
            let bound = bound.get_or_insert_with(|| self.bindings_of(block, ty, depth));
            for at in self.scopes_of((block.0, i), vec![ty.clone()], bound) {
                types.extend(self.types_of(&at, written, depth + 1));
            }
        }
        types
    }

    // ------------------------------------------------------------------
    // An impl block's parameters
    // ------------------------------------------------------------------

    /// What impl block `block`'s generic parameters are, for a value of
    /// type `ty`: read by matching the block's type as written to it
    /// (`impl<T> Vec<T>` for a `Vec<Cell>`: `T` is `Cell`).
    fn bindings_of(&self, block: (u32, u32), ty: &Ty, depth: usize) -> HashMap<String, Vec<Ty>> {
        let of = &self.summaries[block.0 as usize].offers().impls()[block.1 as usize];
        let mut bound: HashMap<String, Vec<Ty>> = HashMap::new();
        let Some(written) = &of.self_type else { return bound };
        for (krate, _) in self.graph.places(&self.files[block.0 as usize]) {
            let at = Written { at: Module { krate, file: block.0, unit: of.unit }, alias: None };
            self.unify(written, ty, &of.generics, &at, &mut bound, depth + 1);
        }
        bound
    }

    /// Bind the generic parameters `params` that `pattern`, a type written
    /// where `written` says, takes where it is matched to `ty`. A pattern
    /// that names an alias is matched as the type the alias writes, each
    /// of the alias's parameters standing for the pattern's argument in
    /// its place (`impl<H, V> GridSplitter<H, V>`, where `type
    /// GridSplitter<H, V> = Grid<H, V, SplitterBehavior>;`).
    fn unify(
        &self,
        pattern: &TypeRef,
        ty: &Ty,
        params: &[String],
        written: &Written<'_>,
        bound: &mut HashMap<String, Vec<Ty>>,
        depth: usize,
    ) {
        if depth > DEPTH {
            return;
        }
        let Some(_nested) = self.nest() else { return };
        match (pattern, ty) {
            (TypeRef::Ref(inner), _) => self.unify(inner, ty, params, written, bound, depth + 1),
            (TypeRef::Path { path, args, .. }, _) => {
                if let [name] = path.segments.as_slice() {
                    if path.root.is_none() {
                        match written.alias {
                            // Inside an alias's type, its own parameters
                            // stand for what the pattern gives them.
                            Some(alias) => {
                                if let Some(i) = alias.params.iter().position(|p| p == name) {
                                    if let Some(arg) = alias.args.get(i) {
                                        self.unify(arg, ty, params, alias.outer, bound, depth + 1);
                                    }
                                    return;
                                }
                            }
                            None if params.contains(name) => {
                                let types = bound.entry(name.clone()).or_default();
                                if !types.contains(ty) {
                                    types.push(ty.clone());
                                }
                                return;
                            }
                            None => {}
                        }
                    }
                }
                // A path without arguments binds nothing: neither it nor
                // an alias it names has any to match (an alias's
                // parameters stand for none).
                if args.is_empty() {
                    return;
                }
                let declared = self.declarations_at(written.at, path);
                if let Ty::Declared(d, given) = ty {
                    if declared.contains(d) {
                        for (i, arg) in args.iter().enumerate() {
                            for t in given.get(i).into_iter().flatten() {
                                self.unify(arg, t, params, written, bound, depth + 1);
                            }
                        }
                        return;
                    }
                }
                for a in declared {
                    let alias = self.declaration(a);
                    let Some(aliased) = alias.ty.as_ref().filter(|_| is_alias(alias)) else {
                        continue;
                    };
                    let stands = Alias { params: &alias.generics, args, outer: written };
                    for &(krate, _) in self.graph.places(&self.files[a.0 as usize]).iter() {
                        let inner = Written {
                            at: Module { krate, file: a.0, unit: alias.unit },
                            alias: Some(stands),
                        };
                        self.unify(aliased, ty, params, &inner, bound, depth + 1);
                    }
                }
            }
            (TypeRef::Slice(inner), Ty::Slice(elements))
            | (TypeRef::Array(inner), Ty::Array(elements)) => {
                for t in elements.iter() {
                    self.unify(inner, t, params, written, bound, depth + 1);
                }
            }
            (TypeRef::Tuple(patterns), Ty::Tuple(elements)) => {
                for (p, types) in patterns.iter().zip(elements.iter()) {
                    for t in types {
                        self.unify(p, t, params, written, bound, depth + 1);
                    }
                }
            }
            _ => {}
        }
    }

    /// The declarations alias `d` stands for, through the aliases it names
    /// in turn; `d` itself where it is no alias. An alias of a type no
    /// declaration is (`type Id = u32;`, `type Pair = (u8, u8);`) stands
    /// for none.
    pub(super) fn unaliased(&self, d: (u32, u32)) -> Vec<(u32, u32)> {
        let mut found = Vec::new();
        let mut seen = vec![d];
        let mut next = vec![d];
        while let Some(d) = next.pop() {
            let declaration = self.declaration(d);
            if !is_alias(declaration) {
                found.push(d);
                continue;
            }
            let Some(TypeRef::Path { path, .. }) = &declaration.ty else { continue };
            for &(krate, _) in self.graph.places(&self.files[d.0 as usize]).iter() {
                let at = Module { krate, file: d.0, unit: declaration.unit };
                let Ok(targets) = self.walk(at, path) else { continue };
                for target in targets.iter() {
                    if let (Some(to), None) = (target.declaration, target.module) {
                        if !seen.contains(&to) && seen.len() < DEPTH {
                            seen.push(to);
                            next.push(to);
                        }
                    }
                }
            }
        }
        found
    }

    // ------------------------------------------------------------------
    // Traits
    // ------------------------------------------------------------------

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
            for at in self.scopes_of(t, Vec::new(), &HashMap::new()) {
                for supertrait in self.declarations_at(at.at, path) {
                    if self.declaration(supertrait).kind == CLASS {
                        found.extend(self.trait_items(supertrait, name, visited));
                    }
                }
            }
        }
        found
    }

    /// The names of trait `t`'s items, and of its supertraits'.
    fn trait_names(&self, t: (u32, u32)) -> Names {
        if let Some(found) = self.trait_names.borrow().get(&t) {
            return found.clone();
        }
        let mut names = HashSet::new();
        let mut visited = HashSet::new();
        let mut pending = vec![t];
        while let Some(t) = pending.pop() {
            if !visited.insert(t) {
                continue;
            }
            for i in self.items_inside(t.0, t.1) {
                names.insert(self.declaration((t.0, i)).name.clone());
            }
            for written in &self.declaration(t).bounds {
                let Some(path) = written.path() else { continue };
                for at in self.scopes_of(t, Vec::new(), &HashMap::new()) {
                    for supertrait in self.declarations_at(at.at, path) {
                        if self.declaration(supertrait).kind == CLASS {
                            pending.push(supertrait);
                        }
                    }
                }
            }
        }
        let names = Rc::new(names);
        if self.depth.get() == 0 {
            self.trait_names.borrow_mut().insert(t, names.clone());
        }
        names
    }

    /// The traits impl block `block` implements: its trait's path, read
    /// where the block is.
    pub(super) fn impl_traits(&self, block: (u32, u32)) -> Declarations {
        if let Some(found) = self.impl_traits.borrow().get(&block) {
            return found.clone();
        }
        // Its trait's path, walked, may name a member of a type whose
        // members ask for this block's trait again: met again, it names
        // nothing there.
        if !self.impl_traits_finding.borrow_mut().insert(block) {
            return Declarations::default();
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
        self.impl_traits_finding.borrow_mut().remove(&block);
        let found = Rc::new(found);
        if self.depth.get() == 0 {
            // Found while no lookup is under way: nothing it met was
            // provisional.
            self.impl_traits.borrow_mut().insert(block, found.clone());
        }
        found
    }

    /// The traits type `d` derives, its derive paths read where it is.
    pub(super) fn derived(&self, d: (u32, u32)) -> Vec<(u32, u32)> {
        let mut found = Vec::new();
        for path in &self.declaration(d).derives {
            for t in self.derive_implements(d, path) {
                if !found.contains(&t) {
                    found.push(t);
                }
            }
        }
        found
    }

    /// The traits the derive `path`, said of declaration `d`, implements:
    /// what it names, read where `d` stands; or, naming no trait, the
    /// built-in derive's, by its name.
    pub(super) fn derive_implements(&self, d: (u32, u32), path: &Path) -> Vec<(u32, u32)> {
        let mut found = Vec::new();
        for at in self.scopes_of(d, Vec::new(), &HashMap::new()) {
            let mut traits: Vec<(u32, u32)> = self
                .declarations_at(at.at, path)
                .into_iter()
                .filter(|&t| self.declaration(t).kind == CLASS)
                .collect();
            if traits.is_empty() {
                if let Some(name) = path.segments.last() {
                    traits.extend(self.built_in_derive(at.at.krate, name).iter().copied());
                }
            }
            for t in traits {
                if !found.contains(&t) {
                    found.push(t);
                }
            }
        }
        found
    }

    /// The trait the built-in derive `name` implements, as crate `krate`'s
    /// model names it ([`CrateInfo::derive_traits`]); none where it names
    /// no such derive.
    ///
    /// [`CrateInfo::derive_traits`]: crate::CrateInfo::derive_traits
    fn built_in_derive(&self, krate: u32, name: &str) -> Declarations {
        let key = (krate, name.to_string());
        if let Some(found) = self.built_in_derives.borrow().get(&key) {
            return found.clone();
        }
        let info = &self.graph.crates()[krate as usize];
        let mut found = Vec::new();
        for (_, path) in info.derive_traits.iter().filter(|(derive, _)| derive == name) {
            for d in self.lang_declarations(krate, path) {
                if self.declaration(d).kind == CLASS && !found.contains(&d) {
                    found.push(d);
                }
            }
        }
        let found = Rc::new(found);
        if self.depth.get() == 0 {
            self.built_in_derives.borrow_mut().insert(key, found.clone());
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

    pub(super) fn declaration(&self, d: (u32, u32)) -> &Declaration {
        &self.summaries[d.0 as usize].offers().declarations()[d.1 as usize]
    }

    /// The declarations `path` names from module `at`.
    fn declarations_at(&self, at: Module, path: &Path) -> Vec<(u32, u32)> {
        match self.walk(at, path) {
            Ok(targets) => targets.iter().filter_map(|t| t.declaration).collect(),
            Err(_) => Vec::new(),
        }
    }

    /// What `path`, written as a type in module `at`, declares, but
    /// modules, which are no type.
    fn type_declarations_at(&self, at: Module, path: &Path) -> Vec<(u32, u32)> {
        match self.walk(at, path) {
            Ok(targets) => targets
                .iter()
                .filter(|t| t.module.is_none())
                .filter_map(|t| t.declaration)
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    /// The declaration declaration `d` is declared inside (a variant's
    /// enum, a field's struct, an item's trait), not an impl block.
    pub(super) fn owner_of(&self, d: (u32, u32)) -> Option<(u32, u32)> {
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
    /// in: its module; the generic parameters of it and of what it is in
    /// (its impl block, the type or trait it is declared inside), each with
    /// the types `bound` gives it; and `self_type` for `Self`.
    fn scopes_of(
        &self,
        d: (u32, u32),
        self_type: Vec<Ty>,
        bound: &HashMap<String, Vec<Ty>>,
    ) -> Vec<Rc<Scope>> {
        let declaration = self.declaration(d);
        let summary = &self.summaries[d.0 as usize];
        let mut names = declaration.generics.clone();
        if let Some(block) = declaration.impl_block {
            if let Some(of) = summary.offers().impls().get(block as usize) {
                names.extend(of.generics.iter().cloned());
            }
        }
        if let Some(owner) = self.owner_of(d) {
            names.extend(self.declaration(owner).generics.iter().cloned());
        }
        let generics: Vec<(String, Vec<Ty>)> = names
            .into_iter()
            .map(|name| {
                let types = bound.get(&name).cloned().unwrap_or_default();
                (name, types)
            })
            .collect();
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

    /// Where impl block `block` writes its type, in each crate its file is
    /// in, its parameters as `bound` gives them.
    fn block_scopes(
        &self,
        block: (u32, u32),
        bound: &HashMap<String, Vec<Ty>>,
        self_type: Vec<Ty>,
    ) -> Vec<Rc<Scope>> {
        let of = &self.summaries[block.0 as usize].offers().impls()[block.1 as usize];
        let generics: Vec<(String, Vec<Ty>)> = of
            .generics
            .iter()
            .map(|name| (name.clone(), bound.get(name).cloned().unwrap_or_default()))
            .collect();
        self.graph
            .places(&self.files[block.0 as usize])
            .iter()
            .map(|&(krate, _)| {
                Rc::new(Scope {
                    at: Module { krate, file: block.0, unit: of.unit },
                    generics: generics.clone(),
                    self_type: self_type.clone(),
                })
            })
            .collect()
    }
}

/// Where a pattern [`Resolver::unify`] matches is written: the module its
/// names are read in, and, inside an alias's type, what the alias's
/// parameters stand for.
struct Written<'a> {
    at: Module,
    alias: Option<Alias<'a>>,
}

/// An alias's parameters, the pattern's arguments each stands for, and
/// where those are written.
#[derive(Clone, Copy)]
struct Alias<'a> {
    params: &'a [String],
    args: &'a [TypeRef],
    outer: &'a Written<'a>,
}

/// Whether `d` is an alias: a type declaration writing the type it is.
/// An impl block's associated type is none.
fn is_alias(d: &Declaration) -> bool {
    d.kind == STRUCT && d.ty.is_some() && d.impl_block.is_none()
}

/// `declarations`, sorted, once each.
fn sorted(mut declarations: Vec<(u32, u32)>) -> Vec<(u32, u32)> {
    declarations.sort_unstable();
    declarations.dedup();
    declarations
}

/// The generic parameters `params` of a type, bound positionally to the
/// types `args` gives each.
fn bind(params: &[String], args: &[Vec<Ty>]) -> HashMap<String, Vec<Ty>> {
    params
        .iter()
        .zip(args.iter())
        .filter(|(_, types)| !types.is_empty())
        .map(|(p, types)| (p.clone(), types.clone()))
        .collect()
}

/// Whether `path` begins with a generic parameter where `scope` is
/// (`T`, `T::default`, `T::Output`).
fn from_parameter(scope: &Scope, path: &Path) -> bool {
    path.root.is_none()
        && path.segments.first().is_some_and(|first| scope.parameter(first).is_some())
}

/// `types`, each once, in their first order.
fn dedup(types: Vec<Ty>) -> Vec<Ty> {
    let mut seen: HashSet<Ty> = HashSet::new();
    types.into_iter().filter(|t| seen.insert(t.clone())).collect()
}
