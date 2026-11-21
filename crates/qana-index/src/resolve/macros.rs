//! Textual names (Rust's macros): found as rustc finds them. See the
//! [module docs](super).

use std::collections::{HashMap, HashSet};
use std::path::{Path as FilePath, PathBuf};
use std::rc::Rc;

use super::{Exported, MacroResolution, Module, Resolution, Resolver, Start};
use crate::{Attribute, CrateInfo, Path, Seen, TextualLink};

/// What a textual search has found, nearest first: definitions in the
/// file the name stands in (by their names' spans) and declarations
/// elsewhere ((file, declaration index)); and whether it is done, which
/// one that surely is (under no condition) makes it.
#[derive(Default)]
struct Found {
    here: Vec<(u32, u32)>,
    declared: Vec<(u32, u32)>,
    done: bool,
}

impl Found {
    /// Take a definition in the file, under a condition or not.
    fn here(&mut self, span: (u32, u32), conditional: bool) {
        self.here.push(span);
        self.done = !conditional;
    }

    /// Take a declaration, under a condition or not.
    fn declared(&mut self, declaration: (u32, u32), conditional: bool) {
        self.declared.push(declaration);
        self.done = !conditional;
    }
}

impl Resolver {
    /// What `path`, a textual name's (Rust's macro's: in `namespace`, one
    /// [`crate::is_textual`] says is so), names where it stands: in `file`,
    /// inside the units with a body named `units` (outermost first), seeing
    /// `seen` there by textual scope ([`crate::textual_at`]). A name alone
    /// is looked for as rustc looks for it: by textual scope, in the file
    /// and then in each module it is declared in, before its declaration;
    /// then among its module's names (not a textual definition made there,
    /// unless exported); then in the macro prelude
    /// ([`CrateInfo::macro_prelude`], and the crates the root loads under
    /// [`CrateInfo::macro_use`]); then in the preludes. A textual
    /// definition under a condition ([`CrateInfo::conditions`]) may not be,
    /// so the search goes on past it: each one it meets is an answer, to
    /// one that surely is ([`MacroResolution::Several`]). A path is
    /// resolved as [`resolve`](Self::resolve) resolves one, a crate's root
    /// having the textual definitions exported from anywhere in it
    /// ([`CrateInfo::macro_export`]). In a file no crate holds, a name
    /// alone sees the textual definitions before it there alone
    /// ([`seen_here`](crate::seen_here)).
    pub fn resolve_macro(
        &self,
        file: &FilePath,
        units: &[String],
        path: &Path,
        namespace: &str,
        seen: &[Seen],
    ) -> MacroResolution {
        if path.root.is_some() || path.segments.len() != 1 {
            return MacroResolution::Elsewhere(self.resolve(file, units, path, Some(namespace)));
        }
        let name = &path.segments[0];
        let places = self.graph.places(file);
        let id = match self.ids.get(file) {
            Some(&id) if !places.is_empty() => id,
            _ => {
                return match crate::seen_here(seen, name) {
                    Some(span) => MacroResolution::Here(span),
                    None => MacroResolution::Elsewhere(Resolution::Escapes { segment: 0 }),
                }
            }
        };
        let mut here: Vec<(u32, u32)> = Vec::new();
        let mut declarations: Vec<(PathBuf, u32)> = Vec::new();
        for (krate, _) in places {
            let info = &self.graph.crates()[krate as usize];
            let mut found = Found::default();
            self.in_seen(krate, id, seen, name, info, false, &mut HashSet::new(), &mut found);
            if !found.done {
                self.outward(krate, id, name, info, &mut found);
            }
            if !found.done {
                let at = Module { krate, file: id, unit: self.unit_named(id, units) };
                let named = self
                    .in_module(at, name, namespace, info)
                    .or_else(|| self.in_macro_prelude(krate, name, namespace, info))
                    .or_else(|| self.in_preludes(krate, name, namespace, info));
                found.declared.extend(named.unwrap_or_default());
            }
            for span in found.here {
                if !here.contains(&span) {
                    here.push(span);
                }
            }
            for (file, i) in found.declared {
                let declaration = (self.files[file as usize].clone(), i);
                if !declarations.contains(&declaration) {
                    declarations.push(declaration);
                }
            }
        }
        match (&here[..], declarations.is_empty()) {
            ([], true) => MacroResolution::Elsewhere(Resolution::Escapes { segment: 0 }),
            ([], false) => {
                declarations.sort();
                MacroResolution::Elsewhere(Resolution::Found { declarations, modules: Vec::new() })
            }
            ([span], true) => MacroResolution::Here(*span),
            _ => MacroResolution::Several { here, declarations },
        }
    }

    /// The textual definitions named `name` that `seen` holds, in file
    /// `file` of crate `krate`, nearest first, into `found`: those there,
    /// and those a module there carries out; each under a condition where
    /// `conditional`, or where it or a module it is carried out of is
    /// under one.
    #[allow(clippy::too_many_arguments)]
    fn in_seen(
        &self,
        krate: u32,
        file: u32,
        seen: &[Seen],
        name: &str,
        info: &CrateInfo,
        conditional: bool,
        visited: &mut HashSet<u32>,
        found: &mut Found,
    ) {
        for s in seen.iter().rev() {
            if found.done {
                return;
            }
            match s {
                Seen::Def { name: n, span, attributes } if n == name => {
                    found.here(*span, conditional || is_conditional(info, attributes));
                }
                Seen::Def { .. } => {}
                Seen::Module { attributes, seen } => {
                    let Some(maybe) = carries_out(info, attributes) else { continue };
                    let conditional = conditional || maybe || is_conditional(info, attributes);
                    self.in_seen(krate, file, seen, name, info, conditional, visited, found);
                }
                Seen::File { units, attributes } => {
                    let Some(maybe) = carries_out(info, attributes) else { continue };
                    let Some(body) = self.body_of(krate, file, units) else { continue };
                    let conditional = conditional || maybe || is_conditional(info, attributes);
                    let summary = self.summaries[body as usize].clone();
                    let links = summary.offers().textual();
                    self.in_links(krate, body, links, name, info, conditional, visited, found);
                }
            }
        }
    }

    /// The file the unit under `units` of `file` has its body in, where it
    /// is a module declared without one there.
    fn body_of(&self, krate: u32, file: u32, units: &[String]) -> Option<u32> {
        let all = self.summaries[file as usize].offers().units();
        let mut at: Option<u32> = None;
        for name in units {
            at = Some(
                all.iter().enumerate().find(|(_, u)| u.parent == at && &u.name == name)?.0 as u32,
            );
        }
        let unit = at?;
        let body = self.graph.body(krate, &self.files[file as usize], unit)?;
        self.ids.get(body).copied()
    }

    /// The textual definitions named `name` in `links`, a module's own in
    /// `file`, nearest first, into `found`: among them, and those a child
    /// module there carries out; under a condition as
    /// [`in_seen`](Self::in_seen) says.
    #[allow(clippy::too_many_arguments)]
    fn in_links(
        &self,
        krate: u32,
        file: u32,
        links: &[TextualLink],
        name: &str,
        info: &CrateInfo,
        conditional: bool,
        visited: &mut HashSet<u32>,
        found: &mut Found,
    ) {
        let summary = self.summaries[file as usize].clone();
        let offers = summary.offers();
        for link in links.iter().rev() {
            if found.done {
                return;
            }
            match link {
                TextualLink::Def { declaration, attributes } => {
                    if offers.declarations()[*declaration as usize].name == name {
                        let conditional = conditional || is_conditional(info, attributes);
                        found.declared((file, *declaration), conditional);
                    }
                }
                TextualLink::Module(unit) => {
                    let unit_info = &offers.units()[*unit as usize];
                    let Some(maybe) = carries_out(info, &unit_info.attributes) else { continue };
                    let conditional =
                        conditional || maybe || is_conditional(info, &unit_info.attributes);
                    if unit_info.inline {
                        let links = &unit_info.textual;
                        self.in_links(krate, file, links, name, info, conditional, visited, found);
                        continue;
                    }
                    let Some(body) = self
                        .graph
                        .body(krate, &self.files[file as usize], *unit)
                        .and_then(|b| self.ids.get(b).copied())
                    else {
                        continue;
                    };
                    // A file is read once on the way: a module tree that
                    // comes back to it is no deeper.
                    if !visited.insert(body) {
                        continue;
                    }
                    let body_summary = self.summaries[body as usize].clone();
                    let links = body_summary.offers().textual();
                    self.in_links(krate, body, links, name, info, conditional, visited, found);
                }
            }
        }
    }

    /// The textual definitions named `name` that file `file`'s start sees,
    /// in crate `krate`, nearest first, into `found`: in each module it is
    /// declared in, before its declaration, out to the crate's root.
    fn outward(&self, krate: u32, file: u32, name: &str, info: &CrateInfo, found: &mut Found) {
        let mut module = Module { krate, file, unit: None };
        let mut seen_files: HashSet<u32> = HashSet::from([file]);
        while let Some((declaring, unit)) = self.declaring_unit(module) {
            if !seen_files.insert(declaring) {
                return;
            }
            let summary = self.summaries[declaring as usize].clone();
            let offers = summary.offers();
            let units = offers.units();
            let mut child = unit;
            loop {
                let container = units[child as usize].parent;
                let links = match container {
                    None => offers.textual(),
                    Some(p) => &units[p as usize].textual,
                };
                if let Some(k) = links.iter().position(|l| *l == TextualLink::Module(child)) {
                    let mut visited = HashSet::new();
                    let before = &links[..k];
                    self.in_links(krate, declaring, before, name, info, false, &mut visited, found);
                    if found.done {
                        return;
                    }
                }
                match container {
                    None => break,
                    Some(p) => child = p,
                }
            }
            module = Module { krate, file: declaring, unit: None };
        }
    }

    /// The declarations named `name` in `namespace` among `module`'s
    /// names, but the textual definitions made in it that no attribute
    /// exports, which textual scope alone sees.
    fn in_module(
        &self,
        module: Module,
        name: &str,
        namespace: &str,
        info: &CrateInfo,
    ) -> Option<Vec<(u32, u32)>> {
        let own = self.textual_defs(module.file);
        let found: Vec<(u32, u32)> = self
            .names_of(module, name)
            .iter()
            .filter_map(|b| b.target.declaration)
            .filter(|&(file, index)| {
                let d = &self.summaries[file as usize].offers().declarations()[index as usize];
                if d.namespace != namespace {
                    return false;
                }
                let made_here = file == module.file && d.unit == module.unit;
                match (made_here, own.get(&index)) {
                    (true, Some(attributes)) => exported(info, attributes),
                    _ => true,
                }
            })
            .collect();
        (!found.is_empty()).then_some(found)
    }

    /// The declarations named `name` in `namespace` among the names of the
    /// roots of the crates in `krate`'s macro prelude: those the model
    /// says, and those its root loads under an attribute that carries
    /// textual names out (all of them, or those its arguments name:
    /// `#[macro_use(a, b)] extern crate x;`).
    fn in_macro_prelude(
        &self,
        krate: u32,
        name: &str,
        namespace: &str,
        info: &CrateInfo,
    ) -> Option<Vec<(u32, u32)>> {
        let root = self.crate_root(krate).module?;
        let mut crates: Vec<Module> = Vec::new();
        for path in &info.macro_prelude {
            if let Ok(targets) = self.walk_from(root, path, Start::Prelude) {
                crates.extend(targets.iter().filter_map(|t| t.module));
            }
        }
        let imports: Vec<crate::Import> = self.imports_of(root).cloned().collect();
        for import in imports.iter().filter(|i| i.loads) {
            let brings = match loaded(info, &import.attributes) {
                None => false,
                Some(Loaded::All) => true,
                Some(Loaded::Named(names)) => names.contains(&name),
            };
            if !brings {
                continue;
            }
            if let Ok(targets) = self.walk_import(root, import) {
                crates.extend(targets.iter().filter_map(|t| t.module));
            }
        }
        self.declared_in(&crates, name, namespace)
    }

    /// The declarations named `name` in `namespace` in `krate`'s preludes.
    fn in_preludes(
        &self,
        krate: u32,
        name: &str,
        namespace: &str,
        info: &CrateInfo,
    ) -> Option<Vec<(u32, u32)>> {
        let root = self.crate_root(krate).module?;
        let mut modules: Vec<Module> = Vec::new();
        for path in &info.preludes {
            if let Ok(targets) = self.walk_from(root, path, Start::Prelude) {
                modules.extend(targets.iter().filter_map(|t| t.module));
            }
        }
        self.declared_in(&modules, name, namespace)
    }

    /// The declarations named `name` in `namespace` among the names of
    /// `modules`.
    fn declared_in(
        &self,
        modules: &[Module],
        name: &str,
        namespace: &str,
    ) -> Option<Vec<(u32, u32)>> {
        let mut found: Vec<(u32, u32)> = Vec::new();
        for &module in modules {
            for binding in self.names_of(module, name).iter() {
                if let Some((file, index)) = binding.target.declaration {
                    let d = &self.summaries[file as usize].offers().declarations()[index as usize];
                    if d.namespace == namespace {
                        found.push((file, index));
                    }
                }
            }
        }
        (!found.is_empty()).then_some(found)
    }

    /// The textual definitions of `file` (its declarations a textual link
    /// is), with the attributes said of each.
    fn textual_defs(&self, file: u32) -> HashMap<u32, Vec<Attribute>> {
        let offers = self.summaries[file as usize].offers();
        let mut out = HashMap::new();
        let mut gather = |links: &[TextualLink]| {
            for link in links {
                if let TextualLink::Def { declaration, attributes } = link {
                    out.insert(*declaration, attributes.clone());
                }
            }
        };
        gather(offers.textual());
        for unit in offers.units() {
            gather(&unit.textual);
        }
        out
    }

    /// Crate `krate`'s exported textual definitions by name: (file,
    /// declaration index). Once gathered, kept.
    pub(super) fn exported_by(&self, krate: u32) -> Exported {
        if let Some(found) = self.exported.borrow().get(&krate) {
            return found.clone();
        }
        let info = &self.graph.crates()[krate as usize];
        let mut found: HashMap<String, Vec<(u32, u32)>> = HashMap::new();
        if !info.macro_export.is_empty() {
            for (file, path) in self.files.iter().enumerate() {
                if !self.graph.places(path).iter().any(|&(k, _)| k == krate) {
                    continue;
                }
                let offers = self.summaries[file].offers();
                for (index, attributes) in self.textual_defs(file as u32) {
                    if exported(info, &attributes) {
                        let name = offers.declarations()[index as usize].name.clone();
                        found.entry(name).or_default().push((file as u32, index));
                    }
                }
            }
        }
        let found = Rc::new(found);
        self.exported.borrow_mut().insert(krate, found.clone());
        found
    }
}

/// Whether `attributes`, a module's, carry its textual definitions out of
/// it: one the model names, without arguments (which rustc refuses there).
/// `Some(true)` where each that does holds only under a condition
/// (`#[cfg_attr(test, macro_use)]`), `None` where none does.
fn carries_out(info: &CrateInfo, attributes: &[Attribute]) -> Option<bool> {
    let carrying =
        attributes.iter().filter(|a| a.args.is_empty() && info.macro_use.contains(&a.path));
    carrying.map(|a| a.conditional).reduce(|a, b| a && b)
}

/// Whether `attributes` put what they are said of under a condition, so
/// that it may not be (Rust's `#[cfg]`, held or itself conditional).
fn is_conditional(info: &CrateInfo, attributes: &[Attribute]) -> bool {
    attributes.iter().any(|a| info.conditions.contains(&a.path))
}

/// Which of a loaded crate's exported macros its import brings into the
/// macro prelude.
enum Loaded<'a> {
    /// Every one: an attribute the model names, without arguments.
    All,
    /// Those the attributes' arguments name (`#[macro_use(a, b)]`).
    Named(Vec<&'a str>),
}

/// What `attributes`, a loading import's, bring of its crate's macros:
/// `None` where no attribute the model names is among them.
fn loaded<'a>(info: &CrateInfo, attributes: &'a [Attribute]) -> Option<Loaded<'a>> {
    let mut named: Option<Vec<&'a str>> = None;
    for a in attributes.iter().filter(|a| info.macro_use.contains(&a.path)) {
        if a.args.is_empty() {
            return Some(Loaded::All);
        }
        let names = named.get_or_insert_with(Vec::new);
        for arg in &a.args {
            if let (None, [one]) = (&arg.root, &arg.segments[..]) {
                names.push(one.as_str());
            }
        }
    }
    named.map(Loaded::Named)
}

/// Whether `attributes` export a textual definition to its crate's root.
fn exported(info: &CrateInfo, attributes: &[Attribute]) -> bool {
    attributes.iter().any(|a| info.macro_export.contains(&a.path))
}
