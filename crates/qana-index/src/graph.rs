//! A project's units as its build system arranges them: its crates, and
//! where the body of each unit declared without one is.
//!
//! A [`Summary`](crate::Summary) knows a file's own units and nothing of
//! where the file is: a `mod a;` in it is a unit whose body is elsewhere.
//! A project model (Rust's reads `cargo metadata` and Rust's rules for
//! module files) says which crates a project has, which file is each
//! crate's root, and which file is the body of each unit declared without
//! one. The [`UnitGraph`] is what it says: from it, each file has its
//! place in each crate it is in, and its module path there.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A crate of a project: what a build system builds from one root file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrateInfo {
    /// Its name, as paths name it (`synkro_canvas`: Rust's `-` is `_`).
    pub name: String,
    /// What the build system builds it as (`lib`, `bin`, `test`,
    /// `example`, `bench`, `proc-macro`, `custom-build`).
    pub kind: String,
    /// The file its root unit's body is.
    pub root: PathBuf,
    /// The edition of its language it is written in (`2021`).
    pub edition: String,
    /// The crates it names without loading them (Rust's extern prelude,
    /// before its `extern crate`s), each by the name it names it by: an
    /// index into [`UnitGraph::crates`]. A path may begin with one, from
    /// anywhere in the crate, and `::` names one.
    pub dependencies: Vec<(String, u32)>,
    /// The crates it may load by name, which it names only once it has
    /// (Rust's `extern crate alloc;`): an index into
    /// [`UnitGraph::crates`]. An import that loads one (`@loads`) made at
    /// the crate's root adds its name to those the crate names.
    pub loadable: Vec<(String, u32)>,
    /// Where it comes from: the project, or a library.
    pub source: CrateSource,
    /// The modules whose names every module of the crate may use after
    /// its own and the crates it names, each by a path from the crate's
    /// root (Rust's standard prelude, `::std::prelude::rust_2021`): a glob
    /// of each that nothing writes.
    pub preludes: Vec<crate::Path>,
    /// The names the language provides, which no source declares (Rust's
    /// primitive types, `u8`): what a path's first name is when it is
    /// nothing else.
    pub builtins: Vec<String>,
    /// The attributes that close a module, and every module inside it, to
    /// the crates its crate names and to the preludes (Rust's
    /// `no_implicit_prelude`): unconditional ones, said of the module.
    /// Paths from a root (`::std`) are not closed to them.
    pub closed_by: Vec<crate::Path>,
    /// The attributes and derives the language provides that declare
    /// nothing (Rust's `cfg`, `allow`, `rustfmt::…`; `Debug`, `Clone`): an
    /// attribute or a derive whose path begins with one of these. Any other
    /// may be a macro declaring names where it stands, so a module where one
    /// appears is open: a name not found there escapes.
    pub inert: Vec<crate::Path>,
    /// Whether an import's path without a root, and a path from `::`, start
    /// at the crate's root (Rust 2015), not where the import stands.
    pub imports_from_root: bool,
    /// The names of crates it may name that the graph does not hold (its
    /// dependencies for another target, or behind a feature it was not
    /// built with): a path that begins with one escapes.
    pub unindexed: Vec<String>,
    /// The trait a member access reaches through a value of a type that
    /// implements it to another value, and its associated type that names
    /// that value's type (Rust's `Deref` and `Target`): the trait's path,
    /// and the associated type's name. `None` where the language has none.
    ///
    /// The path is read from the crate's root; one from the crates
    /// themselves (`::core::ops::Deref`) has a crate the crate names (a
    /// dependency, or one its root loads) as its first segment, never what
    /// the root declares, since the trait is the one that crate declares
    /// whatever else the crate calls by its name (a module `core`).
    pub deref_trait: Option<(crate::Path, String)>,
    /// The trait whose associated type names what a value gives up when it
    /// holds one (Rust's `Try` and `Output`, what `x?` gives): its path,
    /// read as [`Self::deref_trait`]'s is, and the associated type's name.
    pub try_trait: Option<(crate::Path, String)>,
    /// The traits the language's built-in derives implement, by the
    /// derive's name (Rust's `Debug` derives `::core::fmt::Debug`), each
    /// path read as [`Self::deref_trait`]'s is. A derive whose path names
    /// no trait, and whose last name is one of these, implements that
    /// trait: Rust's prelude holds `Debug` and `Hash` as derive macros, not
    /// traits.
    pub derive_traits: Vec<(String, crate::Path)>,
    /// The attributes that carry a module's textual definitions (Rust's
    /// `macro_rules!`, `@def(x, textual)`) out of it, to be seen after its
    /// declaration in the module it is declared in (`#[macro_use] mod m;`);
    /// said of an import that loads a crate at the crate's root, they bring
    /// that crate's root's textual names into every module of this one
    /// (`#[macro_use] extern crate log;`).
    #[serde(default)]
    pub macro_use: Vec<crate::Path>,
    /// The attributes that put a textual definition among its crate's
    /// root's names, wherever it is defined (`#[macro_export]`): a path
    /// from the root names it (`crate::m!`, `other::m!`).
    #[serde(default)]
    pub macro_export: Vec<crate::Path>,
    /// The crates whose roots' textual names every module of the crate may
    /// use after its own names, each by a path from the crate's root (Rust
    /// writes `#[macro_use] extern crate std;` into every crate: `::std`).
    #[serde(default)]
    pub macro_prelude: Vec<crate::Path>,
    /// The attributes under which what they are said of may not be (Rust's
    /// `cfg`): where a summary reads no condition for what one is said of
    /// (a grammar that says no `@condition`), a textual definition under one
    /// may not be, so does not hide an earlier one of its name, which may
    /// be the one a name sees. Where it reads one, the crate's
    /// [`configuration`](Self::configuration) decides.
    #[serde(default)]
    pub conditions: Vec<crate::Path>,
    /// The attributes that make a macro's definition the compiler's, its
    /// body no rules (Rust's `#[rustc_builtin_macro]`).
    #[serde(default)]
    pub builtin_attributes: Vec<crate::Path>,
    /// The compiler's macros whose expansions the model says, by name
    /// (Rust's `cfg_select`): one of them invoked expands as
    /// [`crate::expand`] has it. Another is not expanded.
    #[serde(default)]
    pub builtin_macros: Vec<(String, crate::BuiltinMacro)>,
    /// The attributes, each with the argument that says so, that export a
    /// textual definition whose expansions name their crate's macros by a
    /// name alone (Rust's `#[macro_export(local_inner_macros)]`): an
    /// invocation of a name alone that such a macro writes names what the
    /// name names from its crate's root, as `$crate::name!` would.
    #[serde(default)]
    pub local_inner_macros: Vec<(crate::Path, crate::Path)>,
    /// How deep the expansions of invocations in the crate may nest (Rust's
    /// `#![recursion_limit = "256"]`); `None` for the language's default
    /// ([`crate::RECURSION_LIMIT`]).
    #[serde(default)]
    pub recursion_limit: Option<u32>,
    /// The configuration it is compiled in (Rust's features and `cfg`
    /// options), which decides the conditions its code is under
    /// ([`Condition::holds`](crate::Condition::holds)); by default, one
    /// that decides nothing, so that every condition may hold.
    #[serde(default)]
    pub configuration: crate::Configuration,
}

/// Where a crate comes from.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CrateSource {
    /// The project's own: its files are the project's, indexed as they
    /// change.
    Workspace,
    /// A library from a registry, never changed once fetched; `id` names
    /// its package, version and registry.
    Registry { id: String },
    /// A library from a git revision, never changed once fetched; `id`
    /// names its package and revision.
    Git { id: String },
    /// A library at a path outside the project, which may change.
    Path,
    /// The language's own (Rust's std, from the toolchain's sources);
    /// `id` names the toolchain.
    Toolchain { id: String },
}

impl CrateSource {
    /// Whether the crate is a library: not the project's own.
    pub fn is_library(&self) -> bool {
        !matches!(self, CrateSource::Workspace)
    }

    /// What names a library that never changes once fetched, for keeping
    /// what is read of it; `None` for one that may change (the project's,
    /// a path's).
    pub fn fixed_id(&self) -> Option<&str> {
        match self {
            CrateSource::Registry { id }
            | CrateSource::Git { id }
            | CrateSource::Toolchain { id } => Some(id),
            CrateSource::Workspace | CrateSource::Path => None,
        }
    }
}

/// A project's crates, and where the units declared without a body have
/// theirs. See the [module docs](self).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UnitGraph {
    crates: Vec<CrateInfo>,
    /// Each file's places: in which crate, at which module path, and as
    /// which declared unit (`None` for a crate's root).
    places: HashMap<PathBuf, Vec<Place>>,
    /// The body of each unit declared without one, by its crate, the file
    /// declaring it and its index in that file's summary's units.
    bodies: HashMap<(u32, PathBuf, u32), PathBuf>,
}

/// Where a file is in one crate.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Place {
    krate: u32,
    module_path: Vec<String>,
    declared_by: Option<(PathBuf, u32)>,
}

impl UnitGraph {
    /// A graph of no crates.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a crate; its root file is its root unit's body, at the empty
    /// module path. Its index in [`crates`](Self::crates).
    pub fn add_crate(&mut self, info: CrateInfo) -> u32 {
        let krate = self.crates.len() as u32;
        self.places.entry(info.root.clone()).or_default().push(Place {
            krate,
            module_path: Vec::new(),
            declared_by: None,
        });
        self.crates.push(info);
        krate
    }

    /// In crate `krate`, the unit `unit` (an index into the units of
    /// `declaring`'s summary) has its body in `body`, at `module_path`.
    /// False, and nothing added, when `body` already has a place in the
    /// crate: a file is one unit of a crate at most (a second `mod` of it,
    /// or a cycle of `#[path]`s, is the build system's error).
    pub fn add_body(
        &mut self,
        krate: u32,
        declaring: &Path,
        unit: u32,
        body: PathBuf,
        module_path: Vec<String>,
    ) -> bool {
        let places = self.places.entry(body.clone()).or_default();
        if places.iter().any(|p| p.krate == krate) {
            return false;
        }
        places.push(Place {
            krate,
            module_path,
            declared_by: Some((declaring.to_path_buf(), unit)),
        });
        self.bodies.insert((krate, declaring.to_path_buf(), unit), body);
        true
    }

    /// The crates, in the order they were added.
    pub fn crates(&self) -> &[CrateInfo] {
        &self.crates
    }

    /// The graph, each crate's [`configuration`](CrateInfo::configuration)
    /// one that decides nothing: a resolver over it reads every condition as
    /// one that may hold, as code that is not compiled is read (to go to
    /// what it names, never to judge it).
    pub fn undecided(&self) -> UnitGraph {
        let mut graph = self.clone();
        for krate in &mut graph.crates {
            krate.configuration = crate::Configuration::default();
        }
        graph
    }

    /// Where `file` is: each crate it is in (an index into
    /// [`crates`](Self::crates)) and its module path there, in the order
    /// the crates were added. Empty for a file in no crate.
    pub fn places(&self, file: &Path) -> Vec<(u32, &[String])> {
        let mut places: Vec<(u32, &[String])> = self
            .places
            .get(file)
            .map(|places| places.iter().map(|p| (p.krate, p.module_path.as_slice())).collect())
            .unwrap_or_default();
        places.sort_by_key(|&(krate, _)| krate);
        places
    }

    /// In crate `krate`, the file that is the body of unit `unit` declared
    /// in `declaring`, when the model found it.
    pub fn body(&self, krate: u32, declaring: &Path, unit: u32) -> Option<&Path> {
        self.bodies.get(&(krate, declaring.to_path_buf(), unit)).map(PathBuf::as_path)
    }

    /// In crate `krate`, the unit `file` is the body of: the file declaring
    /// it and its index there. `None` for the crate's root, and for a file
    /// not in the crate.
    pub fn declared_by(&self, krate: u32, file: &Path) -> Option<(&Path, u32)> {
        self.places
            .get(file)?
            .iter()
            .find(|p| p.krate == krate)?
            .declared_by
            .as_ref()
            .map(|(path, unit)| (path.as_path(), *unit))
    }

    /// How many files have a place in some crate.
    pub fn file_count(&self) -> usize {
        self.places.len()
    }
}
