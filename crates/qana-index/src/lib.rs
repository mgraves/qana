//! What a project index keeps about one file of a `.qana`-defined
//! language: its [`Summary`].
//!
//! A summary is computed from the file's text and its language's
//! certified grammar, and from nothing else: no other file, no path, no
//! build system. So the same text is summarized once, wherever it is, and a
//! summary can be kept by its text's hash ([`summary_key`] names the rest of
//! what it depends on).
//!
//! It has three parts, each with a fingerprint of its own, so that a change
//! to a file reaches only as far as the part it changed:
//!
//! - [`Offers`]: what other files may depend on. The units the file holds
//!   ([`Unit`]); its declarations that a body does not hold, each by its
//!   name, kind, namespace, the declarations it is inside, an ordinal
//!   among its namesakes there, its signature, its unit, its
//!   [`Visibility`] and the impl block it is a member of; what it imports
//!   for others to see (its re-exports, [`Import`]); and its impl blocks,
//!   each by the type it is of and the trait it implements ([`Impl`]).
//!   Nothing in it is a position, and it is in a fixed order, so moving
//!   code leaves it alone.
//! - [`Needs`]: what only this file depends on. What it imports for
//!   itself, and the names it refers to that nothing in it defines: they
//!   escape to other files, each with its unit and its path.
//! - [`Locations`]: where each offered declaration, unit, import and impl
//!   block is, and the words the file uses. Navigation reads them; nothing
//!   is resolved from them.
//!
//! A body edit, most edits, changes only the locations.
//!
//! What the parts hold comes from the grammar: a declaration is a construct
//! its outline lists (`@outline`) whose name is a definition (`@def`), with
//! the outline's kind and the binding's namespace; a construct whose name
//! defines nothing (Rust's `impl` blocks) is a container only. Units,
//! paths, visibilities, imports and impl blocks are what the binding's
//! `@unit`, `@root`, `@vis`, `@visibility`, `@import`, `@import_glob`,
//! `@import_prefix` and `@impl` say. A declaration, unit, import or impl
//! block inside a `function` declaration is a body's, and is left out; a
//! body's references are needs of the unit the function is in.
//!
//! Units are numbered within the file: [`Unit`]s are in [`Offers::units`],
//! and a unit is named by its index there, or `None` for the file's own
//! unit, whose name and place a project model knows.

use std::collections::{BTreeSet, HashMap};

use qana_lang::{EmbeddedLang, GrammarArtifact};
use qana_sem::{SemDb, Target};
use serde::{Deserialize, Serialize};

pub use conditions::{cfg_option_values, CfgOptionValue, Condition, Configuration, Holds};
pub use expand::{
    expand, BuiltinMacro, ExpandRequest, ExpansionSummary, MacroSource, MacroTable,
    NestedExpansion, SitePlace,
};
pub use expanded::{
    expand_text, expanded_file, invocations, ExpandedFile, ExpandedText, InvocationAt,
};
pub use graph::{CrateInfo, CrateSource, UnitGraph};
pub use members::{
    member_at, member_at_in, members, members_in, path_at, MemberAccess, MemberKind, Receiver,
};
pub use merge::{merge, merge_where, Expansions};
pub use qana_mbe::{Edition, Form, TokenStream, RECURSION_LIMIT};
pub use qana_sem::{Path, Root};
pub use references::{escaping, references, Reference};
pub use resolve::{Implementation, MacroResolution, Resolution, Resolver, RootMacros};
pub use textual::{is_textual, place_condition, seen_here, textual_at, Seen, TextualLink};

mod conditions;
mod expand;
mod expanded;
mod graph;
mod members;
mod merge;
mod references;
mod resolve;
mod textual;
mod units;

include!(concat!(env!("OUT_DIR"), "/summarizer.rs"));

/// The outline kind of a construct whose declarations are its body's.
const FUNCTION: &str = "function";

/// What a project index keeps about one file. See the [crate docs](crate).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    offers: Offers,
    needs: Needs,
    locations: Locations,
}

impl Summary {
    /// The declarations other files may depend on.
    pub fn offers(&self) -> &Offers {
        &self.offers
    }

    /// The names the file needs from other files.
    pub fn needs(&self) -> &Needs {
        &self.needs
    }

    /// Where the offered declarations are, and the file's words.
    pub fn locations(&self) -> &Locations {
        &self.locations
    }
}

/// What a file offers other files, each in a fixed order: its units, by
/// their names and their parents'; its declarations, by the declarations
/// they are inside, then name, kind and ordinal; its re-exports and impl
/// blocks, by what they say.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Offers {
    attributes: Vec<Attribute>,
    open: bool,
    clean: bool,
    textual: Vec<TextualLink>,
    units: Vec<Unit>,
    declarations: Vec<Declaration>,
    reexports: Vec<Import>,
    impls: Vec<Impl>,
    invocations: Vec<Invocation>,
    macros: Vec<MacroDef>,
    fingerprint: u64,
}

impl Offers {
    /// The attributes said of the file's own unit (Rust's inner attributes
    /// at its top), in the file's order.
    pub fn attributes(&self) -> &[Attribute] {
        &self.attributes
    }

    /// Whether the file's own unit is open: an invocation whose expansion
    /// may declare names (`@expands`) stands among its items, or the text
    /// does not parse cleanly, so that a name it has may be missing here.
    pub fn open(&self) -> bool {
        self.open
    }

    /// The file's own unit's textual links, in its order: its textual
    /// definitions (Rust's `macro_rules!`) and its child modules, what a
    /// name after each sees ([`crate::textual_at`] has the rules).
    pub fn textual(&self) -> &[TextualLink] {
        &self.textual
    }

    /// The units the file holds, besides its own; a unit's index here is
    /// what names it.
    pub fn units(&self) -> &[Unit] {
        &self.units
    }

    /// The declarations, in the offers' order; [`Locations`] follow it.
    pub fn declarations(&self) -> &[Declaration] {
        &self.declarations
    }

    /// The imports others may see: what the file re-exports.
    pub fn reexports(&self) -> &[Import] {
        &self.reexports
    }

    /// The impl blocks; an impl block's index here is what names it.
    pub fn impls(&self) -> &[Impl] {
        &self.impls
    }

    /// Whether the text parses as the grammar has it, with no repair: one
    /// that does not may hold what the summary misses, and each of its
    /// units is open.
    pub fn clean(&self) -> bool {
        self.clean
    }

    /// The invocations among items whose expansions may declare names
    /// there (a module's, an impl block's or a trait's), in the file's
    /// order; an invocation's index here is what names it.
    pub fn invocations(&self) -> &[Invocation] {
        &self.invocations
    }

    /// The macros the file defines outside bodies, in the order of the
    /// declarations they are.
    pub fn macros(&self) -> &[MacroDef] {
        &self.macros
    }

    /// A hash of everything the offers hold: equal offers have equal
    /// fingerprints.
    pub fn fingerprint(&self) -> u64 {
        self.fingerprint
    }
}

/// One declaration a file offers, by what it is, not where.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Declaration {
    /// The names of the constructs it is inside, outermost first: its
    /// module, its type (`["Cell"]` for a method in `impl Cell`).
    pub container: Vec<String>,
    /// Its name.
    pub name: String,
    /// Its outline kind (`function`, `struct`, `module`, `class`,
    /// `constant`, `variable`, `field`).
    pub kind: String,
    /// The binding's namespace; empty for the default one.
    pub namespace: String,
    /// Among declarations of the same container, name and kind, its place
    /// in the file: 0 for the first. What tells two `impl` blocks' `new`
    /// apart without a position.
    pub ordinal: u32,
    /// Its head as written: from the start of its construct as the
    /// grammar has it (or of the line its name is on, if that is later)
    /// to the first `{` or `;` outside parentheses and brackets, its
    /// spaces and line breaks each made one space (`fn new(width: f32) ->
    /// Cell`). What the grammar puts outside the construct (Rust's
    /// visibility, before an item) is not in it.
    pub signature: String,
    /// The unit it is declared in: an index into [`Offers::units`], or
    /// `None` for the file's own.
    pub unit: Option<u32>,
    /// Who may see it. A member's (a field's, an impl block's item's) is
    /// its own, and what its type's visibility allows bounds it.
    pub visibility: Visibility,
    /// The impl block it is a member of: an index into [`Offers::impls`].
    pub impl_block: Option<u32>,
    /// Its type as written, where it writes one (a field's, a const's, a
    /// static's, an alias's target).
    pub ty: Option<TypeRef>,
    /// The type a call of it gives, as written (a function's return
    /// type); `None` where none is written.
    pub returns: Option<TypeRef>,
    /// What it derives (Rust's `#[derive(…)]`), by the paths written, each
    /// with the condition it is derived under beside the declaration's own
    /// (a `#[cfg_attr(p, derive(…))]`'s), if any.
    pub derives: Vec<(Path, Option<Condition>)>,
    /// Its generic parameters' names, in order: types' and constants',
    /// not lifetimes' (`struct Grid<'a, T, const N: usize>`: `T`, `N`), as
    /// [`TypeRef::Path`]'s arguments are.
    pub generics: Vec<String>,
    /// The traits it is bounded by, as written: a trait's supertraits
    /// (`trait Shape: Clone`), an associated type's bounds (`type Item:
    /// Display;`). A relaxed bound (`?Sized`) is none.
    pub bounds: Vec<TypeRef>,
    /// Its positional fields' types, in order: a tuple struct's (`.0`,
    /// `.1`), a tuple variant's (`Some(T)`'s `T`).
    pub positional: Vec<TypeRef>,
    /// Its parameters' types as written, in order (a function's): `None`
    /// for the value a method is called on where none is written
    /// (`&self`), so that a call through its path (`Cell::width(&c)`)
    /// lines up with them.
    #[serde(default)]
    pub params: Vec<Option<TypeRef>>,
    /// The condition it is under: those of the constructs holding it
    /// (Rust's `#[cfg]`s), all of them as one; `None` where it is under
    /// none.
    #[serde(default)]
    pub condition: Option<Condition>,
}

/// A type as a declaration writes it: read, never inferred, its paths
/// as written (resolved where it is used).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum TypeRef {
    /// A type a path names (`Vec<Cell>`, `Self`, `T`), with the generic
    /// arguments and associated-type bindings written on it (`Iterator<Item
    /// = u8>`). Lifetimes are left out; a constant (`Buf<u8, 4>`'s `4`)
    /// is [`TypeRef::Other`] in its place, so that the arguments line up
    /// with [`Declaration::generics`].
    Path { path: Path, args: Vec<TypeRef>, bindings: Vec<(String, TypeRef)> },
    /// A reference or a pointer to a type (`&T`, `&mut T`, `*const T`).
    Ref(Box<TypeRef>),
    /// A slice of a type (`[T]`).
    Slice(Box<TypeRef>),
    /// An array of a type (`[T; 4]`).
    Array(Box<TypeRef>),
    /// A tuple of types; `()` is the empty one.
    Tuple(Vec<TypeRef>),
    /// A trait object (`dyn Display + Send`), by its traits.
    Dyn(Vec<TypeRef>),
    /// A type known by its traits only (`impl Iterator<Item = u8>`).
    Impl(Vec<TypeRef>),
    /// A type a project index does not follow: a function pointer, `!`,
    /// `_`, what a macro expands to, a qualified path; or a generic
    /// argument that is a constant.
    Other,
}

impl TypeRef {
    /// The path that names it, where one does: a [`TypeRef::Path`]'s.
    pub fn path(&self) -> Option<&Path> {
        match self {
            TypeRef::Path { path, .. } => Some(path),
            _ => None,
        }
    }
}

/// Who may see a declaration or an import.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Visibility {
    /// Whoever may see the unit it is in.
    Public,
    /// The unit the path names, from the unit it is in, and that unit's
    /// descendants: `crate` for Rust's `pub(crate)`, `super` for
    /// `pub(super)`, `self` for no visibility at all (the default).
    Within(Path),
}

/// A unit a file holds besides its own: a module, whose body is in the
/// file (`mod a { … }`) or in another one (`mod a;`, which a project model
/// finds).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Unit {
    /// Its name.
    pub name: String,
    /// The unit it is declared in: an index into [`Offers::units`], or
    /// `None` for the file's own.
    pub parent: Option<u32>,
    /// Whether its body is in this file.
    pub inline: bool,
    /// Who may see it.
    pub visibility: Visibility,
    /// The attributes said of it: those on its declaration, then those
    /// at the top of its body where its body is here (Rust's outer, then
    /// inner, attributes), in the file's order.
    pub attributes: Vec<Attribute>,
    /// Whether it is open: an invocation whose expansion may declare names
    /// (`@expands`) stands among its items, or its file's text does not
    /// parse cleanly, so that a name it has may be missing here.
    pub open: bool,
    /// Its textual links, in its order, where its body is here (see
    /// [`Offers::textual`]).
    pub textual: Vec<TextualLink>,
    /// The condition it is under: those of the constructs holding it
    /// (Rust's `#[cfg]`s), all of them as one; `None` where it is under
    /// none.
    #[serde(default)]
    pub condition: Option<Condition>,
}

/// An attribute said of a unit or an import: the path that names it,
/// whether it holds only under a condition (Rust's `cfg_attr(pred, attr)`,
/// whose condition a summary does not decide), and the paths its arguments
/// are, where the grammar reads them (`@attribute(path, args)`).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Attribute {
    /// The path that names it (`no_std`, `rustfmt::skip`).
    pub path: Path,
    /// Whether it holds only under a condition.
    pub conditional: bool,
    /// Its arguments, in order: Rust's `#[macro_use(a, b)]` has `a` and
    /// `b`. Empty where it has none, or where the grammar reads none.
    #[serde(default)]
    pub args: Vec<Path>,
    /// The condition it holds under, where it holds only under one (a
    /// conditional attribute's predicate: Rust's `cfg_attr(pred, …)`, all
    /// of them where one holds another); `None` where it holds wherever
    /// what it is said of is.
    #[serde(default)]
    pub condition: Option<Condition>,
}

/// An invocation among a unit's items, or an impl block's or a trait's,
/// whose expansion may declare names there: what it invokes and where its
/// expansion is read. What its macro is, the textual links before it say
/// ([`TextualLink::Invocation`]), and its path; what it is passed, its
/// text (from [`Locations::invocation_span`]).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Invocation {
    /// The unit it stands in: an index into [`Offers::units`], or `None`
    /// for the file's own.
    pub unit: Option<u32>,
    /// The names of the constructs it stands in, outermost first, as a
    /// declaration there has them ([`Declaration::container`]).
    pub container: Vec<String>,
    /// The impl block it stands among the items of: an index into
    /// [`Offers::impls`].
    pub impl_block: Option<u32>,
    /// Whether it stands among an impl block's or a trait's items, not a
    /// unit's: what its expansion declares are members, not the unit's
    /// names.
    pub members: bool,
    /// The macro's path; `None` where it spells none.
    pub path: Option<Path>,
    /// The binding's namespace the path names a macro in.
    pub namespace: String,
    /// The grammar's rule its expansion is parsed at (an entry: Rust's
    /// `items_opt`, `assoc_items_opt`).
    pub entry: String,
    /// A hash of its text, from its path to its end: what its expansion
    /// depends on, besides its macro.
    pub text: u64,
    /// The attributes said of it, in the file's order.
    pub attributes: Vec<Attribute>,
    /// The expansions it stands in, outermost first, each by the
    /// invocation it expands (its index among those of the level above)
    /// and which of that invocation's macros it is: empty for the file's
    /// own. A project index's merged summary ([`merge`]) has these.
    pub within: Vec<(u32, u32)>,
    /// Its index among the invocations of the text it stands in (the
    /// file's, or its expansion's).
    pub index: u32,
    /// The condition it is under: those of the constructs holding it
    /// (Rust's `#[cfg]`s), all of them as one; `None` where it is under
    /// none.
    #[serde(default)]
    pub condition: Option<Condition>,
}

/// A macro the file defines outside bodies (Rust's `macro_rules!`, and a
/// `macro` item): the declaration it is, how it is written, a hash of its
/// body (what follows its name, [`Locations::macro_body_span`]), and the
/// attributes said of it (Rust's `#[macro_export]`, `#[rustc_builtin_macro]`).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MacroDef {
    /// The declaration it is: an index into [`Offers::declarations`].
    pub declaration: u32,
    pub form: Form,
    pub body: u64,
    pub attributes: Vec<Attribute>,
    /// The expansion that defines it, as [`Invocation::within`] says one:
    /// empty for the file's own.
    pub within: Vec<(u32, u32)>,
    /// Its index among the macros of the text it stands in.
    pub index: u32,
    /// The condition it is under: those of the constructs holding it
    /// (Rust's `#[cfg]`s), all of them as one; `None` where it is under
    /// none.
    #[serde(default)]
    pub condition: Option<Condition>,
}

/// What a unit imports: one declaration, under a name, or every name of a
/// unit (a glob).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Import {
    /// The unit it is made in: an index into [`Offers::units`], or `None`
    /// for the file's own.
    pub unit: Option<u32>,
    /// What it imports, from where it is made.
    pub path: Path,
    /// The name it is imported under: `None` for a glob, and for an import
    /// that names nothing (Rust's `as _`).
    pub name: Option<String>,
    /// Whether it imports every name of the unit `path` names.
    pub glob: bool,
    /// Who may see it: an import others may see is a re-export.
    pub visibility: Visibility,
    /// The attributes said of it (on the item that makes it), in the
    /// file's order.
    pub attributes: Vec<Attribute>,
    /// Whether it loads the library its path names (Rust's `extern
    /// crate`): made at a crate's root, its name is one every module of
    /// the crate may use.
    pub loads: bool,
    /// The condition it is under: those of the constructs holding it
    /// (Rust's `#[cfg]`s), all of them as one; `None` where it is under
    /// none.
    #[serde(default)]
    pub condition: Option<Condition>,
}

/// An impl block: what it is of, and of which trait. Its items, and the
/// associated types it binds (`type Item = &'a T;`), are declarations whose
/// [`Declaration::impl_block`] is it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Impl {
    /// The unit it is in: an index into [`Offers::units`], or `None` for
    /// the file's own.
    pub unit: Option<u32>,
    /// The type it is of, as written (`Iter<'a, T>`, `[T]`): as the
    /// grammar's `@type_ref` forms read it, or, where the grammar writes
    /// none there, as the path it spells. `None` where neither reads one.
    pub self_type: Option<TypeRef>,
    /// The trait it implements, if it implements one.
    pub trait_path: Option<Path>,
    /// Its generic parameters' names, in order, as
    /// [`Declaration::generics`] (`impl<'a, T> Iterator for Iter<'a, T>`:
    /// `T`).
    pub generics: Vec<String>,
    /// The condition it is under: those of the constructs holding it
    /// (Rust's `#[cfg]`s), all of them as one; `None` where it is under
    /// none.
    #[serde(default)]
    pub condition: Option<Condition>,
}

/// What only a file depends on: what it imports for itself, and the names
/// it refers to that nothing in it defines, which escape to other files.
/// Each sorted, each once.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Needs {
    imports: Vec<Import>,
    escaping: Vec<EscapingName>,
    fingerprint: u64,
}

impl Needs {
    /// The imports no other unit may see, sorted.
    pub fn imports(&self) -> &[Import] {
        &self.imports
    }

    /// The escaping names, sorted.
    pub fn escaping(&self) -> &[EscapingName] {
        &self.escaping
    }

    /// A hash of the imports and escaping names: equal needs have equal
    /// fingerprints.
    pub fn fingerprint(&self) -> u64 {
        self.fingerprint
    }
}

/// A name a file refers to and does not define: where it is referred to
/// from, and the path that names it. A name in an import's path is not
/// one: the import says it in full.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EscapingName {
    /// The unit the reference is in: an index into [`Offers::units`], or
    /// `None` for the file's own.
    pub unit: Option<u32>,
    /// The binding's namespace the reference looks in; empty for the
    /// default one.
    pub namespace: String,
    /// The path whose last segment is the name: the name alone, or after
    /// its qualifiers (`grid::Cell` for `Cell` in `grid::Cell::new`).
    pub path: Path,
}

/// Where each offered declaration is, in the offers' order, where each
/// unit, import and impl block is, and the words the file uses. Byte
/// offsets are into the file's text; lines and columns count from 0,
/// columns in characters.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Locations {
    names: Vec<(u32, u32)>,
    items: Vec<(u32, u32)>,
    positions: Vec<(u32, u32)>,
    units: Vec<(u32, u32)>,
    unit_items: Vec<(u32, u32)>,
    reexports: Vec<(u32, u32)>,
    imports: Vec<(u32, u32)>,
    impls: Vec<(u32, u32)>,
    impl_heads: Vec<units::ImplHead>,
    derives: Vec<Vec<(u32, u32)>>,
    invocations: Vec<(u32, u32)>,
    invocation_paths: Vec<Option<(u32, u32)>>,
    invocation_path_chars: Vec<u32>,
    invocation_items: Vec<(u32, u32)>,
    invocation_positions: Vec<(u32, u32)>,
    macro_bodies: Vec<(u32, u32)>,
    declared_by: Vec<Option<u32>>,
    words: Vec<String>,
    #[serde(default)]
    condition_regions: Vec<((u32, u32), Condition)>,
    fingerprint: u64,
}

impl Locations {
    /// The byte span of the `i`th offered declaration's name.
    pub fn name_span(&self, i: usize) -> (u32, u32) {
        self.names[i]
    }

    /// The byte span of the `i`th offered declaration's whole construct.
    pub fn item_span(&self, i: usize) -> (u32, u32) {
        self.items[i]
    }

    /// The line and character column the `i`th offered declaration's name
    /// starts at.
    pub fn position(&self, i: usize) -> (u32, u32) {
        self.positions[i]
    }

    /// The byte span of the `i`th offered unit's name.
    pub fn unit_span(&self, i: usize) -> (u32, u32) {
        self.units[i]
    }

    /// The byte span of the item the `i`th offered unit is declared in:
    /// its declaration, with what the grammar puts before it in the list
    /// it is an element of (Rust's attributes and visibility), where a
    /// project model reads what says where its body is (`#[path]`).
    pub fn unit_item_span(&self, i: usize) -> (u32, u32) {
        self.unit_items[i]
    }

    /// The byte span of the `i`th re-export.
    pub fn reexport_span(&self, i: usize) -> (u32, u32) {
        self.reexports[i]
    }

    /// The byte span of the `i`th of the needs' imports.
    pub fn import_span(&self, i: usize) -> (u32, u32) {
        self.imports[i]
    }

    /// The byte span of the `i`th impl block.
    pub fn impl_span(&self, i: usize) -> (u32, u32) {
        self.impls[i]
    }

    /// The byte span of the type the `i`th impl block's head names (`Cell`
    /// in `impl Display for Cell`), where the grammar says which child it
    /// is.
    pub fn impl_type_span(&self, i: usize) -> Option<(u32, u32)> {
        self.impl_heads[i].0
    }

    /// The byte span of the trait the `i`th impl block's head names
    /// (`Display` in `impl Display for Cell`), where it implements one.
    pub fn impl_trait_span(&self, i: usize) -> Option<(u32, u32)> {
        self.impl_heads[i].1
    }

    /// The byte spans of the paths the `i`th offered declaration derives
    /// (`Clone` in `#[derive(Clone)]`), as [`Declaration::derives`] holds
    /// them.
    pub fn derive_spans(&self, i: usize) -> &[(u32, u32)] {
        &self.derives[i]
    }

    /// The byte span of the `i`th invocation, from its path to its end:
    /// what it passes its macro is in it.
    pub fn invocation_span(&self, i: usize) -> (u32, u32) {
        self.invocations[i]
    }

    /// The byte span of the `i`th invocation's path: where what its
    /// macro writes is, where the invocation does not write it.
    pub fn invocation_path_span(&self, i: usize) -> Option<(u32, u32)> {
        self.invocation_paths[i]
    }

    /// How many characters the `i`th invocation's path is written in (its
    /// whole text's, where it has no path): what is selected where a
    /// declaration its macro writes is.
    pub fn invocation_path_length(&self, i: usize) -> u32 {
        self.invocation_path_chars[i]
    }

    /// The byte span of the item the `i`th invocation stands in, with what
    /// the grammar puts before it there.
    pub fn invocation_item_span(&self, i: usize) -> (u32, u32) {
        self.invocation_items[i]
    }

    /// The line and character column the `i`th invocation starts at.
    pub fn invocation_position(&self, i: usize) -> (u32, u32) {
        self.invocation_positions[i]
    }

    /// The byte span of the `i`th macro's body: what follows its name.
    pub fn macro_body_span(&self, i: usize) -> (u32, u32) {
        self.macro_bodies[i]
    }

    /// The invocation the file writes whose expansion (or one nested in
    /// it) declares the `i`th offered declaration, in a merged summary
    /// ([`crate::merge`]): an index into [`Offers::invocations`]. `None`
    /// for a declaration the file's text writes.
    pub fn declared_by(&self, i: usize) -> Option<u32> {
        self.declared_by.get(i).copied().flatten()
    }

    /// The names the file defines or refers to, sorted, each once: where a
    /// search for a name's uses looks first.
    pub fn words(&self) -> &[String] {
        &self.words
    }

    /// Each construct of the file under a condition its attributes say,
    /// bodies' too, in the file's order: its byte span, and its own
    /// condition (those around it are the regions holding it).
    pub fn condition_regions(&self) -> &[((u32, u32), Condition)] {
        &self.condition_regions
    }

    /// The condition what stands at byte span `span` is under: those of
    /// the regions holding it, outermost first, all of them as one; `None`
    /// where none holds it.
    pub fn condition_at(&self, span: (u32, u32)) -> Option<Condition> {
        condition_at(&self.condition_regions, span)
    }

    /// A hash of the locations and words.
    pub fn fingerprint(&self) -> u64 {
        self.fingerprint
    }
}

/// The key to keep the summary of a file under, given its grammar's source,
/// with the file's text's own hash beside it: it changes with the grammar's
/// text, with the code that certifies and parses it
/// ([`GrammarArtifact::key`]), and with the code that summarizes.
pub fn summary_key(grammar_source: &str) -> u64 {
    let mut hash = Fnv::new();
    hash.u64(SUMMARIZER);
    hash.u64(GrammarArtifact::key(grammar_source));
    hash.finish()
}

/// The name the file is known by within the summarizer's own database.
const URI: &str = "file";

/// Summarize `text`, a file of `lang`'s language.
pub fn summarize(lang: &EmbeddedLang, text: &str) -> Summary {
    let session = lang.session(text);
    let tree = session.tree().expect("parsing is total").clone();
    summarize_tree(lang, text, tree)
}

/// Summarize `text` as `tree`, its parse (a file's, or an expansion's at
/// one of the grammar's entries).
pub(crate) fn summarize_tree(
    lang: &EmbeddedLang,
    text: &str,
    tree: std::sync::Arc<qana_grammar::GreenNode>,
) -> Summary {
    let mut db = SemDb::new(lang.binding.clone());
    db.set_types(lang.types.clone());
    db.set_macro_bodies(&lang.macros);
    db.set_tree(URI, tree.clone());
    let symbols = db.symbols(URI);
    let resolutions = db.resolve(URI);
    let mut read = units::read(&tree, &lang.binding, &lang.outline);
    // A text that does not parse cleanly may hold what the summary misses:
    // each of its units is open.
    let clean = parses_cleanly(&tree);
    if !clean {
        read.open = true;
        for (unit, _, _) in &mut read.units {
            unit.open = true;
        }
    }

    // Units, by their names and their parents', a parent before its
    // children (its names are a prefix of theirs); `unit_index` takes a
    // unit's walk index to its place.
    let mut unit_order: Vec<usize> = (0..read.units.len()).collect();
    let unit_key = |mut i: usize| -> Vec<&str> {
        let mut key = Vec::new();
        loop {
            let (unit, _, _) = &read.units[i];
            key.push(unit.name.as_str());
            match unit.parent {
                Some(parent) => i = parent as usize,
                None => break,
            }
        }
        key.reverse();
        key
    };
    unit_order.sort_by_cached_key(|&i| unit_key(i));
    let mut unit_place = vec![0u32; read.units.len()];
    for (place, &i) in unit_order.iter().enumerate() {
        unit_place[i] = place as u32;
    }
    let unit_index = |unit: Option<u32>| unit.map(|u| unit_place[u as usize]);
    let mut units: Vec<Unit> = Vec::new();
    let mut unit_spans: Vec<(u32, u32)> = Vec::new();
    let mut unit_items: Vec<(u32, u32)> = Vec::new();
    for &i in &unit_order {
        let (unit, name, item) = &read.units[i];
        units.push(Unit { parent: unit_index(unit.parent), ..unit.clone() });
        unit_spans.push(*name);
        unit_items.push(*item);
    }

    // Impl blocks, by what they say.
    type Read = (Impl, (u32, u32), units::ImplHead, usize);
    let mut impls: Vec<Read> = read
        .impls
        .iter()
        .enumerate()
        .map(|(i, (block, span, head))| {
            (Impl { unit: unit_index(block.unit), ..block.clone() }, *span, *head, i)
        })
        .collect();
    impls.sort_by(|a, b| a.0.cmp(&b.0));
    let mut impl_place = vec![0u32; impls.len()];
    for (place, (_, _, _, i)) in impls.iter().enumerate() {
        impl_place[*i] = place as u32;
    }
    let mut impl_spans: Vec<(u32, u32)> = Vec::new();
    let mut impl_heads: Vec<units::ImplHead> = Vec::new();
    let impls: Vec<Impl> = impls
        .into_iter()
        .map(|(block, span, head, _)| {
            impl_spans.push(span);
            impl_heads.push(head);
            block
        })
        .collect();

    // Imports, by what they say: those others may see are offered.
    let private = units::private();
    let mut reexports: Vec<(Import, (u32, u32))> = Vec::new();
    let mut imports: Vec<(Import, (u32, u32))> = Vec::new();
    for (import, span) in &read.imports {
        let import = Import { unit: unit_index(import.unit), ..import.clone() };
        if import.visibility == private {
            imports.push((import, *span));
        } else {
            reexports.push((import, *span));
        }
    }
    reexports.sort();
    imports.sort();
    let (reexports, reexport_spans): (Vec<Import>, Vec<(u32, u32)>) = reexports.into_iter().unzip();
    let (imports, import_spans): (Vec<Import>, Vec<(u32, u32)>) = imports.into_iter().unzip();

    // A definition's namespace, by its name's span.
    let namespaces: HashMap<(u32, u32), &str> =
        symbols.defs.iter().map(|d| (d.span, d.ns.as_str())).collect();
    let lines = LineStarts::of(text);

    // The outline in document order; containment is nesting.
    struct Found {
        declaration: Declaration,
        name: (u32, u32),
        item: (u32, u32),
        derives: Vec<(u32, u32)>,
    }
    let mut found: Vec<Found> = Vec::new();
    let outlined = outside_bodies(&tree, &lang.outline);
    for (symbol, container) in outlined.iter().cloned() {
        let defines = namespaces.get(&symbol.selection);
        let place = read.defs.get(&symbol.selection);
        if let (Some(namespace), Some(place)) = (defines, place) {
            found.push(Found {
                declaration: Declaration {
                    container,
                    name: symbol.name.clone(),
                    kind: symbol.kind.to_string(),
                    namespace: namespace.to_string(),
                    ordinal: 0,
                    signature: signature(text, symbol.span, symbol.selection),
                    unit: unit_index(place.unit),
                    visibility: place.visibility.clone(),
                    impl_block: place.impl_block.map(|i| impl_place[i as usize]),
                    ty: place.ty.clone(),
                    returns: place.returns.clone(),
                    derives: place
                        .derives
                        .iter()
                        .map(|(path, _, c)| (path.clone(), c.clone()))
                        .collect(),
                    generics: place.generics.clone(),
                    bounds: place.bounds.clone(),
                    positional: place.positional.clone(),
                    params: place.params.clone(),
                    condition: None,
                },
                name: symbol.selection,
                item: symbol.span,
                derives: place.derives.iter().map(|&(_, span, _)| span).collect(),
            });
        }
    }

    // Ordinals among namesakes, in document order.
    let mut seen: HashMap<(Vec<String>, String, String), u32> = HashMap::new();
    for f in &mut found {
        let d = &f.declaration;
        let count = seen.entry((d.container.clone(), d.name.clone(), d.kind.clone())).or_insert(0);
        f.declaration.ordinal = *count;
        *count += 1;
    }
    found.sort_by(|a, b| a.declaration.cmp(&b.declaration));

    // A reference the walk did not read as a path (one a macro's
    // expansion makes) is its name alone, in the unit around it. A member
    // access's name is none: a value's type says what it names. Nor is a
    // path from `Self`, which names what where it stands says.
    let escaping: Vec<EscapingName> = symbols
        .refs
        .iter()
        .zip(resolutions.iter())
        .filter(|(_, target)| **target == Target::Escapes)
        .filter_map(|(r, _)| match read.refs.get(&r.span) {
            Some(place) if place.imported || place.member => None,
            Some(place) => {
                place.path.clone().filter(|p| p.root != Some(Root::SelfType)).map(|path| {
                    EscapingName { unit: unit_index(place.unit), namespace: r.ns.clone(), path }
                })
            }
            None => Some(EscapingName {
                unit: unit_index(read.unit_at(r.span)),
                namespace: r.ns.clone(),
                path: Path { root: None, segments: vec![r.name.clone()] },
            }),
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let words: Vec<String> = symbols
        .defs
        .iter()
        .map(|d| d.name.clone())
        .chain(symbols.refs.iter().map(|r| r.name.clone()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    let declarations: Vec<Declaration> = found.iter().map(|f| f.declaration.clone()).collect();
    let names: Vec<(u32, u32)> = found.iter().map(|f| f.name).collect();

    // Each module's textual links, a unit by its place.
    let by_name: HashMap<(u32, u32), u32> =
        names.iter().enumerate().map(|(i, &span)| (span, i as u32)).collect();
    let mut own = textual::own_links(&read.textual, &by_name);
    let placed = |links: Vec<textual::UnitLink>| -> Vec<TextualLink> {
        links
            .into_iter()
            .map(|link| match link {
                textual::UnitLink::Def { declaration, attributes } => {
                    TextualLink::Def { declaration, attributes }
                }
                textual::UnitLink::Module(walk) => TextualLink::Module(unit_place[walk as usize]),
                textual::UnitLink::Invocation(index) => TextualLink::Invocation(index),
            })
            .collect()
    };
    let textual = placed(own.remove(&None).unwrap_or_default());
    for (walk, links) in own {
        if let Some(walk) = walk {
            units[unit_place[walk as usize] as usize].textual = placed(links);
        }
    }
    let items: Vec<(u32, u32)> = found.iter().map(|f| f.item).collect();
    let derive_spans: Vec<Vec<(u32, u32)>> = found.iter().map(|f| f.derives.clone()).collect();
    let positions: Vec<(u32, u32)> =
        names.iter().map(|&(start, _)| lines.position(text, start)).collect();

    // Invocations, in the file's order, each in the constructs around it.
    let containing = |span: (u32, u32)| -> Vec<String> {
        outlined
            .iter()
            .filter(|(symbol, _)| symbol.span.0 <= span.0 && span.1 <= symbol.span.1)
            .min_by_key(|(symbol, _)| symbol.span.1 - symbol.span.0)
            .map(|(symbol, container)| {
                let mut names = container.clone();
                names.push(symbol.name.clone());
                names
            })
            .unwrap_or_default()
    };
    let text_of = |span: (u32, u32)| -> &str {
        let end = (span.1 as usize).min(text.len());
        text.get((span.0 as usize).min(end)..end).unwrap_or("")
    };
    let hash_of = |span: (u32, u32)| -> u64 {
        let mut hash = Fnv::new();
        hash.str(text_of(span));
        hash.finish()
    };
    let mut invocations: Vec<Invocation> = Vec::new();
    let mut invocation_spans: Vec<(u32, u32)> = Vec::new();
    let mut invocation_paths: Vec<Option<(u32, u32)>> = Vec::new();
    let mut invocation_path_chars: Vec<u32> = Vec::new();
    let mut invocation_items: Vec<(u32, u32)> = Vec::new();
    let mut invocation_positions: Vec<(u32, u32)> = Vec::new();
    for (index, inv) in read.invocations.iter().enumerate() {
        invocations.push(Invocation {
            unit: unit_index(inv.unit),
            container: containing(inv.span),
            impl_block: inv.impl_block.map(|i| impl_place[i as usize]),
            members: inv.members,
            path: inv.path.clone(),
            namespace: inv.namespace.clone(),
            entry: lang.sg.nt_names[inv.entry as usize].clone(),
            text: hash_of(inv.span),
            attributes: inv.attributes.clone(),
            within: Vec::new(),
            index: index as u32,
            condition: None,
        });
        invocation_spans.push(inv.span);
        invocation_paths.push(inv.path_span);
        invocation_path_chars
            .push(text_of(inv.path_span.unwrap_or(inv.span)).chars().count() as u32);
        invocation_items.push(inv.item);
        invocation_positions.push(lines.position(text, inv.span.0));
    }

    // Macro definitions, by the declarations they are.
    let mut defined: Vec<(MacroDef, (u32, u32))> = read
        .macro_defs
        .iter()
        .filter_map(|def| {
            let declaration = *by_name.get(&def.name)?;
            let form = if def.textual { Form::MacroRules } else { Form::Macro };
            let macro_def = MacroDef {
                declaration,
                form,
                body: hash_of(def.body),
                attributes: def.attributes.clone(),
                within: Vec::new(),
                index: 0,
                condition: None,
            };
            Some((macro_def, def.body))
        })
        .collect();
    defined.sort_by_key(|(def, _)| def.declaration);
    let (mut macros, macro_bodies): (Vec<MacroDef>, Vec<(u32, u32)>) = defined.into_iter().unzip();
    for (index, def) in macros.iter_mut().enumerate() {
        def.index = index as u32;
    }

    // What each offered item is under: the regions holding it.
    let regions = read.regions;
    let under = |span: (u32, u32)| condition_at(&regions, span);
    for (u, span) in units.iter_mut().zip(&unit_spans) {
        u.condition = under(*span);
    }
    let mut declarations = declarations;
    for (d, span) in declarations.iter_mut().zip(&names) {
        d.condition = under(*span);
    }
    let mut reexports = reexports;
    for (i, span) in reexports.iter_mut().zip(&reexport_spans) {
        i.condition = under(*span);
    }
    let mut imports = imports;
    for (i, span) in imports.iter_mut().zip(&import_spans) {
        i.condition = under(*span);
    }
    let mut impls = impls;
    for (i, span) in impls.iter_mut().zip(&impl_spans) {
        i.condition = under(*span);
    }
    for (i, span) in invocations.iter_mut().zip(&invocation_spans) {
        i.condition = under(*span);
    }
    for m in &mut macros {
        m.condition = names.get(m.declaration as usize).and_then(|&span| under(span));
    }

    let names_count = names.len();
    let mut summary = Summary {
        offers: Offers {
            attributes: read.attributes,
            open: read.open,
            clean,
            textual,
            units,
            declarations,
            reexports,
            impls,
            invocations,
            macros,
            fingerprint: 0,
        },
        needs: Needs { imports, escaping, fingerprint: 0 },
        locations: Locations {
            names,
            items,
            positions,
            units: unit_spans,
            unit_items,
            reexports: reexport_spans,
            imports: import_spans,
            impls: impl_spans,
            impl_heads,
            derives: derive_spans,
            invocations: invocation_spans,
            invocation_paths,
            invocation_path_chars,
            invocation_items,
            invocation_positions,
            macro_bodies,
            declared_by: vec![None; names_count],
            words,
            condition_regions: regions,
            fingerprint: 0,
        },
    };
    summary.fingerprint();
    summary
}

/// The condition what stands at `span` is under, of `regions` (each a
/// span and its own condition, in the file's order): those holding it,
/// outermost first, all of them as one.
fn condition_at(regions: &[((u32, u32), Condition)], span: (u32, u32)) -> Option<Condition> {
    let mut holding: Vec<&((u32, u32), Condition)> =
        regions.iter().filter(|(region, _)| region.0 <= span.0 && span.1 <= region.1).collect();
    holding.sort_by_key(|(region, _)| (region.0, std::cmp::Reverse(region.1)));
    Condition::all_of(holding.into_iter().map(|(_, c)| c.clone()).collect())
}

impl Summary {
    /// Set each part's fingerprint from what it holds.
    pub(crate) fn fingerprint(&mut self) {
        self.offers.fingerprint = self.offers.hash();
        self.needs.fingerprint = self.needs.hash();
        self.locations.fingerprint = self.locations.hash();
    }
}

impl Offers {
    fn hash(&self) -> u64 {
        let mut hash = Fnv::new();
        hash.u64(u64::from(self.clean));
        hash.invocations(&self.invocations);
        hash.macros(&self.macros);
        hash.attributes(&self.attributes);
        hash.u64(u64::from(self.open));
        hash.textual(&self.textual);
        hash.u64(self.units.len() as u64);
        for u in &self.units {
            hash.str(&u.name);
            hash.unit(u.parent);
            hash.u64(u64::from(u.inline));
            hash.visibility(&u.visibility);
            hash.attributes(&u.attributes);
            hash.u64(u64::from(u.open));
            hash.textual(&u.textual);
            hash.condition_opt(u.condition.as_ref());
        }
        hash.u64(self.declarations.len() as u64);
        for d in &self.declarations {
            hash.u64(d.container.len() as u64);
            for c in &d.container {
                hash.str(c);
            }
            hash.str(&d.name);
            hash.str(&d.kind);
            hash.str(&d.namespace);
            hash.u64(u64::from(d.ordinal));
            hash.str(&d.signature);
            hash.unit(d.unit);
            hash.visibility(&d.visibility);
            hash.unit(d.impl_block);
            hash.type_opt(d.ty.as_ref());
            hash.type_opt(d.returns.as_ref());
            hash.u64(d.derives.len() as u64);
            for (path, condition) in &d.derives {
                hash.path(path);
                hash.condition_opt(condition.as_ref());
            }
            hash.strs(&d.generics);
            hash.types(&d.bounds);
            hash.types(&d.positional);
            hash.condition_opt(d.condition.as_ref());
        }
        hash.imports(&self.reexports);
        hash.u64(self.impls.len() as u64);
        for i in &self.impls {
            hash.unit(i.unit);
            hash.type_opt(i.self_type.as_ref());
            hash.path_opt(i.trait_path.as_ref());
            hash.strs(&i.generics);
            hash.condition_opt(i.condition.as_ref());
        }
        hash.finish()
    }
}

impl Needs {
    fn hash(&self) -> u64 {
        let mut hash = Fnv::new();
        hash.imports(&self.imports);
        hash.u64(self.escaping.len() as u64);
        for e in &self.escaping {
            hash.unit(e.unit);
            hash.str(&e.namespace);
            hash.path(&e.path);
        }
        hash.finish()
    }
}

impl Locations {
    fn hash(&self) -> u64 {
        let mut hash = Fnv::new();
        for ((name, item), position) in self.names.iter().zip(&self.items).zip(&self.positions) {
            for value in [name.0, name.1, item.0, item.1, position.0, position.1] {
                hash.u64(u64::from(value));
            }
        }
        let heads = self.impl_heads.iter().flat_map(|&(ty, tr)| [ty, tr]).flatten().collect();
        let derived = self.derives.iter().flatten().copied().collect();
        let paths = self.invocation_paths.iter().flatten().copied().collect();
        for spans in [
            &self.units,
            &self.unit_items,
            &self.reexports,
            &self.imports,
            &self.impls,
            &heads,
            &derived,
            &self.invocations,
            &paths,
            &self.invocation_items,
            &self.invocation_positions,
            &self.macro_bodies,
        ] {
            hash.u64(spans.len() as u64);
            for span in spans {
                hash.u64(u64::from(span.0));
                hash.u64(u64::from(span.1));
            }
        }
        for (ty, tr) in &self.impl_heads {
            hash.u64(u64::from(ty.is_some()));
            hash.u64(u64::from(tr.is_some()));
        }
        for d in &self.derives {
            hash.u64(d.len() as u64);
        }
        for by in &self.declared_by {
            hash.unit(*by);
        }
        for chars in &self.invocation_path_chars {
            hash.u64(u64::from(*chars));
        }
        for w in &self.words {
            hash.str(w);
        }
        hash.u64(self.condition_regions.len() as u64);
        for ((start, end), condition) in &self.condition_regions {
            hash.u64(u64::from(*start));
            hash.u64(u64::from(*end));
            hash.condition(condition);
        }
        hash.finish()
    }
}

/// Whether `node` holds no error and no missing token.
fn parses_cleanly(node: &qana_grammar::GreenNode) -> bool {
    node.nt != qana_grammar::green::ERROR_NT
        && node.children.iter().all(|c| match c {
            qana_grammar::GreenChild::Token(t) => !t.is_missing(),
            qana_grammar::GreenChild::Node(n) => parses_cleanly(n),
        })
}

/// The outline's symbols that no function holds (a function's body's are
/// left out), in document order, each with the names of the constructs it
/// is inside, outermost first. Those whose name is a definition are the
/// file's declarations.
fn outside_bodies(
    tree: &qana_grammar::GreenNode,
    outline: &qana_services::OutlineConfig,
) -> Vec<(qana_services::Symbol, Vec<String>)> {
    let mut out = Vec::new();
    // The constructs the walk is inside: their end, their name, whether
    // they are a function's (a body's).
    let mut inside: Vec<(u32, String, bool)> = Vec::new();
    for symbol in qana_services::outline(tree, outline) {
        while inside.last().is_some_and(|&(end, _, _)| end <= symbol.span.0) {
            inside.pop();
        }
        let in_body = inside.iter().any(|(_, _, function)| *function);
        let entered = (symbol.span.1, symbol.name.clone(), symbol.kind == FUNCTION);
        if !in_body {
            out.push((symbol, inside.iter().map(|(_, name, _)| name.clone()).collect()));
        }
        inside.push(entered);
    }
    out
}

/// A declaration's head (see [`Declaration::signature`]): `item` is the
/// construct's span and `name` its name's.
fn signature(text: &str, item: (u32, u32), name: (u32, u32)) -> String {
    let end = (item.1 as usize).min(text.len());
    let start = (item.0 as usize).min(end);
    let name_start = (name.0 as usize).clamp(start, end);
    let name_end = (name.1 as usize).clamp(name_start, end);
    let from = text[..name_start].rfind('\n').map_or(0, |i| i + 1).max(start);
    let mut depth = 0i32;
    let mut to = end;
    for (i, ch) in text[name_end..end].char_indices() {
        match ch {
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            '{' | ';' if depth <= 0 => {
                to = name_end + i;
                break;
            }
            _ => {}
        }
    }
    text[from..to].split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Where each line of a text starts.
struct LineStarts(Vec<usize>);

impl LineStarts {
    fn of(text: &str) -> Self {
        LineStarts(std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).collect())
    }

    /// The text of line `line` of `text`, without its line break.
    fn text<'t>(&self, text: &'t str, line: u32) -> &'t str {
        let start = self.0.get(line as usize).map_or(text.len(), |&start| start.min(text.len()));
        // The next line starts after this one's `\n`.
        let end = self.0.get(line as usize + 1).map_or(text.len(), |&next| next - 1).max(start);
        let line = &text[start..end];
        line.strip_suffix('\r').unwrap_or(line)
    }

    /// The line and character column of byte `at` of `text`.
    fn position(&self, text: &str, at: u32) -> (u32, u32) {
        let at = (at as usize).min(text.len());
        let line = self.0.partition_point(|&start| start <= at) - 1;
        let column = text[self.0[line]..at].chars().count();
        (line as u32, column as u32)
    }
}

/// 64-bit FNV-1a, over values written with their lengths, so that no two
/// sequences of values hash as one.
struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }

    fn bytes(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 ^= u64::from(byte);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    fn u64(&mut self, value: u64) {
        self.bytes(&value.to_le_bytes());
    }

    fn str(&mut self, value: &str) {
        self.u64(value.len() as u64);
        self.bytes(value.as_bytes());
    }

    /// A unit or impl block's index, or none.
    fn unit(&mut self, value: Option<u32>) {
        match value {
            None => self.u64(0),
            Some(i) => self.u64(u64::from(i) + 1),
        }
    }

    fn path(&mut self, path: &Path) {
        match path.root {
            None => self.u64(0),
            Some(Root::Unit(up)) => {
                self.u64(1);
                self.u64(u64::from(up));
            }
            Some(Root::Crate) => self.u64(2),
            Some(Root::Global) => self.u64(3),
            Some(Root::SelfType) => self.u64(4),
            Some(Root::MacroCrate) => self.u64(5),
            Some(Root::CrateAt(k)) => {
                self.u64(6);
                self.u64(u64::from(k));
            }
        }
        self.u64(path.segments.len() as u64);
        for segment in &path.segments {
            self.str(segment);
        }
    }

    fn path_opt(&mut self, path: Option<&Path>) {
        match path {
            None => self.u64(0),
            Some(path) => {
                self.u64(1);
                self.path(path);
            }
        }
    }

    fn visibility(&mut self, visibility: &Visibility) {
        match visibility {
            Visibility::Public => self.u64(0),
            Visibility::Within(path) => {
                self.u64(1);
                self.path(path);
            }
        }
    }

    fn imports(&mut self, imports: &[Import]) {
        self.u64(imports.len() as u64);
        for i in imports {
            self.unit(i.unit);
            self.path(&i.path);
            match &i.name {
                None => self.u64(0),
                Some(name) => {
                    self.u64(1);
                    self.str(name);
                }
            }
            self.u64(u64::from(i.glob));
            self.visibility(&i.visibility);
            self.attributes(&i.attributes);
            self.u64(u64::from(i.loads));
            self.condition_opt(i.condition.as_ref());
        }
    }

    fn condition_opt(&mut self, condition: Option<&Condition>) {
        match condition {
            None => self.u64(0),
            Some(c) => {
                self.u64(1);
                self.condition(c);
            }
        }
    }

    fn condition(&mut self, condition: &Condition) {
        match condition {
            Condition::Option(name) => {
                self.u64(0);
                self.str(name);
            }
            Condition::Value(name, value) => {
                self.u64(1);
                self.str(name);
                self.str(value);
            }
            Condition::All(all) => {
                self.u64(2);
                self.u64(all.len() as u64);
                for c in all {
                    self.condition(c);
                }
            }
            Condition::Any(any) => {
                self.u64(3);
                self.u64(any.len() as u64);
                for c in any {
                    self.condition(c);
                }
            }
            Condition::Not(c) => {
                self.u64(4);
                self.condition(c);
            }
            Condition::Literal(b) => {
                self.u64(5);
                self.u64(u64::from(*b));
            }
            Condition::Unread => self.u64(6),
        }
    }

    fn type_opt(&mut self, ty: Option<&TypeRef>) {
        match ty {
            None => self.u64(0),
            Some(ty) => {
                self.u64(1);
                self.type_ref(ty);
            }
        }
    }

    fn type_ref(&mut self, ty: &TypeRef) {
        match ty {
            TypeRef::Path { path, args, bindings } => {
                self.u64(1);
                self.path(path);
                self.types(args);
                self.u64(bindings.len() as u64);
                for (name, ty) in bindings {
                    self.str(name);
                    self.type_ref(ty);
                }
            }
            TypeRef::Ref(ty) => {
                self.u64(2);
                self.type_ref(ty);
            }
            TypeRef::Slice(ty) => {
                self.u64(3);
                self.type_ref(ty);
            }
            TypeRef::Array(ty) => {
                self.u64(4);
                self.type_ref(ty);
            }
            TypeRef::Tuple(types) => {
                self.u64(5);
                self.types(types);
            }
            TypeRef::Dyn(types) => {
                self.u64(6);
                self.types(types);
            }
            TypeRef::Impl(types) => {
                self.u64(7);
                self.types(types);
            }
            TypeRef::Other => self.u64(8),
        }
    }

    fn types(&mut self, types: &[TypeRef]) {
        self.u64(types.len() as u64);
        for ty in types {
            self.type_ref(ty);
        }
    }

    fn strs(&mut self, strs: &[String]) {
        self.u64(strs.len() as u64);
        for s in strs {
            self.str(s);
        }
    }

    fn attributes(&mut self, attributes: &[Attribute]) {
        self.u64(attributes.len() as u64);
        for a in attributes {
            self.path(&a.path);
            self.u64(u64::from(a.conditional));
            self.u64(a.args.len() as u64);
            for arg in &a.args {
                self.path(arg);
            }
            self.condition_opt(a.condition.as_ref());
        }
    }

    fn invocations(&mut self, invocations: &[Invocation]) {
        self.u64(invocations.len() as u64);
        for i in invocations {
            self.unit(i.unit);
            self.strs(&i.container);
            self.unit(i.impl_block);
            self.u64(u64::from(i.members));
            self.path_opt(i.path.as_ref());
            self.str(&i.namespace);
            self.str(&i.entry);
            self.u64(i.text);
            self.attributes(&i.attributes);
            self.u64(i.within.len() as u64);
            for &(at, which) in &i.within {
                self.u64(u64::from(at));
                self.u64(u64::from(which));
            }
            self.u64(u64::from(i.index));
            self.condition_opt(i.condition.as_ref());
        }
    }

    fn macros(&mut self, macros: &[MacroDef]) {
        self.u64(macros.len() as u64);
        for m in macros {
            self.u64(u64::from(m.declaration));
            self.u64(match m.form {
                Form::MacroRules => 0,
                Form::Macro => 1,
            });
            self.u64(m.body);
            self.attributes(&m.attributes);
            self.u64(m.within.len() as u64);
            for &(at, which) in &m.within {
                self.u64(u64::from(at));
                self.u64(u64::from(which));
            }
            self.u64(u64::from(m.index));
            self.condition_opt(m.condition.as_ref());
        }
    }

    fn textual(&mut self, links: &[TextualLink]) {
        self.u64(links.len() as u64);
        for link in links {
            match link {
                TextualLink::Def { declaration, attributes } => {
                    self.u64(0);
                    self.u64(u64::from(*declaration));
                    self.attributes(attributes);
                }
                TextualLink::Module(unit) => {
                    self.u64(1);
                    self.u64(u64::from(*unit));
                }
                TextualLink::Invocation(index) => {
                    self.u64(2);
                    self.u64(u64::from(*index));
                }
            }
        }
    }

    fn finish(&self) -> u64 {
        self.0
    }
}
