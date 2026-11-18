//! Error recovery over `examples/rust/rust.qana` keeps what typing breaks
//! where it was typed: each test is an ordinary keystroke that once took
//! the rest of a file down with it, and says what recovery does instead
//! (`docs/incremental_recovery.md`, "Ranking repairs").

use qana_grammar::{Repair, RepairKind};
use qana_lang::EmbeddedLang;
use std::sync::OnceLock;

const GRAMMAR: &str = include_str!("../../../examples/rust/rust.qana");

fn lang() -> &'static EmbeddedLang {
    static LANG: OnceLock<EmbeddedLang> = OnceLock::new();
    LANG.get_or_init(|| EmbeddedLang::from_qana_source(GRAMMAR).unwrap_or_else(|d| panic!("{d:?}")))
}

fn repairs(src: &str) -> Vec<Repair> {
    lang().session(src).last_repairs
}

fn token(name: &str) -> RepairKind {
    let id = lang().sg.term_names.iter().position(|n| n == name).expect("a token of the grammar");
    RepairKind::Inserted(id as u16)
}

/// An `x` typed after `{` before a statement that begins with a keyword:
/// `;` ends the expression statement `x`, where inserting `}` (which
/// reads as far, and leaves the stack shallower) closed the loop's body,
/// and every `}` after it closed one level too high.
#[test]
fn a_name_typed_before_a_statement_is_ended_not_closed_over() {
    let src = "fn f(v: V) {\n    for p in v {x\n        match p {\n            P::A(a) => {\n                debug_assert!(a.is_empty(), \"a\");\n                b.push(a);\n            }\n            _ => {}\n        }\n    }\n}\n\nfn g() {}\n";
    let r = repairs(src);
    assert_eq!(r.len(), 1, "one repair: {r:?}");
    assert_eq!(r[0].kind, token("SEMI"), "and it ends the statement");
}

/// An `x` typed into an array of pairs reads as an index (`x[3, 4]`) up to
/// the index's `,`. Inserting `]` there validates `, 4 ]` and then closes
/// the array at the real `]`: every later element broke. The repair stays
/// inside the index's brackets and inserts none.
#[test]
fn a_name_typed_into_an_array_of_pairs_stays_in_its_element() {
    let src = "static T: [[u8; 2]; 4] = [\n    [1, 2],x\n    [3, 4],\n    [5, 6],\n    [7, 8],\n];\n\nfn g() {}\n";
    let r = repairs(src);
    assert!(!r.is_empty() && r.len() <= 2, "one recovery of at most two steps: {r:?}");
    let at = r[0].at_terminal;
    assert!(
        r.iter().all(|x| x.at_terminal == at || x.at_terminal == at + 1),
        "made where the index's `,` is: {r:?}"
    );
    let brackets = ["LBRACK", "RBRACK", "LPAREN", "RPAREN", "LBRACE", "RBRACE"].map(token);
    assert!(r.iter().all(|x| !brackets.contains(&x.kind)), "no bracket inserted: {r:?}");
}

/// A stray name before an impl's closing `}` is set aside. Inserting
/// `! {` instead made the real `}` the closer of a fabricated macro call
/// and left the impl open to the end of the file.
#[test]
fn a_stray_name_before_a_closer_is_set_aside() {
    let src =
        "impl A {\n    fn f() {}\n    x\n}\n\n#[derive(Clone)]\npub struct B {\n    c: u8,\n}\n";
    let r = repairs(src);
    assert_eq!(r.iter().map(|x| x.kind.clone()).collect::<Vec<_>>(), [RepairKind::Unwound]);
}

/// A stray name before the next item is set aside too: inserting `! (`
/// made it a macro call whose fabricated `(` took every token after it,
/// the real brackets nested trees of their own, and read as valid to the
/// end of the file.
#[test]
fn a_stray_name_before_an_item_does_not_open_a_macro_call() {
    let src = "struct A;\nx\nimpl A {\n    fn f(&self) -> u8 {\n        if self.0 == 0 { 1 } else { 2 }\n    }\n}\n";
    let r = repairs(src);
    assert_eq!(r.iter().map(|x| x.kind.clone()).collect::<Vec<_>>(), [RepairKind::Unwound]);
}
