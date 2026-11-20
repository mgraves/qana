//! Summaries by unit, of Unitlang, the binding tier's test language for
//! units, paths, visibility and imports: a file's units, what its
//! declarations are in and who may see them, its imports and re-exports,
//! its impl blocks, and its needs as paths from where they stand.

use std::sync::OnceLock;

use qana_index::{summarize, EscapingName, Impl, Import, Path, Root, Summary, Unit, Visibility};
use qana_lang::EmbeddedLang;

const UNITLANG: &str = include_str!("../../../examples/units/unitlang.qana");
const MAIN: &str = include_str!("../../../examples/units/main.ul");

fn unitlang() -> &'static EmbeddedLang {
    static LANG: OnceLock<EmbeddedLang> = OnceLock::new();
    LANG.get_or_init(|| EmbeddedLang::from_qana_source(UNITLANG).expect("unitlang.qana certifies"))
}

fn path(root: Option<Root>, segments: &[&str]) -> Path {
    Path { root, segments: segments.iter().map(|s| s.to_string()).collect() }
}

fn within(root: Root, segments: &[&str]) -> Visibility {
    Visibility::Within(path(Some(root), segments))
}

fn private() -> Visibility {
    within(Root::Unit(0), &[])
}

fn unit(name: &str, parent: Option<u32>, inline: bool, visibility: Visibility) -> Unit {
    Unit { name: name.into(), parent, inline, visibility }
}

/// A declaration as (container, name, unit, visibility, impl block).
type Placed = (String, String, Option<u32>, Visibility, Option<u32>);

/// Each declaration, placed.
fn placed(summary: &Summary) -> Vec<Placed> {
    summary
        .offers()
        .declarations()
        .iter()
        .map(|d| {
            (d.container.join("/"), d.name.clone(), d.unit, d.visibility.clone(), d.impl_block)
        })
        .collect()
}

#[test]
fn a_files_units_are_its_modules_with_their_parents_and_visibility() {
    let summary = summarize(unitlang(), MAIN);
    assert_eq!(
        summary.offers().units(),
        [
            unit("geo", None, true, Visibility::Public),
            unit("inner", Some(0), true, within(Root::Unit(1), &[])),
            unit("shapes", None, false, private()),
            unit("util", None, false, within(Root::Crate, &[])),
        ]
    );
    let locations = summary.locations();
    for (i, u) in summary.offers().units().iter().enumerate() {
        let (start, end) = locations.unit_span(i);
        assert_eq!(&MAIN[start as usize..end as usize], u.name, "where its name is");
    }
}

#[test]
fn each_declaration_has_its_unit_its_visibility_and_its_impl_block() {
    let summary = summarize(unitlang(), MAIN);
    let row = |container: &str,
               name: &str,
               unit: Option<u32>,
               visibility: Visibility,
               block: Option<u32>| {
        (container.to_string(), name.to_string(), unit, visibility, block)
    };
    assert_eq!(
        placed(&summary),
        [
            row("", "geo", None, Visibility::Public, None),
            row("", "shapes", None, private(), None),
            row("", "util", None, within(Root::Crate, &[]), None),
            row("geo", "Point", Some(0), Visibility::Public, None),
            row("geo", "inner", Some(0), within(Root::Unit(1), &[]), None),
            row("geo", "origin", Some(0), private(), None),
            row("geo/Point", "x", Some(0), Visibility::Public, None),
            row("geo/Point", "y", Some(0), private(), None),
            row("geo/inner", "deep", Some(1), within(Root::Crate, &["geo"]), None),
            row("geo::Point", "fmt", None, private(), Some(1)),
            row("geo::Point", "new", None, Visibility::Public, Some(0)),
        ]
    );
}

#[test]
fn impl_blocks_say_what_they_are_of_and_which_trait() {
    let summary = summarize(unitlang(), MAIN);
    let point = path(None, &["geo", "Point"]);
    assert_eq!(
        summary.offers().impls(),
        [
            Impl { unit: None, self_type: Some(point.clone()), trait_path: None },
            Impl { unit: None, self_type: Some(point), trait_path: Some(path(None, &["Display"])) },
        ]
    );
}

#[test]
fn imports_are_paths_after_their_groups_prefix_and_a_public_one_is_offered() {
    let summary = summarize(unitlang(), MAIN);
    let import = |path: Path, name: Option<&str>, glob: bool, visibility: Visibility| Import {
        unit: None,
        path,
        name: name.map(str::to_string),
        glob,
        visibility,
    };
    assert_eq!(
        summary.offers().reexports(),
        [import(path(None, &["geo", "Point"]), Some("Point"), false, Visibility::Public)],
        "`pub use` re-exports"
    );
    assert_eq!(
        summary.needs().imports(),
        [
            import(path(Some(Root::Unit(0)), &["util", "helpers"]), None, false, private()),
            import(path(Some(Root::Crate), &["shapes"]), Some("shapes"), false, private()),
            import(path(Some(Root::Crate), &["shapes", "Circle"]), Some("Round"), false, private()),
            import(path(Some(Root::Crate), &["shapes", "area"]), None, true, private()),
            import(
                path(Some(Root::Global), &["std", "fmt", "Display"]),
                Some("Display"),
                false,
                private()
            ),
        ],
        "`self` in a group is its prefix; `as _` names nothing; a glob names nothing"
    );
}

#[test]
fn a_file_needs_the_paths_it_refers_to_from_the_unit_they_are_in() {
    let summary = summarize(unitlang(), MAIN);
    let need =
        |unit: Option<u32>, path: Path| EscapingName { unit, namespace: String::new(), path };
    assert_eq!(
        summary.needs().escaping(),
        [
            need(None, path(Some(Root::Crate), &["util"])),
            need(None, path(Some(Root::Crate), &["util", "clamp"])),
            need(Some(0), path(None, &["Num"])),
            need(Some(0), path(Some(Root::Unit(1)), &["shapes"])),
            need(Some(0), path(Some(Root::Unit(1)), &["shapes", "Circle"])),
        ],
        "not an import's path, nor `pub(in …)`'s: they are said in full"
    );
}

#[test]
fn roots_add_up_and_one_that_cannot_stand_where_it_is_spells_no_path() {
    let text = "fn f() {\n    super::super::a;\n    self::super::b;\n    crate::super::c;\n    ::krate::d;\n}\n";
    let summary = summarize(unitlang(), text);
    let paths: Vec<&Path> = summary.needs().escaping().iter().map(|e| &e.path).collect();
    assert_eq!(
        paths,
        [
            &path(Some(Root::Unit(1)), &["b"]),
            &path(Some(Root::Unit(2)), &["a"]),
            &path(Some(Root::Global), &["krate", "d"]),
        ]
    );
}

#[test]
fn groups_nest_and_a_group_may_begin_at_a_root() {
    let text = "use a::{b::{c, d as e}, self};\nuse super::*;\nuse ::{x};\nuse crate::{*};\n";
    let summary = summarize(unitlang(), text);
    let said: Vec<(Path, Option<String>, bool)> = summary
        .needs()
        .imports()
        .iter()
        .map(|i| (i.path.clone(), i.name.clone(), i.glob))
        .collect();
    let some = |s: &str| Some(s.to_string());
    assert_eq!(
        said,
        [
            (path(None, &["a"]), some("a"), false),
            (path(None, &["a", "b", "c"]), some("c"), false),
            (path(None, &["a", "b", "d"]), some("e"), false),
            (path(Some(Root::Unit(1)), &[]), None, true),
            (path(Some(Root::Crate), &[]), None, true),
            (path(Some(Root::Global), &["x"]), some("x"), false),
        ]
    );
}

#[test]
fn what_a_function_body_holds_is_the_bodys() {
    let text = "fn f() {\n    mod hidden {}\n    use z;\n    impl Q {}\n    out;\n}\n";
    let summary = summarize(unitlang(), text);
    assert!(summary.offers().units().is_empty(), "no unit");
    assert!(summary.needs().imports().is_empty(), "no import");
    assert!(summary.offers().impls().is_empty(), "no impl block");
    assert!(
        summary
            .needs()
            .escaping()
            .iter()
            .any(|e| e.path == path(None, &["out"]) && e.unit.is_none()),
        "but what it refers to is needed, from the unit the function is in"
    );
}

/// The three fingerprints of `text`'s summary.
fn parts(text: &str) -> (u64, u64, u64) {
    let summary = summarize(unitlang(), text);
    (
        summary.offers().fingerprint(),
        summary.needs().fingerprint(),
        summary.locations().fingerprint(),
    )
}

#[test]
fn an_import_made_public_is_offered_and_a_private_one_changed_is_needed() {
    let before = parts(MAIN);
    let public =
        parts(&MAIN.replace("use self::util::helpers as _;", "pub use self::util::helpers as _;"));
    assert_ne!(before.0, public.0, "a re-export is an offer");
    assert_ne!(before.1, public.1, "and no longer a need");

    let changed =
        parts(&MAIN.replace("use self::util::helpers as _;", "use self::util::others as _;"));
    assert_eq!(before.0, changed.0, "a private import offers nothing");
    assert_ne!(before.1, changed.1, "but is needed");
}

#[test]
fn a_visibility_or_a_unit_changed_changes_the_offers() {
    let before = parts(MAIN);
    let narrowed = parts(&MAIN.replace("pub mod geo", "pub(crate) mod geo"));
    assert_ne!(before.0, narrowed.0, "a unit's visibility");
    let field = parts(&MAIN.replace("        y: Num;", "        pub y: Num;"));
    assert_ne!(before.0, field.0, "a field's visibility");
    let moved = parts(
        &MAIN
            .replace("mod shapes;\n", "")
            .replace("pub mod geo {", "pub mod geo {\n    mod shapes;"),
    );
    assert_ne!(before.0, moved.0, "a unit moved into another");
}

#[test]
fn a_body_edit_still_changes_only_the_locations() {
    let before = parts(MAIN);
    let edited = parts(&MAIN.replace("        Round;\n", "\n        Round;\n"));
    assert_eq!(before.0, edited.0, "offers");
    assert_eq!(before.1, edited.1, "needs");
    assert_ne!(before.2, edited.2, "locations");
}

#[test]
fn a_unit_graph_places_each_file_in_its_crates_at_its_module_path() {
    use qana_index::{CrateInfo, UnitGraph};
    use std::path::{Path as FilePath, PathBuf};
    let krate = |name: &str, kind: &str, root: &str| CrateInfo {
        name: name.into(),
        kind: kind.into(),
        root: PathBuf::from(root),
        edition: "2021".into(),
        dependencies: Vec::new(),
    };
    let mut graph = UnitGraph::new();
    let lib = graph.add_crate(krate("geo", "lib", "/p/src/lib.rs"));
    let test = graph.add_crate(krate("shapes", "test", "/p/tests/shapes.rs"));
    assert!(graph.add_body(
        lib,
        FilePath::new("/p/src/lib.rs"),
        0,
        "/p/src/shapes.rs".into(),
        vec!["shapes".into()]
    ));
    assert!(graph.add_body(
        lib,
        FilePath::new("/p/src/shapes.rs"),
        0,
        "/p/src/shapes/area.rs".into(),
        vec!["shapes".into(), "area".into()]
    ));
    assert!(graph.add_body(
        test,
        FilePath::new("/p/tests/shapes.rs"),
        0,
        "/p/src/shapes.rs".into(),
        vec!["common".into()]
    ));
    assert!(
        !graph.add_body(
            lib,
            FilePath::new("/p/src/shapes/area.rs"),
            0,
            "/p/src/shapes.rs".into(),
            vec![]
        ),
        "a file is one unit of a crate at most"
    );

    let places = |file: &str| -> Vec<(u32, Vec<String>)> {
        graph.places(FilePath::new(file)).into_iter().map(|(k, p)| (k, p.to_vec())).collect()
    };
    assert_eq!(places("/p/src/lib.rs"), [(lib, vec![])]);
    assert_eq!(places("/p/src/shapes/area.rs"), [(lib, vec!["shapes".to_string(), "area".into()])]);
    assert_eq!(
        places("/p/src/shapes.rs"),
        [(lib, vec!["shapes".to_string()]), (test, vec!["common".to_string()])],
        "a file two crates include is in both"
    );
    assert!(places("/p/build.rs").is_empty());
    assert_eq!(
        graph.body(lib, FilePath::new("/p/src/lib.rs"), 0),
        Some(FilePath::new("/p/src/shapes.rs"))
    );
    assert_eq!(
        graph.declared_by(lib, FilePath::new("/p/src/shapes.rs")),
        Some((FilePath::new("/p/src/lib.rs"), 0))
    );
    assert_eq!(
        graph.declared_by(lib, FilePath::new("/p/src/lib.rs")),
        None,
        "a root is declared by none"
    );
    assert_eq!(graph.file_count(), 4);
}
