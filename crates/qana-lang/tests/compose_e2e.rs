//! P7 gates: the composition tier. chartlang hosting `.qana` fenced
//! islands, built by the generic compose() operator and re-certified as
//! ONE product grammar — then every existing guarantee is exercised ON
//! composed documents: losslessness, incremental ≡ batch across island
//! edits, recovery under fence damage, product highlighting, and host
//! semantics flowing around islands.

use qana_engine::{IncSession, Line, LineEdit, LineTerm};
use qana_grammar::green::semantic_eq;
use qana_grammar::GreenNode;
use qana_lang::{chartlang_with_rg_islands, ComposedToolchain};
use qana_sem::SemDb;

fn doc() -> String {
    let mut s = String::new();
    s.push_str("let grammar_version = 3;\n");
    s.push_str("emit(grammar_version, 1);\n");
    s.push_str("```qana\n");
    s.push_str("language Embedded\n");
    s.push_str("token A = /a+/ @style(number)\n");
    s.push_str("token SEMI = \";\"\n");
    s.push_str("rule file = File: item*\n");
    s.push_str("rule item = Item: A SEMI\n");
    s.push_str("```\n");
    s.push_str("let after = grammar_version + 1;\n");
    s.push_str("emit(after, 2);\n");
    s.push_str("```qana\n");
    s.push_str("token B = \"b\"\n");
    s.push_str("rule file2 = File2: B?\n");
    s.push_str("```\n");
    s.push_str("let tail = after;\n");
    s
}

fn find_prod<'g>(n: &'g GreenNode, prod: u16) -> Option<&'g GreenNode> {
    if n.prod == prod {
        return Some(n);
    }
    for c in &n.children {
        if let qana_grammar::GreenChild::Node(m) = c {
            if let Some(hit) = find_prod(m, prod) {
                return Some(hit);
            }
        }
    }
    None
}

#[test]
fn product_certifies_and_parses_composed_documents_losslessly() {
    // Certification (L1/L2 lints + zero LR conflicts on the PRODUCT)
    // happens inside the constructor — the composition theorem, checked.
    let tc = chartlang_with_rg_islands();
    let src = doc();
    let s = IncSession::new(&tc.lexer, &tc.sg, &tc.tables, &src).unwrap();
    assert!(s.last_repairs.is_empty(), "composed doc parses cleanly: {:?}", s.last_repairs);
    let tree = s.tree().unwrap();
    assert_eq!(tree.text(), src, "lossless through the island boundary");

    // The island production exists and CONTAINS a full .qana parse: a
    // guest TokenDef and a guest sugar rule, as guest nonterminals.
    let island_prod = tc.map.islands[0].2;
    let island = find_prod(tree, island_prod).expect("island node");
    assert!(island.text().contains("language Embedded"));
    let qana_token_def = (0..tc.sg.prods.len())
        .find(|&i| tc.sg.prod_name(i) == "QanaTokenDef")
        .unwrap() as u16;
    assert!(
        find_prod(island, qana_token_def).is_some(),
        "guest TokenDef parsed inside the island"
    );
    let qana_sym_star = (0..tc.sg.prods.len())
        .find(|&i| tc.sg.prod_name(i) == "QanaSymNameStar")
        .unwrap() as u16;
    assert!(
        find_prod(island, qana_sym_star).is_some(),
        "guest EBNF sugar parsed inside the island"
    );

    // Product highlighting: chartlang keyword class outside, qana regexp
    // class inside — one legend, one pass.
    let toks = qana_services::semantic_tokens_full(&tc.lexer, &s.buf, &tc.styles);
    let classes: Vec<u32> = toks.data.chunks(5).map(|q| q[3]).collect();
    let legend = &tc.styles.legend;
    let keyword = legend.iter().position(|c| *c == "keyword").unwrap() as u32;
    let regexp = legend.iter().position(|c| *c == "regexp").unwrap() as u32;
    assert!(classes.contains(&keyword), "host keywords styled");
    assert!(classes.contains(&regexp), "guest pattern styled INSIDE the island");
}

#[test]
fn island_edits_hold_the_gate_and_reuse_the_host() {
    let tc = chartlang_with_rg_islands();
    // A larger composed doc: host statements around a sizable island.
    let mut src = String::new();
    for i in 0..120 {
        src.push_str(&format!("let h{i} = {i};\n"));
    }
    src.push_str("```qana\n");
    for i in 0..60 {
        src.push_str(&format!("token T{i} = \"t{i}\"\n"));
    }
    src.push_str("rule file = File: T0\n```\n");
    for i in 0..120 {
        src.push_str(&format!("emit(h{i}, {i});\n"));
    }

    let mut s = IncSession::new(&tc.lexer, &tc.sg, &tc.tables, &src).unwrap();
    assert!(s.last_repairs.is_empty());

    let gate = |s: &IncSession<'_>, tc: &ComposedToolchain| {
        let now = s.buf.reproduce();
        let batch = IncSession::new(&tc.lexer, &tc.sg, &tc.tables, &now).unwrap();
        assert_eq!(s.tree().unwrap().text(), now, "lossless");
        assert!(
            semantic_eq(s.tree().unwrap(), batch.tree().unwrap()),
            "incremental ≡ batch on the composed grammar"
        );
    };

    // Edit INSIDE the island (guest territory).
    let out = s
        .edit(&tc.sg, &tc.tables, &[LineEdit {
            start: 150,
            end: 151,
            replacement: vec![Line::new("token T30 = \"tt30\" @style(string)", LineTerm::Lf)],
        }])
        .unwrap();
    gate(&s, &tc);
    assert!(
        out.stats.reuse_fraction() > 0.9,
        "island edit reuses host AND untouched guest content: {:?}",
        out.stats
    );

    // Edit in the HOST after the island.
    let out = s
        .edit(&tc.sg, &tc.tables, &[LineEdit {
            start: 260,
            end: 261,
            replacement: vec![Line::new("emit(h50, 999);", LineTerm::Lf)],
        }])
        .unwrap();
    gate(&s, &tc);
    assert!(out.stats.reuse_fraction() > 0.9, "host edit reuses the island: {:?}", out.stats);
}

#[test]
fn fence_damage_recovers_and_heals() {
    let tc = chartlang_with_rg_islands();
    let src = doc();
    let mut s = IncSession::new(&tc.lexer, &tc.sg, &tc.tables, &src).unwrap();
    let close_line = src.lines().position(|l| l == "```").unwrap();

    // Delete the first island's CLOSE fence: the island extends (like an
    // unclosed block comment) — parsing stays TOTAL and gate-clean.
    s.edit(&tc.sg, &tc.tables, &[LineEdit {
        start: close_line,
        end: close_line + 1,
        replacement: vec![],
    }])
    .unwrap();
    let now = s.buf.reproduce();
    let batch = IncSession::new(&tc.lexer, &tc.sg, &tc.tables, &now).unwrap();
    assert_eq!(s.tree().unwrap().text(), now, "lossless under fence damage");
    assert!(semantic_eq(s.tree().unwrap(), batch.tree().unwrap()));

    // Restore the fence: clean again.
    s.edit(&tc.sg, &tc.tables, &[LineEdit {
        start: close_line,
        end: close_line,
        replacement: vec![Line::new("```", LineTerm::Lf)],
    }])
    .unwrap();
    assert!(s.last_repairs.is_empty(), "healed: {:?}", s.last_repairs);
    assert_eq!(s.buf.reproduce(), src);
    let batch = IncSession::new(&tc.lexer, &tc.sg, &tc.tables, &src).unwrap();
    assert!(semantic_eq(s.tree().unwrap(), batch.tree().unwrap()));
}

#[test]
fn host_semantics_flow_around_islands_even_broken_ones() {
    let tc = chartlang_with_rg_islands();
    // Garbage INSIDE the island; host defs before and after.
    let src = "let before = 1;\n```qana\n%%% utter garbage (((\n```\nlet after = before;\nemit(after, 1);\n";
    let s = IncSession::new(&tc.lexer, &tc.sg, &tc.tables, src).unwrap();
    assert_eq!(s.tree().unwrap().text(), src, "lossless around a broken island");

    let mut db = SemDb::new(tc.binding.clone());
    db.set_tree("a", s.tree().unwrap().clone());
    // `after = before` resolves across the broken island.
    let off = src.rfind("before").unwrap() as u32;
    let (uri, span) = db.definition("a", off).expect("host ref resolves across the island");
    assert_eq!(uri, "a");
    assert_eq!(&src[span.0 as usize..span.1 as usize], "before");
    // Unresolved diagnostics stay CONTAINED: the island garbage is
    // diagnosed (as sealed guest refs), host names never are.
    let island_start = src.find("```qana").unwrap() as u32;
    let island_end = src.find("\n```\n").unwrap() as u32 + 4;
    for (name, span) in db.unresolved("a") {
        assert!(
            span.0 >= island_start && span.1 <= island_end,
            "unresolved `{name}` must lie inside the island, got {span:?}"
        );
    }
}

/// Guest binding, composed: IntelliSense INSIDE islands. The island is
/// a barrier scope carrying the guest's unordered semantics — forward
/// references between island rules resolve, names are island-local,
/// and the seal holds in both directions.
#[test]
fn island_intellisense_resolves_within_and_never_across() {
    let tc = chartlang_with_rg_islands();
    let src = "\
let shared = 1;\n\
```qana\n\
rule file = File: widget*\n\
rule widget = Widget: A \"x\"\n\
token A = /a+/\n\
token X = \"x\"\n\
```\n\
let out = shared;\n\
```qana\n\
rule widget = W2: B\n\
token B = \"b\"\n\
```\n\
emit(missing_host, 1);\n";
    let s = IncSession::new(&tc.lexer, &tc.sg, &tc.tables, src).unwrap();
    assert!(s.last_repairs.is_empty(), "{:?}", s.last_repairs);
    let mut db = SemDb::new(tc.binding.clone());
    db.set_tree("a", s.tree().unwrap().clone());

    // FORWARD reference inside the island: `widget*` on the first rule
    // line resolves to `rule widget` declared BELOW it.
    let use_at = src.find("widget*").unwrap() as u32;
    let (uri, span) = db.definition("a", use_at).expect("forward island ref resolves");
    assert_eq!(uri, "a");
    let decl_at = src.find("rule widget").unwrap() + "rule ".len();
    assert_eq!(span.0 as usize, decl_at, "resolves to the island's own rule");

    // Same for the token used before declaration.
    let a_use = src.find("Widget: A").unwrap() + "Widget: ".len();
    let (_, span) = db.definition("a", a_use as u32).expect("token ref resolves");
    assert_eq!(&src[span.0 as usize..span.1 as usize], "A");
    assert!(span.0 as usize > a_use, "declared after the use — unordered island scope");

    // References within the island; rename spans stay inside it.
    let (refs, _) = db.references("a", decl_at as u32).expect("island refs");
    assert_eq!(refs.len(), 1, "one use of `widget`: {refs:?}");
    let island2_widget = src.rfind("rule widget").unwrap() + "rule ".len();
    let (refs2, _) = db.references("a", island2_widget as u32).expect("second island");
    assert!(refs2.is_empty(), "islands do not share namespaces: {refs2:?}");

    // THE SEAL, both directions: the host `shared` is not visible as a
    // guest name… (a guest ref to it would be unresolved — covered
    // below by the diagnostics), and island rules are invisible to the
    // host (`missing_host` can't accidentally hit island names).
    let unresolved = db.unresolved("a");
    assert!(
        unresolved.iter().any(|(n, _)| n == "missing_host"),
        "host unresolved diagnosed: {unresolved:?}"
    );
    // Everything INSIDE the well-formed islands resolves: no island
    // name appears in the unresolved list.
    assert!(
        unresolved.iter().all(|(n, _)| n == "missing_host"),
        "island interiors fully resolved: {unresolved:?}"
    );
}

/// A guest reference to a HOST binding is sealed out — unresolved and
/// diagnosed, never a silent cross-language jump.
#[test]
fn guest_refs_never_leak_to_host_bindings() {
    let tc = chartlang_with_rg_islands();
    let src = "let tempting = 1;\n```qana\nrule file = File: tempting\n```\n";
    let s = IncSession::new(&tc.lexer, &tc.sg, &tc.tables, src).unwrap();
    assert!(s.last_repairs.is_empty());
    let mut db = SemDb::new(tc.binding.clone());
    db.set_tree("a", s.tree().unwrap().clone());
    let use_at = src.rfind("tempting").unwrap() as u32;
    assert_eq!(
        db.definition("a", use_at),
        None,
        "the barrier seals the island: no jump to the host let"
    );
    let unresolved = db.unresolved("a");
    assert!(
        unresolved.iter().any(|(n, _)| n == "tempting"),
        "sealed ref is diagnosed: {unresolved:?}"
    );
}

/// Keyword specialization is PER-OWNER: a host keyword is an ordinary
/// identifier inside a guest island. `let` — a chartlang keyword — is a
/// perfectly good rule name in embedded .qana.
#[test]
fn keyword_spaces_stay_separate_across_the_boundary() {
    let tc = chartlang_with_rg_islands();
    let src = "let x = 1;\n```qana\ntoken A = \"a\"\nrule let = IfRule: A\n```\nlet y = x;\n";
    let s = IncSession::new(&tc.lexer, &tc.sg, &tc.tables, src).unwrap();
    assert!(
        s.last_repairs.is_empty(),
        "`let` parses as a guest NAME inside the island: {:?}",
        s.last_repairs
    );
    assert_eq!(s.tree().unwrap().text(), src);
}
