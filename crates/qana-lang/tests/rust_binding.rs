//! `examples/rust/rust.qana` binds names within a file as rustc resolves
//! them, where one file can tell: a `let` reads the binding before it, a
//! condition's bindings reach its block and not its `else`, items are
//! visible throughout their block, members are reached by path only, a
//! module does not see its parent's items, and labels, lifetimes and
//! macros stand in namespaces of their own. A name the file cannot place
//! (the preludes, other files and crates, `Self::`, `crate::`) escapes:
//! it is never a problem.

use qana_engine::IncSession;
use qana_lang::EmbeddedLang;
use qana_sem::SemDb;
use std::sync::OnceLock;

const GRAMMAR: &str = include_str!("../../../examples/rust/rust.qana");

fn lang() -> &'static EmbeddedLang {
    static LANG: OnceLock<EmbeddedLang> = OnceLock::new();
    LANG.get_or_init(|| EmbeddedLang::from_qana_source(GRAMMAR).unwrap_or_else(|d| panic!("{d:?}")))
}

struct Doc {
    db: SemDb,
    text: String,
}

impl Doc {
    fn new(text: &str) -> Doc {
        let l = lang();
        let session = IncSession::new(l.lexer, l.sg, l.tables, text).unwrap();
        assert!(session.last_repairs.is_empty(), "parses without repair: {text}");
        let mut db = SemDb::new(l.binding.clone());
        db.set_tree("d", session.tree().unwrap().clone());
        Doc { db, text: text.to_string() }
    }

    /// The offset of the `nth` (from 0) occurrence of `word` as a whole
    /// word.
    fn at(&self, word: &str, nth: usize) -> u32 {
        let bytes = self.text.as_bytes();
        let ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
        self.text
            .match_indices(word)
            .map(|(i, _)| i)
            .filter(|&i| {
                let before = i == 0 || !ident(bytes[i - 1]);
                let after = i + word.len() == bytes.len() || !ident(bytes[i + word.len()]);
                before && after
            })
            .nth(nth)
            .unwrap_or_else(|| panic!("no occurrence {nth} of `{word}` in {:?}", self.text))
            as u32
    }

    /// Where occurrence `nth` of `word` is defined, as the occurrence
    /// number of the same word there (`None`: nowhere the file says).
    fn goes_to(&mut self, word: &str, nth: usize) -> Option<usize> {
        let at = self.at(word, nth);
        let (_, span) = self.db.definition("d", at)?;
        (0..).find(|&k| self.at(word, k) == span.0)
    }

    fn escapes(&mut self, word: &str, nth: usize) -> bool {
        let at = self.at(word, nth);
        self.db.escapes("d", at)
    }

    /// No name is a problem: nothing unresolved, no path error.
    fn no_problems(&mut self) {
        assert!(self.db.unresolved("d").is_empty(), "{:?}", self.db.unresolved("d"));
        assert!(self.db.qualified_errors("d").is_empty(), "{:?}", self.db.qualified_errors("d"));
    }
}

#[test]
fn a_let_reads_the_binding_before_it() {
    let mut doc = Doc::new("fn f(line: &str) { let line = line.trim(); let n = line.len(); }\n");
    assert_eq!(doc.goes_to("line", 2), Some(0), "the right side reads the parameter");
    assert_eq!(doc.goes_to("line", 3), Some(1), "after it, the `let`");
    doc.no_problems();
}

#[test]
fn a_conditions_bindings_reach_its_block_and_not_its_else() {
    let mut doc = Doc::new("fn f(x: Option<u8>) { if let Some(x) = x { x; } else { x; } x; }\n");
    assert_eq!(doc.goes_to("x", 2), Some(0), "the scrutinee is read before the pattern binds");
    assert_eq!(doc.goes_to("x", 3), Some(1), "the block sees the pattern's `x`");
    assert_eq!(doc.goes_to("x", 4), Some(0), "the `else` does not");
    assert_eq!(doc.goes_to("x", 5), Some(0), "nor what follows");
    doc.no_problems();
}

#[test]
fn a_let_chains_bindings_reach_the_conditions_after_them() {
    let mut doc = Doc::new("fn f(a: Option<u8>) { if let Some(v) = a && v > 1 { v; } }\n");
    assert_eq!(doc.goes_to("v", 1), Some(0));
    assert_eq!(doc.goes_to("v", 2), Some(0));
}

#[test]
fn a_match_arm_scopes_its_pattern_over_its_guard_and_body() {
    let mut doc = Doc::new("fn f(v: u8) -> u8 { match v { n if n > 1 => n, n => n + 1 } }\n");
    assert_eq!(doc.goes_to("n", 1), Some(0), "the guard");
    assert_eq!(doc.goes_to("n", 2), Some(0), "the body");
    assert_eq!(doc.goes_to("n", 4), Some(3), "the next arm's own");
}

#[test]
fn a_loop_reads_what_it_iterates_before_it_binds() {
    let mut doc = Doc::new("fn f(i: Vec<u8>) { for i in i { i; } i; }\n");
    assert_eq!(doc.goes_to("i", 2), Some(0), "`in i` is the parameter");
    assert_eq!(doc.goes_to("i", 3), Some(1), "the body sees the loop's `i`");
    assert_eq!(doc.goes_to("i", 4), Some(0), "after the loop it is gone");
}

#[test]
fn an_item_is_visible_throughout_its_block_and_a_let_only_after_it() {
    let mut doc = Doc::new("fn f() { g(); fn g() {} let a = b; let b = 1; b; }\n");
    assert_eq!(doc.goes_to("g", 0), Some(1), "an item before its declaration");
    assert_eq!(doc.goes_to("b", 0), None, "a `let` not before it");
    assert!(doc.escapes("b", 0), "and the name escapes: it may be an item elsewhere");
    assert_eq!(doc.goes_to("b", 2), Some(1));
    doc.no_problems();
}

#[test]
fn generic_parameters_are_visible_throughout_their_item() {
    let mut doc = Doc::new("fn f<T: Into<U>, U>(t: T) -> U { t.into() }\n");
    assert_eq!(doc.goes_to("U", 0), Some(1), "a bound names a parameter declared after it");
    assert_eq!(doc.goes_to("T", 1), Some(0));
    assert_eq!(doc.goes_to("U", 2), Some(1));
}

#[test]
fn members_are_reached_by_path_and_never_by_a_plain_name() {
    let mut doc = Doc::new(
        "struct S { width: f32 }\n\
         impl S {\n    fn new() -> S { new(); S::new() }\n    fn width(&self) -> f32 { self.width }\n}\n\
         enum E { A, B(u8) }\n\
         fn f() -> E { A; E::A }\n",
    );
    assert_eq!(doc.goes_to("new", 1), None, "a method is not a name inside its impl");
    assert!(doc.escapes("new", 1));
    assert!(doc.escapes("new", 2), "`S::new` waits on the types (an impl is no module)");
    assert_eq!(doc.goes_to("self", 1), Some(0), "`self` is the receiver");
    assert_eq!(doc.goes_to("A", 1), None, "a variant is not a plain name");
    assert_eq!(doc.goes_to("A", 2), Some(0), "`E::A` is the variant");
    doc.no_problems();
}

#[test]
fn a_module_sees_its_own_items_and_not_its_parents() {
    let mut doc = Doc::new(
        "fn helper() {}\n\
         mod m {\n    pub fn f() { helper(); inner(); }\n    fn inner() {}\n}\n\
         fn g() { m::f(); m::inner(); }\n",
    );
    assert_eq!(doc.goes_to("helper", 1), None, "the parent's item, unless imported");
    assert!(doc.escapes("helper", 1));
    assert_eq!(doc.goes_to("inner", 0), Some(1));
    assert_eq!(doc.goes_to("f", 1), Some(0), "`m::f`");
    assert_eq!(doc.goes_to("inner", 2), Some(1), "`m::inner`: a member, whatever its visibility");
    doc.no_problems();
}

#[test]
fn a_use_leaf_defines_its_name_and_its_path_escapes() {
    let mut doc = Doc::new(
        "use std::collections::HashMap;\n\
         use std::fmt::{self, Display as Shown};\n\
         fn f(m: HashMap<u8, u8>) -> impl Shown { HashMap::new() }\n",
    );
    assert_eq!(doc.goes_to("HashMap", 1), Some(0));
    assert_eq!(doc.goes_to("HashMap", 2), Some(0), "the base of `HashMap::new`");
    assert!(doc.escapes("new", 0), "what it imports has members the file cannot see");
    assert!(doc.escapes("std", 0));
    assert_eq!(doc.goes_to("Shown", 1), Some(0), "a rename defines the new name");
    doc.no_problems();
}

#[test]
fn labels_lifetimes_and_macros_stand_in_namespaces_of_their_own() {
    let mut doc = Doc::new(
        "macro_rules! twice { ($e:expr) => { $e * 2 }; }\n\
         fn twice<'a>(x: &'a u8) -> &'a u8 { 'a: loop { break 'a; } twice!(1); x }\n\
         fn r() -> &'static str { \"\" }\n",
    );
    assert_eq!(doc.goes_to("'a", 1), Some(0), "a lifetime is the parameter");
    assert_eq!(doc.goes_to("'a", 2), Some(0));
    assert_eq!(doc.goes_to("'a", 4), Some(3), "a label is the label, not the lifetime");
    assert_eq!(doc.goes_to("twice", 2), Some(0), "`twice!` is the macro, not the function");
    assert!(doc.escapes("'static", 0), "`'static` is declared nowhere");
    doc.no_problems();
}

#[test]
fn a_closure_scopes_its_parameters() {
    let mut doc = Doc::new("fn f(x: u8) { let g = |x: u8| x + 1; x; g(x); }\n");
    assert_eq!(doc.goes_to("x", 2), Some(1), "the closure's own");
    assert_eq!(doc.goes_to("x", 3), Some(0), "the function's");
    assert_eq!(doc.goes_to("g", 1), Some(0));
}

#[test]
fn a_struct_literals_shorthand_reads_the_value() {
    let mut doc = Doc::new("struct P { x: u8 }\nfn f(x: u8) -> P { P { x } }\n");
    assert_eq!(doc.goes_to("x", 2), Some(1), "`P {{ x }}` reads the parameter, not the field");
    assert_eq!(doc.goes_to("P", 2), Some(0));
}

#[test]
fn an_underscore_names_nothing() {
    let mut doc = Doc::new(
        "const _: () = ();\n\
         fn f() { let _ = 1; let v: Vec<_> = Vec::new(); match 1 { _ => {} } _ = v; }\n",
    );
    let syms = doc.db.symbols("d");
    assert!(syms.defs.iter().all(|d| d.name != "_"), "{:?}", syms.defs);
    assert!(syms.refs.iter().all(|r| r.name != "_"), "{:?}", syms.refs);
}

#[test]
fn paths_the_file_cannot_place_escape_without_a_problem() {
    let mut doc = Doc::new(
        "struct T;\nimpl T {\n    fn f(&self) { Self::g(); crate::a::b(); super::c(); <u8>::max_value(); \
         Vec::<u8>::new(); ::std::mem::drop(1); T::h(); }\n}\n",
    );
    for (word, nth) in [("g", 0), ("b", 0), ("c", 0), ("new", 0), ("drop", 0), ("h", 0)] {
        assert!(doc.escapes(word, nth), "{word}");
    }
    doc.no_problems();
}

/// One of this repository's own large files: no name is a problem, and
/// most of its references bind or escape (none is left unresolved).
#[test]
fn a_large_file_of_real_rust_has_no_false_problems() {
    let text = include_str!("../../qana-sem/src/lib.rs");
    let mut doc = Doc::new(text);
    doc.no_problems();
    let syms = doc.db.symbols("d");
    let res = doc.db.resolve("d");
    assert!(syms.refs.len() > 3000, "{}", syms.refs.len());
    let bound = res.iter().filter(|t| matches!(t, qana_sem::Target::Local { .. })).count();
    assert!(
        bound * 2 > syms.refs.len(),
        "most names the file declares bind: {bound} of {}",
        syms.refs.len()
    );
    assert!(
        res.iter().all(|t| *t != qana_sem::Target::Unresolved),
        "every reference binds or escapes"
    );
}
