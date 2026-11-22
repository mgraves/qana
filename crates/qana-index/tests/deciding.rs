//! Deciding conditions: what is there in a crate is what its configuration
//! compiles. Rust's `#[cfg]` twins give one answer, a module file under a
//! condition that does not hold is not there, nor an import, a variant,
//! an impl block's item, a derive or a textual definition under one; a
//! file in two crates is read in each one's configuration. Over a graph
//! that decides nothing ([`UnitGraph::undecided`]), each may be, as before.

use std::path::{Path as FilePath, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, OnceLock};

use qana_index::{
    path_at, summarize, textual_at, Condition, Configuration, CrateInfo, CrateSource, Holds,
    MacroResolution, Path, Resolution, Resolver, Root, Summary, UnitGraph,
};
use qana_lang::live::LiveDoc;
use qana_lang::EmbeddedLang;

const GRAMMAR: &str = include_str!("../../../examples/rust/rust.qana");

fn rust() -> &'static EmbeddedLang {
    static LANG: OnceLock<EmbeddedLang> = OnceLock::new();
    LANG.get_or_init(|| EmbeddedLang::from_qana_source(GRAMMAR).expect("rust.qana certifies"))
}

const LIB: &str = "#[cfg(unix)]
pub fn twin() {}
#[cfg(windows)]
pub fn twin() {}

#[cfg(windows)]
mod win;
#[cfg(unix)]
mod nix;

#[cfg(windows)]
use crate::win::Handle;

pub enum Os {
    #[cfg(windows)]
    Windows,
    Unix,
}

pub trait Debug {}
pub trait Clone {}

#[cfg_attr(windows, derive(Clone))]
#[derive(Debug)]
pub struct Shape;

#[cfg(windows)]
impl Shape {
    pub fn windows_only() {}
}

impl Shape {
    pub fn always() {}
}

#[cfg(doc)]
macro_rules! q { () => {} }
#[cfg(not(doc))]
macro_rules! q { () => {} }
fn use_q() { q!(); }

#[cfg_attr(unix, macro_use)]
mod carried {
    macro_rules! carried_out { () => {} }
}
#[cfg_attr(windows, macro_use)]
mod kept {
    macro_rules! kept_in { () => {} }
}
fn after() { carried_out!(); kept_in!(); }
";

const WIN: &str = "pub struct Handle;\n";
const NIX: &str = "pub struct Fd;\n";

/// Two crates' roots that each hold `shared.rs` as their module `shared`.
const ROOT_A: &str = "mod shared;\n";
const ROOT_B: &str = "mod shared;\n";
const SHARED: &str = "#[cfg(unix)]\npub fn on_unix() {}\npub fn always() {}\n";

const FILES: [(&str, &str); 6] = [
    ("/p/src/lib.rs", LIB),
    ("/p/src/win.rs", WIN),
    ("/p/src/nix.rs", NIX),
    ("/a/lib.rs", ROOT_A),
    ("/b/lib.rs", ROOT_B),
    ("/s/shared.rs", SHARED),
];

fn path(root: Option<Root>, segments: &[&str]) -> Path {
    Path { root, segments: segments.iter().map(|s| s.to_string()).collect() }
}

/// A configuration that sets `set`, deciding `decided`.
fn configured(set: &[&str], decided: &[&str]) -> Configuration {
    Configuration {
        options: set.iter().map(|n| (n.to_string(), None)).collect(),
        decided: decided.iter().map(|n| n.to_string()).collect(),
    }
}

/// What rustc decides on this Mac, as a library is compiled.
fn mac() -> Configuration {
    configured(&["unix"], &["unix", "windows", "doc", "test"])
}

/// What it decides on Windows.
fn windows() -> Configuration {
    configured(&["windows"], &["unix", "windows", "doc", "test"])
}

/// The project: crate `p` (this Mac's), its modules `win` and `nix` in
/// files of their own; crates `a` (this Mac's) and `b` (Windows'), each
/// holding `/s/shared.rs`.
fn graph() -> (UnitGraph, Vec<(PathBuf, Rc<Summary>)>) {
    let summaries: Vec<(PathBuf, Rc<Summary>)> = FILES
        .iter()
        .map(|(file, text)| (PathBuf::from(file), Rc::new(summarize(rust(), text))))
        .collect();
    let unit = |file: &str, name: &str| -> u32 {
        let summary = &summaries.iter().find(|(p, _)| p == FilePath::new(file)).unwrap().1;
        summary.offers().units().iter().position(|u| u.name == name).unwrap() as u32
    };
    let krate = |name: &str, root: &str, configuration: Configuration| CrateInfo {
        name: name.into(),
        kind: "lib".into(),
        root: root.into(),
        edition: "2021".into(),
        dependencies: Vec::new(),
        loadable: Vec::new(),
        source: CrateSource::Workspace,
        preludes: Vec::new(),
        builtins: Vec::new(),
        closed_by: Vec::new(),
        inert: Vec::new(),
        imports_from_root: false,
        unindexed: Vec::new(),
        deref_trait: None,
        try_trait: None,
        derive_traits: Vec::new(),
        macro_use: vec![path(None, &["macro_use"])],
        macro_export: vec![path(None, &["macro_export"])],
        macro_prelude: Vec::new(),
        conditions: vec![path(None, &["cfg"])],
        builtin_attributes: Vec::new(),
        builtin_macros: Vec::new(),
        local_inner_macros: Vec::new(),
        recursion_limit: None,
        configuration,
    };
    let mut graph = UnitGraph::new();
    let p = graph.add_crate(krate("p", "/p/src/lib.rs", mac()));
    for module in ["win", "nix"] {
        let declaring = FilePath::new("/p/src/lib.rs");
        let body = format!("/p/src/{module}.rs").into();
        graph.add_body(p, declaring, unit("/p/src/lib.rs", module), body, vec![module.into()]);
    }
    for (name, root, configuration) in [("a", "/a/lib.rs", mac()), ("b", "/b/lib.rs", windows())] {
        let k = graph.add_crate(krate(name, root, configuration));
        let shared = unit(root, "shared");
        graph.add_body(
            k,
            FilePath::new(root),
            shared,
            "/s/shared.rs".into(),
            vec!["shared".into()],
        );
    }
    (graph, summaries)
}

/// The resolver that decides, and the one that does not.
fn resolvers() -> (Resolver, Resolver) {
    let (graph, summaries) = graph();
    let undecided = graph.undecided();
    (Resolver::new(Rc::new(graph), summaries.clone()), Resolver::new(Rc::new(undecided), summaries))
}

/// What `path`, written at the root of `file`, names: each declaration's
/// file and line, or how it fails.
fn found(resolver: &Resolver, file: &str, path: &Path) -> Vec<String> {
    match resolver.resolve(FilePath::new(file), &[], path, None) {
        Resolution::Found { declarations, modules } => {
            let mut said: Vec<String> = declarations
                .iter()
                .map(|(f, i)| {
                    let text = FILES.iter().find(|(p, _)| FilePath::new(p) == f).unwrap().1;
                    let summary = summarize(rust(), text);
                    let d = &summary.offers().declarations()[*i as usize];
                    let at = summary.locations().name_span(*i as usize).0 as usize;
                    format!("{}:{} {}", f.display(), text[..at].lines().count(), d.name)
                })
                .collect();
            said.extend(modules.iter().map(|m| format!("module {}", m.display())));
            said
        }
        other => vec![format!("{other:?}")],
    }
}

fn named(segments: &[&str]) -> Path {
    path(None, segments)
}

#[test]
fn twins_under_conditions_give_the_one_that_holds() {
    let (decided, undecided) = resolvers();
    let lib = "/p/src/lib.rs";
    assert_eq!(found(&decided, lib, &named(&["twin"])), ["/p/src/lib.rs:2 twin"]);
    assert_eq!(
        found(&undecided, lib, &named(&["twin"])),
        ["/p/src/lib.rs:2 twin", "/p/src/lib.rs:4 twin"],
        "undecided, either may be"
    );
}

#[test]
fn a_module_file_and_an_import_under_a_condition_that_does_not_hold_are_not_there() {
    let (decided, undecided) = resolvers();
    let lib = "/p/src/lib.rs";
    let handle = path(Some(Root::Crate), &["win", "Handle"]);
    assert_ne!(found(&decided, lib, &handle), ["/p/src/win.rs:1 Handle"]);
    assert_eq!(found(&undecided, lib, &handle), ["/p/src/win.rs:1 Handle"]);
    let fd = path(Some(Root::Crate), &["nix", "Fd"]);
    assert_eq!(found(&decided, lib, &fd), ["/p/src/nix.rs:1 Fd"]);
    assert_ne!(
        found(&decided, lib, &named(&["Handle"])),
        ["/p/src/win.rs:1 Handle"],
        "nor the import of it"
    );
    assert_eq!(found(&undecided, lib, &named(&["Handle"])), ["/p/src/win.rs:1 Handle"]);
}

#[test]
fn each_place_holds_where_its_file_and_its_regions_do() {
    let (decided, undecided) = resolvers();
    let windows = Condition::Option("windows".into());
    assert_eq!(decided.holds_at(FilePath::new("/p/src/win.rs"), None), Holds::No);
    assert_eq!(decided.holds_at(FilePath::new("/p/src/nix.rs"), None), Holds::Yes);
    assert_eq!(decided.holds_at(FilePath::new("/p/src/lib.rs"), Some(&windows)), Holds::No);
    assert_eq!(decided.holds_at(FilePath::new("/p/src/lib.rs"), None), Holds::Yes);
    let loom = Condition::Option("loom".into());
    assert_eq!(decided.holds_at(FilePath::new("/p/src/lib.rs"), Some(&loom)), Holds::Maybe);
    assert_eq!(
        decided.holds_at(FilePath::new("/s/shared.rs"), Some(&windows)),
        Holds::Yes,
        "in one of its crates"
    );
    assert_eq!(undecided.holds_at(FilePath::new("/p/src/win.rs"), None), Holds::Maybe);
    assert_eq!(decided.holds_at(FilePath::new("/nowhere.rs"), None), Holds::Maybe);
}

#[test]
fn a_variant_an_impl_blocks_item_and_a_derive_under_one_are_not_there() {
    let (decided, undecided) = resolvers();
    let lib = "/p/src/lib.rs";
    let windows_variant = named(&["Os", "Windows"]);
    assert!(!found(&decided, lib, &windows_variant)[0].contains("Windows"));
    assert_eq!(found(&undecided, lib, &windows_variant), ["/p/src/lib.rs:16 Windows"]);
    assert_eq!(found(&decided, lib, &named(&["Os", "Unix"])), ["/p/src/lib.rs:17 Unix"]);
    let windows_only = named(&["Shape", "windows_only"]);
    assert!(!found(&decided, lib, &windows_only)[0].contains("windows_only"));
    assert_eq!(found(&undecided, lib, &windows_only), ["/p/src/lib.rs:29 windows_only"]);
    assert_eq!(found(&decided, lib, &named(&["Shape", "always"])), ["/p/src/lib.rs:33 always"]);

    let shape = summarize(rust(), LIB)
        .offers()
        .declarations()
        .iter()
        .position(|d| d.name == "Shape")
        .unwrap() as u32;
    let traits = |r: &Resolver| -> Vec<usize> {
        r.derived_traits(FilePath::new(lib), shape).iter().map(Vec::len).collect()
    };
    assert_eq!(traits(&decided), [0, 1], "`Clone` derived only on Windows; `Debug` here");
    assert_eq!(traits(&undecided), [1, 1]);
}

/// What `name!`, the `nth` invocation of it in [`LIB`], names: a
/// definition there by its line, each.
fn macro_named(resolver: &Resolver, name: &str, nth: usize) -> Vec<usize> {
    let file = "/p/src/lib.rs";
    let call = format!("{name}!");
    let at = LIB.match_indices(&call).nth(nth).expect("the invocation").0;
    let offset = (at + name.len() - 1) as u32;
    let mut doc = LiveDoc::open(Arc::new(rust().clone()), file, LIB);
    let (units, path, _) = path_at(&mut doc, offset).expect("a path");
    let seen = textual_at(&mut doc, offset, "macro").expect("a textual name");
    let line = |start: u32| LIB[..start as usize].lines().count();
    match resolver.resolve_macro(FilePath::new(file), &units, &path, "macro", &seen) {
        MacroResolution::Here((start, _)) => vec![line(start)],
        MacroResolution::Several { here, .. } => here.iter().map(|(s, _)| line(*s)).collect(),
        MacroResolution::Elsewhere(_) => Vec::new(),
    }
}

#[test]
fn a_textual_definition_that_does_not_hold_is_not_there_and_one_that_does_hides_the_rest() {
    let (decided, undecided) = resolvers();
    assert_eq!(macro_named(&decided, "q", 0), [39], "the `not(doc)` one alone");
    assert_eq!(macro_named(&undecided, "q", 0), [39, 37], "undecided, each may be");
    assert_eq!(macro_named(&decided, "carried_out", 0), [44], "carried out where `unix` holds");
    assert_eq!(macro_named(&decided, "kept_in", 0), Vec::<usize>::new(), "not where `windows`");
    assert_eq!(macro_named(&undecided, "kept_in", 0), [48], "undecided, it may be");
}

#[test]
fn a_file_in_two_crates_is_read_in_each_ones_configuration() {
    let (decided, _) = resolvers();
    let on_unix = path(Some(Root::Crate), &["shared", "on_unix"]);
    assert_eq!(found(&decided, "/a/lib.rs", &on_unix), ["/s/shared.rs:2 on_unix"]);
    assert_ne!(found(&decided, "/b/lib.rs", &on_unix), ["/s/shared.rs:2 on_unix"]);
    let always = path(Some(Root::Crate), &["shared", "always"]);
    assert_eq!(found(&decided, "/b/lib.rs", &always), ["/s/shared.rs:3 always"]);
}

/// As quote writes `quote!`: twice, each by an invocation under a
/// condition, exported.
const QUOTE: &str = "macro_rules! def { ($($t:tt)*) => { $($t)* }; }
#[cfg(doc)]
def! { #[macro_export] macro_rules! quote { (doc) => {} } }
#[cfg(not(doc))]
def! { #[macro_export] macro_rules! quote { () => {} } }
";

#[test]
fn what_an_invocation_under_a_condition_that_does_not_hold_defines_is_not_there() {
    use qana_index::{expand, merge, Edition, ExpandRequest, Form, MacroSource, RECURSION_LIMIT};
    let own = summarize(rust(), QUOTE);
    let def = " { ($($t:tt)*) => { $($t)* }; }";
    let expansions: Vec<_> = (0..own.offers().invocations().len())
        .map(|i| {
            let span = own.locations().invocation_span(i);
            let request = ExpandRequest {
                definition: MacroSource::Text {
                    body: def.into(),
                    form: Form::MacroRules,
                    edition: Edition::E2021,
                    local_inner: false,
                },
                site: QUOTE[span.0 as usize..span.1 as usize].to_string(),
                site_edition: Edition::E2021,
                input: None,
                entry: "items_opt".into(),
                mark: 1,
                limit: RECURSION_LIMIT as u32,
                local_inner: Vec::new(),
                of_crate: Vec::new(),
                itself: None,
                names: Vec::new(),
                krate: None,
            };
            Rc::new(expand(rust(), &request, &[]))
        })
        .collect();
    let by_index =
        |_: &[(u32, u32)], index: u32| Some(vec![(0, expansions[index as usize].clone(), None)]);
    let merged = Rc::new(merge(&own, &by_index));
    let mut graph = UnitGraph::new();
    let krate = |configuration: Configuration| CrateInfo {
        name: "quote".into(),
        kind: "lib".into(),
        root: "/q/lib.rs".into(),
        edition: "2021".into(),
        dependencies: Vec::new(),
        loadable: Vec::new(),
        source: CrateSource::Workspace,
        preludes: Vec::new(),
        builtins: Vec::new(),
        closed_by: Vec::new(),
        inert: Vec::new(),
        imports_from_root: false,
        unindexed: Vec::new(),
        deref_trait: None,
        try_trait: None,
        derive_traits: Vec::new(),
        macro_use: vec![path(None, &["macro_use"])],
        macro_export: vec![path(None, &["macro_export"])],
        macro_prelude: Vec::new(),
        conditions: vec![path(None, &["cfg"])],
        builtin_attributes: Vec::new(),
        builtin_macros: Vec::new(),
        local_inner_macros: Vec::new(),
        recursion_limit: None,
        configuration,
    };
    graph.add_crate(krate(mac()));
    let files = vec![(PathBuf::from("/q/lib.rs"), merged.clone())];
    let decided = Resolver::new(Rc::new(graph.clone()), files.clone());
    let undecided = Resolver::new(Rc::new(graph.undecided()), files);
    let quote = path(Some(Root::Crate), &["quote"]);
    let answers = |r: &Resolver| -> Vec<String> {
        match r.resolve(FilePath::new("/q/lib.rs"), &[], &quote, Some("macro")) {
            Resolution::Found { declarations, .. } => declarations
                .iter()
                .map(|(_, i)| {
                    let d = &merged.offers().declarations()[*i as usize];
                    d.condition.as_ref().map_or("none".to_string(), |c| c.to_string())
                })
                .collect(),
            other => vec![format!("{other:?}")],
        }
    };
    assert_eq!(answers(&decided), ["not(doc)"], "the one compiled");
    assert_eq!(answers(&undecided), ["doc", "not(doc)"]);
}

#[test]
fn a_crate_no_definition_places_escapes() {
    // `quote!` read through its definition (not its `cfg(doc)` stub)
    // writes `$crate::__private::…` from a body an expansion wrote: where
    // the index cannot say which crate that is, the path is no name it can
    // judge.
    let (decided, _) = resolvers();
    let path = path(Some(Root::MacroCrate), &["__private", "TokenStream"]);
    assert_eq!(
        decided.resolve(FilePath::new("/p/src/lib.rs"), &[], &path, Some("")),
        Resolution::Escapes { segment: 0 }
    );
}
