//! A file read as expanded (for Synkro's project index, phase 7d): every
//! invocation it writes replaced by its expansion, so that its readers
//! ([`references`](crate::references), [`members`](crate::members),
//! [`escaping`](crate::escaping), the editor's queries) read the names an
//! invocation's arguments hold where its expansion puts them, as rustc
//! reads the crate once its macros are expanded.
//!
//! - [`invocations`] says a file's invocations, wherever they stand, with
//!   what resolving each one's macro needs.
//! - [`expand_text`] expands one to text, the invocations it holds whose
//!   macros the job has expanded in their place, each token said as where
//!   the invocation writes it, or as written by a macro.
//! - [`expanded_file`] puts those in the file's text, an expression's in
//!   parentheses, and keeps `macro_rules!` hygiene: the locals a macro
//!   writes are its own. A name the invocation wrote never binds to one,
//!   nor a name a macro wrote to the call site's: such a name takes a name
//!   no source uses.
//! - An invocation inside another's arguments (`vec![a]` in `assert_eq!`)
//!   is no invocation of the file's text: once its outer one is expanded,
//!   the file read as expanded says it ([`ExpandedFile::inner`]), placed in
//!   the file, and given its expansion too, [`expanded_file`] puts that
//!   where the outer expansion copies it, each time it does.
//!
//! [`ExpandedFile`] maps each place of its text back to the file: where
//! the file writes it, else the call (the macro path) of the innermost
//! invocation whose expansion wrote it.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use qana_lang::EmbeddedLang;
use qana_mbe::{MacroRules, Nested, Outcome, Parsed, Token, TokenKind, TokenStream};
use qana_sem::{SemDb, Target};
use serde::{Deserialize, Serialize};

use crate::expand::{rules_of, BuiltinMacro, ExpandRequest, MacroSource, MacroTable};
use crate::textual::{is_textual, replay};
use crate::{outside_bodies, units, LineStarts, MemberAccess, Path, Reference, Root, Seen, URI};

/// An invocation a file writes, wherever it stands (among items, as a
/// statement, an expression, a type or a pattern), or inside another's
/// arguments ([`ExpandedFile::inner`]): what resolving its macro needs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvocationAt {
    /// Its byte span, from its macro's path to its end.
    pub span: (u32, u32),
    /// Its macro's path's byte span: the call.
    pub path_span: Option<(u32, u32)>,
    /// Its macro's path, and the namespace it names a macro in.
    pub path: Option<Path>,
    pub namespace: String,
    /// The grammar's rule its expansion is parsed at.
    pub entry: String,
    /// The names of the units with a body around it, outermost first.
    pub units: Vec<String>,
    /// What its path sees by textual scope, where its namespace is one
    /// textual definitions are in ([`textual_at`](crate::textual_at)).
    pub seen: Option<Vec<Seen>>,
    /// Whether it stands among items, as one a summary records.
    pub among_items: bool,
}

/// The invocations `text`, a file of `lang`'s language, writes, in the
/// text's order. See the [module docs](self).
pub fn invocations(lang: &EmbeddedLang, text: &str) -> Vec<InvocationAt> {
    let session = lang.session(text);
    let tree = session.tree().expect("parsing is total").clone();
    let read = units::read(&tree, &lang.binding, &lang.outline);
    read.all_invocations
        .iter()
        .map(|inv| {
            let units = inv
                .path_span
                .and_then(|(_, end)| qana_sem::path_at(&tree, &lang.binding, end.saturating_sub(1)))
                .map(|(units, _, _)| units)
                .unwrap_or_default();
            let at = inv.path_span.map_or(inv.span.0, |p| p.0);
            InvocationAt {
                span: inv.span,
                path_span: inv.path_span,
                path: inv.path.clone(),
                namespace: inv.namespace.clone(),
                entry: lang.sg.nt_names[inv.entry as usize].clone(),
                units,
                seen: is_textual(lang, &inv.namespace).then(|| replay(&read.textual, at)),
                among_items: inv.among_items,
            }
        })
        .collect()
}

/// An invocation's expansion as text, with the invocations in it whose
/// macros its job had expanded in their place: see the [module
/// docs](self).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpandedText {
    text: String,
    /// Each token of `text`: its span there; where the site writes it (a
    /// span of the request's `site`), or `None` where a macro writes it;
    /// and the expansion that wrote it (its mark: 0 for the site's).
    tokens: Vec<TextToken>,
    /// The paths of the invocations in it that it holds unexpanded, each
    /// once: those no macro the job was given names.
    #[serde(default)]
    wanted: Vec<Path>,
}

/// A token of an [`ExpandedText`]: its span in the text; where the site
/// writes it, or `None` where a macro writes it; and its mark.
type TextToken = ((u32, u32), Option<(u32, u32)>, u32);

/// A token of an expanded file's part: its span in the text; the file's
/// span where the file writes it; and the call of the expansion it stands
/// in.
type PartToken = ((u32, u32), Option<(u32, u32)>, (u32, u32));

/// The names hygiene gives tokens a macro wrote: by the expansion put in
/// (in the order they are) and the token's span in that expansion's own
/// text.
type Renames = HashMap<(usize, (u32, u32)), String>;

impl ExpandedText {
    /// The text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The paths of the invocations its expansions write that it holds
    /// unexpanded, each once, as its job was given nothing they name: a
    /// `$crate` path rooted at [`Root::MacroCrate`] (the table's crate), a
    /// name alone or another path as written. Given their macros too
    /// ([`ExpandRequest::names`]), the job expands them in their place.
    pub fn wanted(&self) -> &[Path] {
        &self.wanted
    }
}

/// The path `tokens` write (an invocation's, before its `!`), as a summary
/// says one: `$crate` rooted at the macro's crate, a leading `::` at the
/// global root, `crate`, `self` and `super`s at theirs. `None` for tokens
/// that are no path.
fn path_of(tokens: &[Token]) -> Option<Path> {
    // Names, each after a `::` but the first, which may follow one.
    let mut names: Vec<&Token> = Vec::new();
    let mut global = false;
    let mut want_name = true;
    for (k, t) in tokens.iter().enumerate() {
        if t.kind == TokenKind::Punct && &*t.text == "::" {
            if k == 0 {
                global = true;
            } else if want_name {
                return None;
            }
            want_name = true;
        } else if t.kind == TokenKind::Ident && want_name {
            names.push(t);
            want_name = false;
        } else {
            return None;
        }
    }
    if want_name {
        return None;
    }
    let mut names = names.as_slice();
    let mut root = global.then_some(Root::Global);
    if !global {
        match names.first().map(|t| &*t.text) {
            Some("$crate") => (root, names) = (Some(Root::MacroCrate), &names[1..]),
            Some("crate") => (root, names) = (Some(Root::Crate), &names[1..]),
            Some("self") => (root, names) = (Some(Root::Unit(0)), &names[1..]),
            Some("super") => {
                let up = names.iter().take_while(|t| &*t.text == "super").count();
                (root, names) = (Some(Root::Unit(up as u32)), &names[up..]);
            }
            _ => {}
        }
    }
    let segments: Vec<String> =
        names.iter().map(|t| t.text.strip_prefix("r#").unwrap_or(&t.text).to_string()).collect();
    (!segments.is_empty()).then_some(Path { root, segments })
}

/// Whether an expansion read at `entry` stands in its place in
/// parentheses: an expression's, a pattern's or a type's. Statements and
/// items stand as they are.
fn parenthesized(entry: &str) -> bool {
    !matches!(entry, "block_stmts" | "items_opt" | "assoc_items_opt" | "foreign_items_opt")
}

/// Expand what `request` asks, in `lang`, to text, and with it each
/// invocation it holds by a path from the macro's crate's root that
/// `table` holds, or by the request's invocation's own name (taking each
/// one's first macro), in turn, to the request's limit: see the [module
/// docs](self). The request's `mark`, `local_inner` and `of_crate` are
/// not read: the invocation is a file's, and the job's macros are all the
/// table's crate's.
pub fn expand_text(
    lang: &EmbeddedLang,
    request: &ExpandRequest,
    table: &MacroTable,
) -> Result<ExpandedText, String> {
    let site =
        TokenStream::lex(lang, &request.site, 0, 0, request.site_edition).map_err(|e| e.message)?;
    let input = match &request.input {
        Some(input) => input.clone(),
        None => site.invocation().ok_or("the site is no invocation")?.1,
    };
    // The table's paths, then the request's names: each path's macros,
    // the first of them read once.
    let mut by_path: HashMap<&Path, usize> = HashMap::new();
    let known: Vec<&(Path, Vec<MacroSource>)> = table.iter().chain(&request.names).collect();
    for (k, (path, _)) in known.iter().enumerate() {
        by_path.entry(path).or_insert(k);
    }
    let itself = request.itself.as_ref();
    let mut read: HashMap<usize, Option<Arc<MacroRules>>> = HashMap::new();
    let mut rules_once = |which: usize, source: &MacroSource| -> Option<Arc<MacroRules>> {
        read.entry(which).or_insert_with(|| rules_of(lang, source).ok()).clone()
    };
    let mut resolve = |tokens: &[Token]| -> Option<Arc<MacroRules>> {
        let path = path_of(tokens)?;
        // The invocation's own name alone names its macro, whoever wrote it.
        if let (None, [name], Some((own, macros))) = (path.root, path.segments.as_slice(), itself) {
            if name == own {
                return rules_once(usize::MAX, macros.first()?);
            }
        }
        let &k = by_path.get(&path)?;
        rules_once(k, known[k].1.first()?)
    };
    let limit = request.limit as usize;
    let mut out = ExpandedText { text: String::new(), tokens: Vec::new(), wanted: Vec::new() };
    match &request.definition {
        MacroSource::Text { .. }
        | MacroSource::Tokens { .. }
        | MacroSource::Builtin {
            macro_: BuiltinMacro::FormatArgs | BuiltinMacro::StandIn(_),
            ..
        } => {
            let rules = rules_of(lang, &request.definition)?;
            let expansion = qana_mbe::expand_nested(
                lang,
                &rules,
                &input,
                &request.entry,
                1,
                limit,
                &mut resolve,
            )
            .map_err(|e| e.message)?;
            render(&expansion.parsed, &expansion.nested, &mut out);
        }
        MacroSource::Builtin { macro_: BuiltinMacro::CfgSelect, .. } => {
            let arms = input.cfg_select_arms().map_err(|e| e.message)?;
            let stream = TokenStream::concat(arms.into_iter().map(|(_, body)| body));
            let (parsed, nested) =
                qana_mbe::expand_all(lang, &stream, &request.entry, limit, &mut resolve)
                    .map_err(|e| e.message)?;
            render(&parsed, &nested, &mut out);
        }
    }
    Ok(out)
}

/// `parsed`'s text into `out`, each expanded invocation of `nested` in its
/// place (rendered the same way), the rest as written; the paths of those
/// nothing named, wanted.
fn render(parsed: &Parsed, nested: &[Nested], out: &mut ExpandedText) {
    for n in nested {
        if let (Outcome::Unresolved, Some(path)) = (&n.outcome, path_of(&n.path)) {
            if !out.wanted.contains(&path) {
                out.wanted.push(path);
            }
        }
    }
    let mut expanded: Vec<(&Nested, &qana_mbe::Expansion)> = nested
        .iter()
        .filter_map(|n| match &n.outcome {
            Outcome::Expanded(e) => Some((n, e.as_ref())),
            _ => None,
        })
        .collect();
    expanded.sort_by_key(|(n, _)| n.span.0);
    let mut at = 0u32;
    for (n, e) in expanded {
        copy(parsed, (at, n.span.0), out);
        let wrap = parenthesized(&n.entry);
        if wrap {
            out.text.push('(');
        }
        render(&e.parsed, &e.nested, out);
        if wrap {
            out.text.push(')');
        }
        at = n.span.1;
    }
    copy(parsed, (at, parsed.text.len() as u32), out);
}

/// The text of `parsed` within `span`, and its tokens, into `out`.
fn copy(parsed: &Parsed, span: (u32, u32), out: &mut ExpandedText) {
    let Some(piece) = parsed.text.get(span.0 as usize..span.1 as usize) else { return };
    let base = out.text.len() as u32;
    out.text.push_str(piece);
    for s in &parsed.tokens {
        if s.span.0 >= span.0 && s.span.1 <= span.1 && s.span.1 > s.span.0 {
            let o = s.token.origin;
            let written = (o.source == 0).then_some((o.start, o.end));
            let at = (base + s.span.0 - span.0, base + s.span.1 - span.0);
            out.tokens.push((at, written, s.token.mark));
        }
    }
}

/// A file read as expanded: see the [module docs](self).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpandedFile {
    text: String,
    /// The file's own text where no invocation is: each piece's span in
    /// `text`, and the file's offset it starts at.
    copied: Vec<((u32, u32), u32)>,
    /// Each invocation the file's text holds, expanded: its span in the
    /// file, the call (its macro's path's span), its span in `text`, and
    /// its tokens (with those of the invocations in its arguments its
    /// expansion holds, expanded).
    parts: Vec<Part>,
    /// The invocations the file writes inside another's arguments that
    /// the text holds unexpanded (see [`inner`](ExpandedFile::inner)).
    inner: Vec<InvocationAt>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Part {
    file: (u32, u32),
    call: (u32, u32),
    at: (u32, u32),
    /// Each token's span in `text`; the file's span where the file
    /// writes it; and the call of the expansion it stands in (an
    /// invocation's in the part's arguments, or the part's).
    tokens: Vec<PartToken>,
}

/// `text`, a file of `lang`'s language, with each of `expansions` (an
/// invocation it writes, and its expansion) in the invocation's place:
/// see the [module docs](self).
///
/// An invocation inside another's arguments (its span inside the other's)
/// is put where the other's expansion copies it whole, each time it does,
/// and is left as tokens where it does not; one that overlaps another
/// otherwise is left as written. The invocations the file writes in the
/// expansions' arguments that are not among `expansions` are the result's
/// [`inner`](ExpandedFile::inner): expanded, and read again with the rest,
/// the names they hold are read too.
pub fn expanded_file(
    lang: &EmbeddedLang,
    text: &str,
    expansions: &[(InvocationAt, ExpandedText)],
) -> ExpandedFile {
    let nodes = nest(expansions);
    // The text, and each token with what wrote it: `None` for the file,
    // else (which expansion put in it, its span there, its mark).
    let first = splice(text, expansions, &nodes, &HashMap::new());
    // Hygiene: what a macro writes as a local, and a name a macro writes
    // that binds to one, take a name of their own expansion. The same
    // reading says the invocations in the expansions' arguments.
    let given: HashSet<(u32, u32)> = expansions.iter().map(|(inv, _)| inv.span).collect();
    let (renames, inner) = read_spliced(lang, &first, &given);
    let mut file = if renames.is_empty() {
        first.file
    } else {
        splice(text, expansions, &nodes, &renames).file
    };
    file.inner = inner;
    file
}

/// The expansions of the invocations directly in an expansion's
/// arguments, by their indices among the expansions, in the file's order.
struct Node {
    children: Vec<usize>,
}

/// `expansions` as a forest: each invocation inside the nearest one whose
/// span holds its own. The roots first, in the file's order.
fn nest(expansions: &[(InvocationAt, ExpandedText)]) -> (Vec<Node>, Vec<usize>) {
    let mut order: Vec<usize> = (0..expansions.len()).collect();
    order.sort_by_key(|&k| (expansions[k].0.span.0, std::cmp::Reverse(expansions[k].0.span.1)));
    let mut nodes: Vec<Node> =
        (0..expansions.len()).map(|_| Node { children: Vec::new() }).collect();
    let mut roots = Vec::new();
    let mut open: Vec<usize> = Vec::new();
    for k in order {
        let span = expansions[k].0.span;
        while let Some(&o) = open.last() {
            if expansions[o].0.span.1 <= span.0 {
                open.pop();
            } else {
                break;
            }
        }
        match open.last() {
            Some(&o) if span.1 <= expansions[o].0.span.1 => nodes[o].children.push(k),
            // One that overlaps the one it starts in is no part of it.
            Some(_) => continue,
            None => roots.push(k),
        }
        open.push(k);
    }
    (nodes, roots)
}

/// A spliced text, and each token a macro wrote in it, by its span's start
/// there: which expansion put in it is in (in the order they are put in),
/// its span in that expansion's own text, and the mark of the expansion
/// that wrote it.
struct Spliced {
    file: ExpandedFile,
    written_by: HashMap<u32, (usize, (u32, u32), u32)>,
}

/// What [`splice`] writes as it goes.
struct Splicer<'a> {
    expansions: &'a [(InvocationAt, ExpandedText)],
    nodes: &'a [Node],
    renames: &'a Renames,
    out: String,
    written_by: HashMap<u32, (usize, (u32, u32), u32)>,
    /// How many expansions have been put in.
    put: usize,
}

/// `text` with each of `expansions` in place as `nodes` nests them, a
/// token `renames` names (by the expansion put in, in the order they are,
/// and its span in its expansion's own text) written as that name.
fn splice(
    text: &str,
    expansions: &[(InvocationAt, ExpandedText)],
    (nodes, roots): &(Vec<Node>, Vec<usize>),
    renames: &Renames,
) -> Spliced {
    let mut s = Splicer {
        expansions,
        nodes,
        renames,
        out: String::with_capacity(text.len()),
        written_by: HashMap::new(),
        put: 0,
    };
    let mut copied = Vec::new();
    let mut parts = Vec::new();
    let mut at = 0u32;
    for &k in roots {
        let inv = &expansions[k].0;
        if inv.span.0 < at || inv.span.1 as usize > text.len() {
            continue;
        }
        let start = s.out.len() as u32;
        s.out.push_str(&text[at as usize..inv.span.0 as usize]);
        if inv.span.0 > at {
            copied.push(((start, s.out.len() as u32), at));
        }
        let begin = s.out.len() as u32;
        let mut tokens = Vec::new();
        s.put(k, &mut tokens);
        let call = inv.path_span.unwrap_or(inv.span);
        parts.push(Part { file: inv.span, call, at: (begin, s.out.len() as u32), tokens });
        at = inv.span.1;
    }
    let start = s.out.len() as u32;
    s.out.push_str(&text[at as usize..]);
    if (at as usize) < text.len() {
        copied.push(((start, s.out.len() as u32), at));
    }
    Spliced {
        file: ExpandedFile { text: s.out, copied, parts, inner: Vec::new() },
        written_by: s.written_by,
    }
}

impl Splicer<'_> {
    /// Put in expansion `k`, and in it, where it copies one of the
    /// invocations in its arguments whole, that one's: its tokens into
    /// `tokens`.
    fn put(&mut self, k: usize, tokens: &mut Vec<PartToken>) {
        let (inv, expansion) = &self.expansions[k];
        let index = self.put;
        self.put += 1;
        let call = inv.path_span.unwrap_or(inv.span);
        let wrap = parenthesized(&inv.entry);
        if wrap {
            self.out.push('(');
        }
        let place =
            |written: Option<(u32, u32)>| written.map(|(s, e)| (inv.span.0 + s, inv.span.0 + e));
        let mut last = 0u32;
        let mut i = 0;
        while i < expansion.tokens.len() {
            let (span, written, mark) = expansion.tokens[i];
            let file = place(written);
            // An invocation in the arguments, copied whole from here: its
            // expansion in its place.
            let copied = file.and_then(|f| {
                self.nodes[k].children.iter().find_map(|&c| {
                    let child = self.expansions[c].0.span;
                    (child.0 == f.0)
                        .then(|| whole(&expansion.tokens[i..], inv.span.0, child))?
                        .map(|n| (c, n))
                })
            });
            if let Some((c, n)) = copied {
                self.out.push_str(&expansion.text[last as usize..span.0 as usize]);
                self.put(c, tokens);
                last = expansion.tokens[i + n - 1].0 .1;
                i += n;
                continue;
            }
            self.out.push_str(&expansion.text[last as usize..span.0 as usize]);
            let here = self.out.len() as u32;
            match self.renames.get(&(index, span)) {
                Some(name) => self.out.push_str(name),
                None => self.out.push_str(&expansion.text[span.0 as usize..span.1 as usize]),
            }
            tokens.push(((here, self.out.len() as u32), file, call));
            if written.is_none() {
                self.written_by.insert(here, (index, span, mark));
            }
            last = span.1;
            i += 1;
        }
        self.out.push_str(&expansion.text[last as usize..]);
        if wrap {
            self.out.push(')');
        }
    }
}

/// How many of `tokens` (an expansion's, its invocation at `site` in the
/// file) are a copy of the file's `span` whole: each the file's, each after
/// the one before it in the file, the first at its start and the last at
/// its end. `None` where they are not.
fn whole(tokens: &[TextToken], site: u32, span: (u32, u32)) -> Option<usize> {
    let mut end = span.0;
    for (n, (_, written, _)) in tokens.iter().enumerate() {
        let (s, e) = (*written)?;
        let (s, e) = (site + s, site + e);
        if s < end || e > span.1 {
            return None;
        }
        end = e;
        if e == span.1 {
            return Some(n + 1);
        }
    }
    None
}

/// What reading `spliced` says: the names `macro_rules!` hygiene gives its
/// tokens (see below), and the invocations the file writes in the
/// expansions' arguments that it holds unexpanded, not those of `given`
/// (spans of the file), placed in the file.
///
/// Hygiene: each local a macro writes, and each name a macro writes that
/// binds to a local, takes a name of the expansion that wrote it, which no
/// source uses (by the expansion put in and the token's span in its own
/// text). A name the file writes keeps its own: it then binds to no local a
/// macro wrote, and a name a macro wrote to none of the file's.
fn read_spliced(
    lang: &EmbeddedLang,
    spliced: &Spliced,
    given: &HashSet<(u32, u32)>,
) -> (Renames, Vec<InvocationAt>) {
    let text = &spliced.file.text;
    let session = lang.session(text);
    let Some(tree) = session.tree().cloned() else { return (HashMap::new(), Vec::new()) };
    let mut db = SemDb::new(lang.binding.clone());
    db.set_types(lang.types.clone());
    db.set_macro_bodies(&lang.macros);
    db.set_tree(URI, tree.clone());
    let symbols = db.symbols(URI);
    let resolutions = db.resolve(URI);
    let read = units::read(&tree, &lang.binding, &lang.outline);
    let declared: HashSet<(u32, u32)> =
        outside_bodies(&tree, &lang.outline).into_iter().map(|(s, _)| s.selection).collect();
    let local = |span: (u32, u32)| !declared.contains(&span) && !read.import_defs.contains(&span);
    // A tag no name in the text holds.
    let mut tag = String::from("__hyg");
    let mut n = 0;
    while text.contains(&tag) {
        n += 1;
        tag = format!("__hyg{n}");
    }
    let mut renames = HashMap::new();
    let mut rename = |at: u32, name: &str| {
        if let Some(&(index, own, mark)) = spliced.written_by.get(&at) {
            renames.insert((index, own), format!("{name}{tag}{index}_{mark}"));
        }
    };
    for d in &symbols.defs {
        if local(d.span) {
            rename(d.span.0, &d.name);
        }
    }
    for (r, target) in symbols.refs.iter().zip(resolutions.iter()) {
        if let Target::Local { def } = target {
            if symbols.defs.get(*def).is_some_and(|d| local(d.span)) {
                rename(r.span.0, &r.name);
            }
        }
    }
    let file = &spliced.file;
    let mut said = HashSet::new();
    let inner = read
        .all_invocations
        .iter()
        .filter(|inv| file.parts.iter().any(|p| p.at.0 <= inv.span.0 && inv.span.1 <= p.at.1))
        .filter_map(|inv| {
            let (span, true) = file.in_file(inv.span)? else { return None };
            let path_span = inv.path_span?;
            let (in_file, true) = file.in_file(path_span)? else { return None };
            // Each once, though an expansion copies it more than once.
            if given.contains(&span) || !said.insert(span) {
                return None;
            }
            let units = qana_sem::path_at(&tree, &lang.binding, path_span.1.saturating_sub(1))
                .map(|(units, _, _)| units)
                .unwrap_or_default();
            let seen = is_textual(lang, &inv.namespace)
                .then(|| file.seen(replay(&read.textual, path_span.0)));
            Some(InvocationAt {
                span,
                path_span: Some(in_file),
                path: inv.path.clone(),
                namespace: inv.namespace.clone(),
                entry: lang.sg.nt_names[inv.entry as usize].clone(),
                units,
                seen,
                among_items: false,
            })
        })
        .collect();
    (renames, inner)
}

impl ExpandedFile {
    /// The file's text, read as expanded.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The invocations the file writes inside another's arguments that the
    /// text holds unexpanded, where an expansion copies them whole: each
    /// placed in the file, with what resolving its macro needs (its units
    /// and textual scope as the text has them, a definition a macro wrote
    /// at the call). Given their expansions too, [`expanded_file`] reads
    /// the names they hold.
    pub fn inner(&self) -> &[InvocationAt] {
        &self.inner
    }

    /// Whether byte `at` of the file stands in an invocation the text
    /// holds the expansion of.
    pub fn expands(&self, at: u32) -> bool {
        let k = self.parts.partition_point(|p| p.file.1 <= at);
        self.parts.get(k).is_some_and(|p| p.file.0 <= at)
    }

    /// Where byte `at` of the file stands in the text: in the file's own
    /// text, or in a token of the expansion where the invocation writes
    /// it. `None` for a byte of an invocation its expansion does not hold
    /// (its delimiters, a token no arm transcribes).
    pub fn in_text(&self, at: u32) -> Option<u32> {
        let k = self.parts.partition_point(|p| p.file.1 <= at);
        if let Some(p) = self.parts.get(k).filter(|p| p.file.0 <= at) {
            return p.tokens.iter().find_map(|((s, _), file, _)| match file {
                Some((fs, fe)) if *fs <= at && at < *fe => Some(s + (at - fs)),
                _ => None,
            });
        }
        // The file's own text: the piece that holds it (its end too, where
        // the file ends there).
        let k = self.copied.partition_point(|&((s, e), from)| from + (e - s) < at);
        let &((s, e), from) = self.copied.get(k)?;
        (from <= at && at <= from + (e - s)).then_some(s + (at - from))
    }

    /// Where `span` of the text stands in the file: the file's span where
    /// the file writes all of it (`true`), else the call: the macro path of
    /// the invocation whose expansion wrote it (`false`), the innermost of
    /// those it stands in.
    pub fn in_file(&self, span: (u32, u32)) -> Option<((u32, u32), bool)> {
        let k = self.parts.partition_point(|p| p.at.1 <= span.0);
        if let Some(p) = self.parts.get(k).filter(|p| p.at.0 <= span.0) {
            let first = p.tokens.partition_point(|(t, _, _)| t.1 <= span.0);
            let covered: Vec<&PartToken> = p.tokens[first..]
                .iter()
                .take_while(|(t, _, _)| t.0 < span.1.max(span.0 + 1))
                .collect();
            let written = match covered.as_slice() {
                [(t, Some(f), _)] if t.0 <= span.0 && span.1 <= t.1 => {
                    Some((f.0 + (span.0 - t.0), f.0 + (span.1 - t.0)))
                }
                [(_, Some(a), _), .., (_, Some(b), _)]
                    if covered.iter().all(|(_, f, _)| f.is_some()) =>
                {
                    (a.0 <= b.1).then_some((a.0, b.1))
                }
                _ => None,
            };
            let call = covered
                .iter()
                .find(|(_, f, _)| f.is_none())
                .or(covered.first())
                .map_or(p.call, |(_, _, call)| *call);
            return Some(match written {
                Some(span) => (span, true),
                None => (call, false),
            });
        }
        let k = self.copied.partition_point(|&((_, e), _)| e < span.0);
        let &((s, e), from) = self.copied.get(k)?;
        (s <= span.0 && span.1 <= e).then_some(((from + span.0 - s, from + span.1 - s), true))
    }

    /// `references`, read from the text, as the file has them: those whose
    /// names the file writes, each once, at its place there (its line and
    /// column in `file_text`, the file's text), what it binds to there
    /// where the file writes that too, and what it sees by textual scope
    /// placed in the file.
    pub fn references(&self, file_text: &str, references: Vec<Reference>) -> Vec<Reference> {
        let lines = LineStarts::of(file_text);
        let mut placed = HashSet::new();
        let mut out = Vec::new();
        for mut r in references {
            let Some((span, true)) = self.in_file(r.span) else { continue };
            if !placed.insert(span) {
                continue;
            }
            let (line, column) = lines.position(file_text, span.0);
            r.span = span;
            r.line = line;
            r.column = column;
            r.line_text = lines.text(file_text, line).to_string();
            r.binds = r.binds.and_then(|b| match self.in_file(b) {
                Some((b, true)) => Some(b),
                _ => None,
            });
            r.textual = r.textual.map(|seen| self.seen(seen));
            out.push(r);
        }
        out.sort_by_key(|r| r.span);
        out
    }

    /// `accesses`, read from the text, as the file has them: those whose
    /// names the file writes, each once, at its place there.
    pub fn members(&self, accesses: Vec<MemberAccess>) -> Vec<MemberAccess> {
        let mut placed = HashSet::new();
        let mut out = Vec::new();
        for mut access in accesses {
            let Some((span, true)) = self.in_file(access.span) else { continue };
            if placed.insert(span) {
                access.span = span;
                out.push(access);
            }
        }
        out.sort_by_key(|a| a.span);
        out
    }

    /// `seen`, read from the text, placed in the file: a definition a macro
    /// writes at the call.
    pub fn seen(&self, seen: Vec<Seen>) -> Vec<Seen> {
        seen.into_iter()
            .map(|s| match s {
                Seen::Def { name, span, attributes } => {
                    let span = self.in_file(span).map_or(span, |(span, _)| span);
                    Seen::Def { name, span, attributes }
                }
                Seen::Module { attributes, seen } => {
                    Seen::Module { attributes, seen: self.seen(seen) }
                }
                file @ Seen::File { .. } => file,
            })
            .collect()
    }
}
