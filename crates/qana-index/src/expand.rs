//! An invocation's expansion, as a worker reads it: the macro's arm that
//! matches what the invocation passes it, transcribed, parsed where the
//! invocation stands, and summarized as a file's text is ([`Summary`]'s
//! offers and needs), each place in it said as where the invocation's
//! text writes it ([`ExpansionSummary`]).
//!
//! One level at a time, as a rule: the invocations an expansion holds are
//! the expansion summary's, each with what it is passed. What each names, a
//! project index resolves, where the outermost invocation stands, and asks
//! for in turn; a summary of its file merged with its expansions
//! ([`crate::merge`]) is what the index resolves over. An invocation whose
//! macros the index has said already is expanded with its parent, in turn,
//! to the recursion limit: one by a path from the macro's crate's root
//! (`$crate::name!`, which a crate's macros write to invoke each other) that
//! the [`MacroTable`] holds, the index asked with the macro; and one by the
//! name alone the invocation is by, which a macro that invokes itself
//! writes ([`ExpandRequest::itself`]), where no expansion between them binds
//! that name.
//!
//! A macro exported with `local_inner_macros` (Rust's) names its crate's
//! macros by a name alone: an invocation of a name alone that one writes
//! (not one a fragment holds) is its crate's, as `$crate::name!` is.
//!
//! What is written where: a token the outermost invocation's text writes
//! (what it passes, forwarded to nested invocations whole) is at its place
//! in that text, the site; one a macro writes is at none, and a project
//! index puts it at the site's macro path, the call.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use qana_lang::EmbeddedLang;
use qana_mbe::{Edition, Form, MacroRules, Parsed, TokenStream};

use crate::{
    summarize_tree, Attribute, Fnv, LineStarts, Locations, Needs, Offers, Path, Root, Summary,
    TextualLink,
};

/// What a worker is asked to expand.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ExpandRequest {
    /// The macro.
    pub definition: MacroSource,
    /// The outermost invocation as its file writes it, from its macro's
    /// path to its end: what the places of the tokens it writes are in.
    pub site: String,
    /// The edition the site is written in.
    pub site_edition: Edition,
    /// What a nested invocation passes its macro, as an expansion wrote
    /// it; `None` for the site's own invocation, which passes what its
    /// text writes inside its delimiters.
    pub input: Option<TokenStream>,
    /// The grammar's rule the expansion is parsed at (an entry).
    pub entry: String,
    /// What the tokens the macro writes are marked with: how deep the
    /// expansion is, from 1 for an invocation a file writes.
    pub mark: u32,
    /// How deep expansions may nest where the site is (its crate's
    /// [`recursion_limit`](crate::CrateInfo::recursion_limit), or
    /// [`RECURSION_LIMIT`](crate::RECURSION_LIMIT)): an invocation the
    /// expansion holds is expanded with it only while it is no deeper.
    pub limit: u32,
    /// The expansions the invocation stands in whose macros are exported
    /// with `local_inner_macros`, by their marks: an invocation of a name
    /// alone that one of them wrote names its crate's macro.
    pub local_inner: Vec<u32>,
    /// The expansions the invocation stands in whose macros are the
    /// table's crate's, by their marks: a `$crate` one of them wrote is the
    /// table's crate, as one the job's own macros write is.
    pub of_crate: Vec<u32>,
    /// Where the invocation names its macro by a name alone (`name!`): the
    /// name, and the macros it names there, one by each answer, all of the
    /// table's crate. A nested
    /// invocation of that name alone names them too (a macro's name is
    /// looked for where the invocation stands, whoever wrote it), unless an
    /// expansion between them binds the name, and is expanded with it.
    pub itself: Option<(String, Vec<MacroSource>)>,
}

/// A macro, as a worker is given it.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MacroSource {
    /// A definition a file writes: its body (what follows its name), how
    /// it is written, the edition it is written in, and whether it is
    /// exported with `local_inner_macros`.
    Text { body: String, form: Form, edition: Edition, local_inner: bool },
    /// A definition an expansion wrote: its body's tokens, how it is
    /// written, and whether it is exported with `local_inner_macros`.
    Tokens { body: TokenStream, form: Form, local_inner: bool },
    /// A macro the compiler defines (its definition says no rules), whose
    /// expansion the model says, with the attribute that puts what it
    /// defines under a condition, where the expansion is a choice among
    /// conditions (Rust's `cfg`).
    Builtin { macro_: BuiltinMacro, condition: Option<Path> },
}

/// The macros an expansion's invocations may name that a worker expands
/// with it ([`expand`]): each path from the macro's crate's root
/// (`$crate::name`, rooted at [`Root::MacroCrate`]) with the macros it
/// names there, one by each answer, all of that crate. A path is in it
/// only where every answer is a [`MacroSource`] the worker can expand.
pub type MacroTable = [(Path, Vec<MacroSource>)];

/// A macro the compiler defines, whose expansion the project model says
/// ([`crate::CrateInfo::builtin_macros`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum BuiltinMacro {
    /// Rust's `cfg_select!`: the items of each arm (`predicate => { … }`,
    /// `_ => { … }`), each under its condition, which a summary does not
    /// decide: all of them, every one an answer.
    CfgSelect,
}

/// An invocation's expansion, read: see the [module docs](self).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExpansionSummary {
    outcome: Result<Expanded, String>,
    fingerprint: u64,
}

/// An invocation in an expansion that was expanded with it: its index
/// among the expansion's invocations, the path its macros are named by in
/// the table, and its expansion by each, in the table's order.
pub type NestedExpansion = (u32, Path, Vec<Rc<ExpansionSummary>>);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Expanded {
    offers: Offers,
    needs: Needs,
    /// Where in the site each place of `offers` and `needs` is written.
    placed: Placed,
    /// What each invocation in it passes its macro; `None` for one
    /// expanded with it.
    inputs: Vec<Option<TokenStream>>,
    /// Each macro it defines, its body's tokens.
    bodies: Vec<TokenStream>,
    /// The invocations in it expanded with it.
    nested: Vec<NestedExpansion>,
}

/// A place in an expansion, where the site writes it: its byte span in
/// the site's text, and the line and character column of its start there
/// (from 0, the site's first line counted from where the site begins).
/// `None` for a place a macro writes.
pub type SitePlace = Option<((u32, u32), (u32, u32))>;

/// Where the site writes each place an expansion's summary has, field by
/// field as [`Locations`] has them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Placed {
    pub names: Vec<SitePlace>,
    pub items: Vec<SitePlace>,
    pub units: Vec<SitePlace>,
    pub unit_items: Vec<SitePlace>,
    pub reexports: Vec<SitePlace>,
    pub imports: Vec<SitePlace>,
    pub impls: Vec<SitePlace>,
    pub impl_heads: Vec<(Option<SitePlace>, Option<SitePlace>)>,
    pub derives: Vec<Vec<SitePlace>>,
    pub invocations: Vec<SitePlace>,
    pub invocation_paths: Vec<Option<SitePlace>>,
    pub invocation_items: Vec<SitePlace>,
    pub macro_bodies: Vec<SitePlace>,
}

impl ExpansionSummary {
    fn new(outcome: Result<Expanded, String>) -> ExpansionSummary {
        let mut hash = Fnv::new();
        match &outcome {
            Ok(e) => {
                hash.u64(0);
                hash.u64(e.offers.fingerprint());
                hash.u64(e.needs.fingerprint());
                hash.places(&e.placed);
                hash.hashed(&e.inputs);
                hash.hashed(&e.bodies);
                hash.u64(e.nested.len() as u64);
                for (i, path, expansions) in &e.nested {
                    hash.u64(u64::from(*i));
                    hash.hashed(path);
                    hash.u64(expansions.len() as u64);
                    for expansion in expansions {
                        hash.u64(expansion.fingerprint);
                    }
                }
            }
            Err(why) => {
                hash.u64(1);
                hash.str(why);
            }
        }
        let fingerprint = hash.finish();
        ExpansionSummary { outcome, fingerprint }
    }

    /// No expansion, for `why` (rustc's message): what a project index
    /// keeps for an invocation it does not ask a worker to expand, nested
    /// past the recursion limit ([`ExpandRequest::limit`]).
    pub fn failed(why: String) -> ExpansionSummary {
        ExpansionSummary::new(Err(why))
    }

    /// Why there is no expansion, where there is none: the macro's
    /// definition does not read, no arm matches what the invocation
    /// passes, the expansion does not parse where the invocation stands.
    /// rustc's message.
    pub fn error(&self) -> Option<&str> {
        self.outcome.as_ref().err().map(String::as_str)
    }

    /// What the expansion offers, as a file's summary does: what it
    /// declares, imports for others and implements, the invocations in it
    /// and the macros it defines. `None` where there is no expansion.
    pub fn offers(&self) -> Option<&Offers> {
        self.outcome.as_ref().ok().map(|e| &e.offers)
    }

    /// What the expansion needs: what it imports for itself.
    pub fn needs(&self) -> Option<&Needs> {
        self.outcome.as_ref().ok().map(|e| &e.needs)
    }

    /// What the `i`th invocation in the expansion passes its macro; `None`
    /// for one expanded with it ([`nested`](Self::nested)), whose input
    /// is not kept.
    pub fn input(&self, i: usize) -> Option<&TokenStream> {
        self.outcome.as_ref().ok().and_then(|e| e.inputs.get(i)?.as_ref())
    }

    /// The body of the `k`th macro the expansion defines, its tokens.
    pub fn body(&self, k: usize) -> Option<&TokenStream> {
        self.outcome.as_ref().ok().and_then(|e| e.bodies.get(k))
    }

    /// The invocations in the expansion that were expanded with it, by the
    /// table it was asked with ([`expand`]), each with its expansions.
    pub fn nested(&self) -> &[NestedExpansion] {
        match &self.outcome {
            Ok(e) => &e.nested,
            Err(_) => &[],
        }
    }

    /// The same summary without its token streams (what its invocations
    /// pass and its macros' bodies), nor those of the expansions made with
    /// it: what a store keeps, all a merge reads. Expanding what it holds
    /// needs them: [`has_tokens`](Self::has_tokens) says whether it does.
    pub fn without_tokens(&self) -> ExpansionSummary {
        match &self.outcome {
            Ok(e) => ExpansionSummary {
                outcome: Ok(Expanded {
                    inputs: Vec::new(),
                    bodies: Vec::new(),
                    nested: e
                        .nested
                        .iter()
                        .map(|(i, path, expansions)| {
                            let stripped =
                                expansions.iter().map(|x| Rc::new(x.without_tokens())).collect();
                            (*i, path.clone(), stripped)
                        })
                        .collect(),
                    ..e.clone()
                }),
                fingerprint: self.fingerprint,
            },
            Err(_) => self.clone(),
        }
    }

    /// Whether it holds what each of its invocations passes and each of its
    /// macros' bodies (none was taken away, [`without_tokens`](Self::without_tokens)).
    /// Those of the expansions made with it, each says.
    pub fn has_tokens(&self) -> bool {
        match &self.outcome {
            Ok(e) => {
                e.inputs.len() == e.offers.invocations.len()
                    && e.bodies.len() == e.offers.macros.len()
            }
            Err(_) => true,
        }
    }

    pub(crate) fn placed(&self) -> Option<&Placed> {
        self.outcome.as_ref().ok().map(|e| &e.placed)
    }

    /// A hash of everything it holds, the expansions made with it too:
    /// equal summaries, equal fingerprints.
    pub fn fingerprint(&self) -> u64 {
        self.fingerprint
    }
}

/// Expand what `request` asks, in `lang`, the grammar the site is written
/// in, and with it each invocation it holds by a path `table` holds, in
/// turn, to the request's limit: see the [module docs](self).
pub fn expand(
    lang: &EmbeddedLang,
    request: &ExpandRequest,
    table: &MacroTable,
) -> ExpansionSummary {
    let site = match TokenStream::lex(lang, &request.site, 0, 0, request.site_edition) {
        Ok(site) => site,
        Err(e) => return ExpansionSummary::failed(e.message),
    };
    let input = match &request.input {
        Some(input) => input.clone(),
        None => match site.invocation() {
            Some((_, input)) => input,
            None => return ExpansionSummary::failed("the site is no invocation".to_string()),
        },
    };
    let mut job = Job {
        lang,
        site: &request.site,
        lines: LineStarts::of(&request.site),
        top: request.mark,
        limit: request.limit,
        by_path: table.iter().enumerate().map(|(k, (path, _))| (path, k)).collect(),
        itself: request.itself.as_ref().map(|(name, _)| name.as_str()),
        of_crate: &request.of_crate,
        rules: HashMap::new(),
    };
    let itself: &[MacroSource] = request.itself.as_ref().map_or(&[], |(_, macros)| macros);
    // Each expansion, its parent's before it; made depth first, by a list
    // of what is left rather than by recursion (expansions nest as deep as
    // the limit).
    struct Made {
        outcome: Result<Expanded, String>,
        nested: Vec<(u32, Path, Vec<usize>)>,
    }
    struct Work {
        made: usize,
        definition: Definition,
        input: TokenStream,
        entry: String,
        mark: u32,
        local_inner: Vec<u32>,
        /// Whether an expansion it stands in binds the name the request's
        /// invocation is by.
        bound: bool,
    }
    let mut made: Vec<Option<Made>> = vec![None];
    let mut work = vec![Work {
        made: 0,
        definition: Definition::Asked,
        input,
        entry: request.entry.clone(),
        mark: request.mark,
        local_inner: request.local_inner.clone(),
        bound: false,
    }];
    while let Some(w) = work.pop() {
        let definition = match w.definition {
            Definition::Asked => &request.definition,
            Definition::Table(k, which) => &table[k].1[which],
            Definition::Itself(which) => &itself[which],
        };
        let level = (w.definition, w.mark, w.bound);
        let (outcome, wanted, bound) =
            job.level(definition, level, &w.input, &w.entry, &w.local_inner);
        let mut nested = Vec::new();
        for want in wanted {
            let mut ids = Vec::new();
            let count = match want.table {
                Some(k) => table[k].1.len(),
                None => itself.len(),
            };
            for which in 0..count {
                ids.push(made.len());
                made.push(None);
                work.push(Work {
                    made: *ids.last().expect("just pushed"),
                    definition: match want.table {
                        Some(k) => Definition::Table(k, which),
                        None => Definition::Itself(which),
                    },
                    input: want.input.clone(),
                    entry: want.entry.clone(),
                    mark: w.mark + 1,
                    local_inner: want.local_inner.clone(),
                    bound,
                });
            }
            nested.push((want.invocation, want.path, ids));
        }
        made[w.made] = Some(Made { outcome, nested });
    }
    // Each expansion is made after its parent: from the last, every one's
    // nested expansions are summarized before it.
    let mut summaries: Vec<Option<Rc<ExpansionSummary>>> = vec![None; made.len()];
    for id in (0..made.len()).rev() {
        let Made { outcome, nested } = made[id].take().expect("every expansion is made");
        let outcome = outcome.map(|mut e| {
            e.nested = nested
                .into_iter()
                .map(|(i, path, ids)| {
                    let expansions = ids
                        .iter()
                        .map(|&c| summaries[c].take().expect("a nested expansion is summarized"))
                        .collect();
                    (i, path, expansions)
                })
                .collect();
            e
        });
        summaries[id] = Some(Rc::new(ExpansionSummary::new(outcome)));
    }
    let top = summaries[0].take().expect("the expansion is summarized");
    Rc::try_unwrap(top).unwrap_or_else(|shared| (*shared).clone())
}

/// Which macro an expansion in a job is by: the one asked, one of a path's
/// in the table (its index there, and which answer), or one the request's
/// invocation's name names (which answer).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Definition {
    Asked,
    Table(usize, usize),
    Itself(usize),
}

/// An invocation in an expansion whose macros the job has (by the table's
/// path, or `None`: by the request's invocation's name): to be expanded
/// with it.
struct Wanted {
    invocation: u32,
    path: Path,
    table: Option<usize>,
    input: TokenStream,
    entry: String,
    local_inner: Vec<u32>,
}

/// What a request's expansions share: the grammar, the site, the table.
struct Job<'a> {
    lang: &'a EmbeddedLang,
    site: &'a str,
    lines: LineStarts,
    /// The request's mark: a token marked with it or more was written by
    /// this job's macros, all of the table's crate.
    top: u32,
    limit: u32,
    by_path: HashMap<&'a Path, usize>,
    /// The name alone the request's invocation is by, where it is one.
    itself: Option<&'a str>,
    /// The marks of the expansions before the job's whose macros are the
    /// table's crate's.
    of_crate: &'a [u32],
    /// Each macro read, once.
    rules: HashMap<Definition, Result<Arc<MacroRules>, String>>,
}

impl Job<'_> {
    /// Macro `definition`'s rules, read once a job.
    fn rules(
        &mut self,
        definition: &MacroSource,
        which: Definition,
    ) -> Result<Arc<MacroRules>, String> {
        if let Some(read) = self.rules.get(&which) {
            return read.clone();
        }
        let read = match definition {
            MacroSource::Text { body, form, edition, local_inner } => {
                TokenStream::lex(self.lang, body, 1, 0, *edition)
                    .and_then(|body| MacroRules::read(&body, *form))
                    .map(|rules| if *local_inner { rules.local_inner() } else { rules })
                    .map(Arc::new)
                    .map_err(|e| e.message)
            }
            MacroSource::Tokens { body, form, local_inner } => MacroRules::read(body, *form)
                .map(|rules| if *local_inner { rules.local_inner() } else { rules })
                .map(Arc::new)
                .map_err(|e| e.message),
            MacroSource::Builtin { .. } => Err("a built-in macro has no rules".to_string()),
        };
        self.rules.insert(which, read.clone());
        read
    }

    /// One expansion, by `definition` (`which` it is) of what `input`
    /// passes it, at `entry` and `mark`, where the expansions in
    /// `local_inner` (by their marks) are `local_inner_macros` macros' and
    /// `bound` says whether one it stands in binds the name the request's
    /// invocation is by; with the invocations in it whose macros the job
    /// has, to expand with it, and whether it or one it stands in binds that
    /// name.
    fn level(
        &mut self,
        definition: &MacroSource,
        (which, mark, bound): (Definition, u32, bool),
        input: &TokenStream,
        entry: &str,
        local_inner: &[u32],
    ) -> (Result<Expanded, String>, Vec<Wanted>, bool) {
        match self.expanded(definition, (which, mark, bound), input, entry, local_inner) {
            Ok((expanded, wanted, bound)) => (Ok(expanded), wanted, bound),
            Err(why) => (Err(why), Vec::new(), bound),
        }
    }

    fn expanded(
        &mut self,
        definition: &MacroSource,
        (which, mark, bound): (Definition, u32, bool),
        input: &TokenStream,
        entry: &str,
        inherited: &[u32],
    ) -> Result<(Expanded, Vec<Wanted>, bool), String> {
        let lang = self.lang;
        let mut condition: Option<Path> = None;
        let mut local_inner = inherited.to_vec();
        let tokens = match definition {
            MacroSource::Text { .. } | MacroSource::Tokens { .. } => {
                let rules = self.rules(definition, which)?;
                if rules.is_local_inner() {
                    local_inner.push(mark);
                }
                qana_mbe::expand(lang, &rules, input, mark).map_err(|e| e.message)?
            }
            MacroSource::Builtin { macro_: BuiltinMacro::CfgSelect, condition: cond } => {
                let arms = input.cfg_select_arms().map_err(|e| e.message)?;
                // A lone `_` arm holds whatever the conditions; any other arm
                // holds under its own.
                let chosen = !matches!(arms.as_slice(), [(None, _)]);
                if chosen {
                    condition = cond.clone();
                }
                TokenStream::concat(arms.into_iter().map(|(_, body)| body))
            }
        };
        let parsed = tokens.parse(lang, entry).map_err(|e| e.message)?;
        let mut summary = summarize_tree(lang, &parsed.text, parsed.tree.clone());
        let mut changed = false;
        if let Some(condition) = condition {
            let under = Attribute { path: condition, conditional: false, args: Vec::new() };
            let mark = |links: &mut Vec<TextualLink>| {
                for link in links {
                    if let TextualLink::Def { attributes, .. } = link {
                        attributes.push(under.clone());
                    }
                }
            };
            mark(&mut summary.offers.textual);
            for unit in &mut summary.offers.units {
                mark(&mut unit.textual);
            }
            changed = true;
        }
        // A name alone that a `local_inner_macros` macro wrote is its
        // crate's.
        if !local_inner.is_empty() {
            for (i, inv) in summary.offers.invocations.iter_mut().enumerate() {
                let Some(path) = &mut inv.path else { continue };
                if path.root.is_some() || path.segments.len() != 1 {
                    continue;
                }
                let written = summary.locations.invocation_paths[i]
                    .and_then(|span| parsed.token_at(span.0))
                    .is_some_and(|t| local_inner.contains(&t.mark));
                if written {
                    path.root = Some(Root::MacroCrate);
                    changed = true;
                }
            }
        }
        if changed {
            summary.fingerprint();
        }
        let bound = bound || self.itself.is_some_and(|name| binds(&summary, name));
        let wanted = if mark < self.limit {
            self.wanted(&summary, &parsed, &local_inner, bound)
        } else {
            Vec::new()
        };
        let at = Sites { site: self.site, lines: &self.lines, parsed: &parsed };
        let locations = &summary.locations;
        let placed = at.placed(locations);
        let inputs = (0..summary.offers.invocations.len())
            .map(|i| {
                if wanted.iter().any(|w| w.invocation as usize == i) {
                    return None;
                }
                Some(
                    parsed
                        .stream(locations.invocation_span(i))
                        .invocation()
                        .map(|(_, input)| input)
                        .unwrap_or_default(),
                )
            })
            .collect();
        let bodies = (0..summary.offers.macros.len())
            .map(|k| parsed.stream(locations.macro_body_span(k)))
            .collect();
        let expanded = Expanded {
            offers: summary.offers,
            needs: summary.needs,
            placed,
            inputs,
            bodies,
            nested: Vec::new(),
        };
        Ok((expanded, wanted, bound))
    }

    /// The invocations in an expansion (`summary`, of `parsed`) whose
    /// macros the job has, which are expanded with it: by a path from the
    /// macro's crate's root that the table holds, written by this job's
    /// macros or another of the table's crate's (`$crate` is then the
    /// table's crate); and by the name alone the request's invocation is
    /// by, where nothing `bound` it between.
    fn wanted(
        &self,
        summary: &Summary,
        parsed: &Parsed,
        local_inner: &[u32],
        bound: bool,
    ) -> Vec<Wanted> {
        let mut wanted = Vec::new();
        for (i, inv) in summary.offers.invocations.iter().enumerate() {
            let Some(path) = &inv.path else { continue };
            let table = match (path.root, self.itself) {
                (Some(Root::MacroCrate), _) => {
                    let Some(&table) = self.by_path.get(path) else { continue };
                    let ours = summary.locations.invocation_paths[i]
                        .and_then(|span| parsed.token_at(span.0))
                        .is_some_and(|t| t.mark >= self.top || self.of_crate.contains(&t.mark));
                    if !ours {
                        continue;
                    }
                    Some(table)
                }
                (None, Some(name)) if !bound && path.segments == [name] => None,
                _ => continue,
            };
            let Some((_, input)) = parsed.stream(summary.locations.invocation_span(i)).invocation()
            else {
                continue;
            };
            wanted.push(Wanted {
                invocation: i as u32,
                path: path.clone(),
                table,
                input,
                entry: inv.entry.clone(),
                local_inner: local_inner.to_vec(),
            });
        }
        wanted
    }
}

/// Whether an expansion (`summary`) may bind `name` for an invocation of it
/// that stands after it: it declares something of that name, imports it
/// (or anything, by a glob, or a crate's macros), or declares a module
/// whose body is another file, which may.
fn binds(summary: &Summary, name: &str) -> bool {
    let offers = &summary.offers;
    offers.declarations.iter().any(|d| d.name == name)
        || offers.reexports.iter().chain(&summary.needs.imports).any(|import| {
            import.glob
                || import.loads
                || import.name.as_deref().or(import.path.segments.last().map(String::as_str))
                    == Some(name)
        })
        || offers.units.iter().any(|unit| !unit.inline)
}

/// What maps a span of an expansion's text to the site's.
struct Sites<'a> {
    site: &'a str,
    lines: &'a LineStarts,
    parsed: &'a Parsed,
}

impl<'a> Sites<'a> {
    /// Where the site writes the expansion's span `span`: where every
    /// token of it is the site's, from its first token's start to its
    /// last's end.
    fn place(&self, span: (u32, u32)) -> SitePlace {
        let tokens = &self.parsed.tokens;
        let first = tokens.partition_point(|t| t.span.1 <= span.0);
        let within: Vec<_> =
            tokens[first..].iter().take_while(|t| t.span.0 < span.1.max(span.0 + 1)).collect();
        let (head, tail) = (within.first()?, within.last()?);
        if within.iter().any(|t| t.token.origin.source != 0) {
            return None;
        }
        let (start, end) = (head.token.origin.start, tail.token.origin.end);
        if end < start || end as usize > self.site.len() {
            return None;
        }
        Some(((start, end), self.lines.position(self.site, start)))
    }

    fn placed(&self, l: &Locations) -> Placed {
        let all = |spans: &[(u32, u32)]| spans.iter().map(|&s| self.place(s)).collect();
        Placed {
            names: all(&l.names),
            items: all(&l.items),
            units: all(&l.units),
            unit_items: all(&l.unit_items),
            reexports: all(&l.reexports),
            imports: all(&l.imports),
            impls: all(&l.impls),
            impl_heads: l
                .impl_heads
                .iter()
                .map(|&(ty, tr)| (ty.map(|s| self.place(s)), tr.map(|s| self.place(s))))
                .collect(),
            derives: l.derives.iter().map(|spans| all(spans)).collect(),
            invocations: all(&l.invocations),
            invocation_paths: l.invocation_paths.iter().map(|p| p.map(|s| self.place(s))).collect(),
            invocation_items: all(&l.invocation_items),
            macro_bodies: all(&l.macro_bodies),
        }
    }
}

impl Fnv {
    fn places(&mut self, p: &Placed) {
        let place = |hash: &mut Fnv, place: &SitePlace| match place {
            None => hash.u64(0),
            Some(((s, e), (line, column))) => {
                hash.u64(1);
                for v in [*s, *e, *line, *column] {
                    hash.u64(u64::from(v));
                }
            }
        };
        for list in [
            &p.names,
            &p.items,
            &p.units,
            &p.unit_items,
            &p.reexports,
            &p.imports,
            &p.impls,
            &p.invocations,
            &p.invocation_items,
            &p.macro_bodies,
        ] {
            self.u64(list.len() as u64);
            for x in list {
                place(self, x);
            }
        }
        for (ty, tr) in &p.impl_heads {
            for x in [ty, tr] {
                match x {
                    None => self.u64(2),
                    Some(x) => place(self, x),
                }
            }
        }
        for d in &p.derives {
            self.u64(d.len() as u64);
            for x in d {
                place(self, x);
            }
        }
        for x in &p.invocation_paths {
            match x {
                None => self.u64(2),
                Some(x) => place(self, x),
            }
        }
    }

    /// Values by their `Hash`, written through this hasher.
    pub(crate) fn hashed<T: std::hash::Hash>(&mut self, value: &T) {
        value.hash(self);
    }
}

impl std::hash::Hasher for Fnv {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        self.bytes(bytes);
    }
}
