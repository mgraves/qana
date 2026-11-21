//! The Rust Reference's rules for macros by example, as rustc applies
//! them: each fragment kind, repetitions and their separators, forwarding
//! a fragment to another macro, `$crate`, doc comments, the editions'
//! fragments, meta-variable expressions, textual scope, `local_inner_macros`,
//! the recursion limit, and what rustc refuses.
//!
//! `tests/fixtures/reference/` holds the programs and what `rustc +nightly
//! -Ztrace-macros` (nightly 1.94.0, f52090008 2025-12-10) printed for
//! them; the errors are rustc's messages for the programs written out
//! below, which it refuses.

mod common;

use std::collections::HashMap;
use std::sync::Arc;

use common::{check, check_in, check_with, definition, lang, lex, lex_in, texts, trace};
use qana_mbe::{
    expand_all, expand_nested, Edition, Expansion, Form, MacroRules, Nested, Outcome, Token,
    RECURSION_LIMIT,
};

/// The first expansion of macro `name`, depth first.
fn find<'a>(nested: &'a [Nested], name: &str) -> Option<&'a Expansion> {
    for n in nested {
        if let Outcome::Expanded(e) = &n.outcome {
            if n.path.last().is_some_and(|t| &*t.text == name) {
                return Some(e);
            }
            if let Some(found) = find(&e.nested, name) {
                return Some(found);
            }
        }
    }
    None
}

fn no_macros(_: &[Token], _: Option<u32>) -> Option<std::sync::Arc<MacroRules>> {
    None
}

#[test]
fn fragments_repetitions_forwarding_and_doc_comments_expand_as_rustc_expands_them() {
    let program = include_str!("fixtures/reference/reference.rs");
    let (made, nested) = check_in(
        program,
        Edition::E2021,
        &HashMap::new(),
        trace(include_str!("fixtures/reference/reference.trace")),
        &[],
        &["stringify"],
        &[],
    );
    assert_eq!(made, 42);

    // `$e * 2` with `$e` = `1 + 1` doubles the sum: the fragment stays one
    // node, which the expansion's text writes in parentheses.
    let double = find(&nested, "double").expect("double!");
    assert_eq!(double.parsed.text, "( 1 + 1 ) * 2");
    let one = double.parsed.tokens.iter().find(|t| &*t.token.text == "1").expect("`1`");
    assert_eq!(one.token.mark, 0, "the invocation wrote the fragment");
    let call = program.find("double!(1 + 1)").expect("the call") as u32;
    assert_eq!(one.token.origin.start, call + "double!(".len() as u32);
    let star = double.parsed.tokens.iter().find(|t| &*t.token.text == "*").expect("`*`");
    assert_eq!(star.token.mark, double.mark, "the macro wrote the operator");

    // `$crate` is written by the macro, where its definition is.
    let with_crate = find(&nested, "with_crate").expect("with_crate!");
    let dollar_crate = &with_crate.parsed.tokens[0].token;
    assert_eq!(&*dollar_crate.text, "$crate");
    assert_eq!(dollar_crate.mark, with_crate.mark);
    assert_eq!(dollar_crate.origin.start, program.find("$crate::helper").expect("it") as u32);

    // A macro a macro defines is seen after it.
    assert!(find(&nested, "seven").is_some(), "seven!, which make! defines");
}

#[test]
fn what_the_corpus_showed_expands_as_rustc_expands_it() {
    // Expressions with attributes, a statement macro's trailing
    // invocation, `$($s:stmt)*`'s empty statements, a field's visibility,
    // an item's attributes, a cast's type before `<<`.
    let (made, nested) = check_in(
        include_str!("fixtures/reference/corpus_cases.rs"),
        Edition::E2021,
        &HashMap::new(),
        trace(include_str!("fixtures/reference/corpus_cases.trace")),
        &[],
        &[],
        &[],
    );
    assert_eq!(made, 8);
    let shift = find(&nested, "shift").expect("shift!");
    assert!(shift.parsed.text.contains("1 as ( u32 ) << 3"), "{}", shift.parsed.text);
    let trailing = find(&nested, "outer_stmts").expect("outer_stmts!");
    assert_eq!(trailing.nested[0].entry, "block_stmts", "a statement, as rustc reads it");
}

#[test]
fn the_editions_fragments_match_as_rustc_matches_them() {
    let program = include_str!("fixtures/reference/editions.rs");
    for (edition, steps) in [
        (Edition::E2018, include_str!("fixtures/reference/editions_2018.trace")),
        (Edition::E2021, include_str!("fixtures/reference/editions_2021.trace")),
        (Edition::E2024, include_str!("fixtures/reference/editions_2024.trace")),
    ] {
        let (made, _) = check_in(program, edition, &HashMap::new(), trace(steps), &[], &[], &[]);
        assert_eq!(made, 4, "{edition:?}");
    }
}

#[test]
fn meta_variable_expressions_expand_as_rustc_expands_them() {
    let made = check(
        include_str!("fixtures/reference/metavar.rs"),
        &HashMap::new(),
        trace(include_str!("fixtures/reference/metavar.trace")),
        &[],
        &[],
    );
    assert_eq!(made, 6);
}

#[test]
fn a_macro_rules_is_seen_after_it_to_the_end_of_its_block_or_module() {
    // rustc: `a!` after the block and `b!` after `mod m` are "cannot find
    // macro … in this scope"; `c!`, after a `#[macro_use]` module, is seen.
    let mut resolve = no_macros;
    let (_, nested) = expand_all(
        lang(),
        &lex(include_str!("fixtures/reference/scope.rs"), 1),
        "file",
        RECURSION_LIMIT,
        &mut resolve,
    )
    .expect("the program parses");
    let outcomes: Vec<(String, bool)> = nested
        .iter()
        .map(|n| (n.path[0].text.to_string(), matches!(n.outcome, Outcome::Expanded(_))))
        .collect();
    let expect: Vec<(String, bool)> = [("a", true), ("a", false), ("b", false), ("c", true)]
        .iter()
        .map(|&(n, e)| (n.to_string(), e))
        .collect();
    assert_eq!(outcomes, expect);
}

#[test]
fn a_local_inner_macros_macro_names_its_helpers_as_its_crate_does() {
    // rustc: `helper!` that `outer!` and `calls!` write is the crate's
    // exported one, `$crate::helper!` (`+ 1`), though `main`'s own
    // `helper!` (`* 100`) is in textual scope; the `helper` `main` passes
    // `calls!` is `main`'s.
    let program = include_str!("fixtures/reference/local_inner.rs");
    let exported = definition(program, 1, "macro_rules!", "helper", Form::MacroRules);
    let mut asked: Vec<String> = Vec::new();
    let mut resolve = |path: &[Token], _: Option<u32>| {
        let path = path.iter().map(|t| t.text.to_string()).collect::<Vec<_>>().join(" ");
        asked.push(path.clone());
        (path == "$crate :: helper").then(|| exported.clone())
    };
    let (made, nested) = check_with(
        program,
        Edition::E2021,
        &mut resolve,
        trace(include_str!("fixtures/reference/local_inner.trace")),
        &[],
        &[],
        &[],
    );
    assert_eq!(made, 6);
    assert_eq!(asked, ["$crate :: helper", "$crate :: helper"], "the written names, by path");
    let outer = find(&nested, "outer").expect("outer!");
    let path = &outer.nested[0].path;
    assert_eq!(texts(&outer.tokens)[0], "helper", "the expansion is as rustc writes it");
    assert!(path.iter().all(|t| t.mark == outer.mark), "the path the expansion wrote");
}

/// A program of `deep!` recursing `n` times, in a crate whose
/// `recursion_limit` is `limit`: (expansions made, the innermost's error
/// if it failed).
fn deep_to(n: usize, limit: usize) -> (usize, Option<String>) {
    let program = format!(
        "macro_rules! deep {{ () => {{ 0 }}; (x $($t:tt)*) => {{ deep!($($t)*) }}; }}\n\
         fn main() {{ let _ = deep!({}); }}\n",
        vec!["x"; n].join(" ")
    );
    let mut resolve = no_macros;
    let (_, nested) =
        expand_all(lang(), &lex(&program, 1), "file", limit, &mut resolve).expect("parses");
    innermost(&nested)
}

fn deep(n: usize) -> (usize, Option<String>) {
    deep_to(n, RECURSION_LIMIT)
}

/// How deep `nested`'s first invocation nests, and the innermost's error.
fn innermost(nested: &[Nested]) -> (usize, Option<String>) {
    let mut made = 0usize;
    let mut level = nested;
    loop {
        match &level[0].outcome {
            Outcome::Expanded(e) => {
                made += 1;
                if e.nested.is_empty() {
                    return (made, None);
                }
                level = &e.nested;
            }
            Outcome::Failed(err) => return (made, Some(err.message.clone())),
            Outcome::Unresolved => panic!("deep! is defined"),
        }
    }
}

#[test]
fn expansions_nest_to_rustcs_recursion_limit() {
    // rustc: 127 recursions expand (128 nested expansions); 128 reach the
    // limit.
    assert_eq!(deep(127), (128, None));
    assert_eq!(
        deep(128),
        (128, Some("recursion limit reached while expanding `deep!`".to_string()))
    );
    // rustc, with `#![recursion_limit = "256"]`: 255 recursions expand, 256
    // reach it.
    assert_eq!(deep_to(255, 256), (256, None));
    assert_eq!(
        deep_to(256, 256),
        (256, Some("recursion limit reached while expanding `deep!`".to_string()))
    );
}

#[test]
fn an_expansion_made_deep_nests_only_to_the_limit() {
    // An invocation an expansion 100 deep wrote is 101 deep: 27 more
    // recursions reach 128, a 28th the limit.
    let rules = Arc::new(
        MacroRules::read(
            &lex("{ () => { 0 }; (x $($t:tt)*) => { deep!($($t)*) }; }", 1),
            Form::MacroRules,
        )
        .expect("deep!"),
    );
    let at = |n: usize| {
        let mut resolve = |path: &[Token], _: Option<u32>| {
            path.last().is_some_and(|t| &*t.text == "deep").then(|| rules.clone())
        };
        let input = lex(&vec!["x"; n].join(" "), 0);
        let top =
            expand_nested(lang(), &rules, &input, "attrs_expr", 101, RECURSION_LIMIT, &mut resolve)
                .expect("the top expansion is made");
        let (made, error) = innermost(&top.nested);
        (made + 1, error)
    };
    assert_eq!(at(27), (28, None));
    assert_eq!(at(28), (28, Some("recursion limit reached while expanding `deep!`".to_string())));
}

/// What reading `macro_rules! m …` in `program` says.
fn read_error(program: &str) -> String {
    let at = program.find("macro_rules! m").expect("m") + "macro_rules! m".len();
    let end = program[at..].find(" }\n").map(|k| at + k + 2).unwrap_or(program.len());
    let def = lex_in(&program[at..end], 1, Edition::E2021);
    MacroRules::read(&def, Form::MacroRules).expect_err("rustc refuses it").message
}

#[test]
fn definitions_rustc_refuses_are_refused() {
    for (program, rustc) in [
        (
            "macro_rules! m { ($e:expr $f:expr) => {} }\n",
            "`$e:expr` is followed by `$f:expr`, which is not allowed for `expr` fragments",
        ),
        (
            "macro_rules! m { ($p:pat | $q:pat) => {} }\n",
            "`$p:pat` is followed by `|`, which is not allowed for `pat` fragments",
        ),
        ("macro_rules! m { ($($v:vis)*) => {} }\n", "repetition matches empty token tree"),
        ("macro_rules! m { ($a:ident $a:ident) => {} }\n", "duplicate matcher binding"),
        ("macro_rules! m { ($a) => {} }\n", "missing fragment specifier"),
        (
            "macro_rules! m { ($(a),?) => {} }\n",
            "the `?` macro repetition operator does not take a separator",
        ),
    ] {
        let ours = read_error(program);
        assert!(ours.starts_with(rustc), "{program}: ours {ours:?}, rustc's {rustc:?}");
    }
}

#[test]
fn invocations_rustc_refuses_fail() {
    for (program, failing, rustc) in [
        // A forwarded `expr` is one node, which no literal token matches;
        // a `tt` is its token (see reference.rs's `outer_tt!`).
        (
            "macro_rules! bar { (3) => {} } macro_rules! foo { ($l:expr) => { bar!($l); } }\n\
             fn f() { foo!(3); }\n",
            "bar",
            "no rules expected `expr` metavariable",
        ),
        (
            "macro_rules! ambiguity { ($($i:ident)* $j:ident) => { }; }\n\
             fn f() { ambiguity!(error); }\n",
            "ambiguity",
            "local ambiguity when calling macro",
        ),
        (
            "macro_rules! m { ($($a:ident),* ; $($b:ident),*) => { $(fn $a() {} fn $b() {})* } }\n\
             m!(a1, a2 ; b1, b2, b3);\n",
            "m",
            "meta-variable `a` repeats 2 times, but `b` repeats 3 times",
        ),
        (
            "macro_rules! m { ($($a:ident),*) => { fn $a() {} } }\nm!(x, y);\n",
            "m",
            "variable `a` is still repeating at this depth",
        ),
        (
            "macro_rules! m { ($e:expr) => {} }\nfn f() { m!(1 +); }\n",
            "m",
            "expected expression, found end of macro arguments",
        ),
        (
            "macro_rules! r { () => { r!(); } }\nfn f() { r!(); }\n",
            "r",
            "recursion limit reached while expanding `r!`",
        ),
    ] {
        let mut resolve = no_macros;
        let (_, nested) =
            expand_all(lang(), &lex(program, 1), "file", RECURSION_LIMIT, &mut resolve)
                .expect("parses");
        let mut failed = Vec::new();
        common::made(&nested, &mut Vec::new(), &mut Vec::new(), &mut failed);
        assert_eq!(failed.len(), 1, "{program}: one invocation fails: {failed:?}");
        let (name, message) = &failed[0];
        assert_eq!(name, failing, "{program}");
        assert!(message.starts_with(rustc), "{program}: ours {message:?}, rustc's {rustc:?}");
    }
}
