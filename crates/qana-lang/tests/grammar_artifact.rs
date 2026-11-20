//! A certified grammar is an artifact: it serializes to bytes, and the
//! bytes load back as the same language.
//!
//! For each grammar qana ships, the artifact certified from its source
//! goes through `postcard` (the encoding Synkro keeps artifacts in) and
//! back. The decoded artifact must write the same bytes again, a second
//! certification must write them too, and the language loaded from the
//! decoded artifact must serve real documents exactly as the one served
//! from the certified artifact does: the same trees and repairs, paint,
//! semantic tokens, outline, folds, binding, types, completion and
//! diagnostics, through the services and through a live document, before
//! and after edits. A refused grammar's diagnostics make the same journey.
//!
//! Each grammar is certified once for the whole file (Rust's takes tens of
//! seconds in a debug build), and once more by the test that certifying
//! again writes the same bytes, which runs beside the others.

use linework::{Limner, LineEdit};
use qana_grammar::{GreenChild, GreenNode, LrTables};
use qana_lang::compile::QanaDiag;
use qana_lang::live::LiveDoc;
use qana_lang::{compile_source, EmbeddedLang, GrammarArtifact, QanaToolchain};
use qana_sem::SemDb;
use std::sync::{Arc, OnceLock};

const RUST: &str = include_str!("../../../examples/rust/rust.qana");
const C: &str = include_str!("../../../examples/c/c.qana");
const STRUCTS: &str = include_str!("../../../examples/structs/structlang.qana");
const QANA: &str = include_str!("../qana.qana");

/// The grammars qana ships.
#[derive(Clone, Copy, Debug)]
enum Grammar {
    Rust,
    C,
    Structs,
    Qana,
}

impl Grammar {
    const ALL: [Grammar; 4] = [Grammar::Rust, Grammar::C, Grammar::Structs, Grammar::Qana];

    fn source(self) -> &'static str {
        match self {
            Grammar::Rust => RUST,
            Grammar::C => C,
            Grammar::Structs => STRUCTS,
            Grammar::Qana => QANA,
        }
    }
}

/// A grammar certified: the language served from the certified artifact,
/// and the bytes postcard writes the artifact as.
struct Certified {
    fresh: EmbeddedLang,
    bytes: Vec<u8>,
}

/// `grammar` certified, once for this file.
fn certified(grammar: Grammar) -> &'static Certified {
    static CERTIFIED: [OnceLock<Certified>; 4] = [const { OnceLock::new() }; 4];
    CERTIFIED[grammar as usize].get_or_init(|| {
        let artifact =
            GrammarArtifact::certify(grammar.source()).unwrap_or_else(|d| panic!("{d:?}"));
        let bytes = postcard::to_allocvec(&artifact).expect("an artifact serializes");
        Certified { fresh: EmbeddedLang::from_artifact(artifact), bytes }
    })
}

/// The index of the first byte at which `a` and `b` differ, or where the
/// shorter ends.
fn first_difference(a: &[u8], b: &[u8]) -> Option<usize> {
    a.iter().zip(b).position(|(x, y)| x != y).or((a.len() != b.len()).then(|| a.len().min(b.len())))
}

/// `grammar`'s certified artifact read back from its bytes, and the
/// language loaded from it. The decoded artifact writes the same bytes.
fn loaded(grammar: Grammar) -> EmbeddedLang {
    let Certified { fresh, bytes } = certified(grammar);
    let decoded: GrammarArtifact = postcard::from_bytes(bytes).expect("its bytes decode");
    assert_eq!(decoded.name(), fresh.name);
    assert_eq!(decoded.extensions(), fresh.extensions);
    let rewritten = postcard::to_allocvec(&decoded).expect("a decoded artifact serializes");
    assert_eq!(
        first_difference(bytes, &rewritten),
        None,
        "the decoded `{}` artifact writes the {} bytes it was read from",
        fresh.name,
        bytes.len()
    );
    EmbeddedLang::from_artifact(decoded)
}

/// The start of every real token of `tree` (trivia and repair tokens
/// aside): where names, facts and completions are asked for.
fn token_starts(tree: &GreenNode) -> Vec<u32> {
    fn walk(n: &GreenNode, off: &mut u32, out: &mut Vec<u32>) {
        for c in &n.children {
            match c {
                GreenChild::Node(m) => walk(m, off, out),
                GreenChild::Token(t) => {
                    if !t.trivia && !t.is_missing() {
                        out.push(*off);
                    }
                    *off += c.width();
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(tree, &mut 0, &mut out);
    out
}

/// Every `step`-th of `offsets`, and the last: a sample, for queries that
/// each read much of the document.
fn sample(offsets: &[u32], step: usize) -> Vec<u32> {
    let mut out: Vec<u32> = offsets.iter().copied().step_by(step).collect();
    out.extend(offsets.last().copied());
    out
}

/// `fresh` and `loaded` serve `text` (the document `doc`) identically.
fn same_service(fresh: &EmbeddedLang, loaded: &EmbeddedLang, doc: &str, text: &str) {
    let langs = [fresh, loaded];
    let [a, b] = langs.map(|l| l.session(text));
    let tree = a.tree().expect("total");
    assert_eq!(b.tree(), Some(tree), "{doc}: the same tree");
    assert_eq!(a.last_repairs, b.last_repairs, "{doc}: the same repairs");
    let starts = token_starts(tree);
    assert!(!starts.is_empty(), "{doc} has tokens");

    let [tokens_a, tokens_b] = [(fresh, &a), (loaded, &b)].map(|(l, s)| {
        qana_services::semantic_tokens_full(l.lexer, &s.buf, s.tree().expect("total"), &l.styles)
    });
    assert_eq!(tokens_a, tokens_b, "{doc}: the same semantic tokens");
    assert_eq!(
        qana_services::syntax_classes(tree, &fresh.styles),
        qana_services::syntax_classes(tree, &loaded.styles),
        "{doc}: the same syntax classes"
    );
    assert_eq!(
        qana_services::outline(tree, &fresh.outline),
        qana_services::outline(tree, &loaded.outline),
        "{doc}: the same outline"
    );
    assert_eq!(
        qana_services::folds(tree, &fresh.folds),
        qana_services::folds(tree, &loaded.folds),
        "{doc}: the same folds"
    );
    let [diags_a, diags_b] = [(fresh, &a), (loaded, &b)]
        .map(|(l, s)| qana_services::diagnostics(l.lexer, &s.buf, l.sg, &s.last_repairs));
    assert_eq!(diags_a, diags_b, "{doc}: the same diagnostics");
    for offset in sample(&starts, starts.len().div_ceil(48)) {
        let [items_a, items_b] = [(fresh, &a), (loaded, &b)]
            .map(|(l, s)| qana_services::completion_at(l.lexer, &s.buf, l.sg, l.tables, offset));
        assert_eq!(items_a, items_b, "{doc}: the same completions at {offset}");
    }

    // Binding and types, through the semantic database the editor's live
    // document builds.
    let [mut db_a, mut db_b] = langs.map(|l| {
        let mut db = SemDb::new(l.binding.clone());
        db.set_types(l.types.clone());
        db.set_macro_bodies(&l.macros);
        db.set_tree(doc, tree.clone());
        db
    });
    assert_eq!(db_a.symbols(doc), db_b.symbols(doc), "{doc}: the same symbols");
    assert_eq!(db_a.unresolved(doc), db_b.unresolved(doc), "{doc}: the same unresolved names");
    assert_eq!(
        db_a.qualified_errors(doc),
        db_b.qualified_errors(doc),
        "{doc}: the same path errors"
    );
    assert_eq!(db_a.not_exported(doc), db_b.not_exported(doc), "{doc}: the same exports");
    assert_eq!(
        format!("{:?}", db_a.types(doc)),
        format!("{:?}", db_b.types(doc)),
        "{doc}: the same types"
    );
    for &offset in &starts {
        assert_eq!(
            db_a.definition(doc, offset),
            db_b.definition(doc, offset),
            "{doc}: the same definition at {offset}"
        );
        assert_eq!(
            db_a.escapes(doc, offset),
            db_b.escapes(doc, offset),
            "{doc}: the same escape at {offset}"
        );
    }
    // Each of these reads the whole file.
    for offset in sample(&starts, starts.len().div_ceil(160)) {
        assert_eq!(
            db_a.references(doc, offset),
            db_b.references(doc, offset),
            "{doc}: the same references at {offset}"
        );
    }
    for offset in sample(&starts, starts.len().div_ceil(48)) {
        assert_eq!(
            db_a.names_in_scope(doc, offset),
            db_b.names_in_scope(doc, offset),
            "{doc}: the same names in scope at {offset}"
        );
    }
}

/// What the live document `l` says beyond its paint: its marks, folds and
/// hints, and the facts and the enclosing marks at each of `offsets`.
fn live_answers(l: &mut LiveDoc, offsets: &[u32]) -> String {
    let facts: Vec<_> = offsets.iter().map(|&offset| (offset, l.facts(offset))).collect();
    let enclosing: Vec<_> = offsets.iter().map(|&offset| l.enclosing(offset)).collect();
    format!(
        "marks {:?}\nfolds {:?}\nhints {:?}\nfacts {facts:?}\nenclosing {enclosing:?}",
        l.marks(),
        l.folds_shared(),
        l.hints()
    )
}

/// `fresh` and `loaded` serve `text` identically as live documents: open,
/// and through edits that break the document, mend it and change it.
fn same_live_document(fresh: &EmbeddedLang, loaded: &EmbeddedLang, doc: &str, text: &str) {
    let [mut a, mut b] = [fresh, loaded].map(|l| LiveDoc::open(Arc::new(l.clone()), doc, text));
    assert_eq!(a.legend(), b.legend(), "{doc}: the same legend");
    assert_eq!(a.mark_legend(), b.mark_legend(), "{doc}: the same mark legend");
    assert_eq!(a.fold_legend(), b.fold_legend(), "{doc}: the same fold legend");
    assert_eq!(Limner::open(&mut a, text), Limner::open(&mut b, text), "{doc}: the same paint");
    let starts = token_starts(fresh.session(text).tree().expect("total"));
    assert_eq!(live_answers(&mut a, &starts), live_answers(&mut b, &starts), "{doc}: open");

    let lines: Vec<&str> = text.lines().collect();
    let mid = (lines.len() / 2) as u32;
    let edits = [
        // A line no grammar parses, in the middle: recovery.
        LineEdit { start: mid, end: mid, lines: vec!["}} )( @@ {{ ;".to_string()] },
        // Gone again.
        LineEdit { start: mid, end: mid + 1, lines: vec![] },
        // A line doubled.
        LineEdit {
            start: mid,
            end: mid + 1,
            lines: vec![lines[mid as usize].to_string(), lines[mid as usize].to_string()],
        },
        // The first line gone.
        LineEdit { start: 0, end: 1, lines: vec![] },
    ];
    for (k, edit) in edits.iter().enumerate() {
        assert_eq!(a.edit(edit), b.edit(edit), "{doc}: the same paint after edit {k}");
        let text_now = a.text();
        assert_eq!(text_now, b.text(), "{doc}: the same text after edit {k}");
        let starts = token_starts(fresh.session(&text_now).tree().expect("total"));
        let starts = sample(&starts, 5);
        assert_eq!(
            live_answers(&mut a, &starts),
            live_answers(&mut b, &starts),
            "{doc}: after edit {k}"
        );
    }
}

/// `grammar` certified and loaded from its bytes serves each of `docs`
/// identically.
fn loads_as_certified(grammar: Grammar, docs: &[(&str, &str)]) {
    let loaded = loaded(grammar);
    let fresh = &certified(grammar).fresh;
    for &(doc, text) in docs {
        same_service(fresh, &loaded, doc, text);
        same_live_document(fresh, &loaded, doc, text);
    }
}

#[test]
fn rust_loads_as_certified() {
    loads_as_certified(
        Grammar::Rust,
        &[
            ("lexer.rs", include_str!("../../qana-grammar/src/lexer.rs")),
            ("live.rs", include_str!("../src/live.rs")),
        ],
    );
}

#[test]
fn c_loads_as_certified() {
    loads_as_certified(
        Grammar::C,
        &[
            ("demo.c", include_str!("../../../examples/c/demo.c")),
            ("demo.exp.c", include_str!("../../../examples/c/demo.exp.c")),
        ],
    );
}

#[test]
fn structlang_loads_as_certified() {
    loads_as_certified(
        Grammar::Structs,
        &[
            ("demo.sl", include_str!("../../../examples/structs/demo.sl")),
            ("demo.exp.sl", include_str!("../../../examples/structs/demo.exp.sl")),
        ],
    );
}

#[test]
fn qana_loads_as_certified() {
    loads_as_certified(
        Grammar::Qana,
        &[("qana.qana", QANA), ("rust.qana", RUST), ("c.qana", C), ("structlang.qana", STRUCTS)],
    );
}

/// Certifying is deterministic: a grammar certified again writes the
/// same bytes, so an artifact is a function of its grammar's text.
#[test]
fn certifying_again_writes_the_same_bytes() {
    for grammar in Grammar::ALL {
        let again = GrammarArtifact::certify(grammar.source()).unwrap_or_else(|d| panic!("{d:?}"));
        let again = postcard::to_allocvec(&again).expect("an artifact serializes");
        assert_eq!(
            first_difference(&certified(grammar).bytes, &again),
            None,
            "certifying {grammar:?} again writes the same bytes"
        );
    }
}

/// A refused grammar's diagnostics are data too: a worker that certifies
/// sends them back as bytes.
#[test]
fn a_refusal_round_trips() {
    // Each grammar, and what its refusal says.
    let refused = [
        // A parse repair in the grammar's own text.
        ("token A = \"a\"\nrule file = File: (A\n", "parse repair"),
        // A token that matches the empty string.
        ("token BAD = /a*/\nrule file = File:\n", "empty string"),
        // An unbounded mode stack.
        ("mode M {\ntoken IN = \"x\" @push(M)\n}\ntoken A = \"a\"\nrule file = File:\n", "L2"),
        // The dangling else.
        (
            "token IF = \"if\"\ntoken ELSE = \"else\"\ntoken X = \"x\"\n\
             rule file = File: stmt\n\
             rule stmt =\n  | S: \"if\" stmt\n  | SE: \"if\" stmt \"else\" stmt\n  | SX: \"x\"\n",
            "shift/reduce",
        ),
    ];
    for (src, says) in refused {
        let diags = GrammarArtifact::certify(src).expect_err("refused");
        assert!(diags.iter().any(|d| d.msg.contains(says)), "{src}: {diags:?}");
        let bytes = postcard::to_allocvec(&diags).expect("diagnostics serialize");
        let back: Vec<QanaDiag> = postcard::from_bytes(&bytes).expect("their bytes decode");
        assert_eq!(back, diags);
    }
}

/// An artifact never holds a conflict (certification refuses them), but
/// LR tables are data whatever they hold: tables with conflicts read back
/// with every conflict as it was.
#[test]
fn tables_with_conflicts_round_trip() {
    let src = "token IF = \"if\"\ntoken ELSE = \"else\"\ntoken X = \"x\"\n\
               rule file = File: stmt\n\
               rule stmt =\n  | S: \"if\" stmt\n  | SE: \"if\" stmt \"else\" stmt\n  | SX: \"x\"\n";
    let out = compile_source(&QanaToolchain::new(), src);
    assert!(out.diags.is_empty(), "compiles as values: {:?}", out.diags);
    let tables = qana_grammar::build_lr(&out.def.sg);
    assert!(!tables.conflicts.is_empty(), "the dangling else conflicts");
    let bytes = postcard::to_allocvec(&tables).expect("tables serialize");
    let back: LrTables = postcard::from_bytes(&bytes).expect("their bytes decode");
    assert_eq!(first_difference(&bytes, &postcard::to_allocvec(&back).unwrap()), None);
    assert_eq!(back.conflicts.len(), tables.conflicts.len());
    for (was, is) in tables.conflicts.iter().zip(&back.conflicts) {
        assert_eq!(
            (was.state, was.lookahead, was.kind, &was.items, &was.example, &was.prods),
            (is.state, is.lookahead, is.kind, &is.items, &is.example, &is.prods)
        );
    }
}

#[test]
fn an_artifacts_key_follows_its_grammars_text() {
    let rust = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/rust/rust.qana"
    ))
    .expect("rust.qana");
    assert_eq!(GrammarArtifact::key(&rust), GrammarArtifact::key(&rust.clone()), "the same text");
    let mut edited = rust.clone();
    edited.push(' ');
    assert_ne!(GrammarArtifact::key(&rust), GrammarArtifact::key(&edited), "one byte more");
    assert_ne!(GrammarArtifact::key(""), GrammarArtifact::key(" "));
}
