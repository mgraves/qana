//! P4 gates: binding correctness (shadowing, definition-before-use,
//! cross-file), THE FIREWALL (body edits never invalidate other files'
//! resolutions — proven by recompute counters), and the differential
//! gate (incremental DB ≡ fresh DB after arbitrary edit sequences).

use qana_engine::*;
use qana_grammar::demo::{demo_grammar, demo_syn_grammar};
use qana_grammar::{build_lr, CompiledLexer, LrTables, SynGrammar};
use qana_sem::{demo_binding_config, SemDb, Target};

struct Pipe {
    lexer: CompiledLexer,
    sg: SynGrammar,
    tables: LrTables,
}

fn pipe() -> Pipe {
    let (g, ids) = demo_grammar();
    let lexer = CompiledLexer::build(&g).unwrap();
    let sg = demo_syn_grammar(&ids, &lexer.vocab);
    let tables = build_lr(&sg);
    assert!(tables.conflicts.is_empty());
    Pipe { lexer, sg, tables }
}

fn session<'l>(p: &'l Pipe, src: &str) -> IncSession<'l> {
    IncSession::new(&p.lexer, &p.sg, &p.tables, src).unwrap()
}

fn db_with(p: &Pipe, docs: &[(&str, &IncSession<'_>)]) -> SemDb {
    let mut db = SemDb::new(demo_binding_config(&p.sg));
    for (uri, s) in docs {
        db.set_tree(uri, s.tree().unwrap().clone());
    }
    db
}

#[test]
fn scoping_shadowing_and_definition_before_use() {
    let p = pipe();
    let src = "let x = 1;\nlet y = x;\n{ let x = 2;\n  let z = x;\n}\nlet w = x;\nlet q = missing;\n";
    let s = session(&p, src);
    let mut db = db_with(&p, &[("a", &s)]);
    let st = db.symbols("a");
    let res = db.resolve("a");

    let ref_named = |name: &str, nth: usize| {
        st.refs
            .iter()
            .enumerate()
            .filter(|(_, r)| r.name == name)
            .nth(nth)
            .map(|(i, _)| i)
            .unwrap()
    };
    let def_span = |i: usize| st.defs[i].span;

    // y = x → outer x (order 1).
    let Target::Local { def } = res[ref_named("x", 0)] else { panic!() };
    assert_eq!(&src[def_span(def).0 as usize..def_span(def).1 as usize], "x");
    assert!(st.defs[def].top_level);

    // z = x → SHADOWED inner x (block scope).
    let Target::Local { def } = res[ref_named("x", 1)] else { panic!() };
    assert!(!st.defs[def].top_level, "inner x wins inside the block");

    // w = x → back to the OUTER x after the block closes.
    let Target::Local { def } = res[ref_named("x", 2)] else { panic!() };
    assert!(st.defs[def].top_level, "outer x visible again");

    // missing → unresolved, and diagnosed.
    assert_eq!(res[ref_named("missing", 0)], Target::Unresolved);
    let unres = db.unresolved("a");
    assert_eq!(unres.len(), 1);
    assert_eq!(unres[0].0, "missing");
    assert_eq!(&src[unres[0].1 .0 as usize..unres[0].1 .1 as usize], "missing");
}

#[test]
fn use_before_definition_is_unresolved() {
    let p = pipe();
    let s = session(&p, "let a = later;\nlet later = 1;\n");
    let mut db = db_with(&p, &[("a", &s)]);
    let res = db.resolve("a");
    assert_eq!(res[0], Target::Unresolved, "definition-before-use scoping");
}

#[test]
fn cross_file_resolution_through_signatures() {
    let p = pipe();
    let a = session(&p, "let shared = 42;\n{ let hidden = 1; }\n");
    let b = session(&p, "let use1 = shared;\nlet use2 = hidden;\n");
    let mut db = db_with(&p, &[("a", &a), ("b", &b)]);
    let res = db.resolve("b");
    // `shared` is a's top-level export.
    assert!(matches!(&res[0], Target::Foreign { uri, .. } if uri == "a"));
    // `hidden` is block-local in a: NOT exported.
    assert_eq!(res[1], Target::Unresolved);

    // Navigation: definition of `shared` from file b lands in file a.
    let off = b.buf.reproduce().find("shared").unwrap() as u32;
    let (uri, span) = db.definition("b", off).unwrap();
    assert_eq!(uri, "a");
    assert_eq!(&a.buf.reproduce()[span.0 as usize..span.1 as usize], "shared");

    // References from the def site see the foreign use.
    let (refs, _) = db.references("a", span.0).unwrap();
    assert!(refs.iter().any(|(u, _)| u == "b"));
}

#[test]
fn firewall_body_edits_never_cross_files_or_items() {
    let p = pipe();
    // File a = 2 top-level ITEMS: a let and an if-block.
    let src_a = "let exported = 1;\nif (exported) {\n  let body = 2;\n  emit(body, 1);\n}\n";
    let mut a = session(&p, src_a);
    let b = session(&p, "let user = exported;\n");
    let mut db = db_with(&p, &[("a", &a), ("b", &b)]);
    db.unresolved("a");
    db.unresolved("b");
    let base = db.stats;

    // BODY edit in a's second item. The signature of `a` is unchanged
    // AND no top-level name sequence moved, so: exactly ONE fragment
    // walk and ONE item resolution — the other item and all of b are
    // memoized (L9, now at item granularity).
    a.edit(&p.sg, &p.tables, &[LineEdit {
        start: 2,
        end: 3,
        replacement: vec![Line::new("  let body = 99;", LineTerm::Lf)],
    }])
    .unwrap();
    db.set_tree("a", a.tree().unwrap().clone());
    db.unresolved("a");
    db.unresolved("b");
    let after_body = db.stats;
    assert_eq!(
        after_body.fragments_computed - base.fragments_computed,
        1,
        "one fragment walk for the edited item"
    );
    assert_eq!(
        after_body.item_resolves_computed - base.item_resolves_computed,
        1,
        "one item resolution: the edited item; sibling and b memoized"
    );

    // SIGNATURE edit in a: rename the exported def. Downstream items of
    // a (env changed) and b (foreign fingerprint moved) recompute their
    // RESOLUTIONS — but still only ONE fragment walk.
    a.edit(&p.sg, &p.tables, &[LineEdit {
        start: 0,
        end: 1,
        replacement: vec![Line::new("let renamed = 1;", LineTerm::Lf)],
    }])
    .unwrap();
    db.set_tree("a", a.tree().unwrap().clone());
    db.unresolved("a");
    let res_b = db.resolve("b");
    let after_sig = db.stats;
    // Two fragments, not one: the newline between the items is trivia
    // attached INSIDE the second item's leading spine (losslessness),
    // so re-lexing it re-anchors the neighbor's Arc. Bounded to exactly
    // one right neighbor, and the recomputed fragment is value-equal.
    assert_eq!(
        after_sig.fragments_computed - after_body.fragments_computed,
        2,
        "edited item + its right neighbor (trailing-newline adjacency)"
    );
    assert_eq!(
        after_sig.item_resolves_computed - after_body.item_resolves_computed,
        3,
        "edited item + its downstream sibling + b's single item"
    );
    assert_eq!(res_b[0], Target::Unresolved, "b's use of the old name breaks");
}

#[test]
fn rename_edits_are_complete_and_correct() {
    let p = pipe();
    let src_a = "let shared = 1;\nlet local = shared;\n";
    let src_b = "let mirror = shared;\n";
    let a = session(&p, src_a);
    let b = session(&p, src_b);
    let mut db = db_with(&p, &[("a", &a), ("b", &b)]);
    let off = src_a.find("shared").unwrap() as u32;
    let edits = db.rename_edits("a", off).unwrap();
    // Apply the edits textually and re-check: no unresolved names remain.
    let apply = |src: &str, spans: &[(u32, u32)]| {
        let mut out = src.to_string();
        let mut spans = spans.to_vec();
        spans.sort_by_key(|s| std::cmp::Reverse(s.0));
        for (s, e) in spans {
            out.replace_range(s as usize..e as usize, "fresh_name");
        }
        out
    };
    let new_a = apply(src_a, &edits["a"]);
    let new_b = apply(src_b, &edits["b"]);
    assert_eq!(new_a, "let fresh_name = 1;\nlet local = fresh_name;\n");
    assert_eq!(new_b, "let mirror = fresh_name;\n");
    let a2 = session(&p, &new_a);
    let b2 = session(&p, &new_b);
    let mut db2 = db_with(&p, &[("a", &a2), ("b", &b2)]);
    assert!(db2.unresolved("a").is_empty());
    assert!(db2.unresolved("b").is_empty());
}

#[test]
fn differential_incremental_db_equals_fresh_db() {
    let p = pipe();
    const POOL: &[&str] = &[
        "let x = 1 + 2;",
        "let y = x;",
        "{ let x = 9;\n  let inner = x; }",
        "emit(x, y);",
        "let z = missing_thing;",
        "// comment",
    ];
    let mut srcs =
        vec!["let x = 1;\nlet y = x;\n".to_string(), "let user = x;\nlet v = y;\n".to_string()];
    let mut sessions: Vec<IncSession<'_>> =
        srcs.iter().map(|s| session(&p, s)).collect();
    let mut db = SemDb::new(demo_binding_config(&p.sg));
    db.set_tree("a", sessions[0].tree().unwrap().clone());
    db.set_tree("b", sessions[1].tree().unwrap().clone());

    let mut seed = 0x5EEDu64;
    let mut rand = move |n: usize| {
        seed ^= seed >> 12;
        seed ^= seed << 25;
        seed ^= seed >> 27;
        (seed.wrapping_mul(0x2545F4914F6CDD1D) % n.max(1) as u64) as usize
    };
    for _round in 0..25 {
        let which = rand(2);
        let uri = if which == 0 { "a" } else { "b" };
        let lines = sessions[which].buf.lines.len() - 1;
        let start = rand(lines.max(1));
        let text = POOL[rand(POOL.len())];
        let replacement: Vec<Line> = text
            .split('\n')
            .map(|t| Line::new(t, LineTerm::Lf))
            .collect();
        sessions[which]
            .edit(&p.sg, &p.tables, &[LineEdit {
                start,
                end: (start + 1).min(lines),
                replacement,
            }])
            .unwrap();
        db.set_tree(uri, sessions[which].tree().unwrap().clone());

        // Differential: a from-scratch DB must agree on every query.
        let mut fresh = SemDb::new(demo_binding_config(&p.sg));
        fresh.set_tree("a", sessions[0].tree().unwrap().clone());
        fresh.set_tree("b", sessions[1].tree().unwrap().clone());
        for u in ["a", "b"] {
            assert_eq!(db.resolve(u), fresh.resolve(u), "resolutions must agree for {u}");
            assert_eq!(db.unresolved(u), fresh.unresolved(u));
        }
        srcs[0] = sessions[0].buf.reproduce();
        srcs[1] = sessions[1].buf.reproduce();
    }
}

/// The per-item claim beyond P4: inserting a NON-DEFINING statement
/// mid-file leaves every downstream item's resolution memoized (the
/// top-level name sequence — the environment fingerprint — is
/// unchanged), even though every downstream item shifted position.
#[test]
fn inserting_a_non_def_item_leaves_downstream_memoized() {
    let p = pipe();
    let src: String = (0..40)
        .map(|i| format!("let v{i} = {};\nemit(v{i}, 1);\n", if i == 0 { "1".into() } else { format!("v{}", i - 1) }))
        .collect();
    let mut s = session(&p, &src);
    let mut db = db_with(&p, &[("a", &s)]);
    db.unresolved("a");
    let base = db.stats;

    // Insert `emit(v3, 9);` mid-file: no top-level def added.
    s.edit(&p.sg, &p.tables, &[LineEdit {
        start: 20,
        end: 20,
        replacement: vec![Line::new("emit(v3, 9);", LineTerm::Lf)],
    }])
    .unwrap();
    db.set_tree("a", s.tree().unwrap().clone());
    assert!(db.unresolved("a").is_empty());
    let after = db.stats;
    assert!(
        after.fragments_computed - base.fragments_computed <= 2,
        "edit-sized fragments (inserted item + trivia-adjacent neighbor), got {}",
        after.fragments_computed - base.fragments_computed
    );
    assert!(
        after.item_resolves_computed - base.item_resolves_computed <= 2,
        "downstream items stay memoized despite shifting position, got {}",
        after.item_resolves_computed - base.item_resolves_computed
    );

    // And navigation still lands correctly after the shift.
    let now = s.buf.reproduce();
    let off = now.rfind("emit(v39").unwrap() as u32 + 5;
    let (uri, span) = db.definition("a", off).expect("v39 resolves");
    assert_eq!(uri, "a");
    assert_eq!(&now[span.0 as usize..span.1 as usize], "v39");
}

#[test]
fn names_in_scope_orders_innermost_first() {
    let p = pipe();
    let src = "let outer = 1;\n{ let inner = 2;\n  emit(inner, 1);\n}\n";
    let s = session(&p, src);
    let mut db = db_with(&p, &[("a", &s)]);
    let off = src.find("emit").unwrap() as u32;
    let names = db.names_in_scope("a", off);
    let inner_pos = names.iter().position(|n| n == "inner").unwrap();
    let outer_pos = names.iter().position(|n| n == "outer").unwrap();
    assert!(inner_pos < outer_pos, "innermost first: {names:?}");
}
