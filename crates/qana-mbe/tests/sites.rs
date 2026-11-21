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
