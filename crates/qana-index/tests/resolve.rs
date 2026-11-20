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
        Resolution::Escapes { segment } => Err(segment),
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
        Resolution::Escapes { segment: 0 },
        "not from the root"
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
