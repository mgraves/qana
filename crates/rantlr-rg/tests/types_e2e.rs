//! Gates for the declared type tier (v0): the vocabulary and rules are
//! GRAMMAR-AUTHOR DATA (`@type` annotations), the checker is one
//! generic engine, and a grammar that declares nothing gets a tier of
//! exactly nothing. Malformed declarations are refused at grammar
//! compile time with spans — the envelope pattern extended to types.

use rantlr_engine::IncSession;
use rantlr_rg::compile::certify;
use rantlr_rg::{compile_source, RgToolchain};
use rantlr_sem::SemDb;

const TY_LANG: &str = r#"
language Ty

token WS     = /\s+/ @trivia
token NUMBER = /\d+/ @style(number)
token STRING = /"(\\.|[^"\\])*"/ @style(string)
token IDENT  = /[\a_][\w_]*/ @specialize @style(variable)
token PLUS   = "+" @style(operator)
token AMP    = "&" @style(operator)
token EQ     = "=" @style(operator)
token SEMI   = ";" @style(punctuation)
token LPAREN = "(" @style(bracket)
token RPAREN = ")" @style(bracket)

keywords IDENT = let

pair LPAREN RPAREN

prec left "+"
prec left "&"

start file

rule file = File: stmts

rule stmts = stmt*

rule stmt = LetStmt: "let" name:IDENT "=" e:expr ";" @def(name) @type(def, e)

rule expr =
  | AddExpr:   expr "+" expr @type(sig, Num, Num, Num)
  | JoinExpr:  expr "&" expr @type(sig, t, t, t)
  | ParenExpr: "(" e:expr ")" @type(of, e)
  | NumLit:    NUMBER @type(Num)
  | StrLit:    STRING @type(Str)
  | NameRef:   name:IDENT @ref(name) @type(ref)
"#;

/// Compile + certify + parse + run the tier over `doc`.
fn tier(doc: &str) -> (rantlr_sem::TypeReport, String) {
    let tc = RgToolchain::new();
    let out = compile_source(&tc, TY_LANG);
    assert!(out.diags.is_empty(), "test grammar compiles: {:?}", out.diags);
    let (lexer, tables) = certify(&out.def).expect("in envelope");
    let session = IncSession::new(&lexer, &out.def.sg, &tables, doc).unwrap();
    let mut db = SemDb::new(out.def.binding.clone());
    db.set_types(out.def.types.clone());
    db.set_tree("d", session.tree().unwrap().clone());
    (db.types("d"), doc.to_string())
}

fn def_types(r: &rantlr_sem::TypeReport, doc: &str) -> Vec<(String, String)> {
    r.def_types
        .iter()
        .map(|&((a, b), t)| (doc[a as usize..b as usize].to_string(), r.atoms[t as usize].clone()))
        .collect()
}

/// The vocabulary is the grammar's own invention, types synthesize
/// bottom-up, and they flow through NAMES via the binding tier —
/// including through def→ref chains that need iteration to converge.
#[test]
fn types_flow_through_rules_and_names() {
    let (r, doc) = tier("let a = 1 + 2;\nlet b = a;\nlet c = b + 3;\nlet s = \"x\" & \"y\";\n");
    assert_eq!(r.atoms, vec!["Num".to_string(), "Str".to_string()], "declared vocabulary only");
    assert_eq!(
        def_types(&r, &doc),
        vec![
            ("a".to_string(), "Num".to_string()),
            ("b".to_string(), "Num".to_string()),
            ("c".to_string(), "Num".to_string()),
            ("s".to_string(), "Str".to_string()),
        ],
        "def→ref chains converge (b needs a's type, c needs b's)"
    );
    assert!(r.diags.is_empty(), "clean program: {:?}", r.diags);
}

/// Signature variables unify per node: `t & t` accepts both Num pairs
/// and Str pairs but rejects a mixed pair — with the diagnostic on the
/// exact operand, carrying both type names.
#[test]
fn sig_variables_unify_and_mismatches_carry_spans() {
    let (r, doc) = tier("let ok1 = 1 & 2;\nlet ok2 = \"a\" & \"b\";\nlet bad = 1 & \"b\";\n");
    let defs = def_types(&r, &doc);
    assert_eq!(defs[0], ("ok1".to_string(), "Num".to_string()));
    assert_eq!(defs[1], ("ok2".to_string(), "Str".to_string()));
    assert_eq!(r.diags.len(), 1, "exactly the mixed pair errs: {:?}", r.diags);
    let d = &r.diags[0];
    assert!(d.msg.contains("`Num`") && d.msg.contains("`Str`"), "names both types: {}", d.msg);
    let target = doc.rfind("\"b\"").unwrap() as u32;
    assert!(
        d.span.0 <= target && target < d.span.1,
        "span {:?} covers the offending operand at {target}",
        d.span
    );
}

/// Unknown never cascades: an unresolved name types as nothing and
/// produces NO type error (the binding tier already reports it), and a
/// parse-repaired region refuses to type rather than mis-type.
#[test]
fn unknown_and_error_regions_stay_silent() {
    let (r, _) = tier("let a = ghost + 1;\n");
    assert!(r.diags.is_empty(), "unresolved ref must not become a type error: {:?}", r.diags);
    let (r2, _) = tier("let a = 1 + ;\n");
    assert!(r2.diags.is_empty(), "repaired regions do not type-check: {:?}", r2.diags);
}

/// A grammar that declares nothing HAS nothing: empty report, no rules.
#[test]
fn no_declarations_no_tier() {
    let tc = RgToolchain::new();
    let stripped: String =
        TY_LANG.lines().map(|l| {
            let mut l = l.to_string();
            while let Some(i) = l.find("@type(") {
                let end = l[i..].find(')').map(|e| i + e + 1).unwrap_or(l.len());
                l.replace_range(i..end, "");
            }
            l + "\n"
        }).collect();
    let out = compile_source(&tc, &stripped);
    assert!(out.diags.is_empty(), "{:?}", out.diags);
    assert!(out.def.types.rules.is_empty() && out.def.types.atoms.is_empty());
    let (lexer, tables) = certify(&out.def).unwrap();
    let session = IncSession::new(&lexer, &out.def.sg, &tables, "let a = 1;\n").unwrap();
    let mut db = SemDb::new(out.def.binding.clone());
    db.set_types(out.def.types.clone());
    db.set_tree("d", session.tree().unwrap().clone());
    let r = db.types("d");
    assert!(r.types.is_empty() && r.def_types.is_empty() && r.diags.is_empty());
}

/// Malformed declarations are refused at GRAMMAR compile time, with
/// spans — arity checked against the production, labels resolved, and
/// the def/ref forms cross-checked against the binding annotations.
#[test]
fn malformed_type_declarations_are_refused_with_spans() {
    let tc = RgToolchain::new();
    let refusal = |broken: &str, expect: &str| {
        let src = TY_LANG.replace("@type(sig, Num, Num, Num)", broken);
        assert_ne!(src, TY_LANG, "replacement applied: {broken}");
        let out = compile_source(&tc, &src);
        let hit = out.diags.iter().find(|d| d.msg.contains(expect));
        assert!(
            hit.is_some(),
            "`{broken}` must be refused mentioning `{expect}`; got {:?}",
            out.diags
        );
        assert_ne!(hit.unwrap().span, (0, 0), "refusal carries a real span");
    };
    refusal("@type(sig, Num, Num, Num, Num)", "3 parameter(s) but the alternative has 2");
    refusal("@type(sig, Num, Num)", "1 parameter(s) but the alternative has 2");
    refusal("@type(of, nolabel)", "no symbol labeled `nolabel`");
    refusal("@type(banana, Num)", "unknown @type form `banana`");
    refusal("@type(lower)", "not a type atom");
    refusal("@type(Num) @type(Str)", "at most one @type");

    // def/ref forms require their binding counterparts.
    let src = TY_LANG.replace("@ref(name) @type(ref)", "@type(ref)");
    let out = compile_source(&tc, &src);
    assert!(
        out.diags.iter().any(|d| d.msg.contains("requires @ref")),
        "@type(ref) without @ref refused: {:?}",
        out.diags
    );
}

/// `compose_types` mirrors `compose_binding`: host rules at unchanged
/// ids, guest rules at the product's offsets, vocabularies merged BY
/// NAME (a shared atom name is one type across the boundary).
#[test]
fn compose_types_offsets_and_merges_vocabularies() {
    use rantlr_sem::{compose_types, TyTerm, TypeConfig, TypeRule};
    let mut host = TypeConfig::default();
    let num = host.intern("Num");
    host.rules.push((1, 2, TypeRule::Const(num)));
    let mut guest = TypeConfig::default();
    let g_str = guest.intern("Str");
    let g_num = guest.intern("Num");
    guest.rules.push((0, 0, TypeRule::Const(g_str)));
    guest.rules.push((3, 4, TypeRule::Sig {
        params: vec![TyTerm::Atom(g_num), TyTerm::Var(0)],
        result: TyTerm::Var(0),
    }));

    let out = compose_types(&host, &guest, 10, 20);
    assert_eq!(out.atoms, vec!["Num".to_string(), "Str".to_string()], "merged by name");
    assert_eq!(out.rules[0], (1, 2, TypeRule::Const(0)), "host untouched");
    assert_eq!(out.rules[1], (10, 20, TypeRule::Const(1)), "guest offset; Str remapped");
    match &out.rules[2] {
        (13, 24, TypeRule::Sig { params, result }) => {
            assert_eq!(params[0], TyTerm::Atom(0), "guest Num unified with host Num");
            assert_eq!(params[1], TyTerm::Var(0), "variables pass through");
            assert_eq!(*result, TyTerm::Var(0));
        }
        other => panic!("unexpected rule: {other:?}"),
    }
}

const STRUCT_LANG: &str = r#"
language Sl

token WS     = /\s+/ @trivia
token NUMBER = /\d+/ @style(number)
token IDENT  = /[\a_][\w_]*/ @specialize @style(variable)
token LBRACE = "{" @style(bracket)
token RBRACE = "}" @style(bracket)
token COLON  = ":" @style(punctuation)
token EQ     = "=" @style(operator)
token SEMI   = ";" @style(punctuation)

keywords IDENT = struct let new

pair LBRACE RBRACE

start file

rule file = File: stmts @scope(unordered)

rule stmts = stmt*

rule stmt =
  | StructDecl: "struct" name:IDENT "{" "}" @def(name) @type(deftype)
  | LetStmt:    "let" name:IDENT ti:typed_init ";" @def(name) @type(def, ti)
  | BlockStmt:  block

rule typed_init = TypedInit: ":" t:ty "=" e:expr @type(sig, t, t, t)

rule block = Block: "{" stmts "}" @scope

rule ty = TyName: name:IDENT @ref(name) @type(named)

rule expr =
  | NewExpr:  "new" name:IDENT @ref(name) @type(named)
  | NumLit:   NUMBER @type(Num)
  | NameRef:  name:IDENT @ref(name) @type(ref)
"#;

fn struct_tier(doc: &str) -> (rantlr_sem::TypeReport, String) {
    let tc = RgToolchain::new();
    let out = compile_source(&tc, STRUCT_LANG);
    assert!(out.diags.is_empty(), "struct grammar compiles: {:?}", out.diags);
    let (lexer, tables) = certify(&out.def).expect("in envelope");
    let session = IncSession::new(&lexer, &out.def.sg, &tables, doc).unwrap();
    let mut db = SemDb::new(out.def.binding.clone());
    db.set_types(out.def.types.clone());
    db.set_tree("d", session.tree().unwrap().clone());
    (db.types("d"), doc.to_string())
}

/// `deftype` opens the vocabulary at DOCUMENT level: struct declarations
/// introduce types, `named` annotations denote them, values flow through
/// defs and refs, and mismatches name the document's own types.
#[test]
fn document_types_open_the_vocabulary() {
    let (r, doc) = struct_tier(
        "struct P { }\nstruct Q { }\nlet a: P = new P;\nlet b: P = a;\nlet c: P = new Q;\nlet d: P = 1;\n",
    );
    assert_eq!(r.grammar_atoms, 1, "grammar declared only Num");
    assert_eq!(r.atoms, vec!["Num", "P", "Q"], "vocabulary = atoms + document types");
    let defs = def_types(&r, &doc);
    assert_eq!(defs[0], ("a".to_string(), "P".to_string()));
    assert_eq!(defs[1], ("b".to_string(), "P".to_string()), "doc types flow through refs");
    assert_eq!(r.diags.len(), 2, "{:?}", r.diags);
    assert!(r.diags[0].msg.contains("expected `P`, found `Q`"), "{}", r.diags[0].msg);
    assert!(r.diags[1].msg.contains("expected `P`, found `Num`"), "{}", r.diags[1].msg);
}

/// A resolved name that is not a `deftype` is diagnosed in type
/// position; an UNRESOLVED name stays silent (the binding tier owns it)
/// and the initializer's type flows through the unification instead.
#[test]
fn non_type_names_are_diagnosed_unresolved_stay_silent() {
    let (r, _) = struct_tier("let v: Num = 1;\nlet w: v = 2;\n");
    assert_eq!(r.diags.len(), 1, "{:?}", r.diags);
    assert!(r.diags[0].msg.contains("does not denote a type"), "{}", r.diags[0].msg);

    let (r2, doc2) = struct_tier("let x: ghost = 1;\n");
    assert!(r2.diags.is_empty(), "unresolved type name is the binding tier's report");
    assert_eq!(def_types(&r2, &doc2)[0].1, "Num", "initializer type flows when annotation is unknown");
}

/// THE v1 property: type identity is the DECLARATION SITE. Two structs
/// named `T` in different scopes are different types — which `T` an
/// annotation denotes is decided by ordinary scoped name resolution,
/// so shadowing produces a real mismatch even though both display `T`.
#[test]
fn nominal_identity_is_the_declaration_site() {
    let (r, _) = struct_tier(
        "struct T { }\nlet outer: T = new T;\n{\n  struct T { }\n  let inner: T = new T;\n  let cross: T = outer;\n}\n",
    );
    assert_eq!(r.atoms, vec!["Num", "T", "T"], "two distinct types, same display name");
    assert_eq!(r.diags.len(), 1, "inner/outer cross-assignment mismatches: {:?}", r.diags);
    assert!(
        r.diags[0].msg.contains("expected `T`, found `T`"),
        "nominal-by-site even under one display name: {}",
        r.diags[0].msg
    );
}

/// The new forms carry the same compile-time cross-checks as def/ref.
#[test]
fn deftype_and_named_require_their_binding_counterparts() {
    let tc = RgToolchain::new();
    let src = STRUCT_LANG.replace("@def(name) @type(deftype)", "@type(deftype)");
    let out = compile_source(&tc, &src);
    assert!(
        out.diags.iter().any(|d| d.msg.contains("requires @def")),
        "@type(deftype) without @def refused: {:?}",
        out.diags
    );
    let src = STRUCT_LANG.replace("rule ty = TyName: name:IDENT @ref(name) @type(named)",
                                  "rule ty = TyName: name:IDENT @type(named)");
    let out = compile_source(&tc, &src);
    assert!(
        out.diags.iter().any(|d| d.msg.contains("requires @ref")),
        "@type(named) without @ref refused: {:?}",
        out.diags
    );
}
