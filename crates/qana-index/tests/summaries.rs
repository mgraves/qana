//! Summaries of real Rust and C: what a file offers, needs and where its
//! declarations are, and which part an edit changes.

use std::sync::OnceLock;

use qana_index::{summarize, summary_key, EscapingName, Summary};
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
        namespace: namespace.into(),
        name: name.into(),
    };
    for escaping in [need("", "Vec"), need("", "String"), need("macro", "format")] {
        assert!(needs.contains(&escaping), "{escaping:?} in {needs:?}");
    }
    assert!(
        !needs.iter().any(|n| n.name == "Cell" || n.name == "show"),
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
