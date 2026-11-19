//! `extensions`: a grammar names the file extensions that are its
//! language's, so an editor can pick a language for a file without a table
//! of its own. The list is read when the grammar compiles, before (and
//! without) certification, and a certified language carries it.

use qana_lang::{compile_source, EmbeddedLang, QanaToolchain};

const RUST: &str = include_str!("../../../examples/rust/rust.qana");
const C: &str = include_str!("../../../examples/c/c.qana");
const STRUCTLANG: &str = include_str!("../../../examples/structs/structlang.qana");
const CHARTLANG: &str = include_str!("../../../examples/mini/chartlang.qana");

/// A grammar's declared extensions, read without certifying it.
fn declared(src: &str) -> Vec<String> {
    let out = compile_source(&QanaToolchain::new(), src);
    assert!(out.diags.is_empty(), "compiles cleanly: {:?}", out.diags);
    out.def.extensions
}

/// The messages compiling `src` reports.
fn messages(src: &str) -> Vec<String> {
    compile_source(&QanaToolchain::new(), src)
        .diags
        .into_iter()
        .map(|d| d.msg)
        .collect()
}

const HEAD: &str = "language Tiny\n";
const BODY: &str = "token WS = /\\s+/ @trivia\ntoken A = \"a\"\nstart file\nrule file = A\n";

#[test]
fn the_shipped_grammars_declare_their_extensions() {
    assert_eq!(declared(RUST), ["rs"]);
    assert_eq!(declared(C), ["c", "h"]);
    assert_eq!(declared(STRUCTLANG), ["sl"]);
    assert!(declared(CHARTLANG).is_empty(), "declaring none is allowed");
}

#[test]
fn a_certified_language_carries_its_name_and_extensions() {
    let lang = EmbeddedLang::from_qana_source(STRUCTLANG).expect("structlang certifies");
    assert_eq!(lang.name, "Structlang");
    assert_eq!(lang.extensions, ["sl"]);
}

#[test]
fn extensions_may_be_names_or_strings() {
    let src = format!("{HEAD}extensions tiny \"t2\"\n{BODY}");
    assert_eq!(declared(&src), ["tiny", "t2"]);
}

#[test]
fn a_misdeclared_extension_is_refused_with_the_reason() {
    let refused = |list: &str| messages(&format!("{HEAD}extensions {list}\n{BODY}"));
    assert_eq!(
        refused("\".rs\""),
        ["an extension is written without its dot: `rs`, not `.rs`"]
    );
    assert_eq!(refused("\"\""), ["an extension is not empty"]);
    assert_eq!(refused("\"tar.gz\""), ["`tar.gz` is not a file extension"]);
    assert_eq!(refused("\"a b\""), ["`a b` is not a file extension"]);
    assert_eq!(
        refused("\"rs\" \"RS\""),
        ["extension `RS` is declared twice"]
    );
    assert_eq!(
        messages(&format!("{HEAD}extensions \"a\"\nextensions \"b\"\n{BODY}")),
        ["duplicate `extensions`"]
    );
}
