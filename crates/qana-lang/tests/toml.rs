//! TOML (`examples/toml/toml.qana`): what it reads and what it refuses, as
//! TOML 1.1.0 says (toml.io/en/v1.1.0); what it colours, outlines and
//! folds; and that an edit reads as the edited text read afresh, an
//! array's line going on over the lines the array runs over.

use linework::{Limner, LineEdit};
use qana_grammar::green::{GreenChild, GreenNode};
use qana_lang::live::LiveDoc;
use qana_lang::EmbeddedLang;
use qana_services::syntax_classes;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

const GRAMMAR: &str = include_str!("../../../examples/toml/toml.qana");
const SAMPLE: &str = include_str!("../../../examples/toml/sample.toml");

fn lang() -> &'static Arc<EmbeddedLang> {
    static LANG: OnceLock<Arc<EmbeddedLang>> = OnceLock::new();
    LANG.get_or_init(|| {
        Arc::new(EmbeddedLang::from_qana_source(GRAMMAR).unwrap_or_else(|d| panic!("{d:?}")))
    })
}

/// The errors reading `src` finds, each as the text it is at and its
/// message.
fn errors(src: &str) -> Vec<(String, String)> {
    let lang = lang();
    let session = lang.session(src);
    qana_services::diagnostics(lang.lexer, &session.buf, lang.sg, &session.last_repairs)
        .into_iter()
        .map(|d| {
            let (start, end) = (d.span.0 as usize, (d.span.1 as usize).min(src.len()));
            (src[start..end.max(start)].to_string(), d.message)
        })
        .collect()
}

#[test]
fn toml_reads_every_form_of_the_sample() {
    assert_eq!(errors(SAMPLE), Vec::<(String, String)>::new());
}

#[test]
fn toml_reads_what_the_spec_allows() {
    for src in [
        "",
        "# only a comment\n",
        "3.14159 = \"pi\"\n",
        "1979-05-27 = 1\n",
        "true = false\n",
        "inf = nan\n",
        "a . b . 'c' = 1\n",
        "\"\" = 'an empty quoted key'\n",
        "x = { 3.14 = 1, true = false, 1979-05-27 = 2 }\n",
        "x = [ # a comment\n  1,\n\n  # another\n  2,\n]\n",
        "x = { a = 1, }\n",
        "x = {\n  a = 1,\n  b = [ 1,\n    2 ],\n}\ny = 2\n",
        "x = \"\"\"a\"\"\"\"\n",
        "x = \"\"\"a\"\"\"\"\"\n",
        "x = '''a'''''\n",
        "x = \"\"\"\\\n   trimmed\"\"\"\n",
        "x = \"\"\"\\   \n   trimmed\"\"\"\n",
        "x = \"\\e\\x41\\u00e9\\U0001F600\\b\\t\\n\\f\\r\\\"\\\\\"\n",
        "x = 1979-05-27 07:32\n",
        "x = 1979-05-27t07:32:00.25z\n",
        "x = 1979-05-27T07:32:00+05:30\n",
        "t = 07:32\n",
        "x = -0\ny = +0\nz = 1e06\nw = 5E+22\nv = -inf\nu = +nan\n",
        "x = 0xDEAD_beef\ny = 0o01_7\nz = 0b0_1\n",
        "[a.'b'.\"c\"] # a comment\n[[d]]\n[[d]]\n",
        "[ spaced . header ]\nkey = 1\n",
        "x = [ [ [ [ [ [ [ 1 ] ] ] ] ] ] ]\n",
    ] {
        assert_eq!(errors(src), Vec::<(String, String)>::new(), "{src:?}");
    }
}

#[test]
fn toml_refuses_what_the_spec_refuses() {
    for src in [
        // A pair ends with its line, and so does a header; a key and its
        // `=` share one.
        "a = 1 b = 2\n",
        "[a] b = 1\n",
        "a\n= 1\n",
        "a =\n1\n",
        "= 1\n",
        "x = [\n  1\n] y = 2\n",
        "x = { a = 1 } b = 2\n",
        // Numbers.
        "x = 123abc\n",
        "x = 01\n",
        "x = +0x1\n",
        "x = 0x_1\n",
        "x = 1__0\n",
        "x = 1.\n",
        "x = .5\n",
        "x = 3.e+20\n",
        "x = Inf\n",
        // Booleans are lowercase.
        "x = TRUE\n",
        // Dates and times.
        "x = 1979-05-27T\n",
        // Strings.
        "x = \"\\q\"\n",
        "x = \"unterminated\n",
        "x = 'unterminated\n",
        "x = \"\"\"\\q\"\"\"\n",
        "x = \"\"\"a\\ b\"\"\"\n",
        "x = \"\"\"a\"\"\"\"\"\"\n",
        // Arrays and inline tables.
        "x = [1 2]\n",
        "x = [1,,2]\n",
        "x = { a = 1 b = 2 }\n",
        "x = 1 ]\n",
        // Headers.
        "[a\n",
        "[[a]\n",
        "[a]]\n",
        "[]\n",
    ] {
        assert!(!errors(src).is_empty(), "{src:?} is refused");
    }
}

/// The lexer's mode stack holds eight modes (`max_stack`): a pair's line
/// takes one, so arrays and inline tables nest seven deep in a value, and
/// an eighth is read wrong to its end, an error where TOML has none.
#[test]
fn toml_nests_arrays_as_deep_as_the_mode_stack_holds() {
    assert!(errors("x = [ [ [ [ [ [ { a = 1 } ] ] ] ] ] ]\n").is_empty(), "seven deep");
    assert!(
        !errors("x = [ [ [ [ [ [ [ \"\"\"\nm\"\"\" ] ] ] ] ] ] ]\n").is_empty(),
        "a multi-line string inside the seventh is an eighth mode"
    );
    assert!(!errors("x = [ [ [ [ [ [ [ [ 1 ] ] ] ] ] ] ] ]\n").is_empty(), "eight deep");
}

/// Every token of `src` with a class: its text and its class, in order:
/// the class its syntax gives it, else its kind's.
fn classes(src: &str) -> Vec<(String, &'static str)> {
    let lang = lang();
    let session = lang.session(src);
    assert!(session.last_repairs.is_empty(), "the sample parses: {:?}", session.last_repairs);
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
            let class = by_syntax.get(&at).copied().or_else(|| lang.styles.class_of(t.id));
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

fn class(text: &str, class: &'static str) -> (String, &'static str) {
    (text.to_string(), class)
}

#[test]
fn toml_colours_keys_values_and_headers() {
    let src = "\
# a comment
name = \"geo\" # after
true = 1e3
[package.\"meta\"]
on = true
at = 1979-05-27
list = [ 'a', { k = 0x1F } ]
[[bin]]
";
    assert_eq!(
        classes(src),
        [
            class("# a comment", "comment"),
            class("name", "variable"),
            class("=", "operator"),
            class("\"geo\"", "string"),
            class("# after", "comment"),
            class("true", "variable"),
            class("=", "operator"),
            class("1e3", "number"),
            class("[", "bracket"),
            class("package", "type"),
            class(".", "punctuation"),
            class("\"meta\"", "type"),
            class("]", "bracket"),
            class("on", "variable"),
            class("=", "operator"),
            class("true", "keyword"),
            class("at", "variable"),
            class("=", "operator"),
            class("1979-05-27", "number"),
            class("list", "variable"),
            class("=", "operator"),
            class("[", "bracket"),
            class("'a'", "string"),
            class(",", "punctuation"),
            class("{", "bracket"),
            class("k", "variable"),
            class("=", "operator"),
            class("0x1F", "number"),
            class("}", "bracket"),
            class("]", "bracket"),
            class("[[", "bracket"),
            class("bin", "type"),
            class("]]", "bracket"),
        ]
    );
}

/// Each outline entry of `src`: its kind, its name, and how deeply it is
/// nested.
fn outline(src: &str) -> Vec<(&'static str, String, usize)> {
    let lang = lang();
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

fn entry(kind: &'static str, name: &str, depth: usize) -> (&'static str, String, usize) {
    (kind, name.to_string(), depth)
}

#[test]
fn toml_outlines_tables_and_their_keys() {
    let src = "\
title = \"geo\"
point = { x = 1, y.z = 2 }

[package]
name = \"geo\"

[dependencies.serde]
features = [\"derive\"]

[[bin]]
name = \"first\"
";
    assert_eq!(
        outline(src),
        [
            entry("variable", "title", 0),
            entry("variable", "point", 0),
            entry("variable", "x", 1),
            entry("variable", "y.z", 1),
            entry("module", "package", 0),
            entry("variable", "name", 1),
            entry("module", "dependencies.serde", 0),
            entry("variable", "features", 1),
            entry("struct", "bin", 0),
            entry("variable", "name", 1),
        ]
    );
}

/// Each fold of `src`: its kind, its head's text, and its first and last
/// lines (from 1).
fn folds_of(tree: &GreenNode, src: &str) -> Vec<(String, String, usize, usize)> {
    let lang = lang();
    let line = |off: u32| src[..off as usize].matches('\n').count() + 1;
    qana_services::folds(tree, &lang.folds)
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

fn folds(src: &str) -> Vec<(String, String, usize, usize)> {
    let session = lang().session(src);
    assert!(session.last_repairs.is_empty(), "the sample parses: {:?}", session.last_repairs);
    folds_of(session.tree().expect("total"), src)
}

fn fold(kind: &str, head: &str, first: usize, last: usize) -> (String, String, usize, usize) {
    (kind.to_string(), head.to_string(), first, last)
}

#[test]
fn toml_folds_tables_arrays_inline_tables_strings_and_comment_runs() {
    let src = "\
# One comment line folds nothing.
x = [
  1, # nor does a comment after a value
  # but two lines
  # of comments do
  2,
]
y = { a = 1,
  b = 2 }
z = \"\"\"
text\"\"\"

[package]
name = \"geo\"

[[bin]]
name = \"first\"
";
    assert_eq!(
        folds(src),
        [
            fold("list", "[", 2, 7),
            fold("comment", "# but two lines", 4, 5),
            fold("list", "{", 8, 9),
            fold("string", "\"\"\"", 10, 11),
            fold("table", "[", 13, 14),
            fold("table", "[[", 16, 17),
        ]
    );
}

/// The tokens of `tree`: each its kind and its text, in order.
fn tokens(tree: &GreenNode) -> Vec<(u16, String)> {
    fn walk(n: &GreenNode, out: &mut Vec<(u16, String)>) {
        for child in &n.children {
            match child {
                GreenChild::Node(m) => walk(m, out),
                GreenChild::Token(t) => out.push((t.id, t.text.clone())),
            }
        }
    }
    let mut out = Vec::new();
    walk(tree, &mut out);
    out
}

/// What an editor reads of a live document: its tokens, its outline's
/// marks and its folds' marks, and the folds its tree makes.
type Read = (Vec<(u16, String)>, String, String, Vec<(String, String, usize, usize)>);

fn read(doc: &mut LiveDoc) -> Read {
    let text = doc.text();
    let marks = format!("{:?}", doc.marks());
    let fold_marks = format!("{:?}", doc.folds_shared());
    let (_, tree) = doc.tree().expect("a tree");
    (tokens(tree), marks, fold_marks, folds_of(tree, &text))
}

#[test]
fn an_edit_reads_as_the_edited_text_read_afresh() {
    let start = "\
a = 1
b = 2

[t]
c = 3
d = 4
";
    let mut doc = LiveDoc::open(lang().clone(), "edit.toml", start);
    doc.open(start);
    // Each edit replaces lines (from 0, the first to before the last)
    // with others.
    let edits: [(u32, u32, &[&str]); 7] = [
        // `a`'s value opens an array: `b = 2` and the header are in it
        // now, errors.
        (0, 1, &["a = ["]),
        // Close it on the next line: `b = 2` is a pair again after it.
        (1, 1, &["  1, 2 ]"]),
        // A multi-line string, unclosed, runs over the rest.
        (3, 3, &["s = \"\"\""]),
        // Close it after `c = 3`: the header and `c = 3` are its text.
        (7, 7, &["\"\"\""]),
        // Take its opener away: the header is a header again, and the
        // closer is an error of its own.
        (3, 4, &[""]),
        // An inline table over lines in the table.
        (8, 9, &["d = {", "  e = 1,", "}"]),
        // A pair after its closer, on its line: an error.
        (10, 11, &["} f = 2"]),
    ];
    for (n, &(start_line, end_line, lines)) in edits.iter().enumerate() {
        doc.edit(&LineEdit {
            start: start_line,
            end: end_line,
            lines: lines.iter().map(|l| l.to_string()).collect(),
        });
        let text = doc.text();
        let mut fresh = LiveDoc::open(lang().clone(), "fresh.toml", &text);
        fresh.open(&text);
        assert_eq!(read(&mut doc), read(&mut fresh), "after edit {n}: {text:?}");
    }
}
