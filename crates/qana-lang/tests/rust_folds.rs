//! What folds in Rust (`examples/rust/rust.qana`'s `@fold`s, and
//! comments over lines): each fold by its kind, its head and the lines it
//! spans, over a sample of the constructs that fold.

use qana_lang::EmbeddedLang;

const GRAMMAR: &str = include_str!("../../../examples/rust/rust.qana");

/// Each fold of `src`: its kind, its head's text, and its first and last
/// lines (from 1).
fn folds(src: &str) -> Vec<(String, String, usize, usize)> {
    let lang = EmbeddedLang::from_qana_source(GRAMMAR).unwrap_or_else(|d| panic!("{d:?}"));
    let session = lang.session(src);
    assert!(session.last_repairs.is_empty(), "the sample parses: {:?}", session.last_repairs);
    let line = |off: u32| src[..off as usize].matches('\n').count() + 1;
    qana_services::folds(session.tree().expect("total"), &lang.folds)
        .into_iter()
        .map(|f| {
            (
                lang.folds.legend[f.category as usize].clone(),
                src[f.head.0 as usize..f.head.1 as usize].to_string(),
                line(f.span.0),
                line(f.span.1 - 1),
            )
        })
        .collect()
}

fn fold(kind: &str, head: &str, first: usize, last: usize) -> (String, String, usize, usize) {
    (kind.to_string(), head.to_string(), first, last)
}

#[test]
fn rust_folds_bodies_lists_strings_and_comments() {
    let src = "\
//! A module doc,
//! over two lines.

use std::collections::{
    HashMap,
    HashSet,
};

/* a block
   comment */
pub struct Point<
    T,
> {
    x: T,
    y: T,
}

impl<T> Point<T> {
    fn new(
        x: T,
        y: T,
    ) -> Self {
        let s = \"a
b\";
        match x {
            _ => Point { x, y },
        }
    }
}

fn one_line() { f(1, 2); }

macro_rules! m {
    () => {};
}
";
    assert_eq!(
        folds(src),
        [
            fold("comment", "//! A module doc,", 1, 2),
            fold("list", "{", 4, 7),
            fold("comment", "/*", 9, 10),
            fold("list", "<", 11, 13),
            fold("block", "{", 13, 16),
            fold("block", "{", 18, 29),
            fold("list", "(", 19, 22),
            fold("block", "{", 22, 28),
            fold("string", "\"", 23, 24),
            fold("block", "{", 25, 27),
            fold("block", "{", 33, 35),
        ],
        "a construct on one line folds nothing"
    );
}

/// Calls, arrays, struct literals, closures' blocks, and a macro's token
/// trees, in each expression tier.
#[test]
fn rust_folds_expressions() {
    let src = "\
fn f() {
    let a = [
        1,
    ];
    g(
        a,
    );
    if h(
        1,
    ) {
        x.m(
            2,
        )
    } else {
        S {
            a: 1,
        }
    }
    vec![
        3,
    ];
    let c = |x| {
        x
    };
}
";
    assert_eq!(
        folds(src),
        [
            fold("block", "{", 1, 25),
            fold("list", "[", 2, 4),
            fold("list", "(", 5, 7),
            fold("list", "(", 8, 10),
            fold("block", "{", 10, 14),
            fold("list", "(", 11, 13),
            fold("block", "{", 14, 18),
            fold("list", "{", 15, 17),
            fold("list", "[", 19, 21),
            fold("block", "{", 22, 24),
        ]
    );
}
