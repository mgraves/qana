//! A file's references to names, for finding usages: where each is, its
//! path, whether it is in an import, and what it binds to in the file.

use std::sync::OnceLock;

use qana_index::{references, Path, Reference, Root};
use qana_lang::EmbeddedLang;

const UNITLANG: &str = include_str!("../../../examples/units/unitlang.qana");

fn unitlang() -> &'static EmbeddedLang {
    static LANG: OnceLock<EmbeddedLang> = OnceLock::new();
    LANG.get_or_init(|| EmbeddedLang::from_qana_source(UNITLANG).expect("unitlang.qana certifies"))
}

const TEXT: &str = "mod geo {
    pub struct Point {
    }
}

use geo::Point;
use geo::Point as P;

fn f() {
    Point;
    P;
    crate::geo::Point;
    other;
}

fn g() {
    fn Point() {}
    Point;
}
";

fn path(root: Option<Root>, segments: &[&str]) -> Option<Path> {
    Some(Path { root, segments: segments.iter().map(|s| s.to_string()).collect() })
}

/// (line, column, path, in an import, local, whether it binds in the file)
fn said(r: &Reference) -> (u32, u32, Option<Path>, bool, bool, bool) {
    (r.line, r.column, r.path.clone(), r.in_import, r.local, r.binds.is_some())
}

#[test]
fn each_reference_to_a_name_says_where_it_is_its_path_and_what_it_binds() {
    let found = references(unitlang(), TEXT, &["Point".to_string(), "P".to_string()]);
    assert_eq!(
        found.iter().map(said).collect::<Vec<_>>(),
        [
            (5, 9, path(None, &["geo", "Point"]), true, false, false),
            (6, 9, path(None, &["geo", "Point"]), true, false, false),
            (9, 4, path(None, &["Point"]), false, false, true),
            (10, 4, path(None, &["P"]), false, false, true),
            (11, 16, path(Some(Root::Crate), &["geo", "Point"]), false, false, false),
            (17, 4, path(None, &["Point"]), false, true, true),
        ],
        "an import's name is not a local; a body's own item is"
    );
    assert_eq!(found[2].line_text, "    Point;");
    let at = |span: (u32, u32)| &TEXT[span.0 as usize..span.1 as usize];
    assert_eq!(at(found[2].binds.unwrap()), "Point", "the `use` leaf it binds to");
    assert_eq!(at(found[3].binds.unwrap()), "P", "the rename it binds to");
    assert_eq!(found[3].binds.unwrap().0 as usize, TEXT.find("P;\n\nfn f").unwrap());
}

#[test]
fn a_reference_inside_a_unit_says_its_units_and_a_line_its_text_without_its_break() {
    let text = "mod a {\r\n    mod b {\r\n        fn h() { x; }\r\n    }\r\n}\r\n";
    let found = references(unitlang(), text, &["x".to_string()]);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].units, ["a", "b"]);
    assert_eq!(found[0].line_text, "        fn h() { x; }");
    assert!(
        references(unitlang(), text, &["y".to_string()]).is_empty(),
        "only the names asked for"
    );
}
