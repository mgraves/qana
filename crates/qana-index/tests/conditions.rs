//! Conditions read and kept: each construct's (Rust's `#[cfg(…)]`), as a
//! summary has it. What each offered item is under (those of the regions
//! holding it, as one), every region with its own condition, bodies'
//! too, and what an expansion declares, under its invocation's condition.

use std::rc::Rc;
use std::sync::OnceLock;

use qana_index::{
    expand, merge, summarize, Condition, Configuration, CrateInfo, CrateSource, Edition,
    ExpandRequest, Form, Holds, MacroSource, Summary, RECURSION_LIMIT,
};
use qana_lang::EmbeddedLang;

const GRAMMAR: &str = include_str!("../../../examples/rust/rust.qana");

fn rust() -> &'static EmbeddedLang {
    static LANG: OnceLock<EmbeddedLang> = OnceLock::new();
    LANG.get_or_init(|| EmbeddedLang::from_qana_source(GRAMMAR).expect("rust.qana certifies"))
}

fn option(name: &str) -> Condition {
    Condition::Option(name.into())
}

fn value(name: &str, value: &str) -> Condition {
    Condition::Value(name.into(), value.into())
}

fn not(c: Condition) -> Condition {
    Condition::Not(Box::new(c))
}

/// Under the file's own `#![cfg(not(windows))]` and `inner`.
fn in_file(inner: Condition) -> Option<Condition> {
    Some(Condition::All(vec![not(option("windows")), inner]))
}

const CONDITIONS: &str = r#"#![cfg(not(windows))]

#[cfg(test)]
mod tests;

#[cfg(feature = "serde")]
use serde::Serialize;

#[cfg(feature = "std")]
pub use std::vec::Vec;

#[cfg(unix)]
pub fn on_unix() {}

#[cfg(all(unix, not(target_os = "macos")))]
pub struct Linuxish {
    #[cfg(debug_assertions)]
    pub checked: u8,
    pub always: u8,
}

#[cfg(any(test, doc))]
impl Linuxish {
    pub fn helper() {}
}

#[cfg_attr(feature = "x", cfg(loom))]
pub fn maybe() {}

#[cfg(doc)]
stub! {}

#[cfg(not(doc))]
#[macro_export]
macro_rules! real {
    () => {};
}

#[cfg(target(os = "linux", arch = "x86_64"))]
pub fn target_form() {}

#[cfg(version("1.80"))]
pub fn unread() {}

#[cfg(true)]
pub fn literal() {}

#[cfg(unix)]
#[cfg(feature = r"raw")]
pub fn two() {}

pub fn body(cfg: u8) -> u8 {
    #[cfg(windows)]
    let x = 1;
    match cfg {
        #[cfg(unix)]
        0 => {}
        _ => {}
    }
    cfg(cfg)
}
"#;

fn summary() -> Summary {
    summarize(rust(), CONDITIONS)
}

/// The condition of the declaration named `name`.
fn declared(summary: &Summary, name: &str) -> Option<Condition> {
    summary
        .offers()
        .declarations()
        .iter()
        .find(|d| d.name == name)
        .unwrap_or_else(|| panic!("{name} is declared"))
        .condition
        .clone()
}

#[test]
fn each_offered_item_is_under_the_conditions_holding_it() {
    let summary = summary();
    let offers = summary.offers();
    let tests = offers
        .units()
        .iter()
        .find(|u| u.name == "tests")
        .expect("a unit");
    assert_eq!(tests.condition, in_file(option("test")));
    let private = summary
        .needs()
        .imports()
        .iter()
        .find(|i| i.path.segments[0] == "serde");
    assert_eq!(
        private.expect("a private import").condition,
        in_file(value("feature", "serde"))
    );
    assert_eq!(
        offers.reexports()[0].condition,
        in_file(value("feature", "std"))
    );
    assert_eq!(declared(&summary, "on_unix"), in_file(option("unix")));
    let linuxish = Condition::All(vec![option("unix"), not(value("target_os", "macos"))]);
    assert_eq!(declared(&summary, "Linuxish"), in_file(linuxish.clone()));
    assert_eq!(
        declared(&summary, "checked"),
        Some(Condition::All(vec![
            not(option("windows")),
            linuxish.clone(),
            option("debug_assertions")
        ])),
        "a field, under its struct's and its own"
    );
    assert_eq!(declared(&summary, "always"), in_file(linuxish));
    let tests_or_doc = Condition::Any(vec![option("test"), option("doc")]);
    assert_eq!(offers.impls()[0].condition, in_file(tests_or_doc.clone()));
    assert_eq!(
        declared(&summary, "helper"),
        in_file(tests_or_doc),
        "an impl block's item"
    );
    assert_eq!(offers.invocations()[0].condition, in_file(option("doc")));
    let real = offers.macros().first().expect("a macro");
    assert_eq!(real.condition, in_file(not(option("doc"))));
    assert_eq!(
        declared(&summary, "body"),
        Some(not(option("windows"))),
        "the file's own alone"
    );
}

#[test]
fn each_way_of_writing_a_predicate_reads_as_rust_means_it() {
    let summary = summary();
    assert_eq!(
        declared(&summary, "maybe"),
        in_file(Condition::Any(vec![
            not(value("feature", "x")),
            option("loom")
        ])),
        "a `cfg` a `cfg_attr` holds holds where its predicate does"
    );
    assert_eq!(
        declared(&summary, "target_form"),
        in_file(Condition::All(vec![
            value("target_os", "linux"),
            value("target_arch", "x86_64")
        ]))
    );
    assert_eq!(declared(&summary, "unread"), in_file(Condition::Unread));
    assert_eq!(
        declared(&summary, "literal"),
        in_file(Condition::Literal(true))
    );
    assert_eq!(
        declared(&summary, "two"),
        in_file(Condition::All(vec![
            option("unix"),
            value("feature", "raw")
        ])),
        "two conditions, both; a raw string's text"
    );
}

#[test]
fn a_body_keeps_its_regions_and_a_function_named_cfg_is_a_name() {
    let summary = summary();
    let text = |span: (u32, u32)| &CONDITIONS[span.0 as usize..span.1 as usize];
    let regions: Vec<(&str, &Condition)> = summary
        .locations()
        .condition_regions()
        .iter()
        .map(|(span, c)| (text(*span), c))
        .collect();
    assert!(
        regions.contains(&("#[cfg(windows)]\n    let x = 1;", &option("windows"))),
        "a statement's, from its attribute to its end: {regions:?}"
    );
    assert!(regions.contains(&("#[cfg(unix)]\n        0 => {}", &option("unix"))));
    assert!(regions.contains(&("#[cfg(test)]\nmod tests;", &option("test"))));
    assert!(regions.contains(&(
        "#[cfg(debug_assertions)]\n    pub checked: u8",
        &option("debug_assertions")
    )));
    let x = CONDITIONS.find("let x").expect("x") as u32 + 4;
    assert_eq!(
        summary.locations().condition_at((x, x + 1)),
        Some(Condition::All(vec![
            not(option("windows")),
            option("windows")
        ]))
    );
    assert_eq!(
        summary.locations().condition_regions()[0].0,
        (0, CONDITIONS.len() as u32),
        "the file's own, around everything"
    );
    assert!(
        summary
            .offers()
            .declarations()
            .iter()
            .any(|d| d.name == "body"),
        "a call of a function named `cfg` parses"
    );
}

#[test]
fn a_file_without_conditions_has_none_and_a_conditions_change_reaches_its_part() {
    let plain = summarize(rust(), "pub fn a() {}\nmod b;\n");
    assert!(plain.locations().condition_regions().is_empty());
    assert!(plain
        .offers()
        .declarations()
        .iter()
        .all(|d| d.condition.is_none()));
    assert!(plain.offers().units().iter().all(|u| u.condition.is_none()));

    let unix = summarize(
        rust(),
        "#[cfg(unix)]\npub fn a() {\n    #[cfg(test)]\n    let x = 1;\n}\n",
    );
    let windows = summarize(
        rust(),
        "#[cfg(windows)]\npub fn a() {\n    #[cfg(test)]\n    let x = 1;\n}\n",
    );
    let tested = summarize(
        rust(),
        "#[cfg(unix)]\npub fn a() {\n    #[cfg(loom)]\n    let x = 1;\n}\n",
    );
    assert_ne!(
        unix.offers().fingerprint(),
        windows.offers().fingerprint(),
        "an offered item's condition is what others depend on"
    );
    assert_eq!(
        unix.offers().fingerprint(),
        tested.offers().fingerprint(),
        "a body's is not"
    );
    assert_ne!(
        unix.locations().fingerprint(),
        tested.locations().fingerprint()
    );
}

const MADE: &str = "#[cfg(test)]\nmake!(made);\n";

#[test]
fn what_an_expansion_declares_is_under_its_invocations_condition() {
    let own = summarize(rust(), MADE);
    assert_eq!(
        own.offers().invocations()[0].condition,
        Some(option("test"))
    );
    let span = own.locations().invocation_span(0);
    let request = ExpandRequest {
        definition: MacroSource::Text {
            body: " { ($n:ident) => { #[cfg(unix)] pub fn $n() {} pub fn always() {} }; }".into(),
            form: Form::MacroRules,
            edition: Edition::E2021,
            local_inner: false,
        },
        site: MADE[span.0 as usize..span.1 as usize].to_string(),
        site_edition: Edition::E2021,
        input: None,
        entry: "items_opt".into(),
        mark: 1,
        limit: RECURSION_LIMIT as u32,
        local_inner: Vec::new(),
        of_crate: Vec::new(),
        itself: None,
        names: Vec::new(),
        krate: None,
    };
    let expansion = Rc::new(expand(rust(), &request, &[]));
    let expansions = |_: &[(u32, u32)], _: u32| Some(vec![(0, expansion.clone(), None)]);
    let merged = merge(&own, &expansions);
    let of = |name: &str| declared(&merged, name);
    assert_eq!(
        of("made"),
        Some(Condition::All(vec![option("test"), option("unix")])),
        "its invocation's, then its own"
    );
    assert_eq!(of("always"), Some(option("test")));
}

fn krate(configuration: Configuration) -> CrateInfo {
    CrateInfo {
        name: "geo".into(),
        kind: "lib".into(),
        root: "/p/src/lib.rs".into(),
        edition: "2021".into(),
        dependencies: Vec::new(),
        loadable: Vec::new(),
        source: CrateSource::Workspace,
        preludes: Vec::new(),
        builtins: Vec::new(),
        closed_by: Vec::new(),
        inert: Vec::new(),
        imports_from_root: false,
        unindexed: Vec::new(),
        deref_trait: None,
        try_trait: None,
        derive_traits: Vec::new(),
        macro_use: Vec::new(),
        macro_export: Vec::new(),
        macro_prelude: Vec::new(),
        conditions: Vec::new(),
        builtin_attributes: Vec::new(),
        builtin_macros: Vec::new(),
        local_inner_macros: Vec::new(),
        recursion_limit: None,
        configuration,
    }
}

#[test]
fn a_crate_keeps_its_configuration_and_by_default_decides_nothing() {
    let configuration = Configuration {
        options: [
            ("unix".to_string(), None),
            ("feature".into(), Some("std".into())),
        ]
        .into(),
        decided: ["unix", "windows", "feature"].map(String::from).into(),
    };
    let geo = krate(configuration.clone());
    let bytes = postcard::to_allocvec(&geo).expect("a crate encodes");
    let back: CrateInfo = postcard::from_bytes(&bytes).expect("and decodes");
    assert_eq!(back.configuration, configuration);
    assert_eq!(option("windows").holds(&back.configuration), Holds::No);
    assert_eq!(
        value("feature", "std").holds(&back.configuration),
        Holds::Yes
    );

    let undecided = krate(Configuration::default());
    for c in [
        option("unix"),
        not(option("unix")),
        value("feature", "std"),
        Condition::All(vec![option("test"), option("doc")]),
    ] {
        assert_eq!(c.holds(&undecided.configuration), Holds::Maybe, "{c:?}");
    }
    assert_eq!(
        Condition::Literal(false).holds(&undecided.configuration),
        Holds::No
    );
}
