//! What a project index reads of an expansion: the tokens written within a
//! span (a nested invocation's, a `macro_rules!` it defines), an
//! invocation's path and input, and rustc's built-in `cfg_select!`'s arms.

mod common;

use common::{lex, texts};
use qana_mbe::{expand, Form, MacroRules, TokenStream};

#[test]
fn an_invocations_path_and_input_are_read_from_its_tokens() {
    let (path, input) = lex("foo::bar!(a, b);", 0).invocation().expect("an invocation");
    let path: Vec<&str> = path.iter().map(|t| &*t.text).collect();
    assert_eq!(path, ["foo", "::", "bar"]);
    assert_eq!(texts(&input), ["a", ",", "b"]);
    assert!(lex("m! { x }", 0).invocation().is_some(), "braces, no `;`");
    assert!(lex("m!(x) y", 0).invocation().is_none(), "more after it");
    assert!(lex("macro_rules! m { () => {} }", 0).invocation().is_none(), "a definition");
}

#[test]
fn a_nested_invocations_input_keeps_a_fragment_whole() {
    let rules = MacroRules::read(&lex("{ ($t:ty) => { inner!($t); }; }", 1), Form::MacroRules)
        .expect("it reads");
    let tokens = expand(common::lang(), &rules, &lex("Vec<u8>", 0), 1).expect("it expands");
    let parsed = tokens.parse(common::lang(), "items_opt").expect("it parses");
    let start = parsed.tokens.iter().find(|t| &*t.token.text == "inner").expect("inner").span.0;
    let nested = parsed.stream((start, parsed.text.len() as u32));
    let (path, input) = nested.invocation().expect("an invocation");
    assert_eq!(&*path[0].text, "inner");
    assert_eq!(path[0].mark, 1, "the macro wrote it");
    assert_eq!(texts(&input), ["Vec", "<", "u8", ">"]);
    let again = expand(
        common::lang(),
        &MacroRules::read(&lex("{ ($t:ty) => { struct S($t); }; }", 2), Form::MacroRules)
            .expect("it reads"),
        &input,
        2,
    )
    .expect("a `ty` fragment matches `$t:ty` whole");
    assert_eq!(texts(&again), ["struct", "S", "(", "Vec", "<", "u8", ">", ")", ";"]);
}

#[test]
fn cfg_select_arms_are_its_predicates_and_braced_items() {
    let arms = lex("unix => { fn a() {} } target_os = \"wasi\" => { fn b() {} }, _ => {}", 0)
        .cfg_select_arms()
        .expect("arms");
    let read: Vec<(Option<Vec<String>>, Vec<String>)> =
        arms.iter().map(|(p, body)| (p.as_ref().map(texts), texts(body))).collect();
    assert_eq!(
        read,
        [
            (
                Some(vec!["unix".to_string()]),
                ["fn", "a", "(", ")", "{", "}"].map(String::from).to_vec()
            ),
            (
                Some(["target_os", "=", "\"wasi\""].map(String::from).to_vec()),
                ["fn", "b", "(", ")", "{", "}"].map(String::from).to_vec()
            ),
            (None, Vec::new()),
        ]
    );
    for (text, rustc) in [
        ("unix { }", "expected `=>` after a `cfg_select!` predicate"),
        ("unix => fn a() {}", "expected `{` after `=>` in `cfg_select!`"),
        ("=> {}", "expected a `cfg_select!` predicate"),
    ] {
        let message = lex(text, 0).cfg_select_arms().expect_err("refused").message;
        assert_eq!(message, rustc, "{text}");
    }
    assert!(TokenStream::default().cfg_select_arms().expect("none").is_empty());
}

/// The options `text`'s cfg predicates write: each name and its literal's
/// text.
fn options(text: &str) -> Vec<(String, String)> {
    lex(text, 0)
        .cfg_option_literals()
        .into_iter()
        .map(|(name, literal)| (name, literal.text.to_string()))
        .collect()
}

fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
}

#[test]
fn cfg_predicates_name_their_options_wherever_rustc_reads_one() {
    assert_eq!(
        options(
            "#[cfg(any(feature = \"a\", all(unix, not(feature = r#\"b\"#))))]\nfn f() {}\n\
             #![cfg(target_os = \"linux\")]"
        ),
        pairs(&[("feature", "\"a\""), ("feature", "r#\"b\"#"), ("target_os", "\"linux\"")]),
        "attributes, nested lists, raw strings"
    );
    assert_eq!(
        options(
            "#[cfg_attr(feature = \"c\", derive(Debug), cfg_attr(feature = \"d\", x = \"no\"))]"
        ),
        pairs(&[("feature", "\"c\""), ("feature", "\"d\"")]),
        "a `cfg_attr`'s predicate, not its attributes' `name = value`s, but a nested one's"
    );
    assert_eq!(
        options("fn g() -> bool { cfg!(feature = \"e\") && h(x = \"no\") }"),
        pairs(&[("feature", "\"e\"")]),
        "`cfg!`, and no other call's arguments"
    );
    assert_eq!(
        options(
            "cfg_select! { feature = \"f\" => { const X: &str = \"no\"; let y = \"no\"; } \
             _ => { #[cfg(feature = \"g\")] fn i() {} } }"
        ),
        pairs(&[("feature", "\"f\""), ("feature", "\"g\"")]),
        "`cfg_select!`'s predicates, and what its bodies hold"
    );
    assert_eq!(
        options(
            "#[cfg_attr(docsrs, doc(cfg(feature = \"h\")))]\n\
             macro_rules! m { () => { #[cfg(feature = \"i\")] fn j() {} }; }"
        ),
        pairs(&[("feature", "\"h\""), ("feature", "\"i\"")]),
        "`doc(cfg(…))`, and a macro's body"
    );
    assert!(options("#[path = \"x.rs\"] mod x; fn cfg(a: u8) {}").is_empty());
}
