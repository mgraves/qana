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
    /// The project's crates it may name, each by the name it names it
    /// by: an index into [`UnitGraph::crates`].
    pub dependencies: Vec<(String, u32)>,
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
