//! An alternative's `@style(class, label…)`: what it paints, how a class
//! given to a rule reaches a token (`@style(inherit, label…)`, lists and
//! options), and what the compiler refuses.

use qana_lang::compile::QanaDiag;
use qana_lang::EmbeddedLang;
use qana_services::syntax_classes;

const HEAD: &str = r#"
language Mini

token WS = /\s+/ @trivia
token NUM = /\d+/ @style(number)
token IDENT = /[\a_][\w_]*/ @specialize @style(variable)
token LP = "(" @style(bracket)
token RP = ")" @style(bracket)
token COLON = ":" @style(punctuation)
token COMMA = "," @style(punctuation)
token DOT = "." @style(punctuation)
token SEMI = ";" @style(punctuation)

keywords IDENT = let fn

start file
rule file = File: stmt*
"#;

/// Each token the syntax of `rules` paints in `src`: its text and class.
fn painted(rules: &str, src: &str) -> Vec<(String, &'static str)> {
    let lang = EmbeddedLang::from_qana_source(&format!("{HEAD}{rules}"))
        .unwrap_or_else(|d| panic!("{d:?}"));
    let session = lang.session(src);
    syntax_classes(session.tree().expect("total"), &lang.styles)
        .into_iter()
        .map(|((a, b), class)| {
            (
                src[a as usize..b as usize].to_string(),
                lang.styles.legend[class as usize],
            )
        })
        .collect()
}

fn diags(rules: &str) -> Vec<QanaDiag> {
    match EmbeddedLang::from_qana_source(&format!("{HEAD}{rules}")) {
        Ok(_) => Vec::new(),
        Err(d) => d,
    }
}

fn p(text: &str, class: &'static str) -> (String, &'static str) {
    (text.to_string(), class)
}

const PATHS: &str = r#"
rule stmt =
  | Let:  "let" n:IDENT COLON t:path SEMI @style(type, t)
  | Call: c:path LP args RP SEMI @style(function, c)
rule path =
  | One:  n:IDENT @style(inherit, n)
  | More: path DOT n:IDENT @style(inherit, n)
rule args = arg* % COMMA
rule arg = Arg: path
"#;

#[test]
fn a_class_given_to_a_rule_lands_where_it_passes_it() {
    assert_eq!(
        painted(PATHS, "let x: a.b.c; f(a.b); m.g();"),
        [p("c", "type"), p("f", "function"), p("g", "function")],
    );
}

#[test]
fn a_list_and_an_option_pass_a_class_to_their_elements() {
    let rules = r#"
rule stmt =
  | Types: "let" ts:names SEMI @style(type, ts)
  | Maybe: "fn" m:maybe SEMI @style(function, m)
rule names = name+ % COMMA
rule name = Name: n:IDENT @style(inherit, n)
rule maybe = name?
"#;
    assert_eq!(
        painted(rules, "let a, b, c; fn f; fn;"),
        [
            p("a", "type"),
            p("b", "type"),
            p("c", "type"),
            p("f", "function")
        ],
    );
}

#[test]
fn a_token_recovery_inserted_holds_its_place() {
    // `b` is missing: the name after it is still the alternative's third
    // symbol.
    let rules = r#"
rule stmt = Pair: "let" COLON n:IDENT SEMI @style(type, n)
"#;
    assert_eq!(painted(rules, "let x;"), [p("x", "type")]);
}

#[test]
fn what_the_compiler_refuses() {
    let has = |rules: &str, needle: &str| {
        let d = diags(rules);
        assert!(
            d.iter().any(|d| d.msg.contains(needle)),
            "expected `{needle}` in {d:?}"
        );
    };
    has(
        "rule stmt = S: n:IDENT SEMI @style(sparkle, n)",
        "unknown style class `sparkle`",
    );
    has(
        "rule stmt = S: n:IDENT SEMI @style(type)",
        "takes a class and the labels it paints",
    );
    has(
        "rule stmt = S: n:IDENT SEMI @style(type, n) @style(function, n)",
        "`n` is styled twice",
    );
    has(
        "rule stmt = S: n:IDENT SEMI @style(type, m)",
        "no symbol labeled `m`",
    );
    // A rule given a class must pass it on to a token.
    has(
        "rule stmt = S: t:ty SEMI @style(type, t)\nrule ty = Ty: IDENT",
        "`t` is the rule `ty`, which passes no class on to a token",
    );
    // Through a rule that passes it on to one that does not.
    has(
        "rule stmt = S: t:outer SEMI @style(type, t)\nrule outer = O: i:inner @style(inherit, i)\nrule inner = I: IDENT",
        "`i` is the rule `inner`, which passes no class on",
    );
    assert!(diags(
        "rule stmt = S: t:ty SEMI @style(type, t)\nrule ty = Ty: n:IDENT @style(inherit, n)"
    )
    .is_empty());
}
