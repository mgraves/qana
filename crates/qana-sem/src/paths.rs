//! Paths as a grammar spells them: `crate::grid::Cell`, `super::Row`,
//! `::serde::Serialize`, `Vec::new`.
//!
//! A path is a root and segments. What the grammar's productions say of a
//! subtree decides what path it spells, read from its node down:
//!
//! - a token is a one-segment path, its text;
//! - a node with `@qualify(base, name)` is its base's path, then the name
//!   its `name` child holds;
//! - a node with `@root(kind[, first])` is that root, after the root of
//!   the first of its child nodes that spells a bare root (`super` on
//!   `super` goes one unit further up), then the name `first` holds;
//! - a node that passes its name to one child (`@style(inherit, …)`, an
//!   option's element) is the path that child spells;
//! - any other node is the path its only symbol child spells, when that
//!   child is a node; a node with several symbol children, or only a
//!   token it does not pass a name to, spells no path.
//!
//! A path spells nothing when a name it needs is missing, or when its root
//! cannot stand where it does (`crate` or `::` beneath `super`).

use qana_grammar::{GreenChild, GreenNode};
use serde::{Deserialize, Serialize};

use crate::{name_child, symbol_indices, BindingConfig, RefKind};

/// Where a path begins, when it does not begin where it stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Root {
    /// The unit the path stands in (`self`: 0), or one of its ancestors:
    /// `super` is 1, `super::super` 2.
    Unit(u32),
    /// The root unit of the crate the path stands in (`crate`).
    Crate,
    /// The crates themselves (`::name`): the first segment names one.
    Global,
    /// The type of the impl block or trait the path stands in (Rust's
    /// `Self`): what it names depends on where it stands, which a path
    /// alone does not say.
    SelfType,
}

impl Root {
    /// This root, said of a path whose head already has the root `inner`:
    /// `super` on `self` or `super` goes one unit further; nothing else
    /// stands on a root.
    fn on(self, inner: Option<Root>) -> Option<Root> {
        match (self, inner) {
            (root, None) => Some(root),
            (Root::Unit(up), Some(Root::Unit(below))) => Some(Root::Unit(up + below)),
            _ => None,
        }
    }
}

/// A path: its root, when it has one, then its segments' names. A path
/// with no root is looked up from where it stands.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Path {
    /// Where it begins: `None` for a path that begins with a name looked
    /// up from where it stands.
    pub root: Option<Root>,
    /// The names after the root, in order.
    pub segments: Vec<String>,
}

impl Path {
    /// The path the `k`th symbol child of `n` spells (see the [module
    /// docs](self)).
    pub fn of_child(n: &GreenNode, k: usize, cfg: &BindingConfig) -> Option<Path> {
        let child = n
            .children
            .iter()
            .zip(symbol_indices(n))
            .find_map(|(c, index)| (index == Some(k)).then_some(c))?;
        match child {
            GreenChild::Token(t) => Some(Path { root: None, segments: vec![t.text.clone()] }),
            GreenChild::Node(m) => Path::of_node(m, cfg),
        }
    }

    /// The path node `m` spells (see the [module docs](self)).
    pub fn of_node(m: &GreenNode, cfg: &BindingConfig) -> Option<Path> {
        if let Some(&(_, _, base, name)) =
            cfg.quals.iter().find(|&&(nt, p, _, _)| nt == m.nt && p == m.prod)
        {
            let mut path = Path::of_child(m, base, cfg)?;
            path.segments.push(name_child(m, name, cfg)?.0);
            return Some(path);
        }
        if let Some(&(_, _, root, first)) =
            cfg.roots.iter().find(|&&(nt, p, _, _)| nt == m.nt && p == m.prod)
        {
            let inner = m.children.iter().find_map(|c| match c {
                GreenChild::Node(child) => Path::of_node(child, cfg)
                    .filter(|p| p.root.is_some() && p.segments.is_empty())
                    .and_then(|p| p.root),
                GreenChild::Token(_) => None,
            });
            let root = root.on(inner)?;
            let segments = match first {
                Some(k) => vec![name_child(m, k, cfg)?.0],
                None => Vec::new(),
            };
            return Some(Path { root: Some(root), segments });
        }
        if let Some(&(_, _, k)) = cfg.passes.iter().find(|&&(nt, p, _)| nt == m.nt && p == m.prod) {
            return Path::of_child(m, k, cfg);
        }
        let mut symbols = m.children.iter().zip(symbol_indices(m)).filter(|(_, i)| i.is_some());
        match (symbols.next(), symbols.next()) {
            (Some((GreenChild::Node(only), _)), None) => Path::of_node(only, cfg),
            _ => None,
        }
    }

    /// `self` after `prefix`, as a use group's tree joins its group's
    /// prefix: a path with no root goes on from the prefix, a bare `self`
    /// is the prefix itself, and any other root cannot follow a prefix.
    pub fn after(self, prefix: &Path) -> Option<Path> {
        match self.root {
            None => {
                let mut joined = prefix.clone();
                joined.segments.extend(self.segments);
                Some(joined)
            }
            Some(Root::Unit(0)) if self.segments.is_empty() => Some(prefix.clone()),
            Some(_) => None,
        }
    }
}

/// Where the reference at byte `offset` of `tree` stands, and the path it
/// spells: the names of the units with a body around it, outermost first
/// (`@unit(name, body)`); its path, after the prefixes of the use groups it
/// is in (`@import_prefix`); and whether it is in an import's path (or a
/// visibility's): such a path names what it imports in every namespace. A
/// qualified name's path is its base's then its name; any other
/// reference's is its name. `None` where no reference's name holds
/// `offset`, or where the path spells none.
pub fn path_at(
    tree: &GreenNode,
    cfg: &BindingConfig,
    offset: u32,
) -> Option<(Vec<String>, Path, bool)> {
    reference_at(tree, 0, cfg, offset, Vec::new(), Prefix::None, false)
}

/// The use group prefix a walk to an offset is under.
#[derive(Clone)]
enum Prefix {
    None,
    Path(Path),
    /// A prefix that spells no path: nothing under it does.
    Broken,
}

fn reference_at(
    n: &GreenNode,
    base: u32,
    cfg: &BindingConfig,
    offset: u32,
    units: Vec<String>,
    prefix: Prefix,
    imported: bool,
) -> Option<(Vec<String>, Path, bool)> {
    for &(_, _, k, kind) in cfg.refs.iter().filter(|r| r.0 == n.nt && r.1 == n.prod) {
        let Some((name, off)) = name_child(n, k, cfg) else { continue };
        let (start, end) = (base + off, base + off + name.len() as u32);
        if !(start <= offset && offset < end) {
            continue;
        }
        let qualified = cfg.quals.iter().find(|q| q.0 == n.nt && q.1 == n.prod && q.3 == k);
        let own = match (kind, qualified) {
            (RefKind::Qualified, Some(&(_, _, qbase, _))) => {
                let mut path = Path::of_child(n, qbase, cfg)?;
                path.segments.push(name);
                path
            }
            // A root alone (`Self`, `self`) is that root's path.
            _ => match &n.children.iter().zip(symbol_indices(n)).find(|(_, i)| *i == Some(k)) {
                Some((GreenChild::Node(_), _)) => Path::of_child(n, k, cfg)
                    .filter(|p| p.root.is_some() && p.segments.is_empty())
                    .unwrap_or(Path { root: None, segments: vec![name] }),
                _ => Path { root: None, segments: vec![name] },
            },
        };
        let path = match prefix {
            Prefix::None => own,
            Prefix::Path(prefix) => own.after(&prefix)?,
            Prefix::Broken => return None,
        };
        return Some((units, path, imported));
    }
    let mut off = base;
    for (c, index) in n.children.iter().zip(symbol_indices(n)) {
        let (start, end) = (off, off + c.width());
        off = end;
        let GreenChild::Node(m) = c else { continue };
        if !(start <= offset && offset < end) {
            continue;
        }
        let mut units = units;
        let mut prefix = prefix;
        let mut imported = imported;
        if let Some(k) = index {
            let is = |at: Option<usize>| at == Some(k);
            let (nt, prod) = (n.nt, n.prod);
            imported |= cfg.imports.iter().any(|i| i.0 == nt && i.1 == prod && is(i.2))
                || cfg.prefixes.iter().any(|p| p.0 == nt && p.1 == prod && is(p.2))
                || cfg.vis_levels.iter().any(|v| v.0 == nt && v.1 == prod && is(v.3));
            let unit = cfg.units.iter().find(|u| u.0 == n.nt && u.1 == n.prod && u.3 == Some(k));
            if let Some(&(_, _, name, _)) = unit {
                if let Some((name, _)) = name_child(n, name, cfg) {
                    units.push(name);
                }
            }
            let group = cfg.prefixes.iter().find(|p| p.0 == n.nt && p.1 == n.prod && p.3 == k);
            if let Some(&(_, _, path, _)) = group {
                let own = match path {
                    Some(at) => Path::of_child(n, at, cfg),
                    None => Path::of_node(n, cfg),
                };
                prefix = match (prefix, own) {
                    (Prefix::Broken, _) | (_, None) => Prefix::Broken,
                    (Prefix::None, Some(own)) => Prefix::Path(own),
                    (Prefix::Path(outer), Some(own)) => {
                        own.after(&outer).map_or(Prefix::Broken, Prefix::Path)
                    }
                };
            }
        }
        return reference_at(m, start, cfg, offset, units, prefix, imported);
    }
    None
}
