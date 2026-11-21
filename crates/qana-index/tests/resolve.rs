//! Resolving paths across the files of a small Unitlang project: two
//! crates, modules in files and inline, re-exports, globs and visibility,
//! roots, impl blocks' members and a type's own, a cycle of globs, and
//! what escapes.

use std::path::{Path as FilePath, PathBuf};
use std::rc::Rc;
use std::sync::OnceLock;

use qana_index::{summarize, CrateInfo, Path, Resolution, Resolver, Root, Summary, UnitGraph};
use qana_lang::EmbeddedLang;

const UNITLANG: &str = include_str!("../../../examples/units/unitlang.qana");

fn unitlang() -> &'static EmbeddedLang {
    static LANG: OnceLock<EmbeddedLang> = OnceLock::new();
    LANG.get_or_init(|| EmbeddedLang::from_qana_source(UNITLANG).expect("unitlang.qana certifies"))
}

const LIB: &str = "mod shapes;

pub mod util {
    pub fn clamp() {}
    fn hidden() {}
}

pub use shapes::Circle;
pub use self::util::*;
use crate::shapes::{self, area::*};

struct Point {
    pub x: Num;
}

impl Point {
    pub fn new() {}
}

impl Display for Point {
    fn fmt() {}
}

pub mod a {
    pub use super::b::*;
    pub fn in_a() {}
}

pub mod b {
    pub use super::a::*;
    pub fn in_b() {}
}

fn main() {
    crate::util::clamp;
}
";

const SHAPES: &str = "pub mod area;

pub struct Circle {
}

pub fn round() {
    super::util::clamp;
}
";

const AREA: &str = "pub fn area() {}\n";

const APP: &str = "fn main() {\n    geo::Circle;\n}\n";

/// The project: crate `geo` rooted at `lib.ul`, with `shapes` and
/// `shapes/area` in files of their own, and crate `app`, which names it.
fn project() -> Resolver {
    let files: Vec<(&str, &str)> = vec![
        ("/p/src/lib.ul", LIB),
        ("/p/src/shapes.ul", SHAPES),
        ("/p/src/shapes/area.ul", AREA),
        ("/p/app.ul", APP),
    ];
    let summaries: Vec<(PathBuf, Rc<Summary>)> = files
        .iter()
        .map(|(path, text)| (PathBuf::from(path), Rc::new(summarize(unitlang(), text))))
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
    };
    let mut graph = UnitGraph::new();
    let geo = graph.add_crate(krate("geo", "/p/src/lib.ul", Vec::new()));
    graph.add_crate(krate("app", "/p/app.ul", vec![("geo".into(), geo)]));
    graph.add_body(
        geo,
        FilePath::new("/p/src/lib.ul"),
        unit("/p/src/lib.ul", "shapes"),
        "/p/src/shapes.ul".into(),
        vec!["shapes".into()],
    );
    graph.add_body(
        geo,
        FilePath::new("/p/src/shapes.ul"),
        unit("/p/src/shapes.ul", "area"),
        "/p/src/shapes/area.ul".into(),
        vec!["shapes".into(), "area".into()],
    );
    Resolver::new(Rc::new(graph), summaries)
}

fn path(root: Option<Root>, segments: &[&str]) -> Path {
    Path { root, segments: segments.iter().map(|s| s.to_string()).collect() }
}

/// Declarations as (file, name) and module files, or the segment a path
/// escaped at.
type Named = Result<(Vec<(String, String)>, Vec<String>), usize>;

/// What `path` names from `file`'s own unit, said as (file, declaration
/// name) and module files.
fn names(resolver: &Resolver, file: &str, path: &Path) -> Named {
    match resolver.resolve(FilePath::new(file), &[], path, Some("")) {
        Resolution::Found { declarations, modules } => Ok((
            declarations
                .into_iter()
                .map(|(file, index)| {
                    let text = match file.to_str().unwrap() {
                        "/p/src/lib.ul" => LIB,
                        "/p/src/shapes.ul" => SHAPES,
                        "/p/src/shapes/area.ul" => AREA,
                        _ => APP,
                    };
                    let summary = summarize(unitlang(), text);
                    let name = summary.offers().declarations()[index as usize].name.clone();
                    (file.file_name().unwrap().to_string_lossy().into_owned(), name)
                })
                .collect(),
            modules
                .into_iter()
                .map(|m| m.file_name().unwrap().to_string_lossy().into_owned())
                .collect(),
        )),
        Resolution::Escapes { segment } | Resolution::Unresolved { segment } => Err(segment),
        Resolution::Builtin => panic!("{path:?}: this project has no builtins"),
    }
}

fn decl(file: &str, name: &str) -> (String, String) {
    (file.into(), name.into())
}

#[test]
fn a_path_from_the_crates_root_goes_through_its_modules() {
    let r = project();
    assert_eq!(
        names(&r, "/p/src/lib.ul", &path(Some(Root::Crate), &["util", "clamp"])),
        Ok((vec![decl("lib.ul", "clamp")], vec![]))
    );
    assert_eq!(
        names(&r, "/p/src/lib.ul", &path(Some(Root::Crate), &["shapes", "area", "area"])),
        Ok((vec![decl("area.ul", "area")], vec![])),
        "into the files the graph places"
    );
}

#[test]
fn a_module_with_a_file_of_its_own_is_its_file() {
    let r = project();
    assert_eq!(
        names(&r, "/p/src/lib.ul", &path(None, &["shapes"])),
        Ok((vec![], vec!["shapes.ul".into()])),
        "its `mod` and the `self` imported in a group are the one module"
    );
    assert_eq!(
        names(&r, "/p/src/lib.ul", &path(None, &["util"])),
        Ok((vec![decl("lib.ul", "util")], vec![])),
        "an inline one is its declaration"
    );
}

#[test]
fn super_goes_up_from_a_files_unit_to_the_one_that_declares_it() {
    let r = project();
    assert_eq!(
        names(&r, "/p/src/shapes.ul", &path(Some(Root::Unit(1)), &["util", "clamp"])),
        Ok((vec![decl("lib.ul", "clamp")], vec![]))
    );
    assert_eq!(
        r.resolve(
            FilePath::new("/p/src/lib.ul"),
            &[],
            &path(Some(Root::Unit(1)), &["x"]),
            Some("")
        ),
        Resolution::Escapes { segment: 0 },
        "a crate's root has no parent"
    );
}

#[test]
fn imports_and_reexports_name_what_their_paths_do() {
    let r = project();
    assert_eq!(
        names(&r, "/p/src/lib.ul", &path(None, &["Circle"])),
        Ok((vec![decl("shapes.ul", "Circle")], vec![]))
    );
    assert_eq!(
        names(&r, "/p/app.ul", &path(None, &["geo", "Circle"])),
        Ok((vec![decl("shapes.ul", "Circle")], vec![])),
        "another crate, through the re-export"
    );
    assert_eq!(
        names(&r, "/p/app.ul", &path(Some(Root::Global), &["geo", "shapes", "round"])),
        Ok((vec![decl("shapes.ul", "round")], vec![])),
        "a path is followed into what it may not see: the compiler says the rest"
    );
}

#[test]
fn a_glob_brings_what_the_globbed_module_lets_it_see() {
    let r = project();
    assert_eq!(
        names(&r, "/p/app.ul", &path(None, &["geo", "clamp"])),
        Ok((vec![decl("lib.ul", "clamp")], vec![]))
    );
    assert_eq!(
        names(&r, "/p/app.ul", &path(None, &["geo", "hidden"])),
        Err(1),
        "not a private one"
    );
    assert_eq!(
        names(&r, "/p/src/lib.ul", &path(None, &["area"])),
        Ok((vec![decl("area.ul", "area")], vec![])),
        "a glob in a group"
    );
}

#[test]
fn a_types_members_are_its_own_and_its_impl_blocks_items() {
    let r = project();
    assert_eq!(
        names(&r, "/p/src/lib.ul", &path(None, &["Point", "new"])),
        Ok((vec![decl("lib.ul", "new")], vec![]))
    );
    assert_eq!(
        names(&r, "/p/src/lib.ul", &path(None, &["Point", "fmt"])),
        Ok((vec![decl("lib.ul", "fmt")], vec![])),
        "a trait's impl block too"
    );
    assert_eq!(
        names(&r, "/p/src/lib.ul", &path(None, &["Point", "x"])),
        Ok((vec![decl("lib.ul", "x")], vec![]))
    );
    assert_eq!(names(&r, "/p/src/lib.ul", &path(None, &["Point", "gone"])), Err(1));
}

#[test]
fn globs_in_a_cycle_find_what_is_in_it_and_end() {
    let r = project();
    assert_eq!(
        names(&r, "/p/src/lib.ul", &path(None, &["a", "in_b"])),
        Ok((vec![decl("lib.ul", "in_b")], vec![]))
    );
    assert_eq!(
        names(&r, "/p/src/lib.ul", &path(None, &["b", "in_a"])),
        Ok((vec![decl("lib.ul", "in_a")], vec![]))
    );
    assert_eq!(names(&r, "/p/src/lib.ul", &path(None, &["a", "nowhere"])), Err(1));
    assert_eq!(
        names(&r, "/p/src/lib.ul", &path(None, &["b", "in_b"])),
        Ok((vec![decl("lib.ul", "in_b")], vec![])),
        "and what a cut-short lookup found is not kept short"
    );
}

#[test]
fn what_is_not_in_the_project_escapes_where_it_leaves() {
    let r = project();
    assert_eq!(names(&r, "/p/src/lib.ul", &path(None, &["std", "fmt"])), Err(0));
    assert_eq!(names(&r, "/p/src/lib.ul", &path(Some(Root::Crate), &["util", "gone"])), Err(1));
    assert_eq!(
        r.resolve(FilePath::new("/p/elsewhere.ul"), &[], &path(None, &["x"]), Some("")),
        Resolution::Escapes { segment: 0 },
        "a file in no crate"
    );
}

#[test]
fn a_path_inside_an_inline_unit_starts_there() {
    let r = project();
    let in_util = r.resolve(
        FilePath::new("/p/src/lib.ul"),
        &["util".to_string()],
        &path(None, &["hidden"]),
        Some(""),
    );
    let from_root =
        r.resolve(FilePath::new("/p/src/lib.ul"), &[], &path(None, &["util", "hidden"]), Some(""));
    assert!(matches!(&in_util, Resolution::Found { declarations, .. } if declarations.len() == 1));
    assert_eq!(in_util, from_root);
    assert_eq!(
        r.resolve(FilePath::new("/p/src/lib.ul"), &[], &path(None, &["hidden"]), Some("")),
        Resolution::Unresolved { segment: 0 },
        "not from the root, whose glob of `util` does not bring what it hides"
    );
}

#[test]
fn a_declaration_is_found_in_its_namespace_or_in_any() {
    let r = project();
    let circle = path(None, &["Circle"]);
    let lib = FilePath::new("/p/src/lib.ul");
    assert!(matches!(r.resolve(lib, &[], &circle, Some("")), Resolution::Found { .. }));
    assert!(
        matches!(r.resolve(lib, &[], &circle, Some("import")), Resolution::Escapes { .. }),
        "not in a namespace it is not declared in"
    );
    assert!(
        matches!(r.resolve(lib, &[], &circle, None), Resolution::Found { .. }),
        "an import's path names it in any"
    );
}

/// A crate of one file, `/w/lib.ul`.
fn one_file(text: &str) -> Resolver {
    let summary = Rc::new(summarize(unitlang(), text));
    let mut graph = UnitGraph::new();
    graph.add_crate(CrateInfo {
        name: "web".into(),
        kind: "lib".into(),
        root: "/w/lib.ul".into(),
        edition: "2021".into(),
        dependencies: Vec::new(),
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
    });
    Resolver::new(Rc::new(graph), vec![(PathBuf::from("/w/lib.ul"), summary)])
}

#[test]
fn a_web_of_globs_is_settled_once_and_every_name_found_through_it() {
    // Forty modules, each globbing its parent and the next, and a root
    // that globs them all: every module is in one cycle with every other,
    // as std's architecture modules are.
    let n = 40;
    let mut text = String::new();
    for i in 0..n {
        text.push_str(&format!(
            "pub mod m{i} {{\n    use super::*;\n    pub use super::m{}::*;\n    pub fn f{i}() {{}}\n}}\n",
            (i + 1) % n
        ));
        text.push_str(&format!("pub use m{i}::*;\n"));
    }
    let r = one_file(&text);
    let start = std::time::Instant::now();
    for i in 0..n {
        let found = r.resolve(
            FilePath::new("/w/lib.ul"),
            &[],
            &path(None, &["m0", &format!("f{i}")]),
            Some(""),
        );
        assert!(
            matches!(&found, Resolution::Found { declarations, .. } if declarations.len() == 1),
            "f{i} through the web: {found:?}"
        );
    }
    assert!(matches!(
        r.resolve(FilePath::new("/w/lib.ul"), &[], &path(None, &["m7", "nowhere"]), Some("")),
        Resolution::Unresolved { segment: 1 }
    ));
    assert!(
        start.elapsed() < std::time::Duration::from_secs(5),
        "settled once, not explored again for each lookup: {:?}",
        start.elapsed()
    );
}

#[test]
fn a_cycle_inside_a_cycle_is_settled_and_the_outer_one_still_grows() {
    // `a` and `b` glob each other; `c` and `d` glob each other, and `c`
    // globs `a`: the inner cycle settles while the outer is under way.
    let r = one_file(
        "pub mod a {\n    pub use super::b::*;\n    pub fn in_a() {}\n}\n\
         pub mod b {\n    pub use super::a::*;\n    pub use super::c::*;\n    pub fn in_b() {}\n}\n\
         pub mod c {\n    pub use super::d::*;\n    pub use super::a::*;\n    pub fn in_c() {}\n}\n\
         pub mod d {\n    pub use super::c::*;\n    pub fn in_d() {}\n}\n",
    );
    for (from, name) in
        [("a", "in_d"), ("d", "in_a"), ("d", "in_b"), ("b", "in_c"), ("c", "in_b"), ("a", "in_a")]
    {
        let found =
            r.resolve(FilePath::new("/w/lib.ul"), &[], &path(None, &[from, name]), Some(""));
        assert!(
            matches!(&found, Resolution::Found { declarations, .. } if declarations.len() == 1),
            "{from}::{name}: {found:?}"
        );
    }
}

#[test]
fn an_impl_blocks_type_through_another_type_is_found_without_finding_impl_blocks_again() {
    let r = one_file(
        "pub struct Point {\n    pub x: Num;\n}\n\
         impl Point::x {\n    fn on_x() {}\n}\n\
         impl Point {\n    pub fn new() {}\n}\n",
    );
    for (segments, expected) in [(&["Point", "new"][..], "new"), (&["Point", "x"], "x")] {
        let found = r.resolve(FilePath::new("/w/lib.ul"), &[], &path(None, segments), Some(""));
        let Resolution::Found { declarations, .. } = found else {
            panic!("{segments:?}: {found:?}");
        };
        assert_eq!(declarations.len(), 1, "{expected}");
    }
}

// ---------------------------------------------------------------------------
// The crates a crate names, its preludes and its builtins
// ---------------------------------------------------------------------------

/// A crate of std's shape (`/c/lib.ul`): an option module and a prelude.
const CORE: &str = "pub mod option {
    pub struct Option {}
}

pub mod prelude {
    pub mod v1 {
        pub use crate::option::Option;
        pub struct Copy {}
    }
}
";

/// A library only a crate that loads it may name (`/a/lib.ul`).
const ALLOC: &str = "pub mod boxed {
    pub struct Box {}
}
";

/// std (`/s/lib.ul`): its prelude globs core's, and a prelude of its own
/// is its crate's (`vec` uses it).
const STD: &str = "pub mod vec {
    pub struct Vec {}
    pub mod inner {}
}

pub mod prelude {
    pub mod rust_2021 {
        pub use core::prelude::v1::*;
        pub use crate::vec::Vec;
    }
}
";

/// The project's crate (`/w/lib.ul`), naming core and std, loading alloc.
const WEB: &str = "extern crate alloc;
extern crate self as web;

pub mod shapes {
    pub mod deep {}
}

pub mod own {
    pub struct Vec {}
}

pub mod legacy {
    pub mod u8 {
        pub struct Here {}
    }
}

#[no_implicit_prelude]
pub mod hygiene {
    pub mod deeper {}
}

#[when(test, no_implicit_prelude)]
pub mod maybe {}

#[no_implicit_prelude]
mod declared;
";

/// `/w/declared.ul`, closed by the attribute on its declaration's item.
const DECLARED: &str = "pub struct Here {}\n";

/// Another of the project's crates (`/w/other.ul`), naming core and std,
/// which loads nothing.
const OTHER: &str = "pub struct Other {}\n";

const FILES: [(&str, &str); 7] = [
    ("/c/lib.ul", CORE),
    ("/a/lib.ul", ALLOC),
    ("/s/lib.ul", STD),
    ("/w/lib.ul", WEB),
    ("/w/declared.ul", DECLARED),
    ("/w/other.ul", OTHER),
    ("/w/pre.ul", ""),
];

/// core, alloc and std, and the project's crates `web` and `other`, each
/// with std's prelude and the builtins `u8` and `str`; std's own prelude is
/// its `prelude::rust_2021`, from its root.
fn with_preludes() -> Resolver {
    let summaries: Vec<(PathBuf, Rc<Summary>)> = FILES
        .iter()
        .map(|(path, text)| (PathBuf::from(path), Rc::new(summarize(unitlang(), text))))
        .collect();
    let closed_by = vec![path(None, &["no_implicit_prelude"])];
    let krate =
        |name: &str, root: &str, named: Vec<(&str, u32)>, prelude: Option<Path>| CrateInfo {
            name: name.into(),
            kind: "lib".into(),
            root: root.into(),
            edition: "2021".into(),
            dependencies: named.iter().map(|&(n, k)| (n.to_string(), k)).collect(),
            loadable: vec![("alloc".to_string(), 1)],
            source: qana_index::CrateSource::Workspace,
            preludes: prelude.into_iter().collect(),
            builtins: vec!["u8".into(), "str".into()],
            closed_by: closed_by.clone(),
            inert: vec![path(None, &["no_implicit_prelude"])],
            imports_from_root: false,
            unindexed: Vec::new(),
            deref_trait: None,
            try_trait: None,
            derive_traits: Vec::new(),
        };
    let std_prelude = path(Some(Root::Global), &["std", "prelude", "rust_2021"]);
    let mut graph = UnitGraph::new();
    let core = graph.add_crate(krate("core", "/c/lib.ul", Vec::new(), None));
    graph.add_crate(krate("alloc", "/a/lib.ul", vec![("core", core)], None));
    let std = graph.add_crate(krate(
        "std",
        "/s/lib.ul",
        vec![("core", core)],
        Some(path(None, &["prelude", "rust_2021"])),
    ));
    let named = vec![("core", core), ("std", std)];
    let web = graph.add_crate(krate("web", "/w/lib.ul", named.clone(), Some(std_prelude.clone())));
    graph.add_crate(krate("other", "/w/other.ul", named, Some(std_prelude)));
    let declared = summaries[3].1.offers().units().iter().position(|u| u.name == "declared");
    graph.add_body(
        web,
        FilePath::new("/w/lib.ul"),
        declared.unwrap() as u32,
        "/w/declared.ul".into(),
        vec!["declared".into()],
    );
    Resolver::new(Rc::new(graph), summaries)
}

/// What `path` names from inside `units` of `file`, in the default
/// namespace: each declaration as `file:name`, each module as its file; or
/// how it escaped.
fn named(r: &Resolver, file: &str, units: &[&str], path: &Path) -> Result<Vec<String>, String> {
    let units: Vec<String> = units.iter().map(|u| u.to_string()).collect();
    match r.resolve(FilePath::new(file), &units, path, Some("")) {
        Resolution::Found { declarations, modules } => Ok(declarations
            .into_iter()
            .map(|(file, index)| {
                let text = FILES.iter().find(|(p, _)| FilePath::new(p) == file).unwrap().1;
                let summary = summarize(unitlang(), text);
                let name = &summary.offers().declarations()[index as usize].name;
                format!("{}:{name}", file.display())
            })
            .chain(modules.into_iter().map(|m| m.display().to_string()))
            .collect()),
        Resolution::Escapes { segment } => Err(format!("escapes at {segment}")),
        Resolution::Unresolved { segment } => Err(format!("unresolved at {segment}")),
        Resolution::Builtin => Err("builtin".into()),
    }
}

fn one(name: &str) -> Path {
    path(None, &[name])
}

#[test]
fn a_preludes_names_are_every_modules_after_its_own() {
    let r = with_preludes();
    for units in [&[][..], &["shapes"], &["shapes", "deep"]] {
        assert_eq!(named(&r, "/w/lib.ul", units, &one("Vec")), Ok(vec!["/s/lib.ul:Vec".into()]));
        assert_eq!(
            named(&r, "/w/lib.ul", units, &one("Option")),
            Ok(vec!["/c/lib.ul:Option".into()]),
            "through std's prelude's glob of core's"
        );
    }
    assert_eq!(
        named(&r, "/w/lib.ul", &["own"], &one("Vec")),
        Ok(vec!["/w/lib.ul:Vec".into()]),
        "a module's own names come first"
    );
    assert_eq!(named(&r, "/w/declared.ul", &[], &one("Vec")).ok(), None, "closed, below");
    assert_eq!(named(&r, "/w/other.ul", &[], &one("Copy")), Ok(vec!["/c/lib.ul:Copy".into()]));
    assert_eq!(
        named(&r, "/s/lib.ul", &["vec", "inner"], &one("Option")),
        Ok(vec!["/c/lib.ul:Option".into()]),
        "std's prelude is its own `prelude::rust_2021`, from its root"
    );
    assert_eq!(
        named(&r, "/c/lib.ul", &["option"], &one("Vec")),
        Err("unresolved at 0".into()),
        "core has no prelude, and nothing else names it"
    );
}

#[test]
fn the_crates_a_crate_names_are_its_dependencies_and_what_its_root_loads() {
    let r = with_preludes();
    let boxed = path(None, &["alloc", "boxed", "Box"]);
    let global_boxed = path(Some(Root::Global), &["alloc", "boxed", "Box"]);
    for units in [&[][..], &["shapes", "deep"]] {
        assert_eq!(named(&r, "/w/lib.ul", units, &boxed), Ok(vec!["/a/lib.ul:Box".into()]));
        assert_eq!(named(&r, "/w/lib.ul", units, &global_boxed), Ok(vec!["/a/lib.ul:Box".into()]));
        assert_eq!(
            named(&r, "/w/lib.ul", units, &path(None, &["web", "own", "Vec"])),
            Ok(vec!["/w/lib.ul:Vec".into()]),
            "`extern crate self as web` names the crate itself"
        );
        assert_eq!(
            named(&r, "/w/lib.ul", units, &path(None, &["std", "vec", "Vec"])),
            Ok(vec!["/s/lib.ul:Vec".into()])
        );
    }
    assert_eq!(
        named(&r, "/w/other.ul", &[], &boxed),
        Err("unresolved at 0".into()),
        "alloc is loadable, and named only once loaded"
    );
    assert_eq!(named(&r, "/w/other.ul", &[], &global_boxed), Err("unresolved at 0".into()));
}

#[test]
fn a_closed_module_and_those_inside_it_have_neither_crates_nor_preludes() {
    let r = with_preludes();
    for (file, units) in [
        ("/w/lib.ul", &["hygiene"][..]),
        ("/w/lib.ul", &["hygiene", "deeper"]),
        ("/w/declared.ul", &[]),
    ] {
        assert_eq!(named(&r, file, units, &one("Vec")), Err("unresolved at 0".into()), "{units:?}");
        assert_eq!(
            named(&r, file, units, &path(None, &["std", "vec", "Vec"])),
            Err("unresolved at 0".into())
        );
        assert_eq!(
            named(&r, file, units, &path(Some(Root::Global), &["std", "vec", "Vec"])),
            Ok(vec!["/s/lib.ul:Vec".into()]),
            "a path from a root is not closed"
        );
        assert_eq!(named(&r, file, units, &one("u8")), Err("builtin".into()));
    }
    assert_eq!(
        named(&r, "/w/lib.ul", &["maybe"], &one("Vec")),
        Ok(vec!["/s/lib.ul:Vec".into()]),
        "an attribute held under a condition may not hold: the prelude is an answer"
    );
}

#[test]
fn a_builtin_is_what_a_first_name_is_when_it_is_nothing_else() {
    let r = with_preludes();
    assert_eq!(named(&r, "/w/lib.ul", &["shapes"], &one("u8")), Err("builtin".into()));
    assert_eq!(named(&r, "/w/lib.ul", &[], &one("str")), Err("builtin".into()));
    assert_eq!(
        named(&r, "/w/lib.ul", &[], &path(None, &["u8", "MAX"])),
        Err("escapes at 1".into()),
        "what a builtin holds, no summary says"
    );
    assert_eq!(
        named(&r, "/w/lib.ul", &[], &path(Some(Root::Crate), &["u8"])),
        Err("unresolved at 0".into())
    );
    assert_eq!(
        named(&r, "/w/lib.ul", &[], &one("i8")),
        Err("unresolved at 0".into()),
        "not one of this language's"
    );
}

#[test]
fn a_path_past_a_builtins_name_that_names_a_module_alone_is_the_builtins() {
    let r = with_preludes();
    let legacy = &["legacy"][..];
    assert_eq!(
        named(&r, "/w/lib.ul", legacy, &path(None, &["u8", "Here"])),
        Ok(vec!["/w/lib.ul:Here".into()]),
        "the module, where it has the name"
    );
    assert_eq!(
        named(&r, "/w/lib.ul", legacy, &path(None, &["u8", "MAX"])),
        Err("escapes at 1".into()),
        "rustc reads `u8::MAX` past a module `u8` that lacks it as the builtin's"
    );
    assert_eq!(
        named(&r, "/w/lib.ul", &[], &path(None, &["own", "MAX"])),
        Err("unresolved at 1".into()),
        "a module that is no builtin's name names nothing more"
    );
}

/// The names of what `path` names in the one file of [`one_file`]`(text)`.
fn one_file_names(text: &str, path: &Path) -> Result<Vec<String>, usize> {
    let r = one_file(text);
    match r.resolve(FilePath::new("/w/lib.ul"), &[], path, None) {
        Resolution::Found { declarations, .. } => {
            let summary = summarize(unitlang(), text);
            let declared = summary.offers().declarations();
            Ok(declarations
                .into_iter()
                .map(|(_, i)| {
                    let d = &declared[i as usize];
                    let block = d.impl_block.map(|b| {
                        let of = &summary.offers().impls()[b as usize];
                        match &of.trait_path {
                            Some(t) => format!("impl {} for ", t.segments.join("::")),
                            None => "impl ".to_string(),
                        }
                    });
                    format!("{}{}", block.unwrap_or_default(), d.name)
                })
                .collect())
        }
        Resolution::Escapes { segment } | Resolution::Unresolved { segment } => Err(segment),
        Resolution::Builtin => Err(usize::MAX),
    }
}

#[test]
fn a_types_own_members_come_first_then_its_inherent_items_then_its_traits() {
    let text = "struct Outcome {
    pub Ok: Num;
}

impl Try for Outcome {
    fn Ok() {}
    fn into_result() {}
}

impl Outcome {
    fn new() {}
}

impl Make for Outcome {
    fn new() {}
    fn make() {}
}

impl Show for Outcome {
    fn make() {}
}
";
    let at = |segments: &[&str]| one_file_names(text, &path(None, segments));
    assert_eq!(
        at(&["Outcome", "Ok"]),
        Ok(vec!["Ok".into()]),
        "declared inside it, as a variant is"
    );
    assert_eq!(
        at(&["Outcome", "new"]),
        Ok(vec!["impl new".into()]),
        "an inherent item hides a trait's"
    );
    assert_eq!(
        at(&["Outcome", "make"]),
        Ok(vec!["impl Make for make".into(), "impl Show for make".into()]),
        "without one, each trait's is an answer"
    );
    assert_eq!(at(&["Outcome", "into_result"]), Ok(vec!["impl Try for into_result".into()]));
}

#[test]
fn an_impl_block_written_through_a_renamed_import_is_its_types() {
    let text = "pub mod shapes {
    pub struct Circle {}
}

use crate::shapes::Circle as Round;

impl Round {
    fn radius() {}
}

mod elsewhere {
    pub use crate::shapes::Circle as Disc;
    impl Disc {
        fn area() {}
    }
}
";
    let at = |segments: &[&str]| one_file_names(text, &path(None, segments));
    assert_eq!(at(&["shapes", "Circle", "radius"]), Ok(vec!["impl radius".into()]));
    assert_eq!(at(&["shapes", "Circle", "area"]), Ok(vec!["impl area".into()]));
    assert_eq!(at(&["Round", "area"]), Ok(vec!["impl area".into()]), "and the rename's");
}

/// A crate of one file whose language says `allow` is inert, which names
/// `web_sys` on another target.
fn one_file_inert(text: &str) -> Resolver {
    let summary = Rc::new(summarize(unitlang(), text));
    let mut graph = UnitGraph::new();
    graph.add_crate(CrateInfo {
        name: "web".into(),
        kind: "lib".into(),
        root: "/w/lib.ul".into(),
        edition: "2021".into(),
        dependencies: Vec::new(),
        loadable: Vec::new(),
        source: qana_index::CrateSource::Workspace,
        preludes: Vec::new(),
        builtins: Vec::new(),
        closed_by: Vec::new(),
        inert: vec![path(None, &["allow"])],
        imports_from_root: false,
        unindexed: vec!["web_sys".to_string()],
        deref_trait: None,
        try_trait: None,
        derive_traits: Vec::new(),
    });
    Resolver::new(Rc::new(graph), vec![(PathBuf::from("/w/lib.ul"), summary)])
}

#[test]
fn a_name_is_unresolved_only_where_every_place_it_was_looked_for_is_closed() {
    let text = "pub mod plain {
    pub struct Here {}
}

pub mod invoking {
    expand make;
}

pub mod attributed {
    #[magic]
    pub struct There {}
}

pub mod allowed {
    #[allow]
    pub struct Fine {}
}

pub mod outer {
    #[magic]
    pub mod rewritten {
        pub mod inside {}
    }
}

pub mod globbing {
    pub use super::invoking::*;
}

pub mod importing {
    use nowhere::Gone;
}
";
    let r = one_file_inert(text);
    let at = |segments: &[&str], namespace: Option<&str>| {
        r.resolve(FilePath::new("/w/lib.ul"), &[], &path(None, segments), namespace)
    };
    let unresolved = |segment| Resolution::Unresolved { segment };
    let escapes = |segment| Resolution::Escapes { segment };
    assert_eq!(at(&["plain", "Missing"], Some("")), unresolved(1));
    assert_eq!(at(&["Missing"], Some("")), unresolved(0));
    assert_eq!(at(&["allowed", "Missing"], Some("")), unresolved(1), "an inert attribute");
    assert_eq!(at(&["importing", "Other"], Some("")), unresolved(1));
    assert_eq!(at(&["invoking", "Missing"], Some("")), escapes(1), "an invocation may declare it");
    assert_eq!(at(&["attributed", "Missing"], Some("")), escapes(1), "so may an attribute");
    assert_eq!(
        at(&["outer", "rewritten", "inside", "Missing"], Some("")),
        escapes(3),
        "one said of a module may rewrite all it holds"
    );
    assert_eq!(at(&["globbing", "Missing"], Some("")), escapes(1), "a glob of an open module");
    assert_eq!(at(&["importing", "Gone"], Some("")), escapes(1), "an import gives it");
    assert_eq!(at(&["plain", "Here", "member"], Some("")), escapes(2), "a type's members");
    assert_eq!(at(&["plain", "Missing"], Some("macro")), escapes(1), "a named namespace");
    assert_eq!(at(&["plain", "Missing"], None), escapes(1), "an import's last name");
    assert_eq!(at(&["missing", "Here"], None), unresolved(0), "an import's path before it");
    assert_eq!(at(&["web_sys", "window"], Some("")), escapes(0), "a crate the graph does not hold");
    assert_eq!(
        r.resolve(
            FilePath::new("/w/lib.ul"),
            &[],
            &path(Some(Root::Global), &["web_sys", "window"]),
            Some("")
        ),
        escapes(0)
    );
}
