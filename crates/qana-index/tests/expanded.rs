//! A file read as expanded: its invocations wherever they stand, each
//! expanded to text, put in its place with `macro_rules!` hygiene, and its
//! readers' places mapped back to the file.

use std::sync::OnceLock;

use qana_index::{
    expand_text, expanded_file, invocations, members, references, Edition, ExpandRequest,
    ExpandedText, Form, InvocationAt, MacroSource, Path, Root, RECURSION_LIMIT,
};
use qana_lang::EmbeddedLang;

const GRAMMAR: &str = include_str!("../../../examples/rust/rust.qana");

fn rust() -> &'static EmbeddedLang {
    static LANG: OnceLock<EmbeddedLang> = OnceLock::new();
    LANG.get_or_init(|| EmbeddedLang::from_qana_source(GRAMMAR).expect("rust.qana certifies"))
}

/// A definition whose body is `body`.
fn text(body: &str) -> MacroSource {
    MacroSource::Text {
        body: body.to_string(),
        form: Form::MacroRules,
        edition: Edition::E2021,
        local_inner: false,
    }
}

/// `inv`, an invocation of `file`, expanded by `body`, with `table`.
fn expanded(
    file: &str,
    inv: &InvocationAt,
    body: &str,
    table: &[(Path, Vec<MacroSource>)],
) -> ExpandedText {
    let request = ExpandRequest {
        definition: text(body),
        site: file[inv.span.0 as usize..inv.span.1 as usize].to_string(),
        site_edition: Edition::E2021,
        input: None,
        entry: inv.entry.clone(),
        mark: 1,
        limit: RECURSION_LIMIT as u32,
        local_inner: Vec::new(),
        of_crate: Vec::new(),
        itself: None,
    };
    expand_text(rust(), &request, table).expect("it expands")
}

/// Where `needle`'s `nth` occurrence starts in `text`.
fn at(text: &str, needle: &str, nth: usize) -> u32 {
    text.match_indices(needle).nth(nth).expect("the needle").0 as u32
}

const FILE: &str = "macro_rules! check {
    ($a:expr, $b:expr) => {
        match (&$a, &$b) {
            (left, right) => {
                if !(*left == *right) {
                    fail()
                }
            }
        }
    };
}

macro_rules! bind {
    ($n:ident) => {
        let $n = Cell::new();
    };
}

check!(1, 1);

fn f(c: Cell) -> Grid<vec_of!(u8)> {
    check!(c.width(), 3);
    let left = 1;
    check!(left, 2);
    let n = vec![c, other];
    bind!(x);
    x.width()
}
";

const CHECK: &str = " {
    ($a:expr, $b:expr) => {
        match (&$a, &$b) {
            (left, right) => {
                if !(*left == *right) {
                    fail()
                }
            }
        }
    };
}";

const BIND: &str = " {
    ($n:ident) => {
        let $n = Cell::new();
    };
}";

#[test]
fn a_files_invocations_are_every_one_it_writes_wherever_it_stands() {
    let found = invocations(rust(), FILE);
    let read: Vec<(String, &str, bool)> = found
        .iter()
        .map(|i| {
            let path = i.path.as_ref().map(|p| p.segments.join("::")).unwrap_or_default();
            (path, i.entry.as_str(), i.among_items)
        })
        .collect();
    assert_eq!(
        read,
        [
            ("check".to_string(), "items_opt", true),
            ("vec_of".to_string(), "type", false),
            ("check".to_string(), "block_stmts", false),
            ("check".to_string(), "block_stmts", false),
            ("vec".to_string(), "attrs_expr", false),
            ("bind".to_string(), "block_stmts", false),
        ],
        "an item, a type, statements, an expression"
    );
    let first = &found[2];
    assert_eq!(&FILE[first.span.0 as usize..first.span.1 as usize], "check!(c.width(), 3)");
    assert_eq!(first.units, Vec::<String>::new());
    let seen = first.seen.as_ref().expect("a macro's name sees by textual scope");
    assert_eq!(seen.len(), 2, "`check` and `bind`, defined before it: {seen:?}");
}

/// [`FILE`] read as expanded, its `check!`s and `bind!` expanded.
fn file_expanded() -> qana_index::ExpandedFile {
    let found = invocations(rust(), FILE);
    let parts: Vec<(InvocationAt, ExpandedText)> = found
        .iter()
        // The item-position `check!(1, 1);` writes no items: rustc
        // refuses it, and it is left as written.
        .filter(|i| !i.among_items)
        .filter(|i| {
            i.path.as_ref().is_some_and(|p| p.segments == ["check"] || p.segments == ["bind"])
        })
        .map(|i| {
            let body = if i.path.as_ref().unwrap().segments == ["bind"] { BIND } else { CHECK };
            (i.clone(), expanded(FILE, i, body, &[]))
        })
        .collect();
    expanded_file(rust(), FILE, &parts)
}

#[test]
fn names_in_arguments_are_read_where_the_expansion_puts_them_and_placed_in_the_file() {
    let file = file_expanded();
    let names: Vec<String> =
        ["c", "left", "x", "fail", "Cell"].iter().map(|s| s.to_string()).collect();
    let refs = file.references(FILE, references(rust(), file.text(), &names));
    let placed: Vec<(&str, u32)> =
        refs.iter().map(|r| (&FILE[r.span.0 as usize..r.span.1 as usize], r.span.0)).collect();
    // `c` in `check!(c.width(), 3)` is read as a path, at its place.
    let c = at(FILE, "c.width(), 3", 0);
    assert!(placed.contains(&("c", c)), "{placed:?}");
    let r = refs.iter().find(|r| r.span.0 == c).unwrap();
    assert_eq!(
        r.binds,
        Some((at(FILE, "c: Cell", 0), at(FILE, "c: Cell", 0) + 1)),
        "the parameter"
    );
    assert_eq!(r.line, 21, "its line in the file");
    // What the macro writes (`fail`, `Cell`) is no place the file writes.
    assert!(!placed.iter().any(|(name, _)| *name == "fail"), "{placed:?}");

    // `.width()` inside it is a member access, at its place.
    let accesses = file.members(members(rust(), file.text()));
    let width = at(FILE, "width(), 3", 0);
    assert!(accesses.iter().any(|a| a.span == (width, width + 5)), "{accesses:?}");
}

#[test]
fn a_name_the_invocation_writes_never_binds_to_a_local_the_macro_writes() {
    let file = file_expanded();
    assert!(file.text().contains("left__hyg"), "the macro's `left` takes a name of its own");
    let names = vec!["left".to_string()];
    let refs = file.references(FILE, references(rust(), file.text(), &names));
    let site = at(FILE, "left, 2", 0);
    let r = refs.iter().find(|r| r.span.0 == site).expect("`left` in `check!(left, 2)`");
    let def = at(FILE, "left = 1", 0);
    assert_eq!(r.binds, Some((def, def + 4)), "the `let left` before it, not the macro's pattern");
}

#[test]
fn a_statement_macros_binding_of_a_name_the_invocation_writes_binds_the_code_after_it() {
    let file = file_expanded();
    let names = vec!["x".to_string()];
    let refs = file.references(FILE, references(rust(), file.text(), &names));
    let after = at(FILE, "x.width()", 0);
    let r = refs.iter().find(|r| r.span.0 == after).expect("`x` after `bind!(x)`");
    let def = at(FILE, "bind!(x)", 0) + "bind!(".len() as u32;
    assert_eq!(r.binds, Some((def, def + 1)), "the `x` the invocation wrote");
}

#[test]
fn a_byte_of_the_file_maps_to_where_its_expansion_holds_it() {
    let file = file_expanded();
    let c = at(FILE, "c.width(), 3", 0);
    let there = file.in_text(c).expect("held");
    assert_eq!(&file.text()[there as usize..there as usize + 1], "c");
    assert_eq!(file.in_file((there, there + 1)), Some(((c, c + 1), true)));
    assert!(file.expands(c));
    let outside = at(FILE, "fn f", 0);
    assert!(!file.expands(outside));
    let mapped = file.in_text(outside).expect("the file's own text");
    assert_eq!(&file.text()[mapped as usize..mapped as usize + 4], "fn f");
    // A token the macro writes is at the call (the first `fail` is the
    // definition's, which the file writes).
    let fail = at(file.text(), "fail", 1);
    let call = at(FILE, "check!(c.width()", 0);
    assert_eq!(file.in_file((fail, fail + 4)), Some(((call, call + 5), false)));
}

#[test]
fn an_invocation_by_a_path_the_table_holds_is_expanded_in_its_place() {
    let file = "fn g() { outer!(a, b); }\n";
    let found = invocations(rust(), file);
    let outer = " { ($($e:expr),*) => { $( $crate::inner!($e); )* }; }";
    let inner = " { ($e:expr) => { take($e); }; }";
    let table = vec![(
        Path { root: Some(Root::MacroCrate), segments: vec!["inner".to_string()] },
        vec![text(inner)],
    )];
    let made = expanded(file, &found[0], outer, &table);
    assert!(made.text().contains("take"), "{}", made.text());
    assert!(!made.text().contains("inner"), "{}", made.text());
    let read = expanded_file(rust(), file, &[(found[0].clone(), made)]);
    let names = vec!["a".to_string(), "b".to_string()];
    let refs = read.references(file, references(rust(), read.text(), &names));
    let spans: Vec<u32> = refs.iter().map(|r| r.span.0).collect();
    assert_eq!(spans, [at(file, "a,", 0), at(file, "b)", 0)]);
}

const NESTED: &str = "fn h(a: u8, b: u8) {
    check!(twice!(a), pair!(b, a));
    square!(twice!(b));
}
";

const TWICE: &str = " { ($e:expr) => { $e + $e }; }";

const PAIR: &str = " { ($x:expr, $y:expr) => { ($x, $y) }; }";

const SQUARE: &str = " { ($e:expr) => { $e * $e }; }";

/// [`NESTED`]'s `check!` and `square!` expanded, and with them those of
/// the invocations they report in their arguments that `inner` names.
fn nested_read(inner: &[&str]) -> qana_index::ExpandedFile {
    let found = invocations(rust(), NESTED);
    let body = |name: &str| match name {
        "check" => CHECK,
        "square" => SQUARE,
        "twice" => TWICE,
        _ => PAIR,
    };
    let name = |inv: &InvocationAt| inv.path.as_ref().unwrap().segments[0].clone();
    let mut parts: Vec<(InvocationAt, ExpandedText)> = found
        .iter()
        .map(|inv| (inv.clone(), expanded(NESTED, inv, body(&name(inv)), &[])))
        .collect();
    let first = expanded_file(rust(), NESTED, &parts);
    for inv in first.inner() {
        if inner.contains(&name(inv).as_str()) {
            parts.push((inv.clone(), expanded(NESTED, inv, body(&name(inv)), &[])));
        }
    }
    expanded_file(rust(), NESTED, &parts)
}

#[test]
fn the_invocations_in_an_expansions_arguments_are_said_placed_in_the_file() {
    let file = nested_read(&[]);
    let said: Vec<(&str, (u32, u32), &str)> = file
        .inner()
        .iter()
        .map(|i| (i.path.as_ref().unwrap().segments[0].as_str(), i.span, i.entry.as_str()))
        .collect();
    let twice = at(NESTED, "twice!(a)", 0);
    let pair = at(NESTED, "pair!(b, a)", 0);
    let again = at(NESTED, "twice!(b)", 0);
    assert_eq!(
        said,
        [
            ("twice", (twice, twice + 9), "attrs_expr"),
            ("pair", (pair, pair + 11), "attrs_expr"),
            ("twice", (again, again + 9), "attrs_expr"),
        ],
        "each once, where the file writes it, read where its expansion puts it, though \
         `square!` copies its argument twice"
    );
    assert_eq!(file.inner()[0].units, Vec::<String>::new());
}

#[test]
fn an_invocation_in_an_expansions_arguments_is_expanded_where_it_is_copied() {
    let file = nested_read(&["twice", "pair"]);
    assert!(file.inner().is_empty(), "nothing left: {:?}", file.inner());
    // `square!` copies `twice!(b)` twice: each copy holds its expansion.
    assert_eq!(file.text().matches(" + ").count(), 3, "{}", file.text());
    let names = vec!["a".to_string(), "b".to_string()];
    let refs = file.references(NESTED, references(rust(), file.text(), &names));
    let placed: Vec<u32> = refs.iter().map(|r| r.span.0).collect();
    assert_eq!(
        placed,
        [
            at(NESTED, "a), pair", 0),
            at(NESTED, "b, a)", 0),
            at(NESTED, "a));", 0),
            at(NESTED, "b));", 0)
        ],
        "each name the arguments hold, once, at its place"
    );
    let params = [at(NESTED, "a: u8", 0), at(NESTED, "b: u8", 0)];
    assert!(refs.iter().all(|r| r.binds.is_some_and(|b| params.contains(&b.0))), "{refs:?}");
    // A token `twice!` writes is at its call, not `check!`'s.
    let plus = at(file.text(), " + ", 0) + 1;
    let call = at(NESTED, "twice", 0);
    assert_eq!(file.in_file((plus, plus + 1)), Some(((call, call + 5), false)));
    let a = at(NESTED, "a), pair", 0);
    let there = file.in_text(a).expect("held");
    assert_eq!(&file.text()[there as usize..there as usize + 1], "a");
}

#[test]
fn an_invocation_not_given_stays_tokens_and_is_said_again() {
    let file = nested_read(&["twice"]);
    let left: Vec<&str> =
        file.inner().iter().map(|i| i.path.as_ref().unwrap().segments[0].as_str()).collect();
    assert_eq!(left, ["pair"]);
    let names = vec!["b".to_string()];
    let refs = file.references(NESTED, references(rust(), file.text(), &names));
    assert_eq!(
        refs.iter().map(|r| r.span.0).collect::<Vec<_>>(),
        [at(NESTED, "b));", 0)],
        "`pair!`'s `b` is tokens"
    );
}
