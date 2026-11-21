//! Textual scope, rustc's for `macro_rules!` (`@def(x, textual)`): a
//! name sees the textual definitions before it, the latest of its name
//! first; in its scope and in the scopes and modules it stands in, whatever
//! barriers stand between; one inside a block not after the block, one
//! inside a module after it only where the module's attributes carry it
//! out (Rust's `#[macro_use]`, the model's to say:
//! [`CrateInfo::macro_use`](crate::CrateInfo::macro_use)).
//!
//! A file's walk logs its textual definitions, its modules and its scopes
//! in order ([`TextualEvent`]); replayed, the log gives each module's own
//! textual links for its summary ([`TextualLink`]), and what a name at a
//! point of the file sees ([`textual_at`]).

use std::collections::HashMap;

use qana_lang::live::LiveDoc;
use qana_lang::EmbeddedLang;
use serde::{Deserialize, Serialize};

use crate::units::{self, Span, TextualEvent};
use crate::Attribute;

/// A link of a module's textual scope, as its summary keeps it: what is
/// seen after it, in the module's order.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum TextualLink {
    /// A textual definition: its declaration (an index into the offers'
    /// declarations), and the attributes said of it (`#[macro_export]`).
    Def { declaration: u32, attributes: Vec<Attribute> },
    /// A child module (an index into the offers' units): what its own
    /// links are seen after it where its attributes carry them out.
    Module(u32),
    /// An invocation (an index into the offers' invocations): what its
    /// macro is, the links before it say; what its expansion defines is
    /// seen after it.
    Invocation(u32),
}

/// What a name at a point of a file may see by textual scope, from the
/// file's start (the nearest last): see [`textual_at`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Seen {
    /// A textual definition in the file: its name, the name's span, and
    /// the attributes said of it (a `#[cfg]` it may not be under).
    Def { name: String, span: (u32, u32), attributes: Vec<Attribute> },
    /// An inline module that ends before the point: what it saw inside,
    /// seen here where its attributes carry it out.
    Module { attributes: Vec<Attribute>, seen: Vec<Seen> },
    /// A module whose body is another file, declared before the point: the
    /// units its name is under from the file's own (outermost first, its
    /// own last), and the attributes said of it. Its file's own links are
    /// seen here where its attributes carry them out.
    File { units: Vec<String>, attributes: Vec<Attribute> },
}

/// Each module's own textual links, module-level only (what a block holds
/// is no summary's): the file's own (`None`) and each unit's, by its walk
/// index; a definition by the declaration its name's span is.
pub(crate) fn own_links(
    events: &[TextualEvent],
    declarations: &HashMap<Span, u32>,
) -> HashMap<Option<u32>, Vec<UnitLink>> {
    let mut out: HashMap<Option<u32>, Vec<UnitLink>> = HashMap::new();
    // The modules and scopes open: a module by its unit (where a summary
    // reads it), a scope as `None`.
    let mut stack: Vec<Option<Option<u32>>> = vec![Some(None)];
    let module_level = |stack: &[Option<Option<u32>>]| -> Option<Option<u32>> {
        if stack.iter().any(Option::is_none) {
            return None;
        }
        stack.last().copied().flatten()
    };
    for event in events {
        match event {
            TextualEvent::Def { span, attributes, .. } => {
                if let (Some(unit), Some(&declaration)) =
                    (module_level(&stack), declarations.get(span))
                {
                    out.entry(unit)
                        .or_default()
                        .push(UnitLink::Def { declaration, attributes: attributes.clone() });
                }
            }
            TextualEvent::File { unit: Some(child), .. } => {
                if let Some(unit) = module_level(&stack) {
                    out.entry(unit).or_default().push(UnitLink::Module(*child));
                }
            }
            TextualEvent::File { unit: None, .. } => {}
            // Among a module's items or an impl block's: in the module's
            // order, wherever the scopes between are.
            TextualEvent::Invocation { index, .. } => {
                if let Some(Some(unit)) = stack.iter().rev().find(|f| f.is_some()) {
                    out.entry(*unit).or_default().push(UnitLink::Invocation(*index));
                }
            }
            TextualEvent::Open { module: Some(module), .. } => {
                let at = module_level(&stack);
                match (at, module.unit) {
                    (Some(unit), Some(child)) => {
                        out.entry(unit).or_default().push(UnitLink::Module(child));
                        stack.push(Some(Some(child)));
                    }
                    // A body's module: what it holds is no summary's.
                    _ => stack.push(None),
                }
            }
            TextualEvent::Open { module: None, .. } => stack.push(None),
            TextualEvent::Close { .. } => {
                stack.pop();
            }
        }
    }
    out
}

/// A module's own textual link before units are placed: a definition, or a
/// child unit by its walk index.
#[derive(Clone, Debug)]
pub(crate) enum UnitLink {
    Def { declaration: u32, attributes: Vec<Attribute> },
    Module(u32),
    Invocation(u32),
}

/// What a name at byte `offset` of `doc` sees by textual scope: the
/// textual definitions before it in the file, from the file's start (the
/// nearest last), those in a module that ends before it under the module
/// ([`Seen::Module`]), those in a block that ends before it left out, and
/// each module before it whose body is another file ([`Seen::File`]). What
/// a file's start sees is what its module's declaration sees, in its
/// parent: [`Resolver::resolve_macro`](crate::Resolver::resolve_macro)
/// goes on from there. `None` for a name in a namespace no textual
/// definition is in ([`is_textual`]), and where the document has no tree.
pub fn textual_at(doc: &mut LiveDoc, offset: u32, namespace: &str) -> Option<Vec<Seen>> {
    let (lang, tree) = doc.tree()?;
    if !is_textual(lang, namespace) {
        return None;
    }
    let read = units::read(tree, &lang.binding, &lang.outline);
    Some(replay(&read.textual, offset))
}

/// The textual log replayed up to byte `offset`.
pub(crate) fn replay(events: &[TextualEvent], offset: u32) -> Vec<Seen> {
    // The frames open: what each saw so far, and the module it is (with
    // its name and attributes), or `None` for a scope.
    struct Frame {
        seen: Vec<Seen>,
        module: Option<(String, Vec<Attribute>)>,
    }
    let mut stack: Vec<Frame> = vec![Frame { seen: Vec::new(), module: None }];
    let units_here = |stack: &[Frame]| -> Vec<String> {
        stack.iter().filter_map(|f| f.module.as_ref().map(|(name, _)| name.clone())).collect()
    };
    for event in events {
        let at = match event {
            TextualEvent::Def { span, .. } => span.0,
            TextualEvent::File { at, .. } => *at,
            TextualEvent::Invocation { at, .. } => *at,
            TextualEvent::Open { span, .. } => span.0,
            TextualEvent::Close { end } => *end,
        };
        if at >= offset {
            break;
        }
        match event {
            TextualEvent::Def { name, span, attributes } => {
                let top = stack.last_mut().expect("the file's frame");
                top.seen.push(Seen::Def {
                    name: name.clone(),
                    span: *span,
                    attributes: attributes.clone(),
                });
            }
            TextualEvent::File { name, attributes, .. } => {
                let mut units = units_here(&stack);
                units.push(name.clone());
                let top = stack.last_mut().expect("the file's frame");
                top.seen.push(Seen::File { units, attributes: attributes.clone() });
            }
            TextualEvent::Open { module, .. } => stack.push(Frame {
                seen: Vec::new(),
                module: module.as_ref().map(|m| (m.name.clone(), m.attributes.clone())),
            }),
            // What an expansion defines, a live document does not see.
            TextualEvent::Invocation { .. } => {}
            TextualEvent::Close { .. } => {
                let frame = stack.pop().expect("an open frame");
                if let Some((_, attributes)) = frame.module {
                    let top = stack.last_mut().expect("the file's frame");
                    top.seen.push(Seen::Module { attributes, seen: frame.seen });
                }
            }
        }
    }
    // The frames still open hold the offset: what each saw is seen.
    stack.into_iter().flat_map(|f| f.seen).collect()
}

/// The nearest textual definition named `name` that `seen` holds in its
/// file itself, outside the modules there (what a module's attributes
/// carry out of it, and which attributes make a definition conditional,
/// are the model's to say): the name's span. What a name sees where no
/// project index places its file.
pub fn seen_here(seen: &[Seen], name: &str) -> Option<(u32, u32)> {
    seen.iter().rev().find_map(|s| match s {
        Seen::Def { name: n, span, .. } if n == name => Some(*span),
        _ => None,
    })
}

/// Whether `namespace` is one the grammar's textual definitions are in
/// (Rust's `macro`): a name in it is looked for by textual scope first.
pub fn is_textual(lang: &EmbeddedLang, namespace: &str) -> bool {
    let b = &lang.binding;
    b.namespaces.iter().any(|(nt, prod, ns)| ns == namespace && b.textual.contains(&(*nt, *prod)))
}
