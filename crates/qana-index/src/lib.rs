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

pub use graph::{CrateInfo, UnitGraph};
pub use qana_sem::{Path, Root};
pub use resolve::{Resolution, Resolver};

mod graph;
mod resolve;
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
    units: Vec<Unit>,
    declarations: Vec<Declaration>,
    reexports: Vec<Import>,
    impls: Vec<Impl>,
    fingerprint: u64,
}

impl Offers {
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
}

/// An impl block: what it is of, and of which trait.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Impl {
    /// The unit it is in: an index into [`Offers::units`], or `None` for
    /// the file's own.
    pub unit: Option<u32>,
    /// The type it is of; `None` for a type no path names (`&T`, `[T]`).
    pub self_type: Option<Path>,
    /// The trait it implements, if it implements one.
    pub trait_path: Option<Path>,
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
    words: Vec<String>,
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

    /// The names the file defines or refers to, sorted, each once: where a
    /// search for a name's uses looks first.
    pub fn words(&self) -> &[String] {
        &self.words
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
    let mut db = SemDb::new(lang.binding.clone());
    db.set_types(lang.types.clone());
    db.set_macro_bodies(&lang.macros);
    db.set_tree(URI, tree.clone());
    let symbols = db.symbols(URI);
    let resolutions = db.resolve(URI);
    let read = units::read(&tree, &lang.binding, &lang.outline);

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
    let mut impls: Vec<(Impl, (u32, u32), usize)> = read
        .impls
        .iter()
        .enumerate()
        .map(|(i, (block, span))| {
            (Impl { unit: unit_index(block.unit), ..block.clone() }, *span, i)
        })
        .collect();
    impls.sort_by(|a, b| a.0.cmp(&b.0));
    let mut impl_place = vec![0u32; impls.len()];
    for (place, (_, _, i)) in impls.iter().enumerate() {
        impl_place[*i] = place as u32;
    }
    let (impls, impl_spans): (Vec<Impl>, Vec<(u32, u32)>) =
        impls.into_iter().map(|(block, span, _)| (block, span)).unzip();

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
    }
    let mut found: Vec<Found> = Vec::new();
    // The constructs the walk is inside: their end, their name, whether
    // they are a function's (a body's).
    let mut inside: Vec<(u32, String, bool)> = Vec::new();
    for symbol in qana_services::outline(&tree, &lang.outline) {
        while inside.last().is_some_and(|&(end, _, _)| end <= symbol.span.0) {
            inside.pop();
        }
        let in_body = inside.iter().any(|(_, _, function)| *function);
        let defines = namespaces.get(&symbol.selection);
        let place = read.defs.get(&symbol.selection);
        if let (false, Some(namespace), Some(place)) = (in_body, defines, place) {
            found.push(Found {
                declaration: Declaration {
                    container: inside.iter().map(|(_, name, _)| name.clone()).collect(),
                    name: symbol.name.clone(),
                    kind: symbol.kind.to_string(),
                    namespace: namespace.to_string(),
                    ordinal: 0,
                    signature: signature(text, symbol.span, symbol.selection),
                    unit: unit_index(place.unit),
                    visibility: place.visibility.clone(),
                    impl_block: place.impl_block.map(|i| impl_place[i as usize]),
                },
                name: symbol.selection,
                item: symbol.span,
            });
        }
        inside.push((symbol.span.1, symbol.name, symbol.kind == FUNCTION));
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
    // expansion makes) is its name alone, in the unit around it.
    let escaping: Vec<EscapingName> = symbols
        .refs
        .iter()
        .zip(resolutions.iter())
        .filter(|(_, target)| **target == Target::Escapes)
        .filter_map(|(r, _)| match read.refs.get(&r.span) {
            Some(place) if place.imported => None,
            Some(place) => place.path.clone().map(|path| EscapingName {
                unit: unit_index(place.unit),
                namespace: r.ns.clone(),
                path,
            }),
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
    let items: Vec<(u32, u32)> = found.iter().map(|f| f.item).collect();
    let positions: Vec<(u32, u32)> =
        names.iter().map(|&(start, _)| lines.position(text, start)).collect();

    let offers_fingerprint = {
        let mut hash = Fnv::new();
        hash.u64(units.len() as u64);
        for u in &units {
            hash.str(&u.name);
            hash.unit(u.parent);
            hash.u64(u64::from(u.inline));
            hash.visibility(&u.visibility);
        }
        hash.u64(declarations.len() as u64);
        for d in &declarations {
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
        }
        hash.imports(&reexports);
        hash.u64(impls.len() as u64);
        for i in &impls {
            hash.unit(i.unit);
            hash.path_opt(i.self_type.as_ref());
            hash.path_opt(i.trait_path.as_ref());
        }
        hash.finish()
    };
    let needs_fingerprint = {
        let mut hash = Fnv::new();
        hash.imports(&imports);
        hash.u64(escaping.len() as u64);
        for e in &escaping {
            hash.unit(e.unit);
            hash.str(&e.namespace);
            hash.path(&e.path);
        }
        hash.finish()
    };
    let locations_fingerprint = {
        let mut hash = Fnv::new();
        for ((name, item), position) in names.iter().zip(&items).zip(&positions) {
            for value in [name.0, name.1, item.0, item.1, position.0, position.1] {
                hash.u64(u64::from(value));
            }
        }
        for spans in [&unit_spans, &unit_items, &reexport_spans, &import_spans, &impl_spans] {
            hash.u64(spans.len() as u64);
            for span in spans {
                hash.u64(u64::from(span.0));
                hash.u64(u64::from(span.1));
            }
        }
        for w in &words {
            hash.str(w);
        }
        hash.finish()
    };

    Summary {
        offers: Offers { units, declarations, reexports, impls, fingerprint: offers_fingerprint },
        needs: Needs { imports, escaping, fingerprint: needs_fingerprint },
        locations: Locations {
            names,
            items,
            positions,
            units: unit_spans,
            unit_items,
            reexports: reexport_spans,
            imports: import_spans,
            impls: impl_spans,
            words,
            fingerprint: locations_fingerprint,
        },
    }
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
        }
    }

    fn finish(&self) -> u64 {
        self.0
    }
}
