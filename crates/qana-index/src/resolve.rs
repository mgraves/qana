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
//! root, or a crate named by its first segment) or, without one, at the
//! names of the module it stands in and then at the crates its crate may
//! name. Each segment after is a name of the module before it, a member of
//! the type before it (an enum's variants and a trait's items, which are
//! declared inside it, and the items of every impl block whose type
//! resolves to it), or nothing: the path escapes there.
//!
//! What escapes still escapes: a crate outside the project, a language's
//! prelude, what a macro expands to, `Self` (whose type a summary does not
//! say). Nothing is reported unresolved. Visibility decides what a glob
//! brings; a path that names what it may not see is followed anyway, for a
//! reader may go to it (the compiler says the rest).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path as FilePath, PathBuf};
use std::rc::Rc;

use crate::{Declaration, Import, Path, Root, Summary, UnitGraph, Visibility};

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
    /// prelude, a macro's expansion, a member no summary holds.
    Escapes { segment: usize },
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
    /// The impl blocks of each declaration that is a type, once found:
    /// (file, impl index) by (file, declaration index).
    impls: RefCell<Option<ImplBlocks>>,
    /// Whether they are being found: an impl block's type is a path, and
    /// while they are, a type's members are those declared inside it.
    finding_impls: Cell<bool>,
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

/// The impl blocks of each type: (file, impl index) by (file,
/// declaration index).
type ImplBlocks = Rc<HashMap<(u32, u32), Vec<(u32, u32)>>>;

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
            impls: RefCell::new(None),
            finding_impls: Cell::new(false),
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
            return Resolution::Escapes { segment: furthest };
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
                (self.extern_crate(at.krate, name).ok_or(0usize)?, 1)
            }
            None => {
                let name = path.segments.first().ok_or(0usize)?;
                let mut found: Vec<Target> =
                    self.names_of(at, name).iter().map(|b| b.target).collect();
                if found.is_empty() {
                    found = self.extern_crate(at.krate, name).unwrap_or_default();
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
        let mut found = self.declared_inside(file, index, name);
        if let Some(blocks) = self.impl_blocks().get(&(file, index)) {
            for &(block_file, block) in blocks {
                let summary = &self.summaries[block_file as usize];
                for (i, d) in summary.offers().declarations().iter().enumerate() {
                    if d.impl_block == Some(block) && d.name == name {
                        found.push(Target {
                            declaration: Some((block_file, i as u32)),
                            module: None,
                        });
                    }
                }
            }
        }
        found
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

    /// The crate `krate` names `name`, as its root.
    fn extern_crate(&self, krate: u32, name: &str) -> Option<Vec<Target>> {
        let info = &self.graph.crates()[krate as usize];
        let found: Vec<Target> = info
            .dependencies
            .iter()
            .filter(|(named, _)| named == name)
            .map(|&(_, dependency)| self.crate_root(dependency))
            .filter(|t| t.module.is_some())
            .collect();
        (!found.is_empty()).then_some(found)
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
                let file = &self.files[module.file as usize];
                let (declaring, unit) = self.graph.declared_by(module.krate, file)?;
                let declaring = *self.ids.get(declaring)?;
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
            if let Ok(targets) = self.walk(module, &import.path) {
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

    /// The impl blocks of each type, found once: each block's type resolved
    /// from where the block is, in each crate its file is in. While they
    /// are being found (a block's type is a path through a type, say),
    /// there are none: a type's members are then those declared inside it,
    /// for an impl block's type does not name another block's items.
    fn impl_blocks(&self) -> ImplBlocks {
        if let Some(found) = self.impls.borrow().as_ref() {
            return found.clone();
        }
        if self.finding_impls.replace(true) {
            return ImplBlocks::default();
        }
        let mut found: HashMap<(u32, u32), Vec<(u32, u32)>> = HashMap::new();
        for (file, summary) in self.summaries.iter().enumerate() {
            let file = file as u32;
            for (block, i) in summary.offers().impls().iter().zip(0u32..) {
                let Some(self_type) = &block.self_type else { continue };
                for (krate, _) in self.graph.places(&self.files[file as usize]) {
                    let at = Module { krate, file, unit: block.unit };
                    let Ok(targets) = self.walk(at, self_type) else { continue };
                    for target in targets {
                        if let (Some(declaration), None) = (target.declaration, target.module) {
                            let blocks = found.entry(declaration).or_default();
                            if !blocks.contains(&(file, i)) {
                                blocks.push((file, i));
                            }
                        }
                    }
                }
            }
        }
        let found = Rc::new(found);
        *self.impls.borrow_mut() = Some(found.clone());
        self.finding_impls.set(false);
        found
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
