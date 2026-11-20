//! Summaries of real Rust and C: what a file offers, needs and where its
//! declarations are, and which part an edit changes.

use std::sync::OnceLock;

use qana_grammar::green::ERROR_NT;
use qana_grammar::{GreenChild, GreenNode};
use qana_index::{
    summarize, summary_key, Attribute, EscapingName, Impl, Import, Path, Root, Summary, Unit,
    Visibility,
};
use qana_lang::EmbeddedLang;

const RUST_QANA: &str = include_str!("../../../examples/rust/rust.qana");
const C_QANA: &str = include_str!("../../../examples/c/c.qana");

fn rust() -> &'static EmbeddedLang {
    static LANG: OnceLock<EmbeddedLang> = OnceLock::new();
    LANG.get_or_init(|| EmbeddedLang::from_qana_source(RUST_QANA).expect("rust.qana certifies"))
}

const SAMPLE: &str = "use std::fmt::Display;

/// A cell.
pub struct Cell {
    width: f32,
    pub height: f32,
}

impl Cell {
    pub fn new(width: f32) -> Cell {
        fn helper() {}
        Cell { width, height: 0.0 }
    }

    fn area(&self) -> f32 {
        self.width * self.height
    }
}

impl Cell {
    pub fn new_square(size: f32) -> Cell {
        Cell::new(size)
    }
}

pub enum Kind {
    Round,
    Square(f32),
}

pub mod grid {
    pub const SIZE: usize = 8;
}

macro_rules! cells {
    () => {};
}

fn show(v: Vec<u8>) -> String {
    format!(\"{v:?}\")
}
";

/// Each offered declaration as (container, name, kind, namespace,
/// ordinal, signature).
fn offers(summary: &Summary) -> Vec<(String, String, String, String, u32, String)> {
    summary
        .offers()
        .declarations()
        .iter()
        .map(|d| {
            (
                d.container.join("::"),
                d.name.clone(),
                d.kind.clone(),
                d.namespace.clone(),
                d.ordinal,
                d.signature.clone(),
            )
        })
        .collect()
}

fn row(
    container: &str,
    name: &str,
    kind: &str,
    namespace: &str,
    ordinal: u32,
    signature: &str,
) -> (String, String, String, String, u32, String) {
    (container.into(), name.into(), kind.into(), namespace.into(), ordinal, signature.into())
}

#[test]
fn a_rust_file_offers_its_items_and_their_members_and_not_a_bodys() {
    let summary = summarize(rust(), SAMPLE);
    assert_eq!(
        offers(&summary),
        [
            row("", "Cell", "struct", "", 0, "struct Cell"),
            row("", "Kind", "struct", "", 0, "enum Kind"),
            row("", "cells", "function", "macro", 0, "macro_rules! cells"),
            row("", "grid", "module", "", 0, "mod grid"),
            row("", "show", "function", "", 0, "fn show(v: Vec<u8>) -> String"),
            row("Cell", "area", "function", "", 0, "fn area(&self) -> f32"),
            row("Cell", "height", "field", "", 0, "pub height: f32"),
            row("Cell", "new", "function", "", 0, "fn new(width: f32) -> Cell"),
            row("Cell", "new_square", "function", "", 0, "fn new_square(size: f32) -> Cell"),
            row("Cell", "width", "field", "", 0, "width: f32"),
            row("Kind", "Round", "constant", "", 0, "Round"),
            row("Kind", "Square", "constant", "", 0, "Square(f32)"),
            row("grid", "SIZE", "constant", "", 0, "const SIZE: usize = 8"),
        ]
    );
}

#[test]
fn locations_say_where_each_offer_is_in_the_offers_order() {
    let summary = summarize(rust(), SAMPLE);
    let declarations = summary.offers().declarations();
    let locations = summary.locations();
    for (i, d) in declarations.iter().enumerate() {
        let (start, end) = locations.name_span(i);
        assert_eq!(&SAMPLE[start as usize..end as usize], d.name, "the name's span");
        let (item_start, item_end) = locations.item_span(i);
        assert!(item_start <= start && end <= item_end, "inside its construct");
        let (line, column) = locations.position(i);
        let line_text = SAMPLE.lines().nth(line as usize).expect("the line");
        assert!(
            line_text.chars().skip(column as usize).collect::<String>().starts_with(&d.name),
            "{} at line {line}, column {column}",
            d.name
        );
    }
    let words = locations.words();
    for word in ["Cell", "width", "helper", "Vec", "format"] {
        assert!(words.iter().any(|w| w == word), "{word} is a word of the file");
    }
    assert!(words.windows(2).all(|w| w[0] < w[1]), "sorted, each once");
}

#[test]
fn a_file_needs_the_names_it_refers_to_and_does_not_define() {
    let summary = summarize(rust(), SAMPLE);
    let needs = summary.needs().escaping();
    let need = |namespace: &str, name: &str| EscapingName {
        unit: None,
        namespace: namespace.into(),
        path: Path { root: None, segments: vec![name.into()] },
    };
    for escaping in [need("", "Vec"), need("", "String"), need("macro", "format")] {
        assert!(needs.contains(&escaping), "{escaping:?} in {needs:?}");
    }
    assert!(
        !needs.iter().any(|n| n
            .path
            .segments
            .last()
            .is_some_and(|last| last == "Cell" || last == "show")),
        "what the file defines it does not need: {needs:?}"
    );
    assert!(needs.windows(2).all(|w| w[0] < w[1]), "sorted, each once");
}

/// The three fingerprints of `text`'s summary.
fn parts(text: &str) -> (u64, u64, u64) {
    let summary = summarize(rust(), text);
    (
        summary.offers().fingerprint(),
        summary.needs().fingerprint(),
        summary.locations().fingerprint(),
    )
}

#[test]
fn a_body_edit_changes_only_the_locations() {
    let before = parts(SAMPLE);
    let edited = SAMPLE.replace("self.width * self.height", "\n        self.height * self.width");
    let after = parts(&edited);
    assert_eq!(before.0, after.0, "offers");
    assert_eq!(before.1, after.1, "needs");
    assert_ne!(before.2, after.2, "locations");
}

#[test]
fn moving_an_item_changes_only_the_locations() {
    let show = "fn show(v: Vec<u8>) -> String {\n    format!(\"{v:?}\")\n}\n";
    let moved = format!("{show}\n{}", SAMPLE.replace(show, ""));
    let (before, after) = (parts(SAMPLE), parts(&moved));
    assert_eq!(before.0, after.0, "offers");
    assert_eq!(before.1, after.1, "needs");
    assert_ne!(before.2, after.2, "locations");
}

#[test]
fn a_new_item_changes_the_offers_and_a_new_outside_name_the_needs() {
    let before = parts(SAMPLE);
    let added = parts(&format!("{SAMPLE}\npub fn extra() {{}}\n"));
    assert_ne!(before.0, added.0, "a new item is a new offer");
    assert_eq!(before.1, added.1, "and needs nothing new");

    let needing = parts(&SAMPLE.replace(
        "Cell { width, height: 0.0 }",
        "Cell { width, height: HashMap::new().len() as f32 }",
    ));
    assert_eq!(before.0, needing.0, "a body naming something new offers nothing new");
    assert_ne!(before.1, needing.1, "but needs it");
}

#[test]
fn namesakes_are_told_apart_by_their_ordinal() {
    let twice = "impl Cell { fn new() {} }\nimpl Cell { fn new() {} }\n";
    let summary = summarize(rust(), twice);
    let ordinals: Vec<(String, u32)> =
        summary.offers().declarations().iter().map(|d| (d.name.clone(), d.ordinal)).collect();
    assert_eq!(ordinals, [("new".to_string(), 0), ("new".to_string(), 1)]);
}

#[test]
fn a_summary_round_trips_and_its_key_follows_the_grammar() {
    let summary = summarize(rust(), SAMPLE);
    let bytes = postcard::to_allocvec(&summary).expect("a summary encodes");
    let back: Summary = postcard::from_bytes(&bytes).expect("and decodes");
    assert_eq!(back, summary);
    assert_eq!(summary_key(RUST_QANA), summary_key(RUST_QANA));
    assert_ne!(summary_key(RUST_QANA), summary_key(C_QANA));
}

#[test]
fn a_c_file_is_summarized_from_its_own_grammar() {
    let c = EmbeddedLang::from_qana_source(C_QANA).expect("c.qana certifies");
    let summary =
        summarize(&c, "struct point { int x; int y; };\nint main(void) {\n    return 0;\n}\n");
    let names: Vec<(String, String)> =
        summary.offers().declarations().iter().map(|d| (d.name.clone(), d.kind.clone())).collect();
    assert!(names.contains(&("main".to_string(), "function".to_string())), "{names:?}");
    let main = summary.offers().declarations().iter().find(|d| d.name == "main").expect("main");
    assert_eq!(main.signature, "int main(void)");
}

const MODULES: &str = "mod shapes;
pub(crate) mod util;

pub mod geo {
    pub struct Point {
        pub x: f32,
        y: f32,
    }

    pub(super) mod inner {
        pub(in crate::geo) fn deep() {}
    }

    fn origin() -> Point {
        super::shapes::Circle::new()
    }
}

use crate::shapes::{self, Circle as Round, area::*};
pub use geo::Point;
use std::fmt::{self, Display};
use ::serde::Serialize as _;
extern crate alloc;

impl geo::Point {
    pub fn new() -> Self {
        Round::default();
        crate::util::clamp(1.0)
    }
}

impl<T> Display for Wrapper<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Ok(())
    }
}

impl fmt::Debug for geo::Point {}
";

fn path(root: Option<Root>, segments: &[&str]) -> Path {
    Path { root, segments: segments.iter().map(|s| s.to_string()).collect() }
}

fn private() -> Visibility {
    Visibility::Within(path(Some(Root::Unit(0)), &[]))
}

#[test]
fn a_rust_files_summary_holds_its_units_imports_reexports_visibilities_and_impls() {
    let summary = summarize(rust(), MODULES);
    let offers = summary.offers();
    let unit = |name: &str, parent: Option<u32>, inline: bool, visibility: Visibility| Unit {
        name: name.into(),
        parent,
        inline,
        visibility,
        attributes: Vec::new(),
        open: false,
    };
    assert_eq!(
        offers.units(),
        [
            unit("geo", None, true, Visibility::Public),
            unit("inner", Some(0), true, Visibility::Within(path(Some(Root::Unit(1)), &[]))),
            unit("shapes", None, false, private()),
            unit("util", None, false, Visibility::Within(path(Some(Root::Crate), &[]))),
        ]
    );

    let point = path(None, &["geo", "Point"]);
    assert_eq!(
        offers.impls(),
        [
            Impl {
                unit: None,
                self_type: Some(path(None, &["Wrapper"])),
                trait_path: Some(path(None, &["Display"])),
            },
            Impl { unit: None, self_type: Some(point.clone()), trait_path: None },
            Impl {
                unit: None,
                self_type: Some(point.clone()),
                trait_path: Some(path(None, &["fmt", "Debug"])),
            },
        ],
        "generic arguments are not the path"
    );

    let import = |path: Path, name: Option<&str>, glob: bool| Import {
        unit: None,
        path,
        name: name.map(str::to_string),
        glob,
        visibility: private(),
        attributes: Vec::new(),
        loads: false,
    };
    assert_eq!(
        offers.reexports(),
        [Import { visibility: Visibility::Public, ..import(point, Some("Point"), false) }]
    );
    assert_eq!(
        summary.needs().imports(),
        [
            import(path(None, &["std", "fmt"]), Some("fmt"), false),
            import(path(None, &["std", "fmt", "Display"]), Some("Display"), false),
            import(path(Some(Root::Crate), &["shapes"]), Some("shapes"), false),
            import(path(Some(Root::Crate), &["shapes", "Circle"]), Some("Round"), false),
            import(path(Some(Root::Crate), &["shapes", "area"]), None, true),
            Import {
                loads: true,
                ..import(path(Some(Root::Global), &["alloc"]), Some("alloc"), false)
            },
            import(path(Some(Root::Global), &["serde", "Serialize"]), None, false),
        ],
        "`extern crate` imports a crate, and loads it"
    );

    let placed = |name: &str| {
        let d = offers.declarations().iter().find(|d| d.name == name).expect(name);
        (d.unit, d.visibility.clone(), d.impl_block)
    };
    assert_eq!(placed("Point"), (Some(0), Visibility::Public, None));
    assert_eq!(placed("x"), (Some(0), Visibility::Public, None));
    assert_eq!(placed("y"), (Some(0), private(), None));
    assert_eq!(
        placed("deep"),
        (Some(1), Visibility::Within(path(Some(Root::Crate), &["geo"])), None)
    );
    assert_eq!(placed("new"), (None, Visibility::Public, Some(1)));
    assert_eq!(placed("fmt"), (None, private(), Some(0)));
}

#[test]
fn a_rust_files_needs_are_paths_from_where_they_stand_and_not_its_imports() {
    let summary = summarize(rust(), MODULES);
    let needs = summary.needs().escaping();
    let need =
        |unit: Option<u32>, path: Path| EscapingName { unit, namespace: String::new(), path };
    for expected in [
        need(Some(0), path(Some(Root::Unit(1)), &["shapes", "Circle", "new"])),
        need(None, path(Some(Root::Crate), &["util", "clamp"])),
        need(None, path(None, &["fmt", "Formatter"])),
        need(None, path(None, &["Round", "default"])),
    ] {
        assert!(needs.contains(&expected), "{expected:?} in {needs:?}");
    }
    assert!(
        !needs.iter().any(|n| ["std", "serde", "alloc"].contains(&n.path.segments[0].as_str())
            || n.path.root == Some(Root::Crate) && n.path.segments == ["geo"]),
        "what an import or a visibility says in full is not a need: {needs:?}"
    );
}

#[test]
fn a_units_item_is_its_declaration_with_its_attributes_and_visibility() {
    let text = "#[cfg(test)]\n#[path = \"other.rs\"]\npub(crate) mod tests;\n\nmod plain;\n";
    let summary = summarize(rust(), text);
    let item = |i: usize| {
        let (start, end) = summary.locations().unit_item_span(i);
        &text[start as usize..end as usize]
    };
    assert_eq!(summary.offers().units()[1].name, "tests");
    assert_eq!(item(1), "#[cfg(test)]\n#[path = \"other.rs\"]\npub(crate) mod tests;");
    assert_eq!(item(0), "mod plain;");
}

/// Whether `text` parses as Rust without an error or a missing token.
fn parses_cleanly(text: &str) -> bool {
    fn clean(node: &GreenNode) -> bool {
        node.nt != ERROR_NT
            && node.children.iter().all(|c| match c {
                GreenChild::Token(t) => !t.is_missing(),
                GreenChild::Node(n) => clean(n),
            })
    }
    let session = rust().session(text);
    session.tree().is_some_and(|tree| clean(tree))
}

fn attribute(segments: &[&str], conditional: bool) -> Attribute {
    Attribute { path: path(None, segments), conditional }
}

const ATTRIBUTES: &str = "//! A crate.
#![no_std]
#![cfg_attr(not(feature = \"std\"), no_core, allow(dead_code),)]
#![cfg_attr(docsrs, cfg_attr(all(), feature(doc_cfg)))]
#![doc = \"text\"]

#[no_implicit_prelude]
pub mod hygiene {
    #![allow(unused)]
    mod inner {}
}

#[cfg_attr(test, no_implicit_prelude)]
mod declared;

#[prelude_import]
#[allow(unused)]
use prelude::rust_2024::*;

#[macro_use]
extern crate alloc;
extern crate std as realstd;
extern crate self as core;
pub use ::core as __core;

#[derive(Debug)]
#[cfg_attr(test, derive(Clone))]
struct S {
    #[cfg_attr(feature = \"serde\", serde(skip))]
    a: u8,
}

impl S {
    #![cfg_attr(test, allow(unused))]
    fn cfg_attr(&self, x: u8) -> u8 {
        x
    }
}

fn cfg_attr(x: u8) -> u8 {
    S { a: x }.cfg_attr(cfg_attr(x))
}
";

#[test]
fn a_files_inner_attributes_are_its_own_units_and_a_conditional_ones_hold_under_it() {
    assert!(parses_cleanly(ATTRIBUTES), "`cfg_attr` names a function, a method and a call too");
    let summary = summarize(rust(), ATTRIBUTES);
    assert_eq!(
        summary.offers().attributes(),
        [
            attribute(&["no_std"], false),
            attribute(&["cfg_attr"], false),
            attribute(&["no_core"], true),
            attribute(&["allow"], true),
            attribute(&["cfg_attr"], false),
            attribute(&["cfg_attr"], true),
            attribute(&["feature"], true),
            attribute(&["doc"], false),
        ],
        "a struct's, a field's and an impl block's attributes are not the file's"
    );
}

#[test]
fn a_modules_attributes_are_its_items_then_its_bodys() {
    let summary = summarize(rust(), ATTRIBUTES);
    let units = summary.offers().units();
    let of = |name: &str| units.iter().find(|u| u.name == name).map(|u| u.attributes.clone());
    assert_eq!(
        of("hygiene"),
        Some(vec![attribute(&["no_implicit_prelude"], false), attribute(&["allow"], false)])
    );
    assert_eq!(of("inner"), Some(Vec::new()), "what a module says is not its children's");
    assert_eq!(
        of("declared"),
        Some(vec![attribute(&["cfg_attr"], false), attribute(&["no_implicit_prelude"], true)])
    );
}

#[test]
fn an_imports_attributes_are_its_items_and_extern_crate_loads() {
    let summary = summarize(rust(), ATTRIBUTES);
    let imports = summary.needs().imports();
    let glob = imports.iter().find(|i| i.glob).expect("the prelude's glob");
    assert_eq!(glob.path, path(None, &["prelude", "rust_2024"]));
    assert_eq!(
        glob.attributes,
        [attribute(&["prelude_import"], false), attribute(&["allow"], false)]
    );
    let named = |name: &str| imports.iter().find(|i| i.name.as_deref() == Some(name));
    let alloc = named("alloc").expect("extern crate alloc");
    assert_eq!(
        (&alloc.path, alloc.loads, &alloc.attributes),
        (&path(Some(Root::Global), &["alloc"]), true, &vec![attribute(&["macro_use"], false)])
    );
    let realstd = named("realstd").expect("extern crate std as realstd");
    assert_eq!((&realstd.path, realstd.loads), (&path(Some(Root::Global), &["std"]), true));
    let core = named("core").expect("extern crate self as core");
    assert_eq!((&core.path, core.loads), (&path(Some(Root::Crate), &[]), true));
    let reexport =
        summary.offers().reexports().iter().find(|i| i.name.as_deref() == Some("__core"));
    assert_eq!(
        reexport.map(|i| (&i.path, i.loads)),
        Some((&path(Some(Root::Global), &["core"]), false)),
        "a `use` loads nothing"
    );
}

const MACROS: &str = "use std::fmt;

thread_local! {
    static COUNT: u32 = 0;
}

#[derive(Debug, serde::Serialize, Nothing!)]
#[cfg_attr(test, derive(Clone))]
pub struct Point {
    x: Missing,
}

pub mod quiet {
    pub fn f() -> Absent {
        let v = helper();
        {
            println!(\"{}\", Inner);
            nested();
        }
        v
    }
}

pub mod loud {
    lazy_static! {
        static ref X: u8 = 0;
    }
}

fn g() {
    let m!(bound) = 1;
    bound;
}
";

#[test]
fn an_item_macro_opens_its_module_and_one_in_a_block_its_block() {
    let summary = summarize(rust(), MACROS);
    assert!(summary.offers().open(), "`thread_local! {{…}}` among the file's own items");
    let units = summary.offers().units();
    let open = |name: &str| units.iter().find(|u| u.name == name).map(|u| u.open);
    assert_eq!(open("loud"), Some(true));
    assert_eq!(open("quiet"), Some(false), "a macro in a function's block is the block's");
}

#[test]
fn escaping_leaves_out_what_a_macro_in_a_block_around_it_may_declare() {
    let found: Vec<(String, String)> = qana_index::escaping(rust(), MACROS)
        .into_iter()
        .map(|r| (r.namespace, r.path.map(|p| p.segments.join("::")).unwrap_or_default()))
        .collect();
    let has = |ns: &str, path: &str| found.iter().any(|(n, p)| n == ns && p == path);
    assert!(has("", "Missing") && has("", "Absent") && has("", "helper"), "{found:?}");
    assert!(!has("", "Inner") && !has("", "nested"), "the block holds `println!`: {found:?}");
    assert!(!has("", "bound"), "a pattern macro may bind it: {found:?}");
    assert!(has("derive", "serde::Serialize") && has("derive", "Debug") && has("derive", "Clone"));
    assert!(has("derive", "Nothing"), "a macro_rules derive's path");
    assert!(
        found.iter().any(|(n, p)| n == "import" && p == "std::fmt"),
        "an import's path: {found:?}"
    );
}

#[test]
fn an_import_in_a_block_opens_the_block_and_a_file_that_does_not_parse_its_units() {
    let text = "fn h() {
    use crate::settings::{self, Level};
    settings::level();
    Level::Off;
}

fn k() {
    Elsewhere;
}
";
    let found: Vec<String> = qana_index::escaping(rust(), text)
        .into_iter()
        .filter(|r| r.namespace.is_empty())
        .filter_map(|r| r.path.map(|p| p.segments.join("::")))
        .collect();
    assert_eq!(found, ["Elsewhere"], "what the block's import brings, no summary holds");
    assert!(!summarize(rust(), text).offers().open());

    let broken =
        "pub mod inner {\n    pub fn f( {}\n}\nimpl<T: [const] Default> Default for X<T> {}\n";
    let summary = summarize(rust(), broken);
    assert!(summary.offers().open(), "a text that does not parse may hold what it misses");
    assert!(summary.offers().units().iter().all(|u| u.open));
}
