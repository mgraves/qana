//! A file's summary merged with the expansions of its invocations: what a
//! project index resolves over, as rustc resolves a crate once its macros
//! are expanded.
//!
//! Each expansion's offers and needs join the file's after its own, where
//! its invocation stands: its top-level declarations, imports, impl blocks
//! and invocations in the invocation's unit (an impl block's items among
//! that block's members), its own units below it, what it defines by
//! textual scope after the invocation's link. The invocations in an
//! expansion are merged in turn, with theirs. Every place is in the file:
//! where the file's invocation writes it (what it passes, forwarded), or
//! at its macro's path, the call, where a macro writes it.
//!
//! What `$crate` names, the expansion's definition says: a path rooted at
//! it ([`Root::MacroCrate`]) is rooted at the root the index gives
//! ([`Root::CrateAt`]).
//!
//! A unit is open while an invocation among its items is not expanded, or
//! failed to by each of its macros' alternatives (a crate that compiles
//! takes one that expands): the file's own summary says a unit with any
//! invocation is.

use std::rc::Rc;

use crate::expand::SitePlace;
use crate::{
    Declaration, ExpansionSummary, Impl, Import, Invocation, MacroDef, Path, Root, Summary,
    TextualLink, TypeRef, Unit, Visibility,
};

/// An invocation's expansions, as [`merge`] asks for them: given the
/// expansions it stands in (as [`Invocation::within`] says) and its index
/// there, each of its macro's answers' expansion, by which answer it is,
/// with the root `$crate` names there (`None` where it is not known).
/// `None` while not all are known: the unit it stands in stays open.
pub type Expansions<'a> =
    dyn Fn(&[(u32, u32)], u32) -> Option<Vec<(u32, Rc<ExpansionSummary>, Option<Root>)>> + 'a;

/// `own`, a file's summary, merged with its invocations' expansions as
/// `expansions` gives them: see the [module docs](self).
pub fn merge(own: &Summary, expansions: &Expansions) -> Summary {
    let mut merged = own.clone();
    let mut complete: Vec<bool> = Vec::new();
    let mut at = 0usize;
    while at < merged.offers.invocations.len() {
        let done = splice(&mut merged, at, expansions);
        complete.push(done);
        at += 1;
    }

    // Ordinals among namesakes, the file's first.
    let mut seen: std::collections::HashMap<(Vec<String>, String, String), u32> =
        std::collections::HashMap::new();
    for d in &mut merged.offers.declarations {
        let count = seen.entry((d.container.clone(), d.name.clone(), d.kind.clone())).or_insert(0);
        d.ordinal = *count;
        *count += 1;
    }

    // A unit is open while an invocation among its items is not expanded.
    let clean = merged.offers.clean;
    let open_in = |unit: Option<u32>| {
        !clean
            || merged
                .offers
                .invocations
                .iter()
                .zip(&complete)
                .any(|(i, done)| i.unit == unit && !i.members && !done)
    };
    merged.offers.open = open_in(None);
    let opens: Vec<bool> =
        (0..merged.offers.units.len()).map(|u| open_in(Some(u as u32))).collect();
    for (unit, open) in merged.offers.units.iter_mut().zip(opens) {
        unit.open = open;
    }
    merged.fingerprint();
    merged
}

/// Splice the expansions of `merged`'s invocation `at` into it: whether
/// they are all known and one expanded (its macro's alternatives are under
/// conditions, of which a crate that compiles takes one that expands).
fn splice(merged: &mut Summary, at: usize, expansions: &Expansions) -> bool {
    let site = merged.offers.invocations[at].clone();
    let Some(found) = expansions(&site.within, site.index) else { return false };
    // The file's invocation whose text holds the places: the outermost.
    let top = match site.within.first() {
        None => at,
        Some(&(outer, _)) => outer as usize,
    };
    let top_span = merged.locations.invocations[top];
    let top_position = merged.locations.invocation_positions[top];
    let call = merged.locations.invocation_paths[top].unwrap_or(top_span);
    let place = |p: &SitePlace| -> ((u32, u32), (u32, u32)) {
        match p {
            Some(((s, e), (line, column))) => {
                let span = (top_span.0 + s, top_span.0 + e);
                let position = if *line == 0 {
                    (top_position.0, top_position.1 + column)
                } else {
                    (top_position.0 + line, *column)
                };
                (span, position)
            }
            None => (call, top_position),
        }
    };
    let mut expanded = false;
    for (which, expansion, crate_root) in found {
        let (Some(offers), Some(needs), Some(placed)) =
            (expansion.offers(), expansion.needs(), expansion.placed())
        else {
            continue;
        };
        expanded = true;
        let chain: Vec<(u32, u32)> = {
            let mut chain = site.within.clone();
            chain.push((site.index, which));
            chain
        };
        let base_unit = merged.offers.units.len() as u32;
        let base_impl = merged.offers.impls.len() as u32;
        let base_declaration = merged.offers.declarations.len() as u32;
        let base_invocation = merged.offers.invocations.len() as u32;
        let unit = |u: Option<u32>| match u {
            None => site.unit,
            Some(u) => Some(base_unit + u),
        };
        let link = |l: &TextualLink| match l {
            TextualLink::Def { declaration, attributes } => TextualLink::Def {
                declaration: base_declaration + declaration,
                attributes: attributes.clone(),
            },
            TextualLink::Module(u) => TextualLink::Module(base_unit + u),
            TextualLink::Invocation(i) => TextualLink::Invocation(base_invocation + i),
        };
        let rooted = Rooted(crate_root);

        for (k, u) in offers.units.iter().enumerate() {
            merged.offers.units.push(Unit {
                parent: unit(u.parent),
                textual: u.textual.iter().map(link).collect(),
                visibility: rooted.visibility(&u.visibility),
                ..u.clone()
            });
            merged.locations.units.push(place(&placed.units[k]).0);
            merged.locations.unit_items.push(place(&placed.unit_items[k]).0);
        }
        for (k, block) in offers.impls.iter().enumerate() {
            merged.offers.impls.push(Impl {
                unit: unit(block.unit),
                self_type: block.self_type.as_ref().map(|t| rooted.ty(t)),
                trait_path: block.trait_path.as_ref().map(|p| rooted.path(p)),
                generics: block.generics.clone(),
            });
            merged.locations.impls.push(place(&placed.impls[k]).0);
            let (ty, tr) = &placed.impl_heads[k];
            merged
                .locations
                .impl_heads
                .push((ty.as_ref().map(|p| place(p).0), tr.as_ref().map(|p| place(p).0)));
        }
        for (k, d) in offers.declarations.iter().enumerate() {
            let mut container = site.container.clone();
            container.extend(d.container.iter().cloned());
            merged.offers.declarations.push(Declaration {
                container,
                unit: unit(d.unit),
                impl_block: d.impl_block.map(|b| base_impl + b).or(site.impl_block),
                visibility: rooted.visibility(&d.visibility),
                ty: d.ty.as_ref().map(|t| rooted.ty(t)),
                returns: d.returns.as_ref().map(|t| rooted.ty(t)),
                derives: d.derives.iter().map(|p| rooted.path(p)).collect(),
                bounds: d.bounds.iter().map(|t| rooted.ty(t)).collect(),
                positional: d.positional.iter().map(|t| rooted.ty(t)).collect(),
                params: d.params.iter().map(|t| t.as_ref().map(|t| rooted.ty(t))).collect(),
                ..d.clone()
            });
            let (name, position) = place(&placed.names[k]);
            merged.locations.names.push(name);
            merged.locations.positions.push(position);
            merged.locations.items.push(place(&placed.items[k]).0);
            merged.locations.derives.push(placed.derives[k].iter().map(|p| place(p).0).collect());
            merged.locations.declared_by.push(Some(top as u32));
        }
        for (k, import) in offers.reexports.iter().enumerate() {
            merged.offers.reexports.push(rooted.import(import, unit(import.unit)));
            merged.locations.reexports.push(place(&placed.reexports[k]).0);
        }
        for (k, import) in needs.imports.iter().enumerate() {
            merged.needs.imports.push(rooted.import(import, unit(import.unit)));
            merged.locations.imports.push(place(&placed.imports[k]).0);
        }
        for (k, inv) in offers.invocations.iter().enumerate() {
            let mut container = site.container.clone();
            container.extend(inv.container.iter().cloned());
            merged.offers.invocations.push(Invocation {
                unit: unit(inv.unit),
                container,
                impl_block: inv.impl_block.map(|b| base_impl + b).or(site.impl_block),
                members: inv.members || (inv.unit.is_none() && site.members),
                path: inv.path.as_ref().map(|p| rooted.path(p)),
                within: chain.clone(),
                index: k as u32,
                ..inv.clone()
            });
            merged.locations.invocations.push(place(&placed.invocations[k]).0);
            merged
                .locations
                .invocation_paths
                .push(placed.invocation_paths[k].as_ref().map(|p| place(p).0));
            merged.locations.invocation_items.push(place(&placed.invocation_items[k]).0);
            let top_chars = merged.locations.invocation_path_chars[top];
            merged.locations.invocation_path_chars.push(top_chars);
            merged.locations.invocation_positions.push(place(&placed.invocations[k]).1);
        }
        for (k, m) in offers.macros.iter().enumerate() {
            merged.offers.macros.push(MacroDef {
                declaration: base_declaration + m.declaration,
                within: chain.clone(),
                index: k as u32,
                ..m.clone()
            });
            merged.locations.macro_bodies.push(place(&placed.macro_bodies[k]).0);
        }
        // What it defines by textual scope, after its invocation's link.
        let links: Vec<TextualLink> = offers.textual.iter().map(link).collect();
        let module = match site.unit {
            None => &mut merged.offers.textual,
            Some(u) => &mut merged.offers.units[u as usize].textual,
        };
        let after = module
            .iter()
            .position(|l| *l == TextualLink::Invocation(at as u32))
            .map_or(module.len(), |k| k + 1);
        module.splice(after..after, links);
    }
    expanded
}

/// What `$crate` is rooted at in an expansion: `None` where the index
/// does not know, which leaves such a path rooted at
/// [`Root::MacroCrate`], escaping.
struct Rooted(Option<Root>);

impl Rooted {
    fn path(&self, p: &Path) -> Path {
        match (p.root, self.0) {
            (Some(Root::MacroCrate), Some(root)) => Path { root: Some(root), ..p.clone() },
            _ => p.clone(),
        }
    }

    fn ty(&self, t: &TypeRef) -> TypeRef {
        match t {
            TypeRef::Path { path, args, bindings } => TypeRef::Path {
                path: self.path(path),
                args: args.iter().map(|a| self.ty(a)).collect(),
                bindings: bindings.iter().map(|(n, b)| (n.clone(), self.ty(b))).collect(),
            },
            TypeRef::Ref(t) => TypeRef::Ref(Box::new(self.ty(t))),
            TypeRef::Slice(t) => TypeRef::Slice(Box::new(self.ty(t))),
            TypeRef::Array(t) => TypeRef::Array(Box::new(self.ty(t))),
            TypeRef::Tuple(ts) => TypeRef::Tuple(ts.iter().map(|t| self.ty(t)).collect()),
            TypeRef::Dyn(ts) => TypeRef::Dyn(ts.iter().map(|t| self.ty(t)).collect()),
            TypeRef::Impl(ts) => TypeRef::Impl(ts.iter().map(|t| self.ty(t)).collect()),
            TypeRef::Other => TypeRef::Other,
        }
    }

    fn visibility(&self, v: &Visibility) -> Visibility {
        match v {
            Visibility::Public => Visibility::Public,
            Visibility::Within(p) => Visibility::Within(self.path(p)),
        }
    }

    fn import(&self, import: &Import, unit: Option<u32>) -> Import {
        Import {
            unit,
            path: self.path(&import.path),
            visibility: self.visibility(&import.visibility),
            ..import.clone()
        }
    }
}
