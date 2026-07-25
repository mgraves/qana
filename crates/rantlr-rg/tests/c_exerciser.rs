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
/// by context — full declarators at file scope / params / fields, and
/// (since wall 3) pointered declarators at block level too, because
/// the statement-expression tier surrendered the `IDENT *` prefix.
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

    // The wall-2 residue, DEMOLISHED by wall 3: block-level `T *p;`
    // is now a DECLARATION — zero repairs, the head resolves to the
    // typedef, and `p` is a definition (not an unresolved read).
    let residue = IncSession::new(&lexer, &out.def.sg, &tables, "typedef int word;\nint f(void) { word *p; return 0; }\n").unwrap();
    assert!(residue.last_repairs.is_empty(), "T *p; parses clean: {:?}", residue.last_repairs);
    let mut db3 = SemDb::new(out.def.binding.clone());
    db3.set_tree("r", residue.tree().unwrap().clone());
    assert!(db3.unresolved("r").is_empty(), "{:?}", db3.unresolved("r"));
    let names: Vec<String> = db3.symbols("r").defs.iter().map(|d| d.name.clone()).collect();
    assert!(names.iter().any(|n| n == "p"), "p is a definition now: {names:?}");
}

/// C wall 3, the expression-tier split. Statement expressions give up
/// exactly one derivation — a bare identifier as the left operand of
/// `*` — and four doors open at once: pointered typedef locals,
/// keyword-led casts, sizeof(type), and the comma operator. The costs
/// are pinned as precisely as the wins, and the R/R core the
/// convergent spellings dodge is proved by the certifier itself.
#[test]
fn expression_tier_split_opens_four_doors() {
    let tc = RgToolchain::new();
    let out = compile_source(&tc, C_RG);
    let (lexer, tables) = certify(&out.def).expect("the split certifies");

    let parses_clean = |doc: &str| -> bool {
        let s = IncSession::new(&lexer, &out.def.sg, &tables, doc).unwrap();
        s.last_repairs.is_empty()
    };

    // The doors, one representative each — all clean, all resolving.
    let doc = "\
typedef unsigned long word;\n\
int f(void) {\n\
    word *p, **pp, salt = 1;\n\
    unsigned long u = (unsigned long)salt;\n\
    word w = (word)(salt);\n\
    unsigned n = sizeof(unsigned long *) + sizeof(word);\n\
    int i, j;\n\
    for (i = 0, j = 0; i < 3; ++i, ++j)\n\
        u = u + 1, salt = salt + u;\n\
    return (int)(u + w + n + *p + **pp);\n\
}\n";
    let s = IncSession::new(&lexer, &out.def.sg, &tables, doc).unwrap();
    assert!(s.last_repairs.is_empty(), "all four doors parse: {:?}", s.last_repairs);
    let mut db = SemDb::new(out.def.binding.clone());
    db.set_tree("w3", s.tree().unwrap().clone());
    assert!(db.unresolved("w3").is_empty(), "{:?}", db.unresolved("w3"));

    // The flip's cost, pinned: a multiplication STATEMENT led by a
    // bare name now reads as a declaration, so `x * y;` quietly
    // declares y (C's own reading when x names a type), and
    // `x * y + z;` is REFUSED outright.
    assert!(
        parses_clean("int f(int x) { x * y; return 0; }\n"),
        "bare-name star statement parses — as a declaration"
    );
    assert!(
        !parses_clean("int f(int x, int y, int z) { x * y + z; return 0; }\n"),
        "a bare-name multiplication statement is refused"
    );
    // Deeper left spines keep their multiplications: only the bare
    // name surrendered.
    assert!(parses_clean("int f(int x, int y) { (x) * y + 1; return 0; }\n"));
    assert!(parses_clean("int g(void); int f(int y) { g() * y + 1; return 0; }\n"));

    // Bare-typedef casts: juxtaposition refused, call shape converges.
    assert!(!parses_clean("typedef int word;\nint f(int x) { word w = (word) x; return w; }\n"));
    assert!(parses_clean("typedef int word;\nint f(int x) { word w = (word)(x); return w; }\n"));

    // Call arguments never grew a comma operator: still two arguments.
    let two_args = "int add(int a, int b); int f(void) { return add(1, 2); }\n";
    assert!(parses_clean(two_args));

    // THE PROOF BY REFUSAL: ask for the bare-name form directly —
    // `sizeof "(" IDENT ptrs+ ")"` — and the certifier refuses with a
    // shift/reduce counterexample on `*`: the token where sizeof's
    // parenthesized VALUE (multiplication) and the would-be TYPE
    // (pointer) diverge. This is the wall, stated by the machine.
    let probed = C_RG.replace(
        "  | SizeofE:   \"sizeof\" expr\n",
        "  | SizeofE:   \"sizeof\" expr\n  | SizeofTd:  \"sizeof\" \"(\" IDENT td_ptrs \")\" @precedence(\"sizeof\")\n",
    ) + "\nrule td_ptrs = ptr+\n";
    assert_ne!(probed, C_RG, "probe injection took");
    let probe_out = compile_source(&tc, &probed);
    assert!(probe_out.diags.is_empty(), "probe grammar compiles: {:?}", probe_out.diags);
    let err = certify(&probe_out.def).expect_err("the bare-name form must be refused");
    let msg = err.iter().map(|d| d.msg.as_str()).collect::<Vec<_>>().join("\n");
    assert!(
        msg.contains("shift/reduce") && msg.contains("on STAR"),
        "refused with the diverging token named: {msg}"
    );
    assert!(msg.contains("example input"), "and a counterexample trace: {msg}");
}

/// C wall 4: per-NAMESPACE ordering. Tags (`@ns(tag)`) are hoisted —
/// forward-declarable in every scope — while values keep
/// define-before-use. One annotation declares both the partition and
/// the ordering; the engine derives the rest.
#[test]
fn struct_tags_are_forward_declarable() {
    let tc = RgToolchain::new();
    let out = compile_source(&tc, C_RG);
    let (lexer, tables) = certify(&out.def).unwrap();

    // The typedef REFERENCES the tag before its definition — and a
    // value named like the tag coexists without collision.
    let doc = "\
typedef struct node node_t;\n\
struct node { node_t *next; int val; };\n\
int node = 4;\n\
int len(node_t *n) { return node + n->val; }\n";
    let session = IncSession::new(&lexer, &out.def.sg, &tables, doc).unwrap();
    assert!(session.last_repairs.is_empty(), "{:?}", session.last_repairs);
    let mut db = SemDb::new(out.def.binding.clone());
    db.set_tree("d", session.tree().unwrap().clone());
    assert!(db.unresolved("d").is_empty(), "forward tag resolves: {:?}", db.unresolved("d"));

    // The FORWARD tag ref navigates to the later definition site.
    let fwd_ref = doc.find("struct node").unwrap() + "struct ".len();
    let (duri, dspan) = db.definition("d", fwd_ref as u32).expect("tag ref resolves");
    assert_eq!(duri, "d");
    let def_site = doc.find("struct node {").unwrap() + "struct ".len();
    assert_eq!(dspan.0 as usize, def_site, "…to the struct DEFINITION");

    // Namespace partition: the VALUE `node` at its use site navigates
    // to the int variable, never to the tag.
    let val_use = doc.rfind("node +").unwrap();
    let (_, vspan) = db.definition("d", val_use as u32).expect("value resolves");
    assert_eq!(vspan.0 as usize, doc.find("int node").unwrap() + "int ".len());

    // References stay inside their namespace: the tag's references are
    // exactly the two `struct node` sites, not the int or its use.
    let (refs, _) = db.references("d", def_site as u32).expect("tag references");
    assert_eq!(refs.len(), 1, "one tag REFERENCE (the forward one): {refs:?}");
    assert_eq!(refs[0].1 .0 as usize, fwd_ref);

    // Values did NOT become forward: use-before-def still diagnosed.
    let ordered = "int f(void) { return later; }\nint later = 1;\n";
    let s2 = IncSession::new(&lexer, &out.def.sg, &tables, ordered).unwrap();
    let mut db2 = SemDb::new(out.def.binding.clone());
    db2.set_tree("o", s2.tree().unwrap().clone());
    let unres: Vec<String> = db2.unresolved("o").into_iter().map(|(n, _)| n).collect();
    assert_eq!(unres, ["later"], "value ordering is untouched by the tag namespace");

    // And an unknown tag is still a diagnosis, in its own namespace.
    let bad = "typedef struct node node_t;\n";
    let s3 = IncSession::new(&lexer, &out.def.sg, &tables, bad).unwrap();
    let mut db3 = SemDb::new(out.def.binding.clone());
    db3.set_tree("b", s3.tree().unwrap().clone());
    let unres: Vec<String> = db3.unresolved("b").into_iter().map(|(n, _)| n).collect();
    assert_eq!(unres, ["node"], "missing tag diagnosed");
}

/// C wall 5: labels and goto. Two prior walls made this one cheap:
/// the statement tier keeps `:` out of the bare name's lookahead, so
/// `name: stmt` shifts uncontested, and the hoisted label namespace
/// (`@ns(label)`) makes FORWARD gotos resolve. The residue is scoping:
/// labels are block-scoped here, not C's function-scoped, so a goto
/// cannot jump INTO a nested block — pinned below.
#[test]
fn labels_and_goto_navigate() {
    let tc = RgToolchain::new();
    let out = compile_source(&tc, C_RG);
    let (lexer, tables) = certify(&out.def).unwrap();

    // Forward and backward gotos, a value sharing the label's name,
    // and the ternary keeping its `:` — all in one function.
    let doc = "\
int f(int retry) {\n\
    int acc = 0;\n\
retry:\n\
    acc = retry > 0 ? acc + retry : acc;\n\
    retry -= 1;\n\
    if (retry > 0)\n\
        goto retry;\n\
    if (acc > 9)\n\
        goto done;\n\
    acc = 0;\n\
done:\n\
    return acc;\n\
}\n";
    let session = IncSession::new(&lexer, &out.def.sg, &tables, doc).unwrap();
    assert!(session.last_repairs.is_empty(), "labels parse clean: {:?}", session.last_repairs);
    let mut db = SemDb::new(out.def.binding.clone());
    db.set_tree("d", session.tree().unwrap().clone());
    assert!(db.unresolved("d").is_empty(), "{:?}", db.unresolved("d"));

    // `goto retry;` navigates to the LABEL, not the int parameter —
    // the namespaces partition.
    let goto_at = doc.find("goto retry").unwrap() + "goto ".len();
    let (_, dspan) = db.definition("d", goto_at as u32).expect("goto resolves");
    assert_eq!(dspan.0 as usize, doc.find("retry:").unwrap(), "…to the label");

    // The FORWARD goto resolves too (hoisted namespace), landing on
    // the later label.
    let fwd_at = doc.find("goto done").unwrap() + "goto ".len();
    let (_, fspan) = db.definition("d", fwd_at as u32).expect("forward goto resolves");
    assert_eq!(fspan.0 as usize, doc.find("done:").unwrap());

    // And the VALUE `retry` still navigates to the parameter.
    let val_at = doc.find("retry > 0").unwrap();
    let (_, vspan) = db.definition("d", val_at as u32).expect("value resolves");
    assert_eq!(vspan.0 as usize, doc.find("int retry").unwrap() + "int ".len());

    // PINNED residue: labels are block-scoped, so a goto cannot jump
    // INTO a nested block. Parses clean; resolves as "cannot find".
    let into = "int g(int x) { if (x) { inner: x += 1; } goto inner; return x; }\n";
    let s2 = IncSession::new(&lexer, &out.def.sg, &tables, into).unwrap();
    assert!(s2.last_repairs.is_empty(), "{:?}", s2.last_repairs);
    let mut db2 = SemDb::new(out.def.binding.clone());
    db2.set_tree("g", s2.tree().unwrap().clone());
    let unres: Vec<String> = db2.unresolved("g").into_iter().map(|(n, _)| n).collect();
    assert_eq!(unres, ["inner"], "goto INTO a block is the pinned residue");
}

/// C wall 6: function-like macro adjacency. `#define F(x)` versus
/// `#define F (x)` differ by ONE SPACE that lives in trivia — invisible
/// to any grammar. The envelope's answer: adjacency is a LEXER fact.
/// A composite PP-mode token `/name\(/` wins maximal munch exactly
/// when the paren is adjacent, so the two spellings lex differently
/// and parse as different productions — line-local, no flags, no
/// feedback. The residue is pinned: the fn-like macro's NAME lives
/// inside the composite token, so it does not define yet (that is the
/// meta tier's job, which owns macro objects wholesale).
#[test]
fn macro_adjacency_is_a_lexer_fact() {
    let tc = RgToolchain::new();
    let out = compile_source(&tc, C_RG);
    let (lexer, tables) = certify(&out.def).unwrap();

    // One space, two structures. Both parse clean; only the
    // OBJECT-like spelling defines its name.
    let fn_like = "#define SQ(x) ((x) * (x))\n";
    let obj_like = "#define SQ (x) ((x) * (x))\n";
    for (doc, defines) in [(fn_like, false), (obj_like, true)] {
        let s = IncSession::new(&lexer, &out.def.sg, &tables, doc).unwrap();
        assert!(s.last_repairs.is_empty(), "parses clean: {doc:?} {:?}", s.last_repairs);
        let mut db = SemDb::new(out.def.binding.clone());
        db.set_tree("m", s.tree().unwrap().clone());
        let has_sq = db.symbols("m").defs.iter().any(|d| d.name == "SQ");
        assert_eq!(has_sq, defines, "adjacency decides whether SQ is a definition: {doc:?}");
    }

    // Parameters are structure: a two-param macro with a body full of
    // parens and commas parses clean, and `#if defined(X)` keeps
    // working with the composite token in directive bodies.
    let world = "\
#define MAX(a, b) ((a) < (b) ? (b) : (a))\n\
#define EMPTY() 0\n\
#if defined(LIMIT)\n\
#endif\n\
#define LIMIT 9\n\
int use_it(void) { return LIMIT; }\n";
    let s = IncSession::new(&lexer, &out.def.sg, &tables, world).unwrap();
    assert!(s.last_repairs.is_empty(), "{:?}", s.last_repairs);
    let mut db = SemDb::new(out.def.binding.clone());
    db.set_tree("w", s.tree().unwrap().clone());
    assert!(db.unresolved("w").is_empty(), "{:?}", db.unresolved("w"));

    // PINNED residue: a fn-like macro does NOT define its name, so a
    // code use of it is diagnosed unresolved — explicitly, until the
    // meta tier owns macros.
    let residue = "#define TWICE(x) ((x) + (x))\nint y = TWICE;\n";
    let s2 = IncSession::new(&lexer, &out.def.sg, &tables, residue).unwrap();
    assert!(s2.last_repairs.is_empty(), "{:?}", s2.last_repairs);
    let mut db2 = SemDb::new(out.def.binding.clone());
    db2.set_tree("r", s2.tree().unwrap().clone());
    let unres: Vec<String> = db2.unresolved("r").into_iter().map(|(n, _)| n).collect();
    assert_eq!(unres, ["TWICE"], "fn-like names await the meta tier");
}
