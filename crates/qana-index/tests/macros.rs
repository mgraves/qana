//! Macros' names, as rustc finds them: textual scope (in a file, in the
//! modules declared after a definition, out of a `#[macro_use]` module,
//! not out of a block), `#[macro_export]` at the crate's root, the macro
//! prelude (`#[macro_use] extern crate`, std's), and a path; a `macro`
//! item, which is an item of its module.

use std::path::{Path as FilePath, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, OnceLock};

use qana_index::{
    path_at, references, summarize, textual_at, CrateInfo, MacroResolution, Path, Resolution,
    Resolver, Root, Summary, UnitGraph,
};
use qana_lang::live::LiveDoc;
use qana_lang::EmbeddedLang;

const GRAMMAR: &str = include_str!("../../../examples/rust/rust.qana");

fn rust() -> &'static EmbeddedLang {
    static LANG: OnceLock<EmbeddedLang> = OnceLock::new();
    LANG.get_or_init(|| EmbeddedLang::from_qana_source(GRAMMAR).expect("rust.qana certifies"))
}

const LIB: &str = "#[macro_use]
extern crate dep;

macro_rules! early { () => {} }

#[macro_use]
mod macros;

mod a;

#[macro_use]
mod inline {
    macro_rules! from_inline { () => {} }
}

mod b;

fn too_early() { late!(); }

macro_rules! late { () => {} }

fn after() { late!(); }

macro_rules! twice { (a) => {} }
fn first() { twice!(a); }
macro_rules! twice { (b) => {} }
fn second() { twice!(b); }

fn block() {
    macro_rules! local { () => {} }
    local!();
}
fn past_the_block() { local!(); }

mod quiet {
    macro_rules! kept_in { () => {} }
}
fn past_the_module() { kept_in!(); }

fn before_the_item() { by_path!(); }
pub macro by_path { () => {} }

#[cfg(feature = \"a\")]
macro_rules! either { () => {} }
#[cfg(not(feature = \"a\"))]
macro_rules! either { () => {} }
fn both() { either!(); }

macro_rules! sure { () => {} }
#[cfg(test)]
macro_rules! sure { () => {} }
fn sure_or_test() { sure!(); }

macro_rules! tested { () => {} }
#[cfg(test)]
#[macro_use]
mod testing {
    macro_rules! tested { () => {} }
}
fn tested_or_not() { tested!(); }
";

const MACROS: &str = "macro_rules! from_macros { () => {} }

#[macro_export]
macro_rules! exported { () => {} }
";

const A: &str = "fn uses() {
    early!();
    from_macros!();
    late!();
    crate::exported!();
    exported!();
    dep::dep_mac!();
    dep_mac!();
    vec!();
    from_inline!();
    crate::by_path!();
    by_path!();
}
";

const B: &str = "fn uses() { from_inline!(); early!(); }
";

const DEP: &str = "#[macro_export]
macro_rules! dep_mac { () => {} }

#[macro_export]
macro_rules! dep_other { () => {} }
";

/// A crate that loads one of dep's macros by name, and says which to carry
/// out of a module, which rustc refuses.
const PICKY: &str = "#[macro_use(dep_mac)]
extern crate dep;

fn f() { dep_mac!(); dep_other!(); }

#[macro_use(kept_in)]
mod quiet {
    macro_rules! kept_in { () => {} }
}
fn g() { kept_in!(); }
";

const STD: &str = "pub use alloc::vec;
";

const ALLOC: &str = "#[macro_use]
mod macros;
";

const ALLOC_MACROS: &str = "#[macro_export]
macro_rules! vec { () => {} }
";

const FILES: &[(&str, &str)] = &[
    ("/p/src/lib.rs", LIB),
    ("/p/src/macros.rs", MACROS),
    ("/p/src/a.rs", A),
    ("/p/src/b.rs", B),
    ("/d/lib.rs", DEP),
    ("/q/lib.rs", PICKY),
    ("/s/std/lib.rs", STD),
    ("/s/alloc/lib.rs", ALLOC),
    ("/s/alloc/macros.rs", ALLOC_MACROS),
];

fn path(root: Option<Root>, segments: &[&str]) -> Path {
    Path { root, segments: segments.iter().map(|s| s.to_string()).collect() }
}

fn resolver() -> Resolver {
    let summaries: Vec<(PathBuf, Rc<Summary>)> = FILES
        .iter()
        .map(|(file, text)| (PathBuf::from(file), Rc::new(summarize(rust(), text))))
        .collect();
    let unit = |file: &str, name: &str| -> u32 {
        let summary = &summaries.iter().find(|(p, _)| p == FilePath::new(file)).unwrap().1;
        summary.offers().units().iter().position(|u| u.name == name).unwrap() as u32
    };
    let krate = |name: &str, root: &str, dependencies: Vec<(String, u32)>| CrateInfo {
        name: name.into(),
        kind: "lib".into(),
        root: root.into(),
        edition: "2021".into(),
        dependencies,
        loadable: Vec::new(),
        source: qana_index::CrateSource::Workspace,
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
        macro_prelude: vec![path(Some(Root::Global), &["std"])],
        conditions: vec![path(None, &["cfg"])],
        builtin_attributes: Vec::new(),
        builtin_macros: Vec::new(),
        local_inner_macros: Vec::new(),
        recursion_limit: None,
    };
    let mut graph = UnitGraph::new();
    let alloc = graph.add_crate(krate("alloc", "/s/alloc/lib.rs", Vec::new()));
    let std = graph.add_crate(krate("std", "/s/std/lib.rs", vec![("alloc".into(), alloc)]));
    let dep = graph.add_crate(krate("dep", "/d/lib.rs", vec![("std".into(), std)]));
    let app = graph.add_crate(krate(
        "app",
        "/p/src/lib.rs",
        vec![("dep".into(), dep), ("std".into(), std)],
    ));
    graph.add_crate(krate("picky", "/q/lib.rs", vec![("dep".into(), dep)]));
    graph.add_body(
        alloc,
        FilePath::new("/s/alloc/lib.rs"),
        unit("/s/alloc/lib.rs", "macros"),
        "/s/alloc/macros.rs".into(),
        vec!["macros".into()],
    );
    for module in ["macros", "a", "b"] {
        graph.add_body(
            app,
            FilePath::new("/p/src/lib.rs"),
            unit("/p/src/lib.rs", module),
            format!("/p/src/{module}.rs").into(),
            vec![module.into()],
        );
    }
    Resolver::new(Rc::new(graph), summaries)
}

/// What the `nth` invocation of `name!` in `file` names: a definition in
/// the same file (`here`, with the line it is on), declarations elsewhere
/// (file and name), or nothing.
fn named(resolver: &Resolver, file: &str, name: &str, nth: usize) -> Vec<String> {
    let text = FILES.iter().find(|(f, _)| *f == file).unwrap().1;
    let call = format!("{name}!");
    let at = text.match_indices(&call).nth(nth).expect("the invocation").0;
    // The path's last name, where the cursor would be.
    let offset = (at + name.len() - 1) as u32;
    let mut doc = LiveDoc::open(Arc::new(rust().clone()), file, text);
    let (units, path, _) = path_at(&mut doc, offset).expect("a path");
    let seen = textual_at(&mut doc, offset, "macro").expect("a textual name");
    match resolver.resolve_macro(FilePath::new(file), &units, &path, "macro", &seen) {
        MacroResolution::Here((start, _)) => {
            let line = text[..start as usize].lines().count();
            vec![format!("here, line {line}")]
        }
        MacroResolution::Elsewhere(Resolution::Found { declarations, .. }) => declarations
            .iter()
            .map(|(file, i)| {
                let summary = summarize(
                    rust(),
                    FILES.iter().find(|(f, _)| FilePath::new(f) == file).unwrap().1,
                );
                format!("{}: {}", file.display(), summary.offers().declarations()[*i as usize].name)
            })
            .collect(),
        MacroResolution::Elsewhere(other) => vec![format!("{other:?}")],
        MacroResolution::Several { here, declarations } => {
            let mut said: Vec<String> = here
                .iter()
                .map(|(start, _)| format!("here, line {}", text[..*start as usize].lines().count()))
                .collect();
            said.extend(declarations.iter().map(|(file, i)| {
                let summary = summarize(
                    rust(),
                    FILES.iter().find(|(f, _)| FilePath::new(f) == file).unwrap().1,
                );
                format!("{}: {}", file.display(), summary.offers().declarations()[*i as usize].name)
            }));
            said
        }
    }
}

#[test]
fn only_a_namespace_textual_definitions_are_in_is_read_by_textual_scope() {
    let mut doc = LiveDoc::open(Arc::new(rust().clone()), "/p/src/lib.rs", LIB);
    assert!(textual_at(&mut doc, 0, "macro").is_some());
    assert!(textual_at(&mut doc, 0, "").is_none(), "the default namespace's names are not");
}

#[test]
fn a_macro_is_seen_after_its_definition_in_its_file_to_the_end_of_its_block() {
    let r = resolver();
    let lib = "/p/src/lib.rs";
    assert_eq!(named(&r, lib, "late", 1), ["here, line 20"], "after it");
    assert_eq!(
        named(&r, lib, "late", 0),
        ["Escapes { segment: 0 }"],
        "not before it: a textual definition is not among its module's names"
    );
    assert_eq!(named(&r, lib, "twice", 0), ["here, line 24"], "the latest before");
    assert_eq!(named(&r, lib, "twice", 1), ["here, line 26"], "shadowing the first");
    assert_eq!(named(&r, lib, "local", 0), ["here, line 30"], "in its block");
    assert_eq!(named(&r, lib, "local", 1), ["Escapes { segment: 0 }"], "not after the block");
    assert_eq!(
        named(&r, lib, "kept_in", 0),
        ["Escapes { segment: 0 }"],
        "nor after a module that does not carry it out"
    );
}

#[test]
fn a_module_sees_what_its_declaration_sees_and_what_macro_use_carries_out() {
    let r = resolver();
    let a = "/p/src/a.rs";
    assert_eq!(named(&r, a, "early", 0), ["/p/src/lib.rs: early"], "defined before `mod a;`");
    assert_eq!(
        named(&r, a, "from_macros", 0),
        ["/p/src/macros.rs: from_macros"],
        "carried out of `#[macro_use] mod macros;`, declared before"
    );
    assert_eq!(named(&r, a, "late", 0), ["Escapes { segment: 0 }"], "defined after `mod a;`");
    assert_eq!(
        named(&r, a, "from_inline", 0),
        ["Escapes { segment: 0 }"],
        "an inline `#[macro_use]` module after `mod a;`"
    );
    let b = "/p/src/b.rs";
    assert_eq!(
        named(&r, b, "from_inline", 0),
        ["/p/src/lib.rs: from_inline"],
        "and before `mod b;`"
    );
    assert_eq!(named(&r, b, "early", 0), ["/p/src/lib.rs: early"]);
}

#[test]
fn an_exported_macro_is_at_its_crates_root_and_the_macro_prelude_has_std_and_macro_use_crates() {
    let r = resolver();
    let a = "/p/src/a.rs";
    assert_eq!(named(&r, a, "exported", 0), ["/p/src/macros.rs: exported"], "`crate::exported!`");
    assert_eq!(
        named(&r, a, "exported", 1),
        ["/p/src/macros.rs: exported"],
        "carried out of `macros` too"
    );
    assert_eq!(named(&r, a, "dep_mac", 0), ["/d/lib.rs: dep_mac"], "`dep::dep_mac!`");
    assert_eq!(
        named(&r, a, "dep_mac", 1),
        ["/d/lib.rs: dep_mac"],
        "by `#[macro_use] extern crate dep;`"
    );
    assert_eq!(
        named(&r, a, "vec", 0),
        ["/s/alloc/macros.rs: vec"],
        "std's, which std's root imports from alloc's"
    );
}

#[test]
fn a_crates_root_names_its_exported_macros_its_macro_items_and_what_it_imports() {
    let r = resolver();
    // The crates, as `resolver` adds them.
    let (alloc, std, app) = (0u32, 1u32, 3u32);
    // Each name, and what it names: (file, declared name, crate).
    type Named = Vec<(String, Vec<(String, String, u32)>)>;
    let named = |krate: u32| -> Named {
        r.root_macros(krate, "macro")
            .iter()
            .map(|(name, macros)| {
                let macros = macros
                    .iter()
                    .map(|(file, i, k)| {
                        let text = FILES.iter().find(|(f, _)| FilePath::new(f) == file).unwrap().1;
                        let summary = summarize(rust(), text);
                        let declared = summary.offers().declarations()[*i as usize].name.clone();
                        (file.display().to_string(), declared, *k)
                    })
                    .collect();
                (name.clone(), macros)
            })
            .collect()
    };
    let at = |file: &str, name: &str, k: u32| (file.to_string(), name.to_string(), k);
    assert_eq!(
        named(app),
        [
            ("by_path".to_string(), vec![at("/p/src/lib.rs", "by_path", app)]),
            ("exported".to_string(), vec![at("/p/src/macros.rs", "exported", app)]),
        ],
        "`$crate::exported!` from anywhere in the crate, `$crate::by_path!`; no textual \
         definition it does not export"
    );
    assert_eq!(
        named(std),
        [("vec".to_string(), vec![at("/s/alloc/macros.rs", "vec", alloc)])],
        "what its root imports, alloc's"
    );
}

#[test]
fn a_macros_references_say_what_they_see_and_resolve_as_its_invocations_do() {
    let r = resolver();
    let lib = "/p/src/lib.rs";
    let found = references(rust(), LIB, &["twice".to_string(), "late".to_string()]);
    let named: Vec<String> = found
        .iter()
        .filter(|f| f.namespace == "macro")
        .map(|f| {
            let seen = f.textual.as_ref().expect("a macro's reference sees by textual scope");
            let path = f.path.as_ref().expect("its path");
            let name = &LIB[f.span.0 as usize..f.span.1 as usize];
            match r.resolve_macro(FilePath::new(lib), &f.units, path, "macro", seen) {
                MacroResolution::Here((start, _)) => {
                    format!("{name} {}: line {}", f.line + 1, LIB[..start as usize].lines().count())
                }
                MacroResolution::Elsewhere(other) => format!("{name} {}: {other:?}", f.line + 1),
                several => format!("{name} {}: {several:?}", f.line + 1),
            }
        })
        .collect();
    assert_eq!(
        named,
        [
            "late 18: Escapes { segment: 0 }",
            "late 22: line 20",
            "twice 25: line 24",
            "twice 27: line 26",
        ]
    );
    let other = references(rust(), LIB, &["block".to_string()]);
    assert!(
        other.iter().all(|f| f.textual.is_none()),
        "a name in the default namespace sees by its binding"
    );
}

#[test]
fn in_a_file_no_crate_holds_a_macro_is_a_definition_before_it_there() {
    let r = resolver();
    let file = "/loose.rs";
    let text = "macro_rules! here { () => {} }\nfn f() { here!(); there!(); }\nmacro_rules! there { () => {} }\n";
    let mut doc = LiveDoc::open(Arc::new(rust().clone()), file, text);
    let mut named = |call: &str| {
        let offset = text.find(call).expect("the invocation") as u32;
        let (units, path, _) = path_at(&mut doc, offset).expect("a path");
        let seen = textual_at(&mut doc, offset, "macro").expect("a textual name");
        r.resolve_macro(FilePath::new(file), &units, &path, "macro", &seen)
    };
    assert_eq!(named("here!"), MacroResolution::Here((13, 17)));
    assert_eq!(
        named("there!"),
        MacroResolution::Elsewhere(Resolution::Escapes { segment: 0 }),
        "not one after it"
    );
}

#[test]
fn a_macro_item_is_an_item_of_its_module_found_by_path() {
    let r = resolver();
    assert_eq!(
        named(&r, "/p/src/lib.rs", "by_path", 0),
        ["/p/src/lib.rs: by_path"],
        "in its module, before it too"
    );
    let a = "/p/src/a.rs";
    assert_eq!(named(&r, a, "by_path", 0), ["/p/src/lib.rs: by_path"], "`crate::by_path!`");
    assert_eq!(
        named(&r, a, "by_path", 1),
        ["Escapes { segment: 0 }"],
        "not in a module declared after it: no textual scope"
    );
}

#[test]
fn macro_use_with_names_loads_those_alone_and_carries_nothing_out_of_a_module() {
    let r = resolver();
    let picky = "/q/lib.rs";
    assert_eq!(named(&r, picky, "dep_mac", 0), ["/d/lib.rs: dep_mac"], "named");
    assert_eq!(named(&r, picky, "dep_other", 0), ["Escapes { segment: 0 }"], "not named");
    assert_eq!(
        named(&r, picky, "kept_in", 0),
        ["Escapes { segment: 0 }"],
        "rustc refuses arguments on a module's `#[macro_use]`"
    );
}

#[test]
fn a_definition_under_a_condition_does_not_hide_an_earlier_one() {
    let r = resolver();
    let lib = "/p/src/lib.rs";
    let line = |found: &str| {
        let at = LIB.find(found).expect("in the file");
        format!("here, line {}", LIB[..at].matches('\n').count() + 1)
    };
    assert_eq!(
        named(&r, lib, "either", 0),
        [line("macro_rules! either { () => {} }\nfn"), line("macro_rules! either")],
        "each under its own `#[cfg]`: either may be"
    );
    assert_eq!(
        named(&r, lib, "sure", 0),
        [line("macro_rules! sure { () => {} }\nfn"), line("macro_rules! sure")],
        "the one under `#[cfg(test)]`, then the one that surely is, and no further"
    );
    assert_eq!(
        named(&r, lib, "tested", 0),
        [line("macro_rules! tested { () => {} }\n}"), line("macro_rules! tested")],
        "out of a module under a condition"
    );
}
