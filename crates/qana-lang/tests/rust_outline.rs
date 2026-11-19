//! The outline of Rust (`examples/rust/rust.qana`'s `@outline`s): what a
//! construct is called and what kind it is, nested as written. A
//! construct's definition sits on the construct, not inside the symbol its
//! `@outline` labels, so each is named by the text of that symbol: a
//! function by its `name`, an impl by its type.

use qana_lang::EmbeddedLang;

const GRAMMAR: &str = include_str!("../../../examples/rust/rust.qana");

/// Each outline entry of `src`: its kind, its name, and how deeply it is
/// nested.
fn outline(src: &str) -> Vec<(&'static str, String, usize)> {
    let lang = EmbeddedLang::from_qana_source(GRAMMAR).unwrap_or_else(|d| panic!("{d:?}"));
    let session = lang.session(src);
    assert!(session.last_repairs.is_empty(), "the sample parses: {:?}", session.last_repairs);
    let mut open_ends: Vec<u32> = Vec::new();
    qana_services::outline(session.tree().expect("total"), &lang.outline)
        .into_iter()
        .map(|s| {
            while open_ends.last().is_some_and(|&e| s.span.0 >= e) {
                open_ends.pop();
            }
            let depth = open_ends.len();
            open_ends.push(s.span.1);
            assert_eq!(&src[s.selection.0 as usize..s.selection.1 as usize], s.name, "{s:?}");
            (s.kind, s.name, depth)
        })
        .collect()
}

#[test]
fn rust_outlines_items_by_what_they_are_called() {
    let src = "\
pub mod geometry {
    pub struct Point<T> {
        x: T,
    }

    pub enum Shape {
        Dot(Point<u8>),
        Line { a: u8 },
    }

    pub trait Area {
        fn area(&self) -> f64;
    }

    impl<T> Area for Point<T> {
        fn area(&self) -> f64 {
            0.0
        }
    }

    const ORIGIN: u8 = 0;
    static NAME: &str = \"g\";
    type Pair = (u8, u8);
}

macro_rules! twice {
    ($e:expr) => {
        $e * 2
    };
}

fn main() {
    fn inner() {}
}
";
    let want: Vec<(&str, &str, usize)> = vec![
        ("module", "geometry", 0),
        ("struct", "Point", 1),
        ("struct", "Shape", 1),
        ("constant", "Dot", 2),
        ("constant", "Line", 2),
        ("class", "Area", 1),
        ("function", "area", 2),
        ("class", "Point<T>", 1),
        ("function", "area", 2),
        ("constant", "ORIGIN", 1),
        ("constant", "NAME", 1),
        ("struct", "Pair", 1),
        ("function", "twice", 0),
        ("function", "main", 0),
        ("function", "inner", 1),
    ];
    let got = outline(src);
    let got: Vec<(&str, &str, usize)> = got.iter().map(|(k, n, d)| (*k, n.as_str(), *d)).collect();
    assert_eq!(got, want);
}
