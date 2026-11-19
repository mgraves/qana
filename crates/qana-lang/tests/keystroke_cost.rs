//! What a keystroke is allowed to cost, counted and not timed.
//!
//! A keystroke changes one item of a file. The semantic layer and the
//! painter are built so that it re-derives that item and nothing else:
//! one fragment, one resolution, one item's marks. Three things used to
//! break that, each O(file) on every keystroke and each invisible in a
//! small file:
//!
//! * the set of every top-level name in the file was built again to
//!   resolve the one item that changed;
//! * the position index over every top-level name was built again to
//!   repaint an item that refers to one;
//! * the painter put every item's cached marks through a hash map.
//!
//! On a 250,000-line C file that was 9 to 16 ms a keystroke. These tests
//! hold the three out by counting the work, so they do not depend on a
//! clock or on the machine: `SemStats` counts the builds, and the painter
//! reports how many items it had to look up.
//!
//! The pipeline here is `LiveDoc::edit`'s, stage for stage, with the
//! parts in the open so that the counters can be read.

use qana_engine::{IncSession, Line, LineEdit, LineTerm};
use qana_lang::EmbeddedLang;
use qana_sem::{SemDb, SemStats, PIECE_BYTES};
use qana_services::paint::Painter;
use linework::PaintDelta;

const C_QANA: &str = include_str!("../../../examples/c/c.qana");
const RUST_QANA: &str = include_str!("../../../examples/rust/rust.qana");
const URI: &str = "cost.c";

/// A C file of `functions` functions. Each calls the one before it and
/// uses a macro and a struct defined at the top, so every body refers to
/// top-level names: the case that used to cost the most.
fn document(functions: usize) -> String {
    let mut s = String::new();
    s.push_str("#define LIMIT 100\n\n");
    s.push_str("struct pair {\n    int left;\n    int right;\n};\n\n");
    s.push_str("static int fn_0(int a, int b) {\n    return a + b;\n}\n\n");
    for i in 1..functions {
        s.push_str(&format!(
            "static int fn_{i}(int a, int b) {{\n    struct pair p;\n    int acc = fn_{prev}(a, b) + LIMIT;\n    return acc * 2;\n}}\n\n",
            prev = i - 1
        ));
    }
    s
}

struct Doc {
    lang: EmbeddedLang,
    session: IncSession<'static>,
    db: SemDb,
    painter: Painter,
}

/// The C grammar, compiled and certified. Once per test: it is the
/// slowest thing here by far.
fn c_lang() -> EmbeddedLang {
    EmbeddedLang::from_qana_source(C_QANA).expect("c.qana certifies")
}

impl Doc {
    fn open(lang: &EmbeddedLang, text: &str) -> Doc {
        let lang = lang.clone();
        let session = lang.session(text);
        let mut db = SemDb::new(lang.binding.clone());
        db.set_types(lang.types.clone());
        db.set_macro_bodies(&lang.macros);
        db.set_tree(URI, session.tree().expect("total").clone());
        let (painter, _) = Painter::new(&session, &lang.styles, Some((&mut db, URI)));
        Doc { lang, session, db, painter }
    }

    /// Replace line `line` with `text`, as `LiveDoc::edit` does.
    fn retype(&mut self, line: usize, text: &str) -> PaintDelta {
        let outcome = self
            .session
            .edit(self.lang.sg, self.lang.tables, &[LineEdit {
                start: line,
                end: line + 1,
                replacement: vec![Line::new(text, LineTerm::Lf)],
            }])
            .expect("total under errors");
        self.db.set_tree(URI, self.session.tree().expect("total").clone());
        self.painter.update(&self.session, &outcome.damage, &self.lang.styles, Some((&mut self.db, URI)))
    }

    fn line_of(&self, needle: &str) -> usize {
        self.session
            .buf
            .lines
            .iter()
            .position(|l| l.text.contains(needle))
            .unwrap_or_else(|| panic!("no line holds {needle:?}"))
    }
}

fn since(before: SemStats, after: SemStats) -> SemStats {
    SemStats {
        fragments_computed: after.fragments_computed - before.fragments_computed,
        item_resolves_computed: after.item_resolves_computed - before.item_resolves_computed,
        type_item_walks: after.type_item_walks - before.type_item_walks,
        type_passes: after.type_passes - before.type_passes,
        name_set_builds: after.name_set_builds - before.name_set_builds,
        top_index_builds: after.top_index_builds - before.top_index_builds,
        top_scans: after.top_scans - before.top_scans,
        bytes_walked: after.bytes_walked - before.bytes_walked,
        fragments_kept: after.fragments_kept - before.fragments_kept,
    }
}

const FUNCTIONS: usize = 600;

/// Typing in a function body that calls other functions: one fragment,
/// one resolution, one item through the painter, and no pass that builds
/// something from every name in the file.
#[test]
fn a_keystroke_in_a_body_derives_one_item_and_builds_nothing_from_the_file() {
    let mut doc = Doc::open(&c_lang(), &document(FUNCTIONS));

    // Opening is the whole file, and is where the whole-file work
    // belongs. It also shows that the counters below count something.
    let opened = doc.db.stats;
    assert!(opened.fragments_computed as usize > FUNCTIONS, "every item was walked at open");
    assert!(opened.name_set_builds >= 1, "open resolves the file from the set of its names");

    let line = doc.line_of("int acc = fn_299(a, b) + LIMIT;");
    let mut typed = String::from("    int acc = fn_299(a, b) + LIMIT");
    for keystroke in 0..10 {
        typed.push_str(" + 1");
        let before = doc.db.stats;
        let delta = doc.retype(line, &format!("{typed};"));
        let cost = since(before, doc.db.stats);

        assert_eq!(cost.fragments_computed, 1, "keystroke {keystroke}: one item was walked");
        assert_eq!(cost.item_resolves_computed, 1, "keystroke {keystroke}: one item was resolved");
        assert_eq!(
            cost.name_set_builds, 0,
            "keystroke {keystroke}: the set of every top-level name was built again"
        );
        assert_eq!(
            cost.top_index_builds, 0,
            "keystroke {keystroke}: the position index over every top-level name was built again"
        );
        assert!(
            (1..=2).contains(&cost.top_scans),
            "keystroke {keystroke}: the item's names are asked in a pass or two over the \
             file's signatures (one to resolve it, one to paint it), got {}",
            cost.top_scans
        );
        assert_eq!(
            doc.painter.window_items(),
            1,
            "keystroke {keystroke}: the painter looked up one item"
        );
        let (lo, hi) = delta.splice.as_ref().map(|((lo, hi), _)| (*lo, *hi)).expect("a splice");
        assert_eq!((lo as usize, hi as usize), (line, line + 1), "the damage is the line typed in");
        assert!(delta.repaints.is_empty(), "nothing else repaints: {:?}", delta.repaints);
    }
}

/// The names a body refers to are still found: the scan answers what the
/// set answered. A call to a function defined EARLIER resolves; one
/// defined LATER does not (C's root scope is ordered); a struct tag
/// resolves from anywhere (its namespace is hoisted).
#[test]
fn a_keystroke_resolves_its_names_as_an_open_does() {
    let lang = c_lang();
    let mut doc = Doc::open(&lang, &document(FUNCTIONS));
    let line = doc.line_of("int acc = fn_299(a, b) + LIMIT;");
    let cases = [
        // (the line typed, names that must be unresolved after it)
        ("    int acc = fn_10(a, b) + LIMIT;", vec![]),
        ("    int acc = fn_599(a, b) + LIMIT;", vec!["fn_599"]),
        ("    int acc = fn_300(a, b) + LIMIT;", vec![]), // itself: defined by its own item
        ("    int acc = nowhere(a, b) + LIMIT;", vec!["nowhere"]),
        ("    int acc = fn_299(a, b) + NOT_A_MACRO;", vec!["NOT_A_MACRO"]),
        ("    int acc = fn_299(a, b) + LIMIT;", vec![]),
    ];
    for (typed, expect_unresolved) in cases {
        let before = doc.db.stats;
        doc.retype(line, typed);
        let cost = since(before, doc.db.stats);
        assert_eq!(cost.name_set_builds, 0, "{typed}: resolved without the whole-file set");

        let unresolved: Vec<String> = doc.db.unresolved(URI).into_iter().map(|(n, _)| n).collect();
        assert_eq!(unresolved, expect_unresolved, "after typing `{typed}`");

        // And a fresh open of the same text, which resolves the whole
        // file from the set, agrees on every name.
        let now = doc.session.buf.reproduce();
        let mut fresh = Doc::open(&lang, &now);
        assert!(fresh.db.stats.name_set_builds >= 1, "the fresh open used the set");
        assert_eq!(doc.db.unresolved(URI), fresh.db.unresolved(URI), "after typing `{typed}`");
        assert_eq!(doc.db.resolve(URI), fresh.db.resolve(URI), "after typing `{typed}`");
        assert_eq!(doc.painter.frame().lines, fresh.painter.frame().lines, "after typing `{typed}`");
    }
}

/// An edit that changes what later items can see is whole-file work, and
/// takes the whole-file path: renaming a function near the top makes
/// every item after it resolve again, once, from one set.
#[test]
fn renaming_an_early_definition_resolves_the_rest_from_one_set() {
    let lang = c_lang();
    let mut doc = Doc::open(&lang, &document(FUNCTIONS));
    let line = doc.line_of("static int fn_0(int a, int b) {");

    let before = doc.db.stats;
    let delta = doc.retype(line, "static int renamed(int a, int b) {");
    let cost = since(before, doc.db.stats);

    assert_eq!(cost.fragments_computed, 1, "one item was walked");
    assert!(
        cost.item_resolves_computed as usize >= FUNCTIONS - 1,
        "every later item saw a new environment, {} resolved",
        cost.item_resolves_computed
    );
    assert_eq!(cost.name_set_builds, 1, "and they were resolved from one set, built once");
    assert_eq!(
        doc.db.unresolved(URI).into_iter().map(|(n, _)| n).collect::<Vec<_>>(),
        vec!["fn_0"],
        "fn_1 called fn_0, which no longer exists"
    );
    assert!(
        !delta.repaints.is_empty(),
        "the call in fn_1 repaints as unresolved: a line outside the damage"
    );

    let now = doc.session.buf.reproduce();
    let mut fresh = Doc::open(&lang, &now);
    assert_eq!(doc.db.resolve(URI), fresh.db.resolve(URI));
    assert_eq!(doc.painter.frame().lines, fresh.painter.frame().lines);
}

/// A function of thousands of lines.
fn one_large_function(statements: usize) -> String {
    let mut s = String::from("#define LIMIT 100\n\nstatic int big(int a, int b) {\n");
    for i in 0..statements {
        s.push_str(&format!("    int v{i} = a + b + LIMIT;\n"));
    }
    s.push_str("    return a;\n}\n\nstatic int after(void) { return 1; }\n");
    s
}

/// A function of thousands of lines is one item. Typing in it walks the
/// piece of it typed in, and puts the rest of its fragment together from
/// the parts the last revision read: the function is not walked again.
#[test]
fn a_keystroke_in_a_large_function_walks_a_piece_or_two() {
    let text = one_large_function(3000);
    let mut doc = Doc::open(&c_lang(), &text);
    let opened = doc.db.stats;
    assert!(
        opened.bytes_walked as usize > text.len() * 9 / 10,
        "open walks the file, {} bytes of {}",
        opened.bytes_walked,
        text.len()
    );

    let line = doc.line_of("int v1500 = ");
    let mut typed = String::from("    int v1500 = a + b + LIMIT");
    for keystroke in 0..10 {
        typed.push_str(" + 1");
        let before = doc.db.stats;
        let delta = doc.retype(line, &format!("{typed};"));
        let cost = since(before, doc.db.stats);
        assert_eq!(cost.fragments_computed, 1, "keystroke {keystroke}: one item was built");
        assert!(
            cost.bytes_walked <= 2 * PIECE_BYTES as u64,
            "keystroke {keystroke}: a piece or two walked, {} bytes of a {}-byte function",
            cost.bytes_walked,
            text.len()
        );
        assert!(delta.repaints.is_empty(), "keystroke {keystroke}: nothing else repaints");
    }

    // And what was put together is what the open of the same text says.
    let now = doc.session.buf.reproduce();
    let mut fresh = Doc::open(&c_lang(), &now);
    assert_eq!(*doc.db.symbols(URI), *fresh.db.symbols(URI));
    assert_eq!(doc.db.resolve(URI), fresh.db.resolve(URI));
    assert_eq!(doc.painter.frame().lines, fresh.painter.frame().lines);
}

/// A keystroke that leaves an item declaring and referring to exactly
/// what it did, where it did (a number changed for another of the same
/// width), leaves the item's fragment as it was: same identity, same
/// resolution. Nothing is resolved, the painter has no item to look up,
/// and only the line typed on is painted.
#[test]
fn a_keystroke_that_changes_no_name_keeps_the_items_fragment() {
    let mut doc = Doc::open(&c_lang(), &one_large_function(3000));
    let line = doc.line_of("int v1500 = ");
    for (keystroke, digit) in ['7', '8', '9'].into_iter().enumerate() {
        let before = doc.db.stats;
        let delta = doc.retype(line, &format!("    int v1500 = a + b + LIMIT + {digit};"));
        let cost = since(before, doc.db.stats);
        if keystroke == 0 {
            // The first one adds a token: the item changes.
            assert_eq!(cost.fragments_kept, 0);
            continue;
        }
        assert_eq!(cost.fragments_computed, 1, "keystroke {keystroke}: the item was built again");
        assert_eq!(cost.fragments_kept, 1, "keystroke {keystroke}: and was the fragment it had");
        assert_eq!(cost.item_resolves_computed, 0, "keystroke {keystroke}: its resolution was kept");
        assert_eq!(doc.painter.window_items(), 0, "keystroke {keystroke}: no item to look up");
        let (lo, hi) = delta.splice.as_ref().map(|((lo, hi), _)| (*lo, *hi)).expect("a splice");
        assert_eq!((lo as usize, hi as usize), (line, line + 1), "the damage is the line typed in");
        assert!(delta.repaints.is_empty(), "keystroke {keystroke}: nothing else repaints");
    }
}

/// A module of `functions` functions: one item of the file.
fn one_large_module(functions: usize) -> String {
    let mut text = String::from("pub mod m {\n");
    for i in 0..functions {
        text.push_str(&format!("    pub fn f{i}(a: u8) -> u8 {{\n        a + {i}\n    }}\n\n"));
    }
    text.push_str("}\n");
    text
}

/// A grammar that declares no binding — Rust's, its binding taken away.
/// Its items declare nothing, and nothing is walked to find that out:
/// not at open, not on a keystroke inside a module that is one item of
/// eight thousand lines. Each keystroke's fragment is the one the item
/// had, so nothing is resolved and the painter has nothing to look up.
#[test]
fn a_grammar_without_binding_walks_nothing() {
    let mut lang = EmbeddedLang::from_qana_source(RUST_QANA).expect("rust.qana certifies");
    lang.binding = qana_sem::BindingConfig::default();
    let text = one_large_module(2000);
    let mut doc = Doc::open(&lang, &text);
    assert_eq!(doc.db.stats.bytes_walked, 0, "open walks nothing");

    let line = doc.line_of("a + 1000");
    let mut typed = String::from("        a + 1000");
    for keystroke in 0..10 {
        typed.push_str(" + a");
        let before = doc.db.stats;
        let delta = doc.retype(line, &typed);
        let cost = since(before, doc.db.stats);
        assert_eq!(cost.bytes_walked, 0, "keystroke {keystroke}: nothing walked");
        assert!(cost.fragments_computed >= 1, "keystroke {keystroke}: the module was built again");
        assert_eq!(cost.fragments_kept, cost.fragments_computed, "keystroke {keystroke}: and kept");
        assert_eq!(cost.item_resolves_computed, 0, "keystroke {keystroke}: nothing resolved");
        assert_eq!(doc.painter.window_items(), 0, "keystroke {keystroke}: no item to look up");
        assert!(delta.repaints.is_empty(), "keystroke {keystroke}: nothing else repaints");
    }
}

/// Rust's binding: a module of eight thousand lines is one item, and a
/// keystroke in one of its functions walks the piece typed in, putting
/// the rest of the module's fragment together from the parts the last
/// revision read.
#[test]
fn a_keystroke_in_a_large_rust_module_walks_a_piece_or_two() {
    let lang = EmbeddedLang::from_qana_source(RUST_QANA).expect("rust.qana certifies");
    let text = one_large_module(2000);
    let mut doc = Doc::open(&lang, &text);
    let opened = doc.db.stats;
    assert!(
        opened.bytes_walked as usize > text.len() * 9 / 10,
        "open walks the file, {} bytes of {}",
        opened.bytes_walked,
        text.len()
    );

    let line = doc.line_of("a + 1000");
    let mut typed = String::from("        a + 1000");
    for keystroke in 0..10 {
        typed.push_str(" + a");
        let before = doc.db.stats;
        let delta = doc.retype(line, &typed);
        let cost = since(before, doc.db.stats);
        assert_eq!(cost.fragments_computed, 1, "keystroke {keystroke}: one item was built");
        assert!(
            cost.bytes_walked <= 2 * PIECE_BYTES as u64,
            "keystroke {keystroke}: a piece or two walked, {} bytes of a {}-byte module",
            cost.bytes_walked,
            text.len()
        );
        assert!(delta.repaints.is_empty(), "keystroke {keystroke}: nothing else repaints");
    }

    // What was put together is what the open of the same text says.
    let now = doc.session.buf.reproduce();
    let mut fresh = Doc::open(&lang, &now);
    assert_eq!(*doc.db.symbols(URI), *fresh.db.symbols(URI));
    assert_eq!(doc.db.resolve(URI), fresh.db.resolve(URI));
    assert_eq!(doc.painter.frame().lines, fresh.painter.frame().lines);
}

/// The classes the syntax gives are kept by subtree: a keystroke in one
/// function reads again the piece it is in and the constructs around it,
/// not the file.
#[test]
fn a_keystroke_reads_the_syntax_of_the_piece_it_is_in() {
    let mut doc = Doc::open(&c_lang(), &document(FUNCTIONS));
    let size = doc.session.buf.reproduce().len();
    assert_eq!(doc.painter.syntax_read(), size, "opening reads the whole file");

    let line = doc.line_of("int acc = fn_299(a, b) + LIMIT;");
    let mut typed = String::from("    int acc = fn_299(a, b) + LIMIT");
    for keystroke in 0..10 {
        typed.push_str(" + 1");
        doc.retype(line, &format!("{typed};"));
        let read = doc.painter.syntax_read();
        assert!(
            read <= 4 * qana_services::syntax::PIECE_BYTES as usize,
            "keystroke {keystroke}: the syntax of {read} of {size} bytes was read again"
        );
    }
}
