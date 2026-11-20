//! What a project index keeps about one file of a `.qana`-defined
//! language: its [`Summary`].
//!
//! A summary is computed from the file's text and its language's
//! certified grammar, and from nothing else: no other file, no path, no
//! build system. So the same text is summarized once, wherever it is, and a
//! summary can be kept by its text's hash ([`summary_key`] names the rest of
//! what it depends on).
//!
//! It has three parts, each with a fingerprint of its own, so that a change
//! to a file reaches only as far as the part it changed:
//!
//! - [`Offers`]: what other files may depend on. The file's declarations
//!   that a body does not hold, each by its name, kind, namespace, the
//!   declarations it is inside, an ordinal among its namesakes there, and
//!   its signature. Nothing in it is a position, and it is in a fixed
//!   order, so moving code leaves it alone.
//! - [`Needs`]: what only this file depends on. The names it refers to that
//!   nothing in it defines: they escape to other files.
//! - [`Locations`]: where each offered declaration is, and the words the
//!   file uses. Navigation reads them; nothing is resolved from them.
//!
//! A body edit, most edits, changes only the locations.
//!
//! What the parts hold comes from the grammar: a declaration is a construct
//! its outline lists (`@outline`) whose name is a definition (`@def`), with
//! the outline's kind and the binding's namespace; a construct whose name
//! defines nothing (Rust's `impl` blocks) is a container only. A
//! declaration inside a `function` declaration is a body's, and is left
//! out.

use std::collections::{BTreeSet, HashMap};

use qana_lang::{EmbeddedLang, GrammarArtifact};
use qana_sem::{SemDb, Target};
use serde::{Deserialize, Serialize};

include!(concat!(env!("OUT_DIR"), "/summarizer.rs"));

/// The outline kind of a construct whose declarations are its body's.
const FUNCTION: &str = "function";

/// What a project index keeps about one file. See the [crate docs](crate).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    offers: Offers,
    needs: Needs,
    locations: Locations,
}

impl Summary {
    /// The declarations other files may depend on.
    pub fn offers(&self) -> &Offers {
        &self.offers
    }

    /// The names the file needs from other files.
    pub fn needs(&self) -> &Needs {
        &self.needs
    }

    /// Where the offered declarations are, and the file's words.
    pub fn locations(&self) -> &Locations {
        &self.locations
    }
}

/// The declarations a file offers other files, in a fixed order: by the
/// declarations they are inside, then name, kind and ordinal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Offers {
    declarations: Vec<Declaration>,
    fingerprint: u64,
}

impl Offers {
    /// The declarations, in the offers' order; [`Locations`] follow it.
    pub fn declarations(&self) -> &[Declaration] {
        &self.declarations
    }

    /// A hash of everything the offers hold: equal offers have equal
    /// fingerprints.
    pub fn fingerprint(&self) -> u64 {
        self.fingerprint
    }
}

/// One declaration a file offers, by what it is, not where.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Declaration {
    /// The names of the constructs it is inside, outermost first: its
    /// module, its type (`["Cell"]` for a method in `impl Cell`).
    pub container: Vec<String>,
    /// Its name.
    pub name: String,
    /// Its outline kind (`function`, `struct`, `module`, `class`,
    /// `constant`, `variable`, `field`).
    pub kind: String,
    /// The binding's namespace; empty for the default one.
    pub namespace: String,
    /// Among declarations of the same container, name and kind, its place
    /// in the file: 0 for the first. What tells two `impl` blocks' `new`
    /// apart without a position.
    pub ordinal: u32,
    /// Its head as written: from the start of its construct as the
    /// grammar has it (or of the line its name is on, if that is later)
    /// to the first `{` or `;` outside parentheses and brackets, its
    /// spaces and line breaks each made one space (`fn new(width: f32) ->
    /// Cell`). What the grammar puts outside the construct (Rust's
    /// visibility, before an item) is not in it.
    pub signature: String,
}

/// The names a file refers to that nothing in it defines: they escape to
/// other files. Sorted, each once.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Needs {
    escaping: Vec<EscapingName>,
    fingerprint: u64,
}

impl Needs {
    /// The escaping names, sorted.
    pub fn escaping(&self) -> &[EscapingName] {
        &self.escaping
    }

    /// A hash of the escaping names: equal needs have equal fingerprints.
    pub fn fingerprint(&self) -> u64 {
        self.fingerprint
    }
}

/// A name a file refers to and does not define.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EscapingName {
    /// The binding's namespace the reference looks in; empty for the
    /// default one.
    pub namespace: String,
    /// The name.
    pub name: String,
}

/// Where each offered declaration is, in the offers' order, and the words
/// the file uses. Byte offsets are into the file's text; lines and columns
/// count from 0, columns in characters.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Locations {
    names: Vec<(u32, u32)>,
    items: Vec<(u32, u32)>,
    positions: Vec<(u32, u32)>,
    words: Vec<String>,
    fingerprint: u64,
}

impl Locations {
    /// The byte span of the `i`th offered declaration's name.
    pub fn name_span(&self, i: usize) -> (u32, u32) {
        self.names[i]
    }

    /// The byte span of the `i`th offered declaration's whole construct.
    pub fn item_span(&self, i: usize) -> (u32, u32) {
        self.items[i]
    }

    /// The line and character column the `i`th offered declaration's name
    /// starts at.
    pub fn position(&self, i: usize) -> (u32, u32) {
        self.positions[i]
    }

    /// The names the file defines or refers to, sorted, each once: where a
    /// search for a name's uses looks first.
    pub fn words(&self) -> &[String] {
        &self.words
    }

    /// A hash of the locations and words.
    pub fn fingerprint(&self) -> u64 {
        self.fingerprint
    }
}

/// The key to keep the summary of a file under, given its grammar's source,
/// with the file's text's own hash beside it: it changes with the grammar's
/// text, with the code that certifies and parses it
/// ([`GrammarArtifact::key`]), and with the code that summarizes.
pub fn summary_key(grammar_source: &str) -> u64 {
    let mut hash = Fnv::new();
    hash.u64(SUMMARIZER);
    hash.u64(GrammarArtifact::key(grammar_source));
    hash.finish()
}

/// The name the file is known by within the summarizer's own database.
const URI: &str = "file";

/// Summarize `text`, a file of `lang`'s language.
pub fn summarize(lang: &EmbeddedLang, text: &str) -> Summary {
    let session = lang.session(text);
    let tree = session.tree().expect("parsing is total").clone();
    let mut db = SemDb::new(lang.binding.clone());
    db.set_types(lang.types.clone());
    db.set_macro_bodies(&lang.macros);
    db.set_tree(URI, tree.clone());
    let symbols = db.symbols(URI);
    let resolutions = db.resolve(URI);

    // A definition's namespace, by its name's span.
    let namespaces: HashMap<(u32, u32), &str> =
        symbols.defs.iter().map(|d| (d.span, d.ns.as_str())).collect();
    let lines = LineStarts::of(text);

    // The outline in document order; containment is nesting.
    struct Found {
        declaration: Declaration,
        name: (u32, u32),
        item: (u32, u32),
    }
    let mut found: Vec<Found> = Vec::new();
    // The constructs the walk is inside: their end, their name, whether
    // they are a function's (a body's).
    let mut inside: Vec<(u32, String, bool)> = Vec::new();
    for symbol in qana_services::outline(&tree, &lang.outline) {
        while inside.last().is_some_and(|&(end, _, _)| end <= symbol.span.0) {
            inside.pop();
        }
        let in_body = inside.iter().any(|(_, _, function)| *function);
        let defines = namespaces.get(&symbol.selection);
        if let (false, Some(namespace)) = (in_body, defines) {
            found.push(Found {
                declaration: Declaration {
                    container: inside.iter().map(|(_, name, _)| name.clone()).collect(),
                    name: symbol.name.clone(),
                    kind: symbol.kind.to_string(),
                    namespace: namespace.to_string(),
                    ordinal: 0,
                    signature: signature(text, symbol.span, symbol.selection),
                },
                name: symbol.selection,
                item: symbol.span,
            });
        }
        inside.push((symbol.span.1, symbol.name, symbol.kind == FUNCTION));
    }

    // Ordinals among namesakes, in document order.
    let mut seen: HashMap<(Vec<String>, String, String), u32> = HashMap::new();
    for f in &mut found {
        let d = &f.declaration;
        let count = seen.entry((d.container.clone(), d.name.clone(), d.kind.clone())).or_insert(0);
        f.declaration.ordinal = *count;
        *count += 1;
    }
    found.sort_by(|a, b| a.declaration.cmp(&b.declaration));

    let escaping: Vec<EscapingName> = symbols
        .refs
        .iter()
        .zip(resolutions.iter())
        .filter(|(_, target)| **target == Target::Escapes)
        .map(|(r, _)| EscapingName { namespace: r.ns.clone(), name: r.name.clone() })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let words: Vec<String> = symbols
        .defs
        .iter()
        .map(|d| d.name.clone())
        .chain(symbols.refs.iter().map(|r| r.name.clone()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    let declarations: Vec<Declaration> = found.iter().map(|f| f.declaration.clone()).collect();
    let names: Vec<(u32, u32)> = found.iter().map(|f| f.name).collect();
    let items: Vec<(u32, u32)> = found.iter().map(|f| f.item).collect();
    let positions: Vec<(u32, u32)> =
        names.iter().map(|&(start, _)| lines.position(text, start)).collect();

    let offers_fingerprint = {
        let mut hash = Fnv::new();
        for d in &declarations {
            hash.u64(d.container.len() as u64);
            for c in &d.container {
                hash.str(c);
            }
            hash.str(&d.name);
            hash.str(&d.kind);
            hash.str(&d.namespace);
            hash.u64(u64::from(d.ordinal));
            hash.str(&d.signature);
        }
        hash.finish()
    };
    let needs_fingerprint = {
        let mut hash = Fnv::new();
        for e in &escaping {
            hash.str(&e.namespace);
            hash.str(&e.name);
        }
        hash.finish()
    };
    let locations_fingerprint = {
        let mut hash = Fnv::new();
        for ((name, item), position) in names.iter().zip(&items).zip(&positions) {
            for value in [name.0, name.1, item.0, item.1, position.0, position.1] {
                hash.u64(u64::from(value));
            }
        }
        for w in &words {
            hash.str(w);
        }
        hash.finish()
    };

    Summary {
        offers: Offers { declarations, fingerprint: offers_fingerprint },
        needs: Needs { escaping, fingerprint: needs_fingerprint },
        locations: Locations { names, items, positions, words, fingerprint: locations_fingerprint },
    }
}

/// A declaration's head (see [`Declaration::signature`]): `item` is the
/// construct's span and `name` its name's.
fn signature(text: &str, item: (u32, u32), name: (u32, u32)) -> String {
    let end = (item.1 as usize).min(text.len());
    let start = (item.0 as usize).min(end);
    let name_start = (name.0 as usize).clamp(start, end);
    let name_end = (name.1 as usize).clamp(name_start, end);
    let from = text[..name_start].rfind('\n').map_or(0, |i| i + 1).max(start);
    let mut depth = 0i32;
    let mut to = end;
    for (i, ch) in text[name_end..end].char_indices() {
        match ch {
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            '{' | ';' if depth <= 0 => {
                to = name_end + i;
                break;
            }
            _ => {}
        }
    }
    text[from..to].split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Where each line of a text starts.
struct LineStarts(Vec<usize>);

impl LineStarts {
    fn of(text: &str) -> Self {
        LineStarts(std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).collect())
    }

    /// The line and character column of byte `at` of `text`.
    fn position(&self, text: &str, at: u32) -> (u32, u32) {
        let at = (at as usize).min(text.len());
        let line = self.0.partition_point(|&start| start <= at) - 1;
        let column = text[self.0[line]..at].chars().count();
        (line as u32, column as u32)
    }
}

/// 64-bit FNV-1a, over values written with their lengths, so that no two
/// sequences of values hash as one.
struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }

    fn bytes(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 ^= u64::from(byte);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    fn u64(&mut self, value: u64) {
        self.bytes(&value.to_le_bytes());
    }

    fn str(&mut self, value: &str) {
        self.u64(value.len() as u64);
        self.bytes(value.as_bytes());
    }

    fn finish(&self) -> u64 {
        self.0
    }
}
