//! A file's references to names, for finding a declaration's usages: where
//! each is, the path it spells from where it stands, and what it binds to
//! within the file.
//!
//! A project index keeps no references (a summary holds a file's words, not
//! where they are used): a search reads them again, from the files whose
//! words hold the names, with [`references`]. Deciding which name a
//! declaration's usage is takes the whole project, and is the searcher's:
//! a reference bound within its file to a local (a definition the file
//! does not offer, and no import's) is that local's; any other is what its
//! path resolves to across the project.

use std::collections::{HashMap, HashSet};

use qana_lang::EmbeddedLang;
use qana_sem::{SemDb, Target};
use serde::{Deserialize, Serialize};

use crate::{outside_bodies, units, LineStarts, Path, URI};

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
}

/// The references in `text`, a file of `lang`'s language, to any of
/// `names`, in the text's order.
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

    let mut found: Vec<Reference> = Vec::new();
    for (r, target) in symbols.refs.iter().zip(resolutions.iter()) {
        if !wanted.contains(r.name.as_str()) {
            continue;
        }
        let binds = match target {
            Target::Local { def } => symbols.defs.get(*def).map(|d| d.span),
            Target::Foreign { .. } | Target::Escapes | Target::Unresolved => None,
        };
        let local = binds
            .is_some_and(|span| !declared.contains(&span) && !read.import_defs.contains(&span));
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
            in_import: read.refs.get(&r.span).is_some_and(|place| place.imported),
            binds,
            local,
        });
    }
    found.sort_by_key(|r| r.span);
    found
}
