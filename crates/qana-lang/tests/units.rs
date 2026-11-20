//! The binding annotations a project index reads units, paths,
//! visibility, imports, attributes and impl blocks from (`@unit`, `@root`,
//! `@vis`, `@visibility`, `@import`, `@import_glob`, `@import_prefix`,
//! `@loads`, `@attribute`, `@attributes`, `@impl`):
//! what each compiles to, what a path spells, and what each refuses.
//! Unitlang (`examples/units`) is their language; qana-index's tests read
//! its summaries.

use qana_engine::IncSession;
use qana_lang::compile::certify;
use qana_lang::{compile_source, QanaToolchain};
use qana_sem::{Path, Root, SemDb};

const UNITLANG: &str = include_str!("../../../examples/units/unitlang.qana");

/// The diagnostics of a language with one rule, `file`, given as `rule`
/// (and any more rules after it).
fn refusals(rules: &str) -> Vec<String> {
    let lang = format!(
        "language Bad\n\
         token WS = /\\s+/ @trivia\n\
         token IDENT = /[a-z]+/ @specialize\n\
         keywords IDENT = pub crate super self in use mod\n\
         start file\n\
         {rules}\n"
    );
    compile_source(&QanaToolchain::new(), &lang).diags.into_iter().map(|d| d.msg).collect()
}

fn refused(rules: &str, msg: &str) {
    let diags = refusals(rules);
    assert!(diags.iter().any(|d| d.contains(msg)), "`{msg}` in {diags:?}");
}

#[test]
fn unitlang_compiles_each_annotation_into_the_binding() {
    let out = compile_source(&QanaToolchain::new(), UNITLANG);
    assert!(out.diags.is_empty(), "{:?}", out.diags);
    let b = &out.def.binding;
    let prod = |name: &str| {
        (0..out.def.sg.prods.len()).find(|&i| out.def.sg.prod_name(i) == name).unwrap() as u16
    };
    let has = |list: &[(u16, u16)], name: &str| list.iter().any(|&(_, p)| p == prod(name));
    let units: Vec<(u16, u16)> = b.units.iter().map(|e| (e.0, e.1)).collect();
    assert!(has(&units, "ModDecl") && has(&units, "ModDef"));
    assert!(
        b.modules.iter().any(|e| e.1 == prod("ModDef")),
        "a unit with a body is a module of it"
    );
    assert!(b.roots.iter().any(|e| e.1 == prod("SuperRoot") && e.2 == Root::Unit(1)));
    assert!(b.roots.iter().any(|e| e.1 == prod("Global") && e.2 == Root::Global && e.3.is_some()));
    assert!(b.vis_levels.iter().any(|e| e.1 == prod("PubCrate") && e.2 == Some(Root::Crate)));
    assert!(b.vis_levels.iter().any(|e| e.1 == prod("PubIn") && e.2.is_none() && e.3.is_some()));
    assert!(b.vis_items.iter().any(|e| e.1 == prod("Field")));
    assert!(b.imports.iter().any(|e| e.1 == prod("UseGlob") && e.3));
    assert!(b.imports.iter().any(|e| e.1 == prod("UseAllGlob") && e.2.is_none()));
    assert!(b.prefixes.iter().any(|e| e.1 == prod("UseGlobalGroup") && e.2.is_none()));
    assert!(b.impls.iter().any(|e| e.1 == prod("ImplFor") && e.3.is_some()));
    assert!(has(&b.loads, "Extern") && has(&b.loads, "ExternSelf") && !has(&b.loads, "UsePath"));
    assert!(b.attributes.iter().any(|e| e.1 == prod("Attr")));
    assert!(b.attributes.iter().any(|e| e.1 == prod("When")), "a conditional one is one too");
    assert!(b.attribute_lists.iter().any(|e| e.1 == prod("File") && e.3.is_none()));
    assert!(b.attribute_lists.iter().any(|e| e.1 == prod("PubItem") && e.3.is_some()));
    assert!(b.attribute_lists.iter().any(|e| e.1 == prod("When") && e.3.is_none()));
    assert!(!b.module_tier(), "importing a path does not make each file a flat module");
}

#[test]
fn a_unit_with_a_body_binds_its_members_as_a_module_does() {
    let out = compile_source(&QanaToolchain::new(), UNITLANG);
    let (lexer, tables) = certify(&out.def).expect("in envelope");
    let lexer: &'static _ = Box::leak(Box::new(lexer));
    let tables: &'static _ = Box::leak(Box::new(tables));
    let sg: &'static _ = Box::leak(Box::new(out.def.sg.clone()));
    let text = "mod geo { pub struct Point { } }\nfn f() { geo::Point; }\n";
    let session = IncSession::new(lexer, sg, tables, text).unwrap();
    let mut db = SemDb::new(out.def.binding.clone());
    db.set_tree("d", session.tree().unwrap().clone());
    let use_site = text.rfind("Point").unwrap() as u32;
    let (_, span) = db.definition("d", use_site).expect("`geo::Point` binds");
    assert_eq!(span.0 as usize, text.find("Point").unwrap());
}

#[test]
fn a_token_is_a_path_of_one_segment_and_a_qualified_node_its_base_then_its_name() {
    let out = compile_source(&QanaToolchain::new(), UNITLANG);
    let (lexer, tables) = certify(&out.def).expect("in envelope");
    let lexer: &'static _ = Box::leak(Box::new(lexer));
    let tables: &'static _ = Box::leak(Box::new(tables));
    let sg: &'static _ = Box::leak(Box::new(out.def.sg.clone()));
    let session = IncSession::new(lexer, sg, tables, "use crate::a::b;").unwrap();
    let tree = session.tree().unwrap().clone();
    // The `use` item's tree's path, found by what it spells.
    fn find(n: &qana_grammar::GreenNode, cfg: &qana_sem::BindingConfig, out: &mut Vec<Path>) {
        if let Some(p) = Path::of_node(n, cfg) {
            out.push(p);
        }
        for c in &n.children {
            if let qana_grammar::GreenChild::Node(m) = c {
                find(m, cfg, out);
            }
        }
    }
    let mut spelled = Vec::new();
    find(&tree, &out.def.binding, &mut spelled);
    let whole = Path { root: Some(Root::Crate), segments: vec!["a".into(), "b".into()] };
    assert!(spelled.contains(&whole), "{spelled:?}");
    let after = Path { root: None, segments: vec!["c".into()] }.after(&whole);
    assert_eq!(after.map(|p| p.segments), Some(vec!["a".to_string(), "b".into(), "c".into()]));
    let rooted = Path { root: Some(Root::Crate), segments: vec![] };
    assert_eq!(rooted.after(&whole), None, "a rooted path cannot follow a prefix");
    let bare_self = Path { root: Some(Root::Unit(0)), segments: vec![] };
    assert_eq!(bare_self.after(&whole), Some(whole), "a bare `self` is the prefix itself");
}

#[test]
fn a_unit_is_a_definition() {
    refused("rule file = File: \"mod\" n:IDENT @unit(n)", "@unit names the definition");
    refused(
        "rule file = File: \"mod\" n:IDENT b:body @def(n) @unit(n, b) @module(b)\nrule body = Body: IDENT*",
        "already the module of its body",
    );
}

#[test]
fn a_root_is_one_of_four() {
    refused("rule file = File: \"self\" @root(here)", "unknown root `here`");
    refused("rule file = File: \"self\" @root(self) @root(self)", "duplicate @root");
}

#[test]
fn a_visibility_is_one_of_its_levels() {
    refused("rule file = File: \"pub\" @vis(everyone)", "@vis takes");
    refused("rule file = File: \"pub\" @vis(in)", "@vis takes");
    refused(
        "rule file = File: v:IDENT n:IDENT @def(n) @visibility(v)",
        "@visibility takes the labels",
    );
}

#[test]
fn an_alternative_imports_one_way() {
    refused("rule file = File: \"use\" p:IDENT @def(p) @import(p) @import_glob(p)", "not both");
    refused("rule file = File: \"use\" p:IDENT @import_glob(p, p)", "@import_glob takes at most");
    refused("rule file = File: \"use\" p:IDENT @import_prefix(p, q)", "no symbol labeled `q`");
}

#[test]
fn what_loads_is_an_import() {
    refused("rule file = Ext: \"use\" n:IDENT @def(n) @loads", "give the alternative @import");
    refused(
        "rule file = Ext: \"use\" n:IDENT @def(n) @import(n) @loads(n)",
        "@loads takes no arguments",
    );
}

#[test]
fn attributes_are_said_of_a_path_and_listed_in_a_rule() {
    refused("rule file = A: p:IDENT @attribute", "@attribute takes the label of the path");
    refused("rule file = A: p:IDENT @attribute(q)", "no symbol labeled `q`");
    refused("rule file = A: \"use\" a:IDENT @attributes(a)", "a body must be a rule symbol");
    refused(
        "rule file = A: \"use\" a:list @attributes(a, b)\nrule list = IDENT*",
        "no symbol labeled `b`",
    );
}

#[test]
fn an_impl_block_names_its_type() {
    refused("rule file = File: \"impl\" t:IDENT @impl", "@impl takes");
    refused("rule file = File: \"impl\" t:IDENT @impl(x)", "no symbol labeled `x`");
}

#[test]
fn the_path_at_a_name_is_its_qualifiers_then_it_after_its_groups_prefix() {
    let out = compile_source(&QanaToolchain::new(), UNITLANG);
    let (lexer, tables) = certify(&out.def).expect("in envelope");
    let lexer: &'static _ = Box::leak(Box::new(lexer));
    let tables: &'static _ = Box::leak(Box::new(tables));
    let sg: &'static _ = Box::leak(Box::new(out.def.sg.clone()));
    let text =
        "mod m {\n    fn f() {\n        super::x::y;\n    }\n}\nuse a::{b::{c, d as e}, self};\n";
    let session = IncSession::new(lexer, sg, tables, text).unwrap();
    let tree = session.tree().unwrap().clone();
    let at = |word: &str| {
        let offset = text.find(word).expect(word) as u32;
        qana_sem::path_at(&tree, &out.def.binding, offset)
            .map(|(units, path, _)| (units, path.root, path.segments))
    };
    let s = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
    assert_eq!(
        at("y;"),
        Some((s(&["m"]), Some(Root::Unit(1)), s(&["x", "y"]))),
        "in the units it is in"
    );
    assert_eq!(at("x::y"), Some((s(&["m"]), Some(Root::Unit(1)), s(&["x"]))));
    assert_eq!(at("c,"), Some((s(&[]), None, s(&["a", "b", "c"]))), "after its groups' prefixes");
    assert_eq!(
        at("b::"),
        Some((s(&[]), None, s(&["a", "b"]))),
        "a prefix after the groups around it"
    );
    assert_eq!(at("d as"), Some((s(&[]), None, s(&["a", "b", "d"]))));
    assert_eq!(at("e}"), None, "a name a rename defines is no reference");
    assert_eq!(at(" fn"), None, "nor is space");
}

#[test]
fn the_path_at_a_name_says_whether_an_import_names_it() {
    let out = compile_source(&QanaToolchain::new(), UNITLANG);
    let (lexer, tables) = certify(&out.def).expect("in envelope");
    let lexer: &'static _ = Box::leak(Box::new(lexer));
    let tables: &'static _ = Box::leak(Box::new(tables));
    let sg: &'static _ = Box::leak(Box::new(out.def.sg.clone()));
    let text = "use a::b as c;\nuse x::{y};\npub(in crate::v) fn f() {\n    a::b;\n}\n";
    let session = IncSession::new(lexer, sg, tables, text).unwrap();
    let tree = session.tree().unwrap().clone();
    let imported = |word: &str| {
        let offset = text.find(word).expect(word) as u32;
        qana_sem::path_at(&tree, &out.def.binding, offset).map(|(_, _, imported)| imported)
    };
    assert_eq!(imported("b as"), Some(true), "a renamed import's path");
    assert_eq!(imported("x::"), Some(true), "a group's prefix");
    assert_eq!(imported("y}"), Some(true), "a group's tree");
    assert_eq!(imported("v)"), Some(true), "a visibility's path");
    assert_eq!(imported("b;"), Some(false), "a path in a body");
}
