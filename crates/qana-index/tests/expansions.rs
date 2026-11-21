//! Items from macros: what a summary records of a file's invocations and
//! macro definitions, an invocation's expansion read into an expansion
//! summary, a file's summary merged with its expansions, and a resolver
//! over merged summaries.

use std::sync::OnceLock;

use qana_index::{summarize, Form, Path, TextualLink};
use qana_lang::EmbeddedLang;

const GRAMMAR: &str = include_str!("../../../examples/rust/rust.qana");

fn rust() -> &'static EmbeddedLang {
    static LANG: OnceLock<EmbeddedLang> = OnceLock::new();
    LANG.get_or_init(|| EmbeddedLang::from_qana_source(GRAMMAR).expect("rust.qana certifies"))
}

fn path(segments: &[&str]) -> Path {
    Path { root: None, segments: segments.iter().map(|s| s.to_string()).collect() }
}

const SITES: &str = "macro_rules! make { ($n:ident) => { fn $n() {} }; }
#[cfg(test)]
make!(a);
mod m {
    make! { b }
}
impl S {
    make!(c);
}
trait T {
    make!(d);
}
fn body() { make!(e); }
pub macro m2($x:ident) { struct $x; }
";

#[test]
fn a_summary_records_the_invocations_among_items_and_the_macros_defined() {
    let summary = summarize(rust(), SITES);
    let offers = summary.offers();
    // Unit, container, impl block, entry, attributes.
    type Read = (Option<u32>, Vec<String>, Option<u32>, String, usize);
    let read: Vec<Read> = offers
        .invocations()
        .iter()
        .map(|i| (i.unit, i.container.clone(), i.impl_block, i.entry.clone(), i.attributes.len()))
        .collect();
    assert_eq!(
        read,
        [
            (None, vec![], None, "items_opt".to_string(), 1),
            (Some(0), vec!["m".to_string()], None, "items_opt".to_string(), 0),
            (None, vec!["S".to_string()], Some(0), "assoc_items_opt".to_string(), 0),
            (None, vec!["T".to_string()], None, "assoc_items_opt".to_string(), 0),
        ],
        "a module's, an inline module's, an impl block's, a trait's; not a body's"
    );
    assert!(offers.invocations().iter().all(|i| i.path == Some(path(&["make"]))));
    assert!(offers.invocations().iter().all(|i| i.namespace == "macro"));
    let locations = summary.locations();
    let text = |span: (u32, u32)| &SITES[span.0 as usize..span.1 as usize];
    assert_eq!(text(locations.invocation_span(0)), "make!(a);");
    assert_eq!(text(locations.invocation_span(1)), "make! { b }");
    assert_eq!(text(locations.invocation_path_span(2).expect("a path")), "make");
    assert_eq!(locations.invocation_position(2), (7, 4));
    assert_eq!(text(locations.invocation_item_span(0)), "#[cfg(test)]\nmake!(a);");

    let macros: Vec<(String, Form, &str)> = offers
        .macros()
        .iter()
        .enumerate()
        .map(|(k, m)| {
            let name = offers.declarations()[m.declaration as usize].name.clone();
            (name, m.form, text(locations.macro_body_span(k)))
        })
        .collect();
    assert_eq!(
        macros,
        [
            ("m2".to_string(), Form::Macro, "($x:ident) { struct $x; }"),
            ("make".to_string(), Form::MacroRules, " { ($n:ident) => { fn $n() {} }; }"),
        ],
        "by declaration"
    );

    let make = offers.declarations().iter().position(|d| d.name == "make").unwrap() as u32;
    assert!(
        matches!(
            offers.textual(),
            [
                TextualLink::Def { declaration, .. },
                TextualLink::Invocation(0),
                TextualLink::Module(0),
                TextualLink::Invocation(2),
                TextualLink::Invocation(3),
            ] if *declaration == make
        ),
        "{:?}",
        offers.textual()
    );
    assert_eq!(offers.units()[0].textual, [TextualLink::Invocation(1)]);
}

#[test]
fn a_macro_definitions_attributes_say_how_it_is_exported() {
    let summary = summarize(
        rust(),
        "#[macro_export(local_inner_macros)]\nmacro_rules! m { () => {} }\n\
         #[macro_export]\nmacro_rules! n { () => {} }\n",
    );
    let attributes: Vec<(Path, Vec<Path>)> = summary
        .offers()
        .macros()
        .iter()
        .flat_map(|m| m.attributes.iter().map(|a| (a.path.clone(), a.args.clone())))
        .collect();
    assert_eq!(
        attributes,
        [
            (path(&["macro_export"]), vec![path(&["local_inner_macros"])]),
            (path(&["macro_export"]), vec![]),
        ]
    );
}

use qana_index::{
    expand, BuiltinMacro, Edition, ExpandRequest, MacroSource, Root, TypeRef, RECURSION_LIMIT,
};

/// A request to expand `site` (its invocation's own input) by a
/// definition whose body is `body`, at `entry`.
fn request(body: &str, form: Form, site: &str, entry: &str) -> ExpandRequest {
    ExpandRequest {
        definition: text(body, form),
        site: site.to_string(),
        site_edition: Edition::E2021,
        input: None,
        entry: entry.to_string(),
        mark: 1,
        limit: RECURSION_LIMIT as u32,
        local_inner: Vec::new(),
        of_crate: Vec::new(),
        itself: None,
        names: Vec::new(),
    }
}

/// A definition whose body is `body`, in the 2021 edition.
fn text(body: &str, form: Form) -> MacroSource {
    MacroSource::Text { body: body.to_string(), form, edition: Edition::E2021, local_inner: false }
}

const IMPL_META: &str = " { ($ty:ty, $name:literal) => {
    impl $crate::meta::HasMeta for $ty {
        fn name(&self) -> &str { $name }
    }
}; }";

#[test]
fn an_expansion_summary_is_what_the_expansion_declares_where_the_site_writes_it() {
    let site = "crate::impl_meta!(Cell, \"Cell\");";
    let e = expand(rust(), &request(IMPL_META, Form::MacroRules, site, "items_opt"), &[]);
    assert_eq!(e.error(), None);
    let offers = e.offers().expect("an expansion");
    let block = &offers.impls()[0];
    assert_eq!(
        block.trait_path,
        Some(Path {
            root: Some(Root::MacroCrate),
            segments: vec!["meta".to_string(), "HasMeta".to_string()]
        }),
        "`$crate`, which the definition's crate is"
    );
    assert!(
        matches!(&block.self_type, Some(TypeRef::Path { path, .. }) if *path == self::path(&["Cell"]))
    );
    let name = &offers.declarations()[0];
    assert_eq!((name.name.as_str(), name.impl_block), ("name", Some(0)));
    assert!(e.offers().unwrap().invocations().is_empty());
}

const OUTER: &str =
    " { ($(static $n:ident: $t:ty = $e:expr;)*) => { $( $crate::inner!($n, $t, $e); )* }; }";
const INNER: &str = "($n:ident, $t:ty, $e:expr) { const $n: $t = $e; }";

#[test]
fn a_nested_invocation_is_the_summarys_with_what_it_passes_and_expands_in_turn() {
    let site = "tl! { static FOO: u32 = 1; static BAR: u8 = 2; }";
    let outer = expand(rust(), &request(OUTER, Form::MacroRules, site, "items_opt"), &[]);
    assert_eq!(outer.error(), None);
    let invocations = outer.offers().unwrap().invocations();
    assert_eq!(invocations.len(), 2);
    assert_eq!(
        invocations[0].path,
        Some(Path { root: Some(Root::MacroCrate), segments: vec!["inner".to_string()] })
    );
    assert!(outer.offers().unwrap().declarations().is_empty(), "the outer one declares nothing");

    let mut nested = request(INNER, Form::Macro, site, "items_opt");
    nested.input = Some(outer.input(1).expect("its input").clone());
    nested.mark = 2;
    let inner = expand(rust(), &nested, &[]);
    assert_eq!(inner.error(), None);
    let declared = &inner.offers().unwrap().declarations()[0];
    assert_eq!((declared.name.as_str(), declared.kind.as_str()), ("BAR", "constant"));
    assert!(
        matches!(&declared.ty, Some(TypeRef::Path { path, .. }) if *path == self::path(&["u8"]))
    );
}

/// `$crate::name`, as a macro's expansion writes it.
fn crate_path(name: &str) -> Path {
    Path { root: Some(Root::MacroCrate), segments: vec![name.to_string()] }
}

#[test]
fn an_invocation_by_a_path_the_table_holds_is_expanded_with_its_parent() {
    let site = "tl! { static FOO: u32 = 1; static BAR: u8 = 2; }";
    let table = [(crate_path("inner"), vec![text(INNER, Form::Macro)])];
    let outer = expand(rust(), &request(OUTER, Form::MacroRules, site, "items_opt"), &table);
    assert_eq!(outer.error(), None);
    let nested = outer.nested();
    assert_eq!(nested.len(), 2, "both `$crate::inner!`s");
    assert!(outer.input(0).is_none() && outer.input(1).is_none(), "what they pass is not kept");
    assert!(outer.has_tokens());

    // Each is the expansion a request of its own makes.
    let alone = expand(rust(), &request(OUTER, Form::MacroRules, site, "items_opt"), &[]);
    assert!(alone.nested().is_empty(), "with no table, one level");
    for (k, name) in [(0usize, "FOO"), (1, "BAR")] {
        let (i, path, expansions) = &nested[k];
        assert_eq!((*i as usize, path), (k, &crate_path("inner")));
        let mut asked = request(INNER, Form::Macro, site, "items_opt");
        asked.input = Some(alone.input(k).expect("its input").clone());
        asked.mark = 2;
        assert_eq!(*expansions[0], expand(rust(), &asked, &[]), "{name}");
        assert_eq!(expansions[0].offers().unwrap().declarations()[0].name, name);
    }

    // Kept without tokens, the nested ones are without theirs too.
    let kept = outer.without_tokens();
    assert!(!kept.has_tokens());
    assert_eq!(kept.fingerprint(), outer.fingerprint());
    assert_eq!(kept.nested()[1].2[0].offers(), nested[1].2[0].offers());
}

#[test]
fn expansions_nest_with_their_parent_to_the_limit() {
    // `deep!` invokes itself as `$crate::deep!`, one `x` fewer each time.
    let deep = " { () => {}; (x $($t:tt)*) => { $crate::deep! { $($t)* } }; }";
    let table = [(crate_path("deep"), vec![text(deep, Form::MacroRules)])];
    let mut asked = request(deep, Form::MacroRules, "deep! { x x x x x x x x }", "items_opt");
    asked.limit = 5;
    let top = expand(rust(), &asked, &table);
    let mut depth = 1;
    let mut at = &top;
    while let [(0, _, expansions)] = at.nested() {
        assert_eq!(at.input(0), None);
        at = &expansions[0];
        depth += 1;
    }
    assert_eq!(depth, 5, "made to the limit");
    assert_eq!(at.offers().unwrap().invocations().len(), 1, "the sixth is the index's to judge");
    assert!(at.input(0).is_some(), "with what it passes");
}

#[test]
fn a_crate_path_an_earlier_expansion_wrote_nests_where_its_macro_is_the_tables_crates() {
    // `a!` passes `$crate::inner` on to `pass!`, a request of its own,
    // which invokes what it is passed.
    let a = " { () => { pass! { $crate::inner } }; }";
    let first = expand(rust(), &request(a, Form::MacroRules, "a!();", "items_opt"), &[]);
    let pass = " { ($($p:tt)*) => { $($p)*! { pub const X: u8 = 0; } }; }";
    let inner = " { ($($i:item)*) => { $($i)* }; }";
    let table = [(crate_path("inner"), vec![text(inner, Form::MacroRules)])];
    let nested_by = |of_crate: Vec<u32>| {
        let mut asked = request(pass, Form::MacroRules, "a!();", "items_opt");
        asked.input = Some(first.input(0).expect("what `pass!` is passed").clone());
        asked.mark = 2;
        asked.of_crate = of_crate;
        expand(rust(), &asked, &table).nested().len()
    };
    assert_eq!(nested_by(vec![1]), 1, "`a!`'s macro is the table's crate's");
    assert_eq!(nested_by(Vec::new()), 0, "or `$crate` may be another crate");
}

/// How deep `top`'s first nested invocation's expansions nest with it.
fn nested_depth(top: &ExpansionSummary) -> usize {
    let mut depth = 1;
    let mut at = top;
    while let [(0, _, expansions), ..] = at.nested() {
        at = &expansions[0];
        depth += 1;
    }
    depth
}

#[test]
fn a_macro_invoking_itself_by_its_name_alone_nests_with_it() {
    // A tt-muncher: `consts!` invokes `consts!` for what is left.
    let consts = " { () => {}; ($n:ident $($rest:ident)*) => { pub const $n: u8 = 0; consts! { $($rest)* } }; }";
    let mut asked = request(consts, Form::MacroRules, "consts! { A B C D }", "items_opt");
    asked.itself = Some(("consts".to_string(), vec![text(consts, Form::MacroRules)]));
    let top = expand(rust(), &asked, &[]);
    assert_eq!(nested_depth(&top), 5, "A, B, C, D and the empty one, with it");
    assert_eq!(top.nested()[0].1, path(&["consts"]), "by its name");

    // Not where an expansion between binds the name: a macro of that name
    // it defines, or what it imports.
    let shadows = " { () => {}; ($n:ident $($rest:ident)*) => { macro_rules! consts { ($n2:ident) => {} } consts! { $($rest)* } }; }";
    let mut asked = request(shadows, Form::MacroRules, "consts! { A B }", "items_opt");
    asked.itself = Some(("consts".to_string(), vec![text(shadows, Form::MacroRules)]));
    let top = expand(rust(), &asked, &[]);
    assert_eq!(top.error(), None);
    assert!(top.nested().is_empty(), "its own `consts!` is the one it invokes");
    assert!(top.input(0).is_some(), "the index's, with what it passes");
    let imports =
        " { () => {}; ($n:ident $($rest:ident)*) => { use other::*; consts! { $($rest)* } }; }";
    let mut asked = request(imports, Form::MacroRules, "consts! { A B }", "items_opt");
    asked.itself = Some(("consts".to_string(), vec![text(imports, Form::MacroRules)]));
    assert!(expand(rust(), &asked, &[]).nested().is_empty(), "a glob may bring one");
}

#[test]
fn a_local_inner_macros_macro_names_its_crates_macros_by_a_name_alone() {
    // `calls!` writes `helper!` itself and invokes the name it is passed.
    let calls = " { ($m:ident) => { $m! { struct A; } helper! { struct B; } pass! { helper } }; }";
    let mut asked = request(calls, Form::MacroRules, "calls!(helper);", "items_opt");
    asked.definition = MacroSource::Text {
        body: calls.to_string(),
        form: Form::MacroRules,
        edition: Edition::E2021,
        local_inner: true,
    };
    let helper = " { ($($i:item)*) => { $($i)* }; }";
    let table = [(crate_path("helper"), vec![text(helper, Form::MacroRules)])];
    let e = expand(rust(), &asked, &table);
    let paths: Vec<Option<Path>> =
        e.offers().unwrap().invocations().iter().map(|i| i.path.clone()).collect();
    assert_eq!(
        paths,
        [Some(path(&["helper"])), Some(crate_path("helper")), Some(crate_path("pass"))],
        "the name passed in is the caller's; the names it writes, its crate's"
    );
    let nested: Vec<u32> = e.nested().iter().map(|(i, ..)| *i).collect();
    assert_eq!(nested, [1], "the table holds `$crate::helper`, not `$crate::pass`");
    let b = &e.nested()[0].2[0];
    assert_eq!(b.offers().unwrap().declarations()[0].name, "B");

    // A name a `local_inner_macros` macro wrote stays its crate's in the
    // expansions it is passed on to.
    let pass = " { ($m:ident) => { $m! { struct C; } }; }";
    let mut passed = request(pass, Form::MacroRules, "calls!(helper);", "items_opt");
    passed.input = Some(e.input(2).expect("what `pass!` is passed").clone());
    passed.mark = 2;
    let invoked = |local_inner: Vec<u32>| {
        let mut asked = passed.clone();
        asked.local_inner = local_inner;
        expand(rust(), &asked, &[]).offers().unwrap().invocations()[0].path.clone()
    };
    assert_eq!(invoked(vec![1]), Some(crate_path("helper")));
    assert_eq!(invoked(Vec::new()), Some(path(&["helper"])), "where its writer is not one");
}

#[test]
fn an_impl_blocks_items_expand_as_its_members_and_cfg_select_as_each_arm() {
    let body =
        " { (Self = $t:ty) => { pub const fn saturating_sub(self, rhs: Self) -> Self { self } }; }";
    let e = expand(
        rust(),
        &request(body, Form::MacroRules, "uint_like! { Self = u8 }", "assoc_items_opt"),
        &[],
    );
    assert_eq!(e.error(), None);
    let f = &e.offers().unwrap().declarations()[0];
    assert_eq!((f.name.as_str(), f.kind.as_str()), ("saturating_sub", "function"));

    let site = "cfg_select! { unix => { mod native; pub use native::inner; macro_rules! m { () => {} } } _ => { mod os; pub use os::inner; } }";
    let mut select = request("", Form::Macro, site, "items_opt");
    select.definition =
        MacroSource::Builtin { macro_: BuiltinMacro::CfgSelect, condition: Some(path(&["cfg"])) };
    let e = expand(rust(), &select, &[]);
    assert_eq!(e.error(), None);
    let offers = e.offers().unwrap();
    let units: Vec<(&str, bool)> =
        offers.units().iter().map(|u| (u.name.as_str(), u.inline)).collect();
    assert_eq!(units, [("native", false), ("os", false)], "each arm's modules");
    assert_eq!(offers.reexports().len(), 2, "and re-exports");
    assert!(
        offers.textual().iter().any(|link| matches!(link,
            TextualLink::Def { attributes, .. } if attributes.iter().any(|a| a.path == path(&["cfg"])))),
        "a textual definition in an arm is under its condition: {:?}",
        offers.textual()
    );
}

#[test]
fn an_invocation_no_arm_matches_has_rustcs_reason() {
    let e =
        expand(rust(), &request(IMPL_META, Form::MacroRules, "impl_meta!(1 2);", "items_opt"), &[]);
    assert!(e.offers().is_none());
    assert!(e.error().is_some_and(|why| why.starts_with("no rules expected")), "{:?}", e.error());
}

use std::rc::Rc;

use qana_index::{merge, ExpansionSummary, Summary};

const MERGED: &str = "mod meta;
crate::impl_meta!(Cell, \"Cell\");
impl u8 {
    uint_like! { Self = u8 }
}
tl! { static FOO: u32 = 1; }
";

const UINT_LIKE: &str =
    " { (Self = $t:ty) => { pub const fn saturating_sub(self, rhs: Self) -> Self { self } }; }";

/// An invocation's expansions, as [`merge`] asks for them.
type Answers = Option<Vec<(u32, Rc<ExpansionSummary>, Option<Root>)>>;

/// The expansions of [`MERGED`]'s invocations, as a project index would
/// make them: each file invocation's own, and `tl!`'s nested `inner!`.
fn expansions_of(own: &Summary) -> impl Fn(&[(u32, u32)], u32) -> Answers + '_ {
    move |within: &[(u32, u32)], index: u32| {
        let top = within.first().map_or(index, |&(outer, _)| outer) as usize;
        let span = own.locations().invocation_span(top);
        let site = &MERGED[span.0 as usize..span.1 as usize];
        let (body, form, entry) = match (top, within.len()) {
            (0, 0) => (IMPL_META, Form::MacroRules, "items_opt"),
            (1, 0) => (UINT_LIKE, Form::MacroRules, "assoc_items_opt"),
            (2, 0) => (OUTER, Form::MacroRules, "items_opt"),
            (2, 1) => (INNER, Form::Macro, "items_opt"),
            _ => return None,
        };
        let mut asked = request(body, form, site, entry);
        if within.len() == 1 {
            let parent = expand(rust(), &request(OUTER, Form::MacroRules, site, "items_opt"), &[]);
            asked.input = parent.input(index as usize).cloned();
            asked.mark = 2;
        }
        Some(vec![(0, Rc::new(expand(rust(), &asked, &[])), Some(Root::CrateAt(7)))])
    }
}

#[test]
fn a_merged_summary_holds_its_expansions_items_where_their_invocations_stand() {
    let own = summarize(rust(), MERGED);
    assert!(own.offers().open(), "a file with invocations is open");
    let expansions = expansions_of(&own);
    let merged = merge(&own, &expansions);
    let offers = merged.offers();
    let locations = merged.locations();
    let text = |span: (u32, u32)| &MERGED[span.0 as usize..span.1 as usize];

    // `impl_meta!`'s impl block, its trait's path at the definition's crate.
    let (k, block) =
        offers.impls().iter().enumerate().find(|(_, b)| b.trait_path.is_some()).unwrap();
    assert_eq!(
        block.trait_path.as_ref().unwrap().root,
        Some(Root::CrateAt(7)),
        "`$crate` is the crate the index says"
    );
    assert_eq!(
        text(locations.impl_span(k)),
        "crate::impl_meta",
        "the macro writes it: at the call"
    );
    assert_eq!(
        text(locations.impl_type_span(k).unwrap()),
        "Cell",
        "the invocation writes its type"
    );
    let named = |name: &str| offers.declarations().iter().position(|d| d.name == name).unwrap();
    let name = named("name");
    assert_eq!(offers.declarations()[name].impl_block, Some(k as u32));
    assert_eq!(text(locations.name_span(name)), "crate::impl_meta");
    assert_eq!(locations.declared_by(name), Some(0));

    // `uint_like!`'s method, a member of the file's `impl u8`.
    let sub = &offers.declarations()[named("saturating_sub")];
    let own_block = offers.impls().iter().position(|b| b.trait_path.is_none()).unwrap() as u32;
    assert_eq!((sub.impl_block, sub.container.clone()), (Some(own_block), vec!["u8".to_string()]));

    // `tl!`'s nested `inner!`, an invocation of the merged summary, and the
    // constant it declares, where the file writes its name.
    let nested: Vec<(Vec<(u32, u32)>, u32)> =
        offers.invocations().iter().map(|i| (i.within.clone(), i.index)).collect();
    assert_eq!(nested, [(vec![], 0), (vec![], 1), (vec![], 2), (vec![(2, 0)], 0)]);
    assert_eq!(
        offers.invocations()[3].path.as_ref().unwrap().root,
        Some(Root::CrateAt(7)),
        "`$crate::inner!`"
    );
    let foo = named("FOO");
    assert_eq!(text(locations.name_span(foo)), "FOO");
    assert_eq!(locations.position(foo), (5, 13));
    assert_eq!(locations.declared_by(foo), Some(2));
    assert_eq!(offers.declarations()[foo].unit, None);

    assert!(!offers.open(), "every invocation among its items expanded: closed");
    assert_eq!(
        &merged.offers().declarations()[..own.offers().declarations().len()],
        own.offers().declarations(),
        "the file's first"
    );

    let pending = |within: &[(u32, u32)], index: u32| {
        if within.is_empty() && index == 2 {
            None
        } else {
            expansions(within, index)
        }
    };
    assert!(merge(&own, &pending).offers().open(), "one not known yet: open");
    let impl_only = |within: &[(u32, u32)], index: u32| {
        if within.is_empty() && index == 1 {
            None
        } else {
            expansions(within, index)
        }
    };
    assert!(!merge(&own, &impl_only).offers().open(), "an impl block's invocation is no module's");
}

use std::path::{Path as FilePath, PathBuf};

use qana_index::{CrateInfo, CrateSource, Implementation, Resolution, Resolver, UnitGraph};

const META: &str = "pub mod view { pub trait HasMeta { fn name(&self) -> &str; } }
#[macro_export]
macro_rules! impl_meta { ($ty:ty, $name:literal) => { impl $crate::view::HasMeta for $ty { fn name(&self) -> &str { $name } } }; }
";

const KIT: &str = "pub struct Cell;
meta::impl_meta!(Cell, \"Cell\");
";

fn krate(name: &str, root: &str, dependencies: Vec<(String, u32)>) -> CrateInfo {
    CrateInfo {
        name: name.into(),
        kind: "lib".into(),
        root: root.into(),
        edition: "2021".into(),
        dependencies,
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
        macro_use: vec![path(&["macro_use"])],
        macro_export: vec![path(&["macro_export"])],
        macro_prelude: Vec::new(),
        conditions: vec![path(&["cfg"])],
        builtin_attributes: Vec::new(),
        builtin_macros: Vec::new(),
        local_inner_macros: Vec::new(),
        recursion_limit: None,
    }
}

#[test]
fn a_resolver_over_merged_summaries_finds_what_an_expansion_implements() {
    let mut graph = UnitGraph::new();
    let meta = graph.add_crate(krate("meta", "/m/lib.rs", Vec::new()));
    graph.add_crate(krate("kit", "/k/lib.rs", vec![("meta".into(), meta)]));
    let graph = Rc::new(graph);
    let (meta_summary, kit_summary) = (Rc::new(summarize(rust(), META)), summarize(rust(), KIT));
    let resolver = |kit: Summary| {
        Resolver::new(
            graph.clone(),
            vec![
                (PathBuf::from("/m/lib.rs"), meta_summary.clone()),
                (PathBuf::from("/k/lib.rs"), Rc::new(kit)),
            ],
        )
    };
    let kit_file = FilePath::new("/k/lib.rs");
    let missing = Path { root: Some(Root::Crate), segments: vec!["Missing".to_string()] };

    // The invocation names the exported macro, defined in `meta`.
    let before = resolver(kit_summary.clone());
    let invoked = before.invocation_macros(kit_file, 0);
    let impl_meta =
        meta_summary.offers().declarations().iter().position(|d| d.name == "impl_meta").unwrap()
            as u32;
    assert_eq!(invoked, [(PathBuf::from("/m/lib.rs"), impl_meta, meta)]);
    assert_eq!(
        before.resolve(kit_file, &[], &missing, Some("")),
        Resolution::Escapes { segment: 0 },
        "the invocation may declare it"
    );

    // Its expansion, from the definition's body and the invocation's text.
    let macro_def =
        meta_summary.offers().macros().iter().position(|m| m.declaration == impl_meta).unwrap();
    let body = meta_summary.locations().macro_body_span(macro_def);
    let site = kit_summary.locations().invocation_span(0);
    let asked = request(
        &META[body.0 as usize..body.1 as usize],
        Form::MacroRules,
        &KIT[site.0 as usize..site.1 as usize],
        "items_opt",
    );
    let expansion = Rc::new(expand(rust(), &asked, &[]));
    let given = move |within: &[(u32, u32)], index: u32| {
        (within.is_empty() && index == 0)
            .then(|| vec![(0, expansion.clone(), Some(Root::CrateAt(meta)))])
    };
    let after = resolver(merge(&kit_summary, &given));
    let has_meta =
        meta_summary.offers().declarations().iter().position(|d| d.name == "HasMeta").unwrap()
            as u32;
    let found = after.implementations(FilePath::new("/m/lib.rs"), has_meta);
    assert!(
        matches!(found.as_slice(), [Implementation::Block { file, .. }] if file == kit_file),
        "the expansion's `impl $crate::view::HasMeta for Cell`: {found:?}"
    );
    assert_eq!(
        after.resolve(kit_file, &[], &missing, Some("")),
        Resolution::Unresolved { segment: 0 },
        "every invocation expanded: what the module lacks, it lacks"
    );
}
