//! The C-clone exerciser gate: the committed C-subset grammar
//! certifies (this very grammar forced Pager merging into the LR
//! construction — canonical LR(1) never terminated on it), and the
//! committed demo parses losslessly with every reference resolving.

use rantlr_engine::IncSession;
use rantlr_rg::compile::certify;
use rantlr_rg::{compile_source, RgToolchain};
use rantlr_sem::SemDb;

const C_RG: &str = include_str!("../../../examples/c/c.rg");
const DEMO: &str = include_str!("../../../examples/c/demo.c");

#[test]
fn c_subset_certifies_and_serves_the_demo() {
    let tc = RgToolchain::new();
    let out = compile_source(&tc, C_RG);
    assert!(out.diags.is_empty(), "c.rg compiles: {:?}", out.diags);
    let (lexer, tables) = certify(&out.def).expect("C subset is in the envelope");
    assert!(
        tables.n_states < 1000,
        "Pager keeps C tractable (got {} states)",
        tables.n_states
    );
    assert!(!tables.lists.is_empty(), "C's lists are L4-balanced");

    let session = IncSession::new(&lexer, &out.def.sg, &tables, DEMO).unwrap();
    let tree = session.tree().expect("total");
    assert_eq!(tree.text(), DEMO, "lossless");
    assert!(session.last_repairs.is_empty(), "demo parses clean: {:?}", session.last_repairs);

    let mut db = SemDb::new(out.def.binding.clone());
    db.set_tree("demo.c", tree.clone());
    assert!(
        db.unresolved("demo.c").is_empty(),
        "every reference resolves: {:?}",
        db.unresolved("demo.c")
    );
    let syms = db.symbols("demo.c");
    let names: Vec<&str> = syms.defs.iter().map(|d| d.name.as_str()).collect();
    for expected in ["scale", "apply", "main", "point", "color", "LIMIT", "op"] {
        assert!(names.contains(&expected), "def {expected} found: {names:?}");
    }
}

use rantlr_engine::{Line, LineEdit};

/// The preprocessor as a LINE-BOUNDED mode: `#` enters, EOL leaves.
/// Directive modes never reach another line's entry state, `#define`
/// really DEFINES (code references resolve to it), and editing a
/// directive line damages exactly that line.
#[test]
fn directives_are_line_bounded_and_define() {
    let tc = RgToolchain::new();
    let out = compile_source(&tc, C_RG);
    let (lexer, tables) = certify(&out.def).unwrap();
    let doc = "#include <stdio.h>\n#define LIMIT 100\nint use_it(void) { return LIMIT; }\n";
    let mut session = IncSession::new(&lexer, &out.def.sg, &tables, doc).unwrap();
    assert!(session.last_repairs.is_empty(), "{:?}", session.last_repairs);

    // Entry states: every line starts at the BASE state — PP popped at
    // each EOL, so directives are invisible to their neighbors.
    for li in 0..session.buf.lines.len() {
        assert_eq!(
            session.buf.entry_state(li),
            Default::default(),
            "line {li} entry must be the base state"
        );
    }

    // The macro really defines: the code reference resolves to it.
    let mut db = SemDb::new(out.def.binding.clone());
    db.set_tree("d", session.tree().unwrap().clone());
    let use_at = doc.rfind("LIMIT").unwrap() as u32;
    let (_, dspan) = db.definition("d", use_at).expect("macro use resolves");
    assert_eq!(dspan.0 as usize, doc.find("LIMIT").unwrap(), "…to the #define");

    // Editing the directive damages exactly one line.
    let term = session.buf.lines[1].term;
    let outcome = session
        .edit(&out.def.sg, &tables, &[LineEdit {
            start: 1,
            end: 2,
            replacement: vec![Line::new("#define LIMIT 999", term)],
        }])
        .unwrap();
    assert_eq!(outcome.damage.relexed_lines, 1, "directive edits are line-local");
    assert!(session.last_repairs.is_empty());
}

/// The MStack residue regression: deleting a block comment leaves the
/// semantically identical base state below it, and reconvergence must
/// SEE that — one line of damage, not a full-file relex. (Residue
/// above `len` used to defeat the derived equality; pop now clears
/// its slot.)
#[test]
fn deleting_a_block_comment_relexes_one_line() {
    let tc = RgToolchain::new();
    let out = compile_source(&tc, C_RG);
    let (lexer, tables) = certify(&out.def).unwrap();
    let mut doc = String::from("int a;\n/* c */\n");
    for i in 0..200 {
        doc.push_str(&format!("int x{i};\n"));
    }
    let mut session = IncSession::new(&lexer, &out.def.sg, &tables, &doc).unwrap();
    let term = session.buf.lines[1].term;
    let outcome = session
        .edit(&out.def.sg, &tables, &[LineEdit {
            start: 1,
            end: 2,
            replacement: vec![Line::new("", term)],
        }])
        .unwrap();
    assert!(
        outcome.damage.relexed_lines <= 2,
        "comment deletion must reconverge immediately, got {} lines",
        outcome.damage.relexed_lines
    );
}

/// The typedef campaign: heads resolve WITHOUT lexer feedback, split
/// by context — full declarators at file scope / params / fields, the
/// two-identifier signal at block level. The residue is PINNED: a
/// block-local `T *p;` still parses as (broken) multiplication.
#[test]
fn typedef_heads_resolve_without_lexer_feedback() {
    let tc = RgToolchain::new();
    let out = compile_source(&tc, C_RG);
    let (lexer, tables) = certify(&out.def).unwrap();
    let doc = "\
typedef int word;\n\
word global = 1;\n\
word *gp;\n\
word take(word w, word *p, word) { return w; }\n\
struct s { word field; word *fptr; };\n\
int main(void) {\n\
    word local = take(global, gp, 2);\n\
    word arr[3];\n\
    return local + arr[0];\n\
}\n";
    let session = IncSession::new(&lexer, &out.def.sg, &tables, doc).unwrap();
    assert!(session.last_repairs.is_empty(), "typedef world parses clean: {:?}", session.last_repairs);
    let mut db = SemDb::new(out.def.binding.clone());
    db.set_tree("d", session.tree().unwrap().clone());
    assert!(db.unresolved("d").is_empty(), "{:?}", db.unresolved("d"));

    // Every `word` head navigates to the typedef's declarator.
    let def_at = doc.find("word").unwrap() + "typedef int ".len() - "typedef int ".len();
    let _ = def_at;
    let typedef_site = doc.find("word").unwrap() as u32;
    for (i, _) in doc.match_indices("word").skip(1) {
        let (duri, dspan) = db
            .definition("d", i as u32)
            .unwrap_or_else(|| panic!("head at {i} resolves"));
        assert_eq!(duri, "d");
        assert_eq!(dspan.0, typedef_site, "head at {i} lands on the typedef");
    }

    // An unknown type head is a "cannot find" diagnostic — the typo'd
    // typedef case.
    let bad = IncSession::new(&lexer, &out.def.sg, &tables, "wrod x;\n").unwrap();
    let mut db2 = SemDb::new(out.def.binding.clone());
    db2.set_tree("b", bad.tree().unwrap().clone());
    let unres: Vec<String> = db2.unresolved("b").into_iter().map(|(n, _)| n).collect();
    assert_eq!(unres, ["wrod"], "typo'd type is diagnosed by name");

    // PINNED residue: block-level `T *p;` parses CLEANLY — as
    // multiplication (that is the trap: no parse error, wrong
    // reading). The honest signature: zero repairs, `word` resolving
    // to the typedef as a VALUE read, and `p` unresolved. Awaits the
    // expression-tier split; if this stops holding, update the walls.
    let residue = IncSession::new(&lexer, &out.def.sg, &tables, "typedef int word;\nint f(void) { word *p; return 0; }\n").unwrap();
    assert!(residue.last_repairs.is_empty(), "the trap parses clean: {:?}", residue.last_repairs);
    let mut db3 = SemDb::new(out.def.binding.clone());
    db3.set_tree("r", residue.tree().unwrap().clone());
    let unres: Vec<String> = db3.unresolved("r").into_iter().map(|(n, _)| n).collect();
    assert_eq!(unres, ["p"], "the misreading surfaces semantically, not syntactically");
}
