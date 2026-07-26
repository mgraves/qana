//! End-to-end gates on the command surface a new user actually meets.
//!
//! These re-prove the toolchain's headline properties THROUGH the CLI —
//! a scaffolded grammar certifies, an ambiguous one is refused with a
//! counterexample, a broken document still parses losslessly, and an
//! incremental edit agrees with a full reparse — so the quick start in
//! `docs/GUIDE.md` cannot rot without a test going red.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_qana");

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("qana-cli-{tag}-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    dir
}

fn run(args: &[&str]) -> Output {
    Command::new(BIN)
        .args(args)
        .env("NO_COLOR", "1")
        .output()
        .expect("the qana binary runs")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}
fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// Scaffold a starter language and return (grammar, document) paths.
fn scaffold(dir: &Path) -> (String, String) {
    let out = run(&["new", dir.to_str().unwrap(), "--name", "Mylang", "--ext", ".my"]);
    assert!(out.status.success(), "scaffold failed: {}", stderr(&out));
    (
        dir.join("mylang.rg").to_str().unwrap().to_string(),
        dir.join("example.my").to_str().unwrap().to_string(),
    )
}

/// The quick start's first three commands, in order.
#[test]
fn scaffolded_language_certifies_parses_and_resolves() {
    let dir = scratch("new");
    let (g, doc) = scaffold(&dir);

    let check = run(&["check", &g]);
    assert!(check.status.success(), "scaffold must certify: {}", stderr(&check));
    let report = stdout(&check);
    for expected in ["certified", "conflicts", "auto-balanced lists", "binding sites"] {
        assert!(report.contains(expected), "check report missing {expected}:\n{report}");
    }

    let parse = run(&["parse", &g, &doc]);
    assert!(parse.status.success(), "sample must parse: {}", stderr(&parse));
    let tree = stdout(&parse);
    assert!(tree.contains("lossless"), "parse must confirm losslessness:\n{tree}");
    assert!(tree.contains("no errors"), "clean sample must have no errors:\n{tree}");
    // The L4 shape is visible in the tree, not just claimed in docs.
    assert!(tree.contains("balanced list"), "list rules must show as balanced:\n{tree}");

    let defs = run(&["defs", &g, &doc]);
    assert!(defs.status.success());
    let binding = stdout(&defs);
    assert!(binding.contains("every reference resolves"), "sample resolves:\n{binding}");
    assert!(binding.contains("nested scope"), "block scope is reported:\n{binding}");

    std::fs::remove_dir_all(&dir).ok();
}

/// The envelope's whole value: an ambiguous grammar is refused BEFORE it
/// can ship, with an input that demonstrates the ambiguity.
#[test]
fn ambiguous_grammar_is_refused_with_a_counterexample() {
    let dir = scratch("ambig");
    let (g, _) = scaffold(&dir);

    // Deleting the precedence lines makes the expression rule ambiguous.
    let src = std::fs::read_to_string(&g).unwrap();
    let ambiguous: String =
        src.lines().filter(|l| !l.starts_with("prec ")).collect::<Vec<_>>().join("\n");
    let bad = dir.join("ambiguous.rg");
    std::fs::write(&bad, ambiguous).unwrap();

    let out = run(&["check", bad.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1), "an ambiguous grammar must be refused");
    let msg = stderr(&out);
    assert!(msg.contains("conflict"), "refusal names the conflict:\n{msg}");
    assert!(msg.contains("example input"), "refusal carries a counterexample:\n{msg}");
    assert!(msg.contains("L3"), "refusal names the envelope rule:\n{msg}");

    std::fs::remove_dir_all(&dir).ok();
}

/// A PCRE habit that used to compile into a silently wrong token.
#[test]
fn unknown_pattern_escapes_are_refused_not_silently_literal() {
    let dir = scratch("escape");
    let (g, _) = scaffold(&dir);
    let src = std::fs::read_to_string(&g).unwrap();
    let bad = dir.join("escape.rg");
    std::fs::write(&bad, src.replace("/#.*/", "/#[\\s\\S]*/")).unwrap();

    let out = run(&["check", bad.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    let msg = stderr(&out);
    assert!(msg.contains("unknown escape"), "names the problem:\n{msg}");
    assert!(msg.contains("\\d \\a \\w \\s \\t"), "lists what IS supported:\n{msg}");

    std::fs::remove_dir_all(&dir).ok();
}

/// Parsing is total: a document with a syntax error still yields a
/// complete, lossless tree — which is why an editor never goes blank.
#[test]
fn broken_documents_still_parse_losslessly() {
    let dir = scratch("broken");
    let (g, doc) = scaffold(&dir);
    let src = std::fs::read_to_string(&doc).unwrap();
    let broken_path = dir.join("broken.my");
    std::fs::write(&broken_path, src.replacen("let width  = 3;", "let width  = 3", 1)).unwrap();

    let out = run(&["parse", &g, broken_path.to_str().unwrap()]);
    assert!(out.status.success(), "a broken document is still parsed");
    let tree = stdout(&out);
    assert!(tree.contains("lossless"), "still byte-for-byte lossless:\n{tree}");
    assert!(tree.contains("[error]"), "the error region is marked:\n{tree}");
    assert!(stderr(&out).contains("missing"), "and reported: {}", stderr(&out));

    std::fs::remove_dir_all(&dir).ok();
}

/// Incremental reparse must agree with batch — the differential the
/// `edit` command re-runs on every invocation.
#[test]
fn incremental_edit_agrees_with_full_reparse() {
    let dir = scratch("edit");
    let (g, doc) = scaffold(&dir);

    let out = run(&["edit", &g, &doc, "--line", "4", "--text", "let width  = 300;"]);
    assert!(out.status.success(), "edit failed: {}", stderr(&out));
    let report = stdout(&out);
    assert!(
        report.contains("identical to a full reparse"),
        "incremental must equal batch:\n{report}"
    );
    assert!(report.contains("terminals reused"), "reuse is reported:\n{report}");

    std::fs::remove_dir_all(&dir).ok();
}

/// The declared type tier through the shipped binary: the scaffold's
/// grammar declares Num/Str, the sample types cleanly, and a mismatch
/// is reported on the exact operand with exit 1.
#[test]
fn declared_type_tier_reports_defs_and_mismatches()  {
    let dir = scratch("types");
    let (g, doc) = scaffold(&dir);

    let clean = run(&["types", &g, &doc]);
    assert!(clean.status.success(), "clean sample: {}", stderr(&clean));
    let out = stdout(&clean);
    assert!(out.contains("Num"), "defs show the grammar's own vocabulary:\n{out}");
    assert!(out.contains("no type errors"), "clean sample has none:\n{out}");

    let src = std::fs::read_to_string(&doc).unwrap();
    let broken = dir.join("broken.my");
    std::fs::write(&broken, src.replacen("= 3;", "= 3 + \"three\";", 1)).unwrap();
    let bad = run(&["types", &g, broken.to_str().unwrap()]);
    assert_eq!(bad.status.code(), Some(1), "type errors exit 1");
    let msg = stderr(&bad);
    assert!(
        msg.contains("expected `Num`, found `Str`"),
        "names both types from the declared vocabulary:\n{msg}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// The export paths named in the guide actually write files.
#[test]
fn tree_sitter_and_ast_exports_produce_output() {
    let dir = scratch("export");
    let (g, _) = scaffold(&dir);

    let ts_dir = dir.join("ts");
    let ts = run(&["ts", &g, ts_dir.to_str().unwrap()]);
    assert!(ts.status.success(), "tree-sitter emit failed: {}", stderr(&ts));
    let js = std::fs::read_to_string(ts_dir.join("grammar.js")).expect("grammar.js written");
    assert!(js.contains("module.exports"), "emits a tree-sitter grammar");
    assert!(ts_dir.join("queries/highlights.scm").exists(), "emits highlight queries");

    let ast = run(&["ast", &g]);
    assert!(ast.status.success(), "ast emit failed: {}", stderr(&ast));
    assert!(stdout(&ast).contains("pub struct"), "emits typed Rust");

    std::fs::remove_dir_all(&dir).ok();
}

/// The committed structs example stays certified and type-clean: the
/// document's own `struct` declarations extend the vocabulary.
#[test]
fn structs_example_serves_document_level_types() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
    let g = format!("{root}/examples/structs/structlang.rg");
    let doc = format!("{root}/examples/structs/demo.sl");
    let out = run(&["types", &g, &doc]);
    assert!(out.status.success(), "example types clean: {}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("document types: Point, Label"), "vocabulary opened:\n{text}");
    assert!(text.contains("Point") && text.contains("no type errors"), "{text}");
}

/// The meta tier through the CLI: expansion materializes as a
/// deterministic sibling pair (text + provenance), rewrites are
/// write-if-changed, `--check` proves the pair current — and FAILS
/// after tampering. The drift gate IS the read-only model: generated
/// files are ordinary, greppable workspace documents whose staleness
/// is a red exit code, exactly astgen's contract.
#[test]
fn expand_materializes_and_the_drift_gate_bites() {
    let dir = scratch("expand");
    std::fs::create_dir_all(&dir).unwrap();
    let g = dir.join("m.rg");
    let d = dir.join("doc.m");
    std::fs::copy("../../examples/macrolang/mac.rg", &g).unwrap();
    std::fs::write(&d, "macro twice(x) => { x + x }\nlet a = twice!(21);\n").unwrap();
    let (g, d) = (g.to_str().unwrap().to_string(), d.to_str().unwrap().to_string());

    let out = run(&["expand", &g, &d]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("1 substitution(s)"), "{}", stdout(&out));
    let exp = dir.join("doc.exp.m");
    let prov = dir.join("doc.exp.m.prov.json");
    let exp_text = std::fs::read_to_string(&exp).unwrap();
    assert!(exp_text.contains("let a = 21 + 21;"), "{exp_text}");
    let prov_text = std::fs::read_to_string(&prov).unwrap();
    assert!(prov_text.contains("\"kind\": \"arg\""), "{prov_text}");
    assert!(prov_text.contains("\"substitutions\": 1"), "{prov_text}");

    // Unchanged rerun: same bytes, reported as such.
    let out = run(&["expand", &g, &d]);
    assert!(stdout(&out).contains("(unchanged)"), "{}", stdout(&out));

    // Current pair passes --check…
    let out = run(&["expand", &g, &d, "--check"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("is current"), "{}", stdout(&out));

    // …and a tampered materialization is REFUSED (read-only, enforced).
    std::fs::write(&exp, exp_text.replace("21 + 21", "hand-edited")).unwrap();
    let out = run(&["expand", &g, &d, "--check"]);
    assert!(!out.status.success(), "tampering must fail the drift gate");
    assert!(stdout(&out).contains("drifted"), "{}", stdout(&out));
}

/// Cross-file reflection through the CLI: `qana expand` joins
/// same-extension siblings automatically, so a derive written in one
/// file follows a struct declared in another — and the provenance
/// sidecar names that file.
#[test]
fn expand_reflects_a_struct_declared_next_door() {
    let dir = scratch("reflect");
    std::fs::create_dir_all(&dir).unwrap();
    let g = dir.join("s.rg");
    std::fs::copy("../../examples/structs/structlang.rg", &g).unwrap();
    std::fs::write(
        dir.join("lib.sl"),
        "struct Vec3 {\n  x: Num,\n  y: Num,\n  z: Num\n}\n",
    )
    .unwrap();
    let app = dir.join("app.sl");
    std::fs::write(
        &app,
        "let here: Vec3 = new Vec3;\nmacro coords(f, t) => { here.f }\nlet span: Num = coords!{Vec3};\n",
    )
    .unwrap();

    let out = run(&["expand", g.to_str().unwrap(), app.to_str().unwrap()]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = std::fs::read_to_string(dir.join("app.exp.sl")).unwrap();
    assert!(text.contains("let span: Num = here.x + here.y + here.z;"), "{text}");
    let prov = std::fs::read_to_string(dir.join("app.exp.sl.prov.json")).unwrap();
    assert!(prov.contains("lib.sl"), "provenance names the declaring file:\n{prov}");

    // The materialized pair is current, and a new field in the
    // SIBLING makes it stale — the drift gate sees cross-file edits.
    let out = run(&["expand", g.to_str().unwrap(), app.to_str().unwrap(), "--check"]);
    assert!(out.status.success(), "{}", stdout(&out));
    std::fs::write(
        dir.join("lib.sl"),
        "struct Vec3 {\n  x: Num,\n  y: Num,\n  z: Num,\n  w: Num\n}\n",
    )
    .unwrap();
    let out = run(&["expand", g.to_str().unwrap(), app.to_str().unwrap(), "--check"]);
    assert!(!out.status.success(), "a sibling edit must invalidate the materialization");
    assert!(stdout(&out).contains("drifted"), "{}", stdout(&out));
}
