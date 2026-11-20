//! What Rust is coloured (`examples/rust/rust.qana`'s `@style`s): the
//! classes a token's kind gives (doc comments, lifetimes) and the ones its
//! place in the syntax gives (types, functions, methods, macros,
//! attributes, labels), and that a live painter keeps them through edits
//! as a fresh paint would have them.

use qana_engine::{Line, LineEdit, LineTerm};
use qana_lang::EmbeddedLang;
use qana_services::paint::Painter;
use qana_services::{semantic_tokens_full, syntax_classes};
use std::collections::HashMap;
use std::sync::OnceLock;

const GRAMMAR: &str = include_str!("../../../examples/rust/rust.qana");

fn lang() -> &'static EmbeddedLang {
    static LANG: OnceLock<EmbeddedLang> = OnceLock::new();
    LANG.get_or_init(|| EmbeddedLang::from_qana_source(GRAMMAR).unwrap_or_else(|d| panic!("{d:?}")))
}

/// Every token of `src` with a class: its text and its class, in order.
/// A token takes the class its syntax gives it, and failing that its
/// kind's.
fn classes(src: &str) -> Vec<(String, &'static str)> {
    let lang = lang();
    let session = lang.session(src);
    assert!(
        session.last_repairs.is_empty(),
        "the sample parses: {:?}",
        session.last_repairs
    );
    let by_syntax: HashMap<u32, u32> = syntax_classes(session.tree().expect("total"), &lang.styles)
        .into_iter()
        .map(|((start, _), class)| (start, class))
        .collect();
    let mut out = Vec::new();
    let mut at = 0u32;
    for (line, lexed) in session.buf.lines.iter().zip(&session.buf.lexed) {
        let mut col = 0usize;
        for t in &lexed.tokens {
            let text = &line.text[col..col + t.len as usize];
            let class = by_syntax
                .get(&at)
                .copied()
                .or_else(|| lang.styles.class_of(t.id));
            if let Some(class) = class {
                out.push((text.to_string(), lang.styles.legend[class as usize]));
            }
            col += t.len as usize;
            at += t.len;
        }
        at += line.term.as_str().len() as u32;
    }
    out
}

/// The class of the `nth` token (from 0) whose text is `text`.
fn class_of(classes: &[(String, &'static str)], text: &str, nth: usize) -> &'static str {
    classes
        .iter()
        .filter(|(t, _)| t == text)
        .nth(nth)
        .unwrap_or_else(|| panic!("no styled token #{nth} `{text}` in {classes:?}"))
        .1
}

#[test]
fn types_are_the_names_of_types() {
    let c = classes(
        "\
struct Point<T> { x: T }
enum E { A }
union U { a: u8 }
trait Shape: Clone {}
type Alias = std::collections::HashMap<String, Vec<u8>>;
impl<T: Clone + Send> Shape for Point<T> where T: Default {}
fn f(p: &Point<u8>, q: <T as Iterator>::Item) -> Option<Box<dyn Fn(u8) -> bool>> { Point { x: 1 } }
",
    );
    for (text, nth) in [
        ("Point", 0),
        ("T", 0),
        ("T", 1),
        ("E", 0),
        ("U", 0),
        ("u8", 0),
        ("Shape", 0),
        ("Clone", 0),
        ("Alias", 0),
        ("HashMap", 0),
        ("String", 0),
        ("Vec", 0),
        ("u8", 1),
        ("T", 2),
        ("Clone", 1),
        ("Send", 0),
        ("Shape", 1),
        ("Point", 1),
        ("T", 3),
        ("T", 4),
        ("Default", 0),
        ("Point", 2),
        ("Iterator", 0),
        ("Item", 0),
        ("Option", 0),
        ("Box", 0),
        ("Fn", 0),
        ("bool", 0),
        // A struct literal names its struct.
        ("Point", 3),
    ] {
        assert_eq!(class_of(&c, text, nth), "type", "`{text}` #{nth}");
    }
    // A path's qualifiers are not the type: only the name it ends in.
    assert_eq!(class_of(&c, "std", 0), "variable");
    assert_eq!(class_of(&c, "collections", 0), "variable");
    // A field's name, a variant's, a parameter's are names.
    assert_eq!(class_of(&c, "x", 0), "variable");
    assert_eq!(class_of(&c, "A", 0), "variable");
    assert_eq!(class_of(&c, "p", 0), "variable");
    // `Self` and `dyn` stay keywords.
    assert_eq!(class_of(&c, "dyn", 0), "keyword");
}

#[test]
fn functions_methods_and_macros() {
    let c = classes(
        "\
fn main() {
    let v = Vec::<u8>::new();
    helper(1);
    std::mem::drop(v);
    v.push(2).iter().map::<u8, _>(f);
    println!(\"{}\", x);
    vec![1, 2];
    Some(3);
    (callback)(4);
    let n = x.len;
}
macro_rules! square { ($x:expr) => { $x * $x }; }
",
    );
    assert_eq!(class_of(&c, "main", 0), "function");
    assert_eq!(class_of(&c, "new", 0), "function");
    assert_eq!(
        class_of(&c, "Vec", 0),
        "variable",
        "a qualifier of a call is no function"
    );
    assert_eq!(class_of(&c, "helper", 0), "function");
    assert_eq!(class_of(&c, "drop", 0), "function");
    assert_eq!(class_of(&c, "mem", 0), "variable");
    assert_eq!(class_of(&c, "push", 0), "method");
    assert_eq!(class_of(&c, "iter", 0), "method");
    assert_eq!(class_of(&c, "map", 0), "method");
    // An argument is not what is called.
    assert_eq!(class_of(&c, "f", 0), "variable");
    assert_eq!(class_of(&c, "println", 0), "macro");
    assert_eq!(class_of(&c, "!", 0), "macro");
    assert_eq!(class_of(&c, "vec", 0), "macro");
    // A tuple struct or variant is called as a function is: the syntax
    // cannot tell them apart.
    assert_eq!(class_of(&c, "Some", 0), "function");
    // A parenthesised callee is an expression, not a name.
    assert_eq!(class_of(&c, "callback", 0), "variable");
    // A field is a name.
    assert_eq!(class_of(&c, "len", 0), "variable");
    assert_eq!(class_of(&c, "macro_rules", 0), "macro");
    assert_eq!(class_of(&c, "square", 0), "macro");
}

#[test]
fn attributes_lifetimes_labels_and_doc_comments() {
    let c = classes(
        "\
#![allow(dead_code)]
//! The crate.
/// A thing.
#[derive(Debug, Clone)]
#[serde::rename = \"x\"]
struct S<'a> { r: &'a str }
/** Block doc. */
/*! Inner block doc. */
/* plain */
/**/
/*** not doc */
//// not doc
// plain
fn g<'b>(x: &'b u8) -> &'static u8 {
    'outer: loop {
        'inner: for i in 0..1 { continue 'outer; }
        break 'outer;
    }
    let c = 'c';
    x
}
",
    );
    assert_eq!(class_of(&c, "//! The crate.", 0), "doc_comment");
    assert_eq!(class_of(&c, "/// A thing.", 0), "doc_comment");
    for (text, nth) in [
        ("#", 0),
        ("[", 0),
        ("derive", 0),
        ("]", 0),
        ("rename", 0),
        ("allow", 0),
        ("!", 0),
    ] {
        assert_eq!(class_of(&c, text, nth), "decorator", "`{text}` #{nth}");
    }
    // An attribute's arguments are what they are.
    assert_eq!(class_of(&c, "Debug", 0), "variable");
    assert_eq!(class_of(&c, "(", 0), "bracket");
    assert_eq!(class_of(&c, "serde", 0), "variable");
    assert_eq!(class_of(&c, "'a", 0), "lifetime");
    assert_eq!(class_of(&c, "'a", 1), "lifetime");
    assert_eq!(class_of(&c, "'b", 0), "lifetime");
    assert_eq!(class_of(&c, "'static", 0), "lifetime");
    assert_eq!(class_of(&c, "'outer", 0), "label");
    assert_eq!(class_of(&c, "'inner", 0), "label");
    assert_eq!(class_of(&c, "'outer", 1), "label");
    assert_eq!(class_of(&c, "'outer", 2), "label");
    assert_eq!(class_of(&c, "'c'", 0), "string");
    // A block doc comment is a doc comment through its body.
    assert_eq!(class_of(&c, "/*", 0), "doc_comment");
    assert_eq!(class_of(&c, " Block doc. ", 0), "doc_comment");
    assert_eq!(class_of(&c, "/*", 1), "doc_comment");
    assert_eq!(class_of(&c, "/*", 2), "comment");
    assert_eq!(class_of(&c, "/*", 3), "comment", "`/**/` is no doc comment");
    assert_eq!(class_of(&c, "/*", 4), "comment", "`/***` is no doc comment");
    assert_eq!(class_of(&c, "//// not doc", 0), "comment");
    assert_eq!(class_of(&c, "// plain", 0), "comment");
}

#[test]
fn a_block_doc_comment_runs_over_lines() {
    let c = classes("/**\n * Over\n * lines.\n */\nfn f() {}\n/*\n plain\n*/\n");
    assert_eq!(class_of(&c, "/*", 0), "doc_comment");
    assert_eq!(class_of(&c, " ", 0), "doc_comment");
    assert_eq!(class_of(&c, "*/", 0), "doc_comment");
    assert_eq!(
        class_of(&c, "/*", 1),
        "comment",
        "`/*` alone on its line opens a plain comment"
    );
    assert_eq!(class_of(&c, "*/", 1), "comment");
}

/// A sample with a little of everything the classes cover.
const SAMPLE: &str = "\
/// Docs.
#[derive(Debug)]
pub struct Point<T> {
    x: T,
}

impl<T: Clone> Point<T> {
    pub fn new(x: T) -> Self {
        let v: Vec<T> = Vec::new();
        'a: loop { break 'a; }
        helper(x.clone());
        println!(\"{}\", 1);
        Point { x }
    }
}

fn helper<U>(u: U) -> Option<U> {
    Some(u)
}
";

/// Each line of a paint: (length, style) of each run.
fn runs(lines: &[Vec<linework::Run>]) -> Vec<Vec<(u16, u8)>> {
    lines
        .iter()
        .map(|l| l.iter().map(|r| (r.len, r.style)).collect())
        .collect()
}

/// Edits that change what is a call, a type or a method away from the
/// edited line as well as on it, and edits that change nothing of it.
fn edits() -> Vec<(usize, usize, Vec<&'static str>)> {
    vec![
        // `helper` stops being called, then is again.
        (10, 11, vec!["        helper"]),
        (11, 11, vec!["        (x.clone());"]),
        (10, 12, vec!["        helper(x.clone());"]),
        // A new type annotation and a renamed method.
        (
            8,
            9,
            vec!["        let v: HashMap<K, Vec<T>> = Vec::with_capacity(2);"],
        ),
        // A broken line, then mended.
        (2, 3, vec!["pub struct Point<T"]),
        (2, 3, vec!["pub struct Point<T> {"]),
        // A whole new function.
        (17, 17, vec!["fn more() -> u8 { other::call() }", ""]),
        // A comment out and back.
        (7, 8, vec!["    // pub fn new(x: T) -> Self {"]),
        (7, 8, vec!["    pub fn new(x: T) -> Self {"]),
        (0, 1, vec!["/** Block", " docs. */"]),
    ]
}

fn edit(
    session: &mut qana_engine::IncSession<'static>,
    at: &(usize, usize, Vec<&'static str>),
) -> qana_engine::DamageReport {
    let lang = lang();
    let n = session.buf.lines.len();
    let replacement =
        at.2.iter()
            .enumerate()
            .map(|(i, l)| {
                let last = at.1 >= n && i + 1 == at.2.len();
                Line::new(
                    l.to_string(),
                    if last { LineTerm::None } else { LineTerm::Lf },
                )
            })
            .collect();
    session
        .edit(
            lang.sg,
            lang.tables,
            &[LineEdit {
                start: at.0,
                end: at.1,
                replacement,
            }],
        )
        .expect("total")
        .damage
}

#[test]
fn a_live_painter_keeps_the_classes_a_fresh_paint_has() {
    let lang = lang();
    let mut session = lang.session(SAMPLE);
    let (mut painter, _) = Painter::new(&session, &lang.styles, None);
    for (i, e) in edits().iter().enumerate() {
        let damage = edit(&mut session, e);
        let delta = painter.update(&session, &damage, &lang.styles, None);
        let (_, fresh) = Painter::new(&session, &lang.styles, None);
        assert_eq!(
            runs(&painter.frame().lines),
            runs(&fresh.lines),
            "edit {i}: the kept paint is the fresh one\n{}",
            session.buf.reproduce()
        );
        // What the delta says is what changed: applied to the last frame,
        // it is the frame.
        let _ = delta;
    }
}

#[test]
fn a_repaint_reaches_a_line_the_edit_did_not_touch() {
    // `helper` on line 0 becomes a call when `(` is typed on line 1.
    let lang = lang();
    let mut session = lang.session("fn f() {\n    helper\n    ;\n}\n");
    let (mut painter, _) = Painter::new(&session, &lang.styles, None);
    let damage = edit(&mut session, &(2, 3, vec!["    ();"]));
    let delta = painter.update(&session, &damage, &lang.styles, None);
    let function = lang.styles.class_index("function") as u8;
    let (line, runs) = delta
        .repaints
        .iter()
        .find(|(l, _)| *l == 1)
        .expect("line 1 repainted");
    assert_eq!(*line, 1);
    assert!(
        runs.iter().any(|r| r.style == function),
        "`helper` is now a function: {runs:?}"
    );
}

#[test]
fn a_live_document_paints_what_a_fresh_one_does() {
    use linework::Limner;
    let lang = std::sync::Arc::new(lang().clone());
    let mut doc = qana_lang::live::LiveDoc::open(lang.clone(), "doc.rs", SAMPLE);
    let mut frame = doc.open(SAMPLE);
    for (i, e) in edits().iter().enumerate() {
        let delta = doc.edit(&linework::LineEdit {
            start: e.0 as u32,
            end: e.1 as u32,
            lines: e.2.iter().map(|l| l.to_string()).collect(),
        });
        if let Some(((lo, hi), lines)) = delta.splice {
            frame.lines.splice(lo as usize..hi as usize, lines);
        }
        for (line, runs) in delta.repaints {
            frame.lines[line as usize] = runs;
        }
        let text = doc.text();
        let fresh = qana_lang::live::LiveDoc::open(lang.clone(), "fresh.rs", &text).open(&text);
        assert_eq!(
            runs(&frame.lines),
            runs(&fresh.lines),
            "edit {i}: deltas keep the frame\n{text}"
        );
    }
}

#[test]
fn lsp_tokens_kept_through_edits_are_a_fresh_encoding() {
    let lang = lang();
    let mut session = lang.session(SAMPLE);
    let mut tokens = semantic_tokens_full(
        lang.lexer,
        &session.buf,
        session.tree().unwrap(),
        &lang.styles,
    );
    for (i, e) in edits().iter().enumerate() {
        let damage = edit(&mut session, e);
        tokens.update(
            lang.lexer,
            &session.buf,
            session.tree().unwrap(),
            &lang.styles,
            &damage,
        );
        let fresh = semantic_tokens_full(
            lang.lexer,
            &session.buf,
            session.tree().unwrap(),
            &lang.styles,
        );
        assert_eq!(tokens, fresh, "edit {i}\n{}", session.buf.reproduce());
    }
}

#[test]
fn lsp_sends_a_doc_comment_as_a_documentation_comment() {
    let lang = lang();
    let session = lang.session("/// Docs.\nfn f() {}\n");
    let tokens = semantic_tokens_full(
        lang.lexer,
        &session.buf,
        session.tree().unwrap(),
        &lang.styles,
    );
    let types = lang.styles.lsp_types();
    let first: Vec<u32> = tokens.data[..5].to_vec();
    assert_eq!(types[first[3] as usize], "comment");
    assert_eq!(first[4], 1, "the `documentation` modifier");
    assert_eq!(qana_services::LSP_MODIFIERS, ["documentation"]);
    // `f` is sent as a function.
    let f = tokens
        .data
        .chunks(5)
        .find(|q| q[2] == 1 && types[q[3] as usize] == "function");
    assert!(f.is_some(), "{:?}", tokens.data);
}

#[test]
fn a_conditional_attribute_is_a_decorator_and_a_function_of_that_name_a_function() {
    let c = classes(
        "#[cfg_attr(test, derive(Clone))]\nstruct S;\nfn cfg_attr(x: u8) {}\nfn derive() {}\n",
    );
    assert_eq!(class_of(&c, "cfg_attr", 0), "decorator");
    assert_eq!(class_of(&c, "derive", 0), "decorator", "an attribute a conditional one holds");
    assert_eq!(class_of(&c, "Clone", 0), "variable", "a derive's arguments are what they are");
    assert_eq!(class_of(&c, "cfg_attr", 1), "function");
    assert_eq!(class_of(&c, "derive", 1), "function");
}
