//! Resolving paths across a project's files: what `crate::grid::Cell`,
//! `super::Row`, `Cell::new` or a re-exported name names, from where it
//! stands, over a [`UnitGraph`] and the files' [`Summary`]s.
//!
//! A *module* is a unit in one crate: a file's own unit, or a unit whose
//! body is in the file. Its *names* are its items (the declarations made
//! directly in it, its child units among them), what it imports, and,
//! where neither has a name, what its glob imports bring: every name the
//! globbed module has that is visible from it, and an enum's variants.
//! Imports name what their paths resolve to, from the module they are
//! made in, so names depend on one another, in cycles through globs and
//! re-exports. They are found on demand and kept. A lookup that meets one
//! still under way (a cycle) takes what that one has found so far, and is
//! provisional; the outermost lookup of the cycle runs again, each
//! provisional lookup under it found once a round, until a round finds
//! nothing new, and then all are kept. What a name binds only grows from
//! round to round, so the rounds end; a binding found once stays an
//! answer.
//!
//! A path starts at its root (a unit up from where it stands, the crate's
//! root, or a crate its crate names, by its first segment) or, without
//! one, at the first of these that has its first segment:
//!
//! - the names of the module it stands in;
//! - the crates its crate names: those it names without loading them, and
//!   those its root loads under a name (an import that loads, `@loads`:
//!   Rust's `extern crate`, whose own path may name any crate the crate
//!   may load);
//! - the names of its crate's preludes that the module may see, as though
//!   each were globbed into every module;
//! - the names the language provides (builtins), which no source declares:
//!   a path that is one names it, and one that goes on escapes after it.
//!
//! A module with an attribute that closes it ([`CrateInfo::closed_by`],
//! said unconditionally of it or of a module it is inside) has neither the
//! crates nor the preludes; a path from a root still has them.
//!
//! Each segment after is a name of the module before it, a member of the
//! type before it, or nothing: the path escapes there. A type's members
//! are, the first of these that has the name: what is declared inside it
//! (an enum's variants, a trait's items); the items of its inherent impl
//! blocks (every impl block without a trait whose type resolves to it);
//! the items of its impl blocks of traits (which of those traits are in
//! scope is not known here: each is an answer).
//!
//! What escapes still escapes: a crate outside the project, what a macro
//! expands to, `Self` (whose type a summary does not say). Nothing is
//! reported unresolved. Visibility decides what a glob brings; a path that
//! names what it may not see is followed anyway, for a reader may go to it
//! (the compiler says the rest).
//!
//! [`CrateInfo::closed_by`]: crate::CrateInfo::closed_by

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path as FilePath, PathBuf};
use std::rc::Rc;

use crate::{Attribute, Declaration, Import, Path, Root, Summary, UnitGraph, Visibility};

/// What a path names. See the [module docs](self).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolution {
    /// What it names in the project: declarations, each by its file and
    /// its index in that file's offers' declarations; and modules whose
    /// body is a file (a crate's root, `mod a;`'s file), by the file. More
    /// than one where the project declares it more than once (under
    /// `#[cfg]`s, say), or in several namespaces: each is an answer.
    Found { declarations: Vec<(PathBuf, u32)>, modules: Vec<PathBuf> },
    /// It names what the project does not hold, from its `segment`th
    /// segment (0 for the first) on: a crate outside the project, a
    /// macro's expansion, a member no summary holds.
    Escapes { segment: usize },
    /// It is a name the language provides, which no source declares
    /// (Rust's primitive types, `u8`).
    Builtin,
}

/// Resolves paths over a unit graph and the summaries of a project's files,
/// keeping what it finds. Made for one state of the project: when a
/// summary or the graph changes, a new one is made. See the [module
/// docs](self).
pub struct Resolver {
    graph: Rc<UnitGraph>,
    files: Vec<PathBuf>,
    ids: HashMap<PathBuf, u32>,
    summaries: Vec<Rc<Summary>>,
    /// Each module's bindings of a name, as found.
    names: RefCell<HashMap<(Module, String), Bindings>>,
    /// The (module, name) lookups under way, each with its depth: met
    /// again, a cycle.
    open: RefCell<HashMap<(Module, String), usize>>,
    /// How many lookups are under way.
    depth: Cell<usize>,
    /// The shallowest lookup under way that the innermost one has met
    /// (directly or through a provisional answer): `usize::MAX` for none.
    reached: Cell<usize>,
    /// The answers so far of the lookups in a cycle not yet settled: each
    /// with the round it was found in and the shallowest lookup under way
    /// it met.
    provisional: RefCell<HashMap<(Module, String), Provisional>>,
    /// The round being found, the next round's, and whether an answer of
    /// the cycle being settled has grown in it.
    round: Cell<u64>,
    next_round: Cell<u64>,
    grew: Cell<bool>,
    /// How many provisional answers have been made: their order.
    made: Cell<u64>,
    /// Each file-and-unit's items by name, as indexes into its summary's
    /// declarations.
    items: RefCell<HashMap<(u32, Option<u32>), Items>>,
    /// Each impl block by the last name of its type's path, and the names
    /// imports give what they import under another name, once gathered.
    impl_names: RefCell<Option<Rc<ImplNames>>>,
    /// The impl blocks of each declaration that is a type, once found:
    /// (file, impl index) by (file, declaration index).
    impls: RefCell<HashMap<(u32, u32), ImplBlocks>>,
    /// The declarations each impl block's type resolves to, once found:
    /// by (file, impl index).
    impl_types: RefCell<HashMap<(u32, u32), Rc<Vec<(u32, u32)>>>>,
    /// Whether some are being found: an impl block's type is a path, and
    /// while they are, a type's members are those declared inside it.
    finding_impls: Cell<bool>,
    /// Whether each module is closed to its crate's named crates and
    /// preludes, once asked.
    closed: RefCell<HashMap<Module, bool>>,
}

/// A unit in a crate: a file's own (`unit` `None`), or one with its body in
/// the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Module {
    krate: u32,
    file: u32,
    unit: Option<u32>,
}

/// What a name binds: a declaration, a module, or both (a unit's
/// declaration and the module it is), with who may see it, from where.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Target {
    declaration: Option<(u32, u32)>,
    module: Option<Module>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Binding {
    target: Target,
    /// Who may see it, and the module it is declared or imported in.
    visibility: Visibility,
    from: Module,
}

/// What a path's walk has reached, or where it left the project.
type Walked = Result<Vec<Target>, usize>;

/// Where a path's first name may come from, besides the module's names.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Start {
    /// The crates its crate names, then its preludes.
    Anywhere,
    /// The crates its crate names, and those it may load: an import that
    /// loads's path.
    Loading,
    /// The crates its crate names: a prelude's own path.
    Prelude,
}

/// What a name binds in a module.
type Bindings = Rc<Vec<Binding>>;

/// A lookup's answer so far, in a cycle not yet settled: the round it was
/// found in, the shallowest lookup under way it met, and when it was made.
#[derive(Clone)]
struct Provisional {
    found: Bindings,
    round: u64,
    reached: usize,
    made: u64,
}

/// A unit's items by name, as indexes into its file's declarations.
type Items = Rc<HashMap<String, Vec<u32>>>;

/// A type's impl blocks: (file, impl index).
type ImplBlocks = Rc<Vec<(u32, u32)>>;

/// Where a type's impl blocks may be: each impl block (file, impl index) by
/// the last name of its type's path, which is the type's own name or a
/// name an import gives it (`use a::Cell as Grid; impl Grid {}`); and, by
/// the last name of an import's path, the names imports give what it names
/// (`Grid` by `Cell`).
#[derive(Default)]
struct ImplNames {
    by_name: HashMap<String, Vec<(u32, u32)>>,
    renames: HashMap<String, Vec<String>>,
}

impl Resolver {
    /// A resolver over `graph`, and the `summaries` of the project's files
    /// by path.
    pub fn new(
        graph: Rc<UnitGraph>,
        summaries: impl IntoIterator<Item = (PathBuf, Rc<Summary>)>,
    ) -> Self {
        let mut files = Vec::new();
        let mut ids = HashMap::new();
        let mut kept = Vec::new();
        for (path, summary) in summaries {
            ids.insert(path.clone(), files.len() as u32);
            files.push(path);
            kept.push(summary);
        }
        Self {
            graph,
            files,
            ids,
            summaries: kept,
            names: RefCell::default(),
            open: RefCell::default(),
            depth: Cell::new(0),
            reached: Cell::new(usize::MAX),
            provisional: RefCell::default(),
            round: Cell::new(0),
            next_round: Cell::new(1),
            grew: Cell::new(false),
            made: Cell::new(0),
            items: RefCell::default(),
            impl_names: RefCell::new(None),
            impls: RefCell::default(),
            impl_types: RefCell::default(),
            finding_impls: Cell::new(false),
            closed: RefCell::default(),
        }
    }

    /// What `path` names where it stands: in `file`, inside the units with
    /// a body named `units` (outermost first; the file's own unit when
    /// empty), looking for its last segment in `namespace` (empty for the
    /// default one), or in every namespace (`None`: as an import's path
    /// does). In each crate the file is in.
    pub fn resolve(
        &self,
        file: &FilePath,
        units: &[String],
        path: &Path,
        namespace: Option<&str>,
    ) -> Resolution {
        let Some(&id) = self.ids.get(file) else {
            return Resolution::Escapes { segment: 0 };
        };
        let mut declarations: Vec<(PathBuf, u32)> = Vec::new();
        let mut modules: Vec<PathBuf> = Vec::new();
        let mut furthest = 0;
        for (krate, _) in self.graph.places(file) {
            let at = Module { krate, file: id, unit: self.unit_named(id, units) };
            match self.walk(at, path) {
                Ok(targets) => {
                    for target in targets {
                        self.say(target, namespace, &mut declarations, &mut modules);
                    }
                }
                Err(segment) => furthest = furthest.max(segment),
            }
        }
        if declarations.is_empty() && modules.is_empty() {
            // A first name that is nothing else may be the language's.
            let first = path.segments.first().filter(|_| path.root.is_none() && furthest == 0);
            let builtin = first.is_some_and(|name| {
                self.graph.places(file).iter().any(|&(krate, _)| {
                    self.graph.crates()[krate as usize].builtins.iter().any(|b| b == name)
                })
            });
            return match (builtin, path.segments.len()) {
                (true, 1) => Resolution::Builtin,
                (true, _) => Resolution::Escapes { segment: 1 },
                (false, _) => Resolution::Escapes { segment: furthest },
            };
        }
        declarations.sort();
        declarations.dedup();
        modules.sort();
        modules.dedup();
        Resolution::Found { declarations, modules }
    }

    /// What a target is, said as a resolution says it: a module with a
    /// file of its own by its file, anything else by its declaration (in
    /// `namespace`, unless it is a module).
    fn say(
        &self,
        target: Target,
        namespace: Option<&str>,
        declarations: &mut Vec<(PathBuf, u32)>,
        modules: &mut Vec<PathBuf>,
    ) {
        if let Some(Module { file, unit: None, .. }) = target.module {
            modules.push(self.files[file as usize].clone());
            return;
        }
        if let Some((file, index)) = target.declaration {
            let d = &self.summaries[file as usize].offers().declarations()[index as usize];
            if target.module.is_some() || namespace.is_none_or(|ns| d.namespace == ns) {
                declarations.push((self.files[file as usize].clone(), index));
            }
        }
    }

    /// The unit of `file` the chain of names `units` leads to, from the
    /// file's own: as far as the summary has them.
    fn unit_named(&self, file: u32, units: &[String]) -> Option<u32> {
        let all = self.summaries[file as usize].offers().units();
        let mut at: Option<u32> = None;
        for name in units {
            match all.iter().position(|u| u.inline && u.parent == at && u.name == *name) {
                Some(i) => at = Some(i as u32),
                None => break,
            }
        }
        at
    }

    // ------------------------------------------------------------------
    // Paths
    // ------------------------------------------------------------------

    /// Walk `path` from module `at`.
    fn walk(&self, at: Module, path: &Path) -> Walked {
        self.walk_from(at, path, Start::Anywhere)
    }

    /// Walk `path` from module `at`, its first name from the module's
    /// names, then from where `from` says.
    fn walk_from(&self, at: Module, path: &Path, from: Start) -> Walked {
        let (mut current, first) = match path.root {
            Some(Root::Unit(up)) => {
                let mut module = at;
                for _ in 0..up {
                    module = self.parent(module).ok_or(0usize)?;
                }
                (vec![Target { declaration: None, module: Some(module) }], 0)
            }
            Some(Root::Crate) => (vec![self.crate_root(at.krate)], 0),
            Some(Root::Global) => {
                let name = path.segments.first().ok_or(0usize)?;
                let found = self.named_crates(at.krate, name, from == Start::Loading);
                if found.is_empty() {
                    return Err(0);
                }
                (found, 1)
            }
            None => {
                let name = path.segments.first().ok_or(0usize)?;
                let mut found: Vec<Target> =
                    self.names_of(at, name).iter().map(|b| b.target).collect();
                if found.is_empty() && !self.closed(at) {
                    found = self.named_crates(at.krate, name, from == Start::Loading);
                    if found.is_empty() && from == Start::Anywhere {
                        found = self.prelude_names(at, name);
                    }
                }
                if found.is_empty() {
                    return Err(0);
                }
                (found, 1)
            }
        };
        for (i, segment) in path.segments.iter().enumerate().skip(first) {
            let mut next: Vec<Target> = Vec::new();
            for target in &current {
                next.extend(self.member(*target, segment));
            }
            next.sort_by_key(|t| (t.declaration, t.module.map(|m| (m.krate, m.file, m.unit))));
            next.dedup();
            if next.is_empty() {
                return Err(i);
            }
            current = next;
        }
        Ok(current)
    }

    /// What `name` is after `target` in a path: a name of the module it is,
    /// or a member of the type it declares.
    fn member(&self, target: Target, name: &str) -> Vec<Target> {
        if let Some(module) = target.module {
            return self.names_of(module, name).iter().map(|b| b.target).collect();
        }
        let Some((file, index)) = target.declaration else {
            return Vec::new();
        };
        let found = self.declared_inside(file, index, name);
        if !found.is_empty() {
            return found;
        }
        let blocks = self.impls_of(file, index);
        // Its inherent impl blocks' items, or else its trait impl blocks'.
        for inherent in [true, false] {
            let mut found = Vec::new();
            for &(block_file, block) in blocks.iter() {
                let summary = &self.summaries[block_file as usize];
                if summary.offers().impls()[block as usize].trait_path.is_none() != inherent {
                    continue;
                }
                for (i, d) in summary.offers().declarations().iter().enumerate() {
                    if d.impl_block == Some(block) && d.name == name {
                        found.push(Target {
                            declaration: Some((block_file, i as u32)),
                            module: None,
                        });
                    }
                }
            }
            if !found.is_empty() {
                return found;
            }
        }
        Vec::new()
    }

    /// The declarations named `name` made inside declaration `index` of
    /// `file` (an enum's variants, a trait's items, a struct's fields), not
    /// in an impl block.
    fn declared_inside(&self, file: u32, index: u32, name: &str) -> Vec<Target> {
        let declarations = self.summaries[file as usize].offers().declarations();
        let owner = &declarations[index as usize];
        let mut container = owner.container.clone();
        container.push(owner.name.clone());
        declarations
            .iter()
            .enumerate()
            .filter(|(_, d)| d.impl_block.is_none() && d.name == name && d.container == container)
            .map(|(i, _)| Target { declaration: Some((file, i as u32)), module: None })
            .collect()
    }

    // ------------------------------------------------------------------
    // Modules
    // ------------------------------------------------------------------

    fn crate_root(&self, krate: u32) -> Target {
        let root = &self.graph.crates()[krate as usize].root;
        match self.ids.get(root) {
            Some(&file) => {
                Target { declaration: None, module: Some(Module { krate, file, unit: None }) }
            }
            None => Target { declaration: None, module: None },
        }
    }

    /// The crates `krate` names `name`, each as its root: those it names
    /// without loading them, and those its root loads under that name; or,
    /// for an import that loads (`loading`), those it names without loading
    /// them and those it may load.
    fn named_crates(&self, krate: u32, name: &str, loading: bool) -> Vec<Target> {
        let info = &self.graph.crates()[krate as usize];
        let loadable = if loading { info.loadable.as_slice() } else { &[] };
        let mut found: Vec<Target> = info
            .dependencies
            .iter()
            .chain(loadable)
            .filter(|(named, _)| named == name)
            .map(|&(_, dependency)| self.crate_root(dependency))
            .filter(|t| t.module.is_some())
            .collect();
        if !loading {
            if let Some(root) = self.crate_root(krate).module {
                let loads: Vec<Import> = self
                    .imports_of(root)
                    .filter(|i| i.loads && !i.glob && i.name.as_deref() == Some(name))
                    .cloned()
                    .collect();
                for import in &loads {
                    if let Ok(targets) = self.walk_from(root, &import.path, Start::Loading) {
                        found.extend(targets);
                    }
                }
            }
        }
        found
    }

    /// What `name` is in the preludes of `at`'s crate: the names of each
    /// prelude's module that `at` may see.
    fn prelude_names(&self, at: Module, name: &str) -> Vec<Target> {
        let preludes = &self.graph.crates()[at.krate as usize].preludes;
        let Some(root) = self.crate_root(at.krate).module.filter(|_| !preludes.is_empty()) else {
            return Vec::new();
        };
        let mut found = Vec::new();
        for prelude in preludes {
            let Ok(targets) = self.walk_from(root, prelude, Start::Prelude) else { continue };
            for module in targets.iter().filter_map(|t| t.module) {
                for binding in self.names_of(module, name).iter() {
                    if self.visible(binding, at) {
                        found.push(binding.target);
                    }
                }
            }
        }
        found
    }

    /// Whether `module`, or a module it is inside, is closed to its crate's
    /// named crates and preludes: an attribute its crate's
    /// [`closed_by`](crate::CrateInfo::closed_by) holds is said of it
    /// unconditionally.
    fn closed(&self, module: Module) -> bool {
        if let Some(&closed) = self.closed.borrow().get(&module) {
            return closed;
        }
        let closers = &self.graph.crates()[module.krate as usize].closed_by;
        let says = |attributes: &[Attribute]| {
            attributes.iter().any(|a| !a.conditional && closers.contains(&a.path))
        };
        let closed = !closers.is_empty() && {
            let summary = &self.summaries[module.file as usize];
            let own = match module.unit {
                Some(unit) => says(&summary.offers().units()[unit as usize].attributes),
                // A file's own unit: what the file says at its top, and
                // what the unit that declares it says of it.
                None => {
                    says(summary.offers().attributes())
                        || self.declaring_unit(module).is_some_and(|(file, unit)| {
                            says(
                                &self.summaries[file as usize].offers().units()[unit as usize]
                                    .attributes,
                            )
                        })
                }
            };
            own || self.parent(module).is_some_and(|parent| self.closed(parent))
        };
        self.closed.borrow_mut().insert(module, closed);
        closed
    }

    /// The unit that declares a file's own unit `module` without a body,
    /// by its file and its index there; `None` for a crate's root.
    fn declaring_unit(&self, module: Module) -> Option<(u32, u32)> {
        let file = &self.files[module.file as usize];
        let (declaring, unit) = self.graph.declared_by(module.krate, file)?;
        Some((*self.ids.get(declaring)?, unit))
    }

    /// The module `module` is declared in; `None` for a crate's root.
    fn parent(&self, module: Module) -> Option<Module> {
        match module.unit {
            Some(unit) => {
                let parent =
                    self.summaries[module.file as usize].offers().units()[unit as usize].parent;
                Some(Module { unit: parent, ..module })
            }
            None => {
                let (declaring, unit) = self.declaring_unit(module)?;
                let parent =
                    self.summaries[declaring as usize].offers().units()[unit as usize].parent;
                Some(Module { krate: module.krate, file: declaring, unit: parent })
            }
        }
    }

    /// Whether `module` is `ancestor` or inside it.
    fn within(&self, mut module: Module, ancestor: Module) -> bool {
        loop {
            if module == ancestor {
                return true;
            }
            match self.parent(module) {
                Some(parent) => module = parent,
                None => return false,
            }
        }
    }

    /// The target declaration `index` of `file` is, in crate `krate`: the
    /// module too, when it declares a unit.
    fn target_of(&self, krate: u32, file: u32, index: u32) -> Target {
        let summary = &self.summaries[file as usize];
        // A unit's declaration is the one whose name is the unit's name,
        // where it is written.
        let named = summary.locations().name_span(index as usize);
        let unit = (0..summary.offers().units().len())
            .find(|&i| summary.locations().unit_span(i) == named);
        let module = unit.and_then(|i| {
            let unit = &summary.offers().units()[i];
            if unit.inline {
                Some(Module { krate, file, unit: Some(i as u32) })
            } else {
                let body = self.graph.body(krate, &self.files[file as usize], i as u32)?;
                Some(Module { krate, file: *self.ids.get(body)?, unit: None })
            }
        });
        Target { declaration: Some((file, index)), module }
    }

    /// The items of `file`'s unit `unit` by name: its declarations made
    /// directly in it, not inside another declaration.
    fn items_of(&self, file: u32, unit: Option<u32>) -> Items {
        if let Some(items) = self.items.borrow().get(&(file, unit)) {
            return items.clone();
        }
        let summary = &self.summaries[file as usize];
        let units = summary.offers().units();
        let depth = {
            let mut depth = 0;
            let mut at = unit;
            while let Some(i) = at {
                depth += 1;
                at = units[i as usize].parent;
            }
            depth
        };
        let mut items: HashMap<String, Vec<u32>> = HashMap::new();
        for (i, d) in summary.offers().declarations().iter().enumerate() {
            if d.unit == unit && d.impl_block.is_none() && d.container.len() == depth {
                items.entry(d.name.clone()).or_default().push(i as u32);
            }
        }
        let items = Rc::new(items);
        self.items.borrow_mut().insert((file, unit), items.clone());
        items
    }

    /// The imports made in `module`: what it re-exports, then what it
    /// imports for itself.
    fn imports_of(&self, module: Module) -> impl Iterator<Item = &Import> {
        let summary = &self.summaries[module.file as usize];
        summary
            .offers()
            .reexports()
            .iter()
            .chain(summary.needs().imports())
            .filter(move |i| i.unit == module.unit)
    }

    /// What `name` binds in `module`. See the [module docs](self).
    fn names_of(&self, module: Module, name: &str) -> Bindings {
        let key = (module, name.to_string());
        if let Some(found) = self.names.borrow().get(&key) {
            return found.clone();
        }
        // Under way: a cycle. What it has found so far.
        let open = self.open.borrow().get(&key).copied();
        if let Some(depth) = open {
            self.reach(depth);
            let so_far = self.provisional.borrow().get(&key).map(|p| p.found.clone());
            return so_far.unwrap_or_default();
        }
        // Found already in this round of the cycle being settled.
        let this_round =
            self.provisional.borrow().get(&key).filter(|p| p.round == self.round.get()).cloned();
        if let Some(p) = this_round {
            self.reach(p.reached);
            return p.found;
        }

        let depth = self.depth.get();
        self.depth.set(depth + 1);
        self.open.borrow_mut().insert(key.clone(), depth);
        let outer_reached = self.reached.replace(usize::MAX);
        let outer_grew = self.grew.replace(false);
        let outer_round = self.round.get();
        let first = self.made.get();
        let (mut settling, mut provisional) = (false, false);
        let found = loop {
            let before = self.provisional.borrow().get(&key).map(|p| p.found.clone());
            let found = grown(before.as_deref(), self.find_names(module, name));
            let reached = self.reached.get();
            if reached < depth {
                // In a cycle a lookup further out settles.
                self.keep_provisional(&key, &found, before.as_deref(), reached);
                provisional = true;
                break found;
            }
            if reached == usize::MAX && !settling {
                // It met no lookup under way: found.
                self.provisional.borrow_mut().remove(&key);
                self.names.borrow_mut().insert(key.clone(), found.clone());
                break found;
            }
            // The outermost lookup of a cycle: again, in a round of its
            // own, until a round finds nothing new.
            settling = true;
            self.keep_provisional(&key, &found, before.as_deref(), depth);
            if !self.grew.replace(false) {
                self.settle(depth, first);
                break found;
            }
            self.round.set(self.next_round.get());
            self.next_round.set(self.next_round.get() + 1);
            self.reached.set(usize::MAX);
        };
        self.open.borrow_mut().remove(&key);
        self.depth.set(depth);
        self.round.set(outer_round);
        // What grew under a provisional lookup grew in the cycle further
        // out; what grew under a settled one is settled.
        let grew = self.grew.get();
        self.grew.set(outer_grew || (provisional && grew));
        let reached = self.reached.get();
        self.reached.set(if reached < depth { outer_reached.min(reached) } else { outer_reached });
        found
    }

    /// The innermost lookup under way has met the one at `depth`.
    fn reach(&self, depth: usize) {
        self.reached.set(self.reached.get().min(depth));
    }

    /// Keep `found` as `key`'s answer so far, found in this round, having
    /// met the lookup under way at `reached`, and note whether it grew from
    /// `before`.
    fn keep_provisional(
        &self,
        key: &(Module, String),
        found: &Bindings,
        before: Option<&Vec<Binding>>,
        reached: usize,
    ) {
        if before.is_none_or(|b| b.len() < found.len()) {
            self.grew.set(true);
        }
        let made = self.made.get();
        self.made.set(made + 1);
        self.provisional.borrow_mut().insert(
            key.clone(),
            Provisional { found: found.clone(), round: self.round.get(), reached, made },
        );
    }

    /// The cycle whose outermost lookup, at `depth`, began when `first`
    /// provisional answers had been made is settled: the answers made under
    /// it in its last round are kept, and those of its earlier rounds that
    /// the last did not find again are dropped (a lookup finds them anew).
    fn settle(&self, depth: usize, first: u64) {
        let round = self.round.get();
        let mut names = self.names.borrow_mut();
        self.provisional.borrow_mut().retain(|key, p| {
            if p.made < first || p.reached < depth {
                return true;
            }
            if p.round == round {
                names.insert(key.clone(), p.found.clone());
            }
            false
        });
    }

    fn find_names(&self, module: Module, name: &str) -> Vec<Binding> {
        let mut found = self.find_names_once(module, name);
        let mut unique: Vec<Binding> = Vec::with_capacity(found.len());
        for binding in found.drain(..) {
            if !unique.contains(&binding) {
                unique.push(binding);
            }
        }
        unique
    }

    fn find_names_once(&self, module: Module, name: &str) -> Vec<Binding> {
        let summary = self.summaries[module.file as usize].clone();
        let declarations = summary.offers().declarations();
        let mut found: Vec<Binding> = Vec::new();
        if let Some(items) = self.items_of(module.file, module.unit).get(name) {
            for &index in items {
                let d: &Declaration = &declarations[index as usize];
                found.push(Binding {
                    target: self.target_of(module.krate, module.file, index),
                    visibility: d.visibility.clone(),
                    from: module,
                });
            }
        }
        let imports: Vec<Import> = self.imports_of(module).cloned().collect();
        for import in imports.iter().filter(|i| !i.glob && i.name.as_deref() == Some(name)) {
            let from = if import.loads { Start::Loading } else { Start::Anywhere };
            if let Ok(targets) = self.walk_from(module, &import.path, from) {
                for target in targets {
                    found.push(Binding {
                        target,
                        visibility: import.visibility.clone(),
                        from: module,
                    });
                }
            }
        }
        if found.is_empty() {
            for import in imports.iter().filter(|i| i.glob) {
                let Ok(targets) = self.walk(module, &import.path) else { continue };
                for target in targets {
                    match (target.module, target.declaration) {
                        (Some(globbed), _) => {
                            for binding in self.names_of(globbed, name).iter() {
                                if self.visible(binding, module) {
                                    found.push(Binding {
                                        target: binding.target,
                                        visibility: import.visibility.clone(),
                                        from: module,
                                    });
                                }
                            }
                        }
                        (None, Some((file, index))) => {
                            for target in self.declared_inside(file, index, name) {
                                found.push(Binding {
                                    target,
                                    visibility: import.visibility.clone(),
                                    from: module,
                                });
                            }
                        }
                        (None, None) => {}
                    }
                }
            }
        }
        found
    }

    /// Whether `binding` may be seen from `module`.
    fn visible(&self, binding: &Binding, module: Module) -> bool {
        let Visibility::Within(path) = &binding.visibility else {
            return true;
        };
        let within = match path.root {
            Some(Root::Unit(up)) if path.segments.is_empty() => {
                let mut at = Some(binding.from);
                for _ in 0..up {
                    at = at.and_then(|m| self.parent(m));
                }
                at.into_iter().collect::<Vec<_>>()
            }
            Some(Root::Crate) if path.segments.is_empty() => {
                self.crate_root(binding.from.krate).module.into_iter().collect()
            }
            _ => match self.walk(binding.from, path) {
                Ok(targets) => targets.iter().filter_map(|t| t.module).collect(),
                Err(_) => Vec::new(),
            },
        };
        within
            .into_iter()
            .any(|ancestor| module.krate == ancestor.krate && self.within(module, ancestor))
    }

    // ------------------------------------------------------------------
    // Impl blocks
    // ------------------------------------------------------------------

    /// The impl blocks of declaration `index` of `file`, a type, found once:
    /// each block whose type's path ends in its name, or in a name imports
    /// give it (an import whose path ends in one of those, renaming), and
    /// whose type resolves to it. While some are being found (a block's
    /// type is a path through a type, say), there are none: a type's
    /// members are then those declared inside it, for an impl block's type
    /// does not name another block's items. What was found with a lookup's
    /// answer so far, in a cycle not yet settled, is not kept.
    fn impls_of(&self, file: u32, index: u32) -> ImplBlocks {
        if let Some(found) = self.impls.borrow().get(&(file, index)) {
            return found.clone();
        }
        if self.finding_impls.get() {
            return ImplBlocks::default();
        }
        let names = self.impl_names();
        let mut called = vec![self.summaries[file as usize].offers().declarations()
            [index as usize]
            .name
            .clone()];
        let mut at = 0;
        while at < called.len() {
            for rename in names.renames.get(&called[at]).into_iter().flatten() {
                if !called.contains(rename) {
                    called.push(rename.clone());
                }
            }
            at += 1;
        }
        let mut candidates: Vec<(u32, u32)> =
            called.iter().filter_map(|n| names.by_name.get(n)).flatten().copied().collect();
        candidates.sort_unstable();
        candidates.dedup();

        self.finding_impls.set(true);
        let outer_reached = self.reached.replace(usize::MAX);
        let mut settled = true;
        let mut found: Vec<(u32, u32)> = Vec::new();
        for block in candidates {
            let (types, kept) = self.impl_type(block);
            settled &= kept;
            if types.contains(&(file, index)) {
                found.push(block);
            }
        }
        self.finding_impls.set(false);
        self.reached.set(outer_reached.min(self.reached.get()));
        let found = Rc::new(found);
        if settled {
            self.impls.borrow_mut().insert((file, index), found.clone());
        }
        found
    }

    /// The declarations impl block `block`'s type resolves to, from where
    /// the block is, in each crate its file is in; and whether that is kept
    /// (it met no lookup's answer so far). Called while impls are being
    /// found.
    fn impl_type(&self, block: (u32, u32)) -> (Rc<Vec<(u32, u32)>>, bool) {
        if let Some(types) = self.impl_types.borrow().get(&block) {
            return (types.clone(), true);
        }
        let (block_file, i) = block;
        let summary = &self.summaries[block_file as usize];
        let Some(of) = summary.offers().impls().get(i as usize) else {
            return (Rc::default(), true);
        };
        let outer_reached = self.reached.replace(usize::MAX);
        let mut types: Vec<(u32, u32)> = Vec::new();
        if let Some(self_type) = &of.self_type {
            for (krate, _) in self.graph.places(&self.files[block_file as usize]) {
                let at = Module { krate, file: block_file, unit: of.unit };
                let Ok(targets) = self.walk(at, self_type) else { continue };
                for target in targets {
                    if let (Some(declaration), None) = (target.declaration, target.module) {
                        if !types.contains(&declaration) {
                            types.push(declaration);
                        }
                    }
                }
            }
        }
        let reached = self.reached.get();
        self.reached.set(outer_reached.min(reached));
        let types = Rc::new(types);
        let kept = reached == usize::MAX;
        if kept {
            self.impl_types.borrow_mut().insert(block, types.clone());
        }
        (types, kept)
    }

    /// Each impl block by the last name of its type's path, and the names
    /// imports give what their paths name, by the last name of the path:
    /// gathered once.
    fn impl_names(&self) -> Rc<ImplNames> {
        if let Some(names) = self.impl_names.borrow().as_ref() {
            return names.clone();
        }
        let mut names = ImplNames::default();
        for (file, summary) in self.summaries.iter().enumerate() {
            for (i, block) in summary.offers().impls().iter().enumerate() {
                if let Some(last) = block.self_type.as_ref().and_then(|t| t.segments.last()) {
                    names.by_name.entry(last.clone()).or_default().push((file as u32, i as u32));
                }
            }
            let imports = summary.offers().reexports().iter().chain(summary.needs().imports());
            for import in imports.filter(|i| !i.glob) {
                if let (Some(name), Some(last)) = (&import.name, import.path.segments.last()) {
                    if name != last {
                        let renames = names.renames.entry(last.clone()).or_default();
                        if !renames.contains(name) {
                            renames.push(name.clone());
                        }
                    }
                }
            }
        }
        let names = Rc::new(names);
        *self.impl_names.borrow_mut() = Some(names.clone());
        names
    }
}

/// `found`, with what `before` held that it does not: an answer only grows
/// while a cycle is settled.
fn grown(before: Option<&Vec<Binding>>, found: Vec<Binding>) -> Bindings {
    let Some(before) = before else {
        return Rc::new(found);
    };
    let mut all = before.clone();
    for binding in found {
        if !all.contains(&binding) {
            all.push(binding);
        }
    }
    Rc::new(all)
}
