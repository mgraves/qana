//! A file's references to names, for finding a declaration's usages: where
//! each is, the path it spells from where it stands, and what it binds to
//! within the file. And a file's escaping references, for telling which
//! of them name nothing.
//!
//! A project index keeps no references (a summary holds a file's words, not
//! where they are used): a search reads them again, from the files whose
//! words hold the names, with [`references`]. Deciding which name a
//! declaration's usage is takes the whole project, and is the searcher's:
//! a reference bound within its file to a local (a definition the file
//! does not offer, and no import's) is that local's; any other is what its
//! path resolves to across the project. A textual name's (a macro's) is
//! what textual scope finds, which each says it sees
//! ([`Reference::textual`]).

use std::collections::{HashMap, HashSet};

use qana_lang::EmbeddedLang;
use qana_sem::{SemDb, Target};
use serde::{Deserialize, Serialize};

use crate::members::SelfPaths;
use crate::textual::{is_textual, replay};
use crate::{outside_bodies, units, LineStarts, Path, Seen, URI};

/// A reference to one of the names looked for. See the [module
/// docs](self).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reference {
    /// Its name's byte span.
    pub span: (u32, u32),
    /// The line and character column its name starts at, from 0.
    pub line: u32,
    pub column: u32,
    /// Its line's text, without the line break.
    pub line_text: String,
    /// The names of the units with a body around it, outermost first.
    pub units: Vec<String>,
    /// Its path from where it stands (its qualifiers', then its name, after
    /// its use group's prefix); `None` where its qualifiers spell none.
    pub path: Option<Path>,
    /// The binding's namespace it looks in; empty for the default one.
    pub namespace: String,
    /// Whether it is in an import's path (or a visibility's, which is said
    /// in full as an import's is).
    pub in_import: bool,
    /// The byte span of the definition it binds to within the file, if it
    /// binds within the file.
    pub binds: Option<(u32, u32)>,
    /// Whether that definition is a local: one the file does not offer as a
    /// declaration, and no import's.
    pub local: bool,
    /// What it sees by textual scope, where its namespace is one textual
    /// definitions are in (Rust's macros: [`is_textual`](crate::is_textual)),
    /// as [`textual_at`](crate::textual_at) says: what it names is then
    /// what [`Resolver::resolve_macro`](crate::Resolver::resolve_macro)
    /// finds, not what it binds to (the file's binding reads a textual
    /// definition as seen from anywhere in its scope).
    pub textual: Option<Vec<Seen>>,
}

/// The references in `text`, a file of `lang`'s language, to any of
/// `names`, in the text's order. A member access's name is none: what it
/// names is what its value's type has. A path that begins with `Self` is
/// said from the type there (`Self::new` in `impl Grid` is `Grid::new`).
pub fn references(lang: &EmbeddedLang, text: &str, names: &[String]) -> Vec<Reference> {
    let session = lang.session(text);
    let tree = session.tree().expect("parsing is total").clone();
    let mut db = SemDb::new(lang.binding.clone());
    db.set_types(lang.types.clone());
    db.set_macro_bodies(&lang.macros);
    db.set_tree(URI, tree.clone());
    let symbols = db.symbols(URI);
    let resolutions = db.resolve(URI);
    let read = units::read(&tree, &lang.binding, &lang.outline);

    let wanted: HashSet<&str> = names.iter().map(String::as_str).collect();
    let defined: HashMap<(u32, u32), ()> = symbols.defs.iter().map(|d| (d.span, ())).collect();
    let declared: HashSet<(u32, u32)> = outside_bodies(&tree, &lang.outline)
        .into_iter()
        .map(|(symbol, _)| symbol.selection)
        .filter(|span| defined.contains_key(span))
        .collect();
    let lines = LineStarts::of(text);
    // Made when a path that begins with `Self` is met: it says the path
    // from the type there.
    let mut self_paths: Option<SelfPaths> = None;

    let mut found: Vec<Reference> = Vec::new();
    for (r, target) in symbols.refs.iter().zip(resolutions.iter()) {
        if !wanted.contains(r.name.as_str()) {
            continue;
        }
        // A member access's name: what it names, its value's type says
        // ([`members`](crate::members)).
        if read.refs.get(&r.span).is_some_and(|place| place.member) {
            continue;
        }
        let binds = match target {
            Target::Local { def } => symbols.defs.get(*def).map(|d| d.span),
            Target::Foreign { .. } | Target::Escapes | Target::Unresolved => None,
        };
        let local = binds
            .is_some_and(|span| !declared.contains(&span) && !read.import_defs.contains(&span));
        let (units, path) = match qana_sem::path_at(&tree, &lang.binding, r.span.0) {
            Some((units, path, _)) if path.root == Some(qana_sem::Root::SelfType) => {
                let reading = self_paths
                    .get_or_insert_with(|| SelfPaths::new(lang, &tree, &symbols, &resolutions));
                (units, reading.path(&tree, r.span.0, &path))
            }
            Some((units, path, _)) => (units, Some(path)),
            None => (Vec::new(), None),
        };
        let (line, column) = lines.position(text, r.span.0);
        found.push(Reference {
            span: r.span,
            line,
            column,
            line_text: lines.text(text, line).to_string(),
            units,
            path,
            namespace: r.ns.clone(),
            in_import: read.refs.get(&r.span).is_some_and(|place| place.imported),
            binds,
            local,
            textual: is_textual(lang, &r.ns).then(|| replay(&read.textual, r.span.0)),
        });
    }
    found.sort_by_key(|r| r.span);
    found
}

/// The references in `text`, a file of `lang`'s language, that escape it
/// (none of its definitions binds them), in the text's order: each one a
/// project index may resolve, and find names nothing. A member access's
/// name is none (a member is never said to name nothing). Left out are those
/// an expansion made (what the file's own macros expand to), and those in
/// a scope where an invocation whose expansion may declare names stands
/// (`@expands`: a block holding a macro invocation), or in a scope inside
/// one, which may name what it declares. One among a unit's items opens
/// the unit, as its summary says ([`Unit::open`](crate::Unit::open)): a
/// resolver knows that.
pub fn escaping(lang: &EmbeddedLang, text: &str) -> Vec<Reference> {
    let session = lang.session(text);
    let tree = session.tree().expect("parsing is total").clone();
    let mut db = SemDb::new(lang.binding.clone());
    db.set_types(lang.types.clone());
    db.set_macro_bodies(&lang.macros);
    db.set_tree(URI, tree.clone());
    let symbols = db.symbols(URI);
    let resolutions = db.resolve(URI);
    let read = units::read(&tree, &lang.binding, &lang.outline);
    let lines = LineStarts::of(text);

    let mut found: Vec<Reference> = Vec::new();
    for (r, target) in symbols.refs.iter().zip(resolutions.iter()) {
        if *target != Target::Escapes {
            continue;
        }
        let Some(place) = read.refs.get(&r.span) else {
            continue;
        };
        // A member access's name, and a path from `Self`, name what where
        // they stand says; one in a scope an expansion may declare in may
        // name what it declares.
        let from_self =
            place.path.as_ref().is_some_and(|p| p.root == Some(qana_sem::Root::SelfType));
        if place.member
            || from_self
            || place.scopes.iter().any(|scope| read.expanding.contains(scope))
        {
            continue;
        }
        let (units, path) = match qana_sem::path_at(&tree, &lang.binding, r.span.0) {
            Some((units, path, _)) => (units, Some(path)),
            None => (Vec::new(), None),
        };
        let (line, column) = lines.position(text, r.span.0);
        found.push(Reference {
            span: r.span,
            line,
            column,
            line_text: lines.text(text, line).to_string(),
            units,
            path,
            namespace: r.ns.clone(),
            in_import: place.imported,
            binds: None,
            local: false,
            textual: is_textual(lang, &r.ns).then(|| replay(&read.textual, r.span.0)),
        });
    }
    found.sort_by_key(|r| r.span);
    found
}
