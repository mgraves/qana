//! Gates for the declared type tier (v0): the vocabulary and rules are
//! GRAMMAR-AUTHOR DATA (`@type` annotations), the checker is one
//! generic engine, and a grammar that declares nothing gets a tier of
//! exactly nothing. Malformed declarations are refused at grammar
//! compile time with spans — the envelope pattern extended to types.

use qana_engine::IncSession;
use qana_lang::compile::certify;
use qana_lang::{compile_source, QanaToolchain};
use qana_sem::SemDb;

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
fn tier(doc: &str) -> (qana_sem::TypeReport, String) {
    let tc = QanaToolchain::new();
    let out = compile_source(&tc, TY_LANG);
    assert!(out.diags.is_empty(), "test grammar compiles: {:?}", out.diags);
    let (lexer, tables) = certify(&out.def).expect("in envelope");
    let session = IncSession::new(&lexer, &out.def.sg, &tables, doc).unwrap();
    let mut db = SemDb::new(out.def.binding.clone());
    db.set_types(out.def.types.clone());
    db.set_tree("d", session.tree().unwrap().clone());
    (db.types("d"), doc.to_string())
}

fn def_types(r: &qana_sem::TypeReport, doc: &str) -> Vec<(String, String)> {
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
    let tc = QanaToolchain::new();
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
    let tc = QanaToolchain::new();
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
    use qana_sem::{compose_types, TyTerm, TypeConfig, TypeRule};
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

fn struct_tier(doc: &str) -> (qana_sem::TypeReport, String) {
    let tc = QanaToolchain::new();
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
    let tc = QanaToolchain::new();
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

const APP_LANG: &str = r#"
language App

token WS     = /\s+/ @trivia
token NUMBER = /\d+/ @style(number)
token STRING = /"(\\.|[^"\\])*"/ @style(string)
token IDENT  = /[\a_][\w_]*/ @specialize @style(variable)
token LBRACE = "{" @style(bracket)
token RBRACE = "}" @style(bracket)
token LPAREN = "(" @style(bracket)
token RPAREN = ")" @style(bracket)
token COLON  = ":" @style(punctuation)
token COMMA  = "," @style(punctuation)
token SEMI   = ";" @style(punctuation)
token ARROW  = "->" @style(operator)
token EQ     = "=" @style(operator)
token PLUS   = "+" @style(operator)

keywords IDENT = fn let return Num Str

pair LPAREN RPAREN
pair LBRACE RBRACE

prec left "+"

start file

rule file = File: decls @scope(unordered)

rule decls = decl*

rule decl =
  | FnDecl:  "fn" name:IDENT t:fn_tail @def(name) @type(def, t)
  | LetDecl: "let" name:IDENT "=" e:expr ";" @def(name) @type(def, e)

rule fn_tail = FnTail: "(" p:params ")" "->" rt:ty block @scope @type(fn, p, rt)

rule params = param* % ","

rule param = Param: name:IDENT ":" t:ty @def(name) @type(def, t)

rule ty =
  | TyNum: "Num" @type(Num)
  | TyStr: "Str" @type(Str)

rule block = Block: "{" stmts "}" @scope

rule stmts = stmt*

rule stmt =
  | RetStmt:  "return" e:expr ";" @type(returns, e)
  | ExprStmt: expr ";"

rule expr =
  | AddExpr:  expr "+" expr @type(sig, Num, Num, Num)
  | CallExpr: callee:IDENT "(" a:args ")" @ref(callee, call) @type(apply, a)
  | NumLit:   NUMBER @type(Num)
  | StrLit:   STRING @type(Str)
  | NameRef:  name:IDENT @ref(name) @type(ref)

rule args = expr* % ","
"#;

fn app_tier(doc: &str) -> (qana_sem::TypeReport, String) {
    let tc = QanaToolchain::new();
    let out = compile_source(&tc, APP_LANG);
    assert!(out.diags.is_empty(), "app grammar compiles: {:?}", out.diags);
    let (lexer, tables) = certify(&out.def).expect("in envelope");
    let session = IncSession::new(&lexer, &out.def.sg, &tables, doc).unwrap();
    let mut db = SemDb::new(out.def.binding.clone());
    db.set_types(out.def.types.clone());
    db.set_tree("d", session.tree().unwrap().clone());
    (db.types("d"), doc.to_string())
}

/// Arrow types assemble from param defs + the return annotation, the
/// fn NAME carries them, calls check and produce the return — and a
/// zero-parameter arrow works.
#[test]
fn arrows_assemble_and_applications_check() {
    let (r, doc) = app_tier(
        "fn add(a: Num, b: Num) -> Num { return a + b; }\nfn zero() -> Str { return \"z\"; }\nlet x = add(1, 2);\nlet s = zero();\n",
    );
    let defs = def_types(&r, &doc);
    assert_eq!(defs[0], ("add".to_string(), "fn(Num, Num) -> Num".to_string()));
    assert_eq!(defs[3], ("zero".to_string(), "fn() -> Str".to_string()));
    assert_eq!(defs[4], ("x".to_string(), "Num".to_string()), "call produces the return type");
    assert_eq!(defs[5], ("s".to_string(), "Str".to_string()));
    assert!(r.diags.is_empty(), "clean: {:?}", r.diags);
    assert!(r.atoms.iter().any(|a| a.starts_with("fn(")), "arrows live in the global vocabulary");
}

/// Arity, per-argument mismatches (on the exact argument), non-callable
/// callees, and return-vs-declaration are all diagnosed.
#[test]
fn application_failure_modes_are_diagnosed() {
    let (r, doc) = app_tier(
        "fn add(a: Num, b: Num) -> Num { return a + b; }\nfn bad() -> Num { return \"s\"; }\nlet v = 5;\nlet w = v(3);\nlet x = add(1);\nlet y = add(1, \"two\");\n",
    );
    let msgs: Vec<&str> = r.diags.iter().map(|d| d.msg.as_str()).collect();
    assert_eq!(r.diags.len(), 4, "{msgs:?}");
    assert!(msgs.iter().any(|m| m.contains("return type mismatch: expected `Num`, found `Str`")), "{msgs:?}");
    assert!(msgs.iter().any(|m| m.contains("not callable") && m.contains("`Num`")), "{msgs:?}");
    assert!(msgs.iter().any(|m| m.contains("expected 2 argument(s), found 1")), "{msgs:?}");
    let arg = r.diags.iter().find(|d| d.msg.contains("expected `Num`, found `Str`") && !d.msg.contains("return")).expect("arg mismatch");
    let two = doc.rfind("\"two\"").unwrap() as u32;
    assert!(arg.span.0 <= two && two < arg.span.1, "arg diag {:?} covers the exact argument at {two}", arg.span);
}

/// Functions flow like values: a def that carries an arrow via plain
/// def-propagation is callable, and recursion converges through the
/// fixpoint (the call inside the body sees the fn's own arrow on the
/// next pass).
#[test]
fn first_class_flow_and_recursion_converge() {
    let (r, doc) = app_tier(
        "fn add(a: Num, b: Num) -> Num { return a + b; }\nlet g = add;\nlet y = g(1, 2);\nlet bad = g(\"s\", 2);\n",
    );
    let defs = def_types(&r, &doc);
    assert_eq!(defs[3], ("g".to_string(), "fn(Num, Num) -> Num".to_string()), "arrow flows through a plain let");
    assert_eq!(defs[4], ("y".to_string(), "Num".to_string()), "an arrow-carrying name is callable");
    assert_eq!(r.diags.len(), 1, "{:?}", r.diags);
    assert!(r.diags[0].msg.contains("expected `Num`, found `Str`"));

    let (r2, doc2) = app_tier(
        "fn fact(n: Num) -> Num { return fact(n + 1); }\nfn bad(n: Num) -> Num { return bad(\"s\"); }\n",
    );
    assert_eq!(def_types(&r2, &doc2)[0].1, "fn(Num) -> Num");
    assert_eq!(r2.diags.len(), 1, "recursive call checked on pass 2: {:?}", r2.diags);
    assert!(r2.diags[0].msg.contains("expected `Num`, found `Str`"));
}

/// The new forms carry compile-time cross-checks and arity like the rest.
#[test]
fn v2_forms_are_statically_checked() {
    let tc = QanaToolchain::new();
    let src = APP_LANG.replace("@ref(callee, call) @type(apply, a)", "@type(apply, a)");
    let out = compile_source(&tc, &src);
    assert!(out.diags.iter().any(|d| d.msg.contains("requires @ref")), "{:?}", out.diags);

    let src = APP_LANG.replace("@type(fn, p, rt)", "@type(fn, p, nolabel)");
    let out = compile_source(&tc, &src);
    assert!(out.diags.iter().any(|d| d.msg.contains("no symbol labeled `nolabel`")), "{:?}", out.diags);

    let src = APP_LANG.replace("@type(returns, e)", "@type(returns, name)");
    let out = compile_source(&tc, &src);
    assert!(out.diags.iter().any(|d| d.msg.contains("no symbol labeled `name`")), "{:?}", out.diags);
}

const MEM_LANG: &str = r#"
language Mem

token WS     = /\s+/ @trivia
token NUMBER = /\d+/ @style(number)
token IDENT  = /[\a_][\w_]*/ @specialize @style(variable)
token LBRACE = "{" @style(bracket)
token RBRACE = "}" @style(bracket)
token COLON  = ":" @style(punctuation)
token COMMA  = "," @style(punctuation)
token DOT    = "." @style(punctuation)
token EQ     = "=" @style(operator)
token SEMI   = ";" @style(punctuation)

keywords IDENT = struct let new opaque Num

pair LBRACE RBRACE

prec left "."

start file

rule file = File: stmts @scope(unordered)

rule stmts = stmt*

rule stmt =
  | StructDecl: "struct" name:IDENT b:body @def(name) @type(deftype, b)
  | OpaqueDecl: "opaque" name:IDENT ";" @def(name) @type(deftype)
  | LetStmt:    "let" name:IDENT ti:typed_init ";" @def(name) @type(def, ti)

rule body = Body: "{" fields "}" @scope

rule fields = field* % ","

rule field = Field: name:IDENT ":" t:ty @def(name) @type(def, t)

rule typed_init = TypedInit: ":" t:ty "=" e:expr @type(sig, t, t, t)

rule ty =
  | TyNum:  "Num" @type(Num)
  | TyName: name:IDENT @ref(name) @type(named)

rule expr =
  | MemberExpr: b:expr "." m:IDENT @type(member, b, m)
  | NewExpr:    "new" name:IDENT @ref(name) @type(named)
  | NumLit:     NUMBER @type(Num)
  | NameRef:    name:IDENT @ref(name) @type(ref)
"#;

fn mem_tier(doc: &str) -> (qana_sem::TypeReport, String) {
    let tc = QanaToolchain::new();
    let out = compile_source(&tc, MEM_LANG);
    assert!(out.diags.is_empty(), "mem grammar compiles: {:?}", out.diags);
    let (lexer, tables) = certify(&out.def).expect("in envelope");
    let session = IncSession::new(&lexer, &out.def.sg, &tables, doc).unwrap();
    let mut db = SemDb::new(out.def.binding.clone());
    db.set_types(out.def.types.clone());
    db.set_tree("d", session.tree().unwrap().clone());
    (db.types("d"), doc.to_string())
}

/// Members are the typed defs inside a deftype's body: access types,
/// struct-typed fields CHAIN (`l.a.x` — one fixpoint level per hop),
/// and use-before-declaration converges like everything else.
#[test]
fn member_access_types_and_chains() {
    let (r, doc) = mem_tier(
        "let deep: Num = l.a.x;\nstruct Point { x: Num }\nstruct Line { a: Point }\nlet l: Line = new Line;\n",
    );
    let defs = def_types(&r, &doc);
    assert_eq!(defs[0], ("deep".to_string(), "Num".to_string()), "chained member access, declared BELOW its use");
    assert_eq!(defs[1], ("x".to_string(), "Num".to_string()));
    assert_eq!(defs[2], ("a".to_string(), "Point".to_string()), "struct-typed field");
    assert!(r.diags.is_empty(), "clean: {:?}", r.diags);
}

/// Missing members, memberless types, opaque types, and member-fed
/// mismatches all diagnose; a field whose OWN type is unknown makes
/// access silent (the member exists — no false "no member").
#[test]
fn member_failure_modes_and_unknown_fields_stay_silent() {
    let (r, doc) = mem_tier(
        "struct P { x: Num }\nopaque O;\nlet p: P = new P;\nlet o: O = new O;\nlet m: Num = p.z;\nlet n: Num = 5;\nlet q: Num = n.x;\nlet s: O = p.x;\nlet u: Num = o.any;\n",
    );
    let msgs: Vec<&str> = r.diags.iter().map(|d| d.msg.as_str()).collect();
    assert!(msgs.iter().any(|m| m.contains("no member `z` on `P`")), "{msgs:?}");
    assert!(msgs.iter().any(|m| m.contains("type `Num` has no members")), "{msgs:?}");
    assert!(
        msgs.iter().any(|m| m.contains("type `O` has no members")),
        "opaque deftype has no member set: {msgs:?}"
    );
    assert!(
        msgs.iter().any(|m| m.contains("expected `O`, found `Num`")),
        "member type feeds the unification: {msgs:?}"
    );
    assert_eq!(r.diags.len(), 4, "{msgs:?}");
    let z = doc.find("p.z").unwrap() as u32 + 2;
    let zd = r.diags.iter().find(|d| d.msg.contains("`z`")).unwrap();
    assert!(zd.span.0 <= z && z < zd.span.1, "diag on the name token: {:?}", zd.span);

    // A field annotated with an unresolvable type: the member EXISTS,
    // its type is unknown — access stays silent on both counts.
    let (r2, _) = mem_tier("struct P { x: ghost }\nlet p: P = new P;\nlet v: Num = p.x;\n");
    assert!(
        r2.diags.is_empty(),
        "unknown field type must not become 'no member' or a mismatch: {:?}",
        r2.diags
    );
}

/// Static checks: the member name must label a TOKEN, the base a rule,
/// and deftype's optional body label must exist.
#[test]
fn member_forms_are_statically_checked() {
    let tc = QanaToolchain::new();
    let src = MEM_LANG.replace("@type(member, b, m)", "@type(member, m, b)");
    let out = compile_source(&tc, &src);
    assert!(
        out.diags.iter().any(|d| d.msg.contains("labels a token") || d.msg.contains("must be a token")),
        "swapped base/name refused: {:?}",
        out.diags
    );
    let src = MEM_LANG.replace("@type(deftype, b)", "@type(deftype, nolabel)");
    let out = compile_source(&tc, &src);
    assert!(
        out.diags.iter().any(|d| d.msg.contains("no symbol labeled `nolabel`")),
        "{:?}",
        out.diags
    );
}

// ---------------------------------------------------------------------------
// Infrastructure: per-item memoization + cross-file flow
// ---------------------------------------------------------------------------

use qana_engine::{Line, LineEdit};

/// Replace the first line containing `needle` in a live session.
fn edit_line(
    session: &mut qana_engine::IncSession<'_>,
    sg: &qana_grammar::SynGrammar,
    tables: &qana_grammar::LrTables,
    needle: &str,
    replacement: &str,
) {
    let li = session
        .buf
        .lines
        .iter()
        .position(|l| l.text.contains(needle))
        .expect("needle line");
    let term = session.buf.lines[li].term;
    session
        .edit(sg, tables, &[LineEdit { start: li, end: li + 1, replacement: vec![Line::new(replacement, term)] }])
        .expect("edit parses");
}

/// The differential that anchors everything: a memoized report must be
/// IDENTICAL to one computed by a fresh SemDb over the same tree.
fn assert_fresh_equal(db: &mut SemDb, cfg: &qana_lang::compile::LangDef, uri: &str, tree: &std::sync::Arc<qana_grammar::GreenNode>) {
    // Global TypeIds are history-dependent (stable for the session);
    // MEANINGS are not. Compare display-canonically.
    let canon = |r: &qana_sem::TypeReport| {
        let name = |t: qana_sem::TypeId| r.atoms.get(t as usize).cloned().unwrap_or_default();
        (
            r.types.iter().map(|&(s, t)| (s, name(t))).collect::<Vec<_>>(),
            r.def_types.iter().map(|&(s, t)| (s, name(t))).collect::<Vec<_>>(),
            r.diags.clone(),
        )
    };
    let memo = db.types(uri);
    let mut fresh = SemDb::new(cfg.binding.clone());
    fresh.set_types(cfg.types.clone());
    fresh.set_tree(uri, tree.clone());
    let clean = fresh.types(uri);
    assert_eq!(canon(&memo), canon(&clean), "memoized ≡ fresh (canonical)");
}

/// A BODY edit (def types unchanged) re-walks only the edited item, in
/// one pass; a SIGNATURE edit (a def's type changes) ripples — and both
/// stay identical to a from-scratch computation.
#[test]
fn memoization_body_edits_walk_one_item_signature_edits_ripple() {
    let tc = QanaToolchain::new();
    let out = compile_source(&tc, APP_LANG);
    let (lexer, tables) = certify(&out.def).unwrap();
    let mut doc = String::from("fn add(a: Num, b: Num) -> Num { return a + b; }\n");
    for i in 0..20 {
        doc.push_str(&format!("let x{i} = add({i}, {i});\n"));
    }
    let mut session = IncSession::new(&lexer, &out.def.sg, &tables, &doc).unwrap();
    let mut db = SemDb::new(out.def.binding.clone());
    db.set_types(out.def.types.clone());
    db.set_tree("d", session.tree().unwrap().clone());
    let cold = db.types("d");
    assert!(cold.diags.is_empty());
    let cold_walks = db.stats.type_item_walks;
    assert!(cold_walks >= 21, "cold pass walks everything (got {cold_walks})");

    // Body edit: same types, new value.
    edit_line(&mut session, &out.def.sg, &tables, "let x7 ", "let x7 = add(700, 7);");
    db.set_tree("d", session.tree().unwrap().clone());
    let before = (db.stats.type_item_walks, db.stats.type_passes);
    let r = db.types("d");
    assert!(r.diags.is_empty());
    let walks = db.stats.type_item_walks - before.0;
    let passes = db.stats.type_passes - before.1;
    assert!(walks <= 2, "body edit re-walks only the edited item (±newline adjacency), got {walks}");
    assert_eq!(passes, 1, "body edit converges in one pass");
    assert_fresh_equal(&mut db, &out.def, "d", session.tree().unwrap());

    // Signature edit: x7's TYPE changes (Num → Str) — ripple, and the
    // report is still exactly what a cold computation produces.
    edit_line(&mut session, &out.def.sg, &tables, "let x7 ", "let x7 = \"now a string\";");
    db.set_tree("d", session.tree().unwrap().clone());
    let before = db.stats.type_item_walks;
    let r = db.types("d");
    assert!(r.diags.is_empty(), "nothing uses x7, so no diags: {:?}", r.diags);
    let walks = db.stats.type_item_walks - before;
    assert!(walks > 20, "signature edit ripples (got {walks})");
    assert_fresh_equal(&mut db, &out.def, "d", session.tree().unwrap());

    // And a signature edit that IS used downstream updates diagnostics.
    edit_line(&mut session, &out.def.sg, &tables, "fn add", "fn add(a: Num, b: Num) -> Str { return \"s\"; }");
    db.set_tree("d", session.tree().unwrap().clone());
    let r = db.types("d");
    assert!(r.diags.is_empty(), "calls still arity-clean; results now Str: {:?}", r.diags);
    let defs = def_types(&r, &session.buf.reproduce());
    assert_eq!(defs.iter().find(|(n, _)| n == "x3").unwrap().1, "Str", "new return type flowed to every call site");
    assert_fresh_equal(&mut db, &out.def, "d", session.tree().unwrap());
}

/// Cross-file: a reference resolving into another file carries that
/// file's converged type; edits to the dependency are seen on the next
/// query; mutual references terminate.
#[test]
fn cross_file_values_flow_staleness_and_cycles() {
    let tc = QanaToolchain::new();
    let out = compile_source(&tc, APP_LANG);
    let (lexer, tables) = certify(&out.def).unwrap();
    let mut db = SemDb::new(out.def.binding.clone());
    db.set_types(out.def.types.clone());

    let a = IncSession::new(&lexer, &out.def.sg, &tables, "let shared = 1;\n").unwrap();
    db.set_tree("a", a.tree().unwrap().clone());
    let b_doc = "let use_it = shared + 2;\nlet bad = shared;\nfn f(p: Num) -> Num { return p; }\nlet call_it = f(shared);\n";
    let b = IncSession::new(&lexer, &out.def.sg, &tables, b_doc).unwrap();
    db.set_tree("b", b.tree().unwrap().clone());

    let r = db.types("b");
    let defs = def_types(&r, b_doc);
    assert_eq!(defs.iter().find(|(n, _)| n == "use_it").unwrap().1, "Num", "foreign atom type flows");
    assert_eq!(defs.iter().find(|(n, _)| n == "call_it").unwrap().1, "Num", "foreign values feed calls");
    assert!(r.diags.is_empty(), "{:?}", r.diags);

    // The dependency changes type: the next query sees it.
    let a2 = IncSession::new(&lexer, &out.def.sg, &tables, "let shared = \"str\";\n").unwrap();
    db.set_tree("a", a2.tree().unwrap().clone());
    let r = db.types("b");
    assert_eq!(r.diags.len(), 2, "shared + 2 and f(shared) now mismatch: {:?}", r.diags);
    assert!(r.diags.iter().all(|d| d.msg.contains("expected `Num`, found `Str`")));

    // Mutual references terminate (one-hop semantics, no hang).
    let c1 = IncSession::new(&lexer, &out.def.sg, &tables, "let c1 = d1;\n").unwrap();
    let d1 = IncSession::new(&lexer, &out.def.sg, &tables, "let d1 = c1;\n").unwrap();
    db.set_tree("c", c1.tree().unwrap().clone());
    db.set_tree("e", d1.tree().unwrap().clone());
    let _ = db.types("c");
    let _ = db.types("e");
}

/// GLOBAL vocabulary: a struct declared in one file IS a type in
/// another — values flow with their doc type, annotations denote the
/// foreign type, member access reads the foreign member table, and
/// mismatches name it.
#[test]
fn foreign_document_types_flow_globally() {
    let tc = QanaToolchain::new();
    let out = compile_source(&tc, MEM_LANG);
    let (lexer, tables) = certify(&out.def).unwrap();
    let mut db = SemDb::new(out.def.binding.clone());
    db.set_types(out.def.types.clone());

    let a = IncSession::new(&lexer, &out.def.sg, &tables, "struct P { x: Num }\nlet p: P = new P;\n").unwrap();
    db.set_tree("a", a.tree().unwrap().clone());
    let b_doc = "let q: Num = p;\nlet r: P = p;\nlet m: Num = p.x;\nlet bad: Num = p.z;\n";
    let b = IncSession::new(&lexer, &out.def.sg, &tables, b_doc).unwrap();
    db.set_tree("b", b.tree().unwrap().clone());
    let r = db.types("b");
    let defs = def_types(&r, b_doc);
    assert_eq!(defs.iter().find(|(n, _)| n == "r").unwrap().1, "P", "foreign type name denotes in annotations");
    assert_eq!(defs.iter().find(|(n, _)| n == "m").unwrap().1, "Num", "foreign member access types");
    let msgs: Vec<&str> = r.diags.iter().map(|d| d.msg.as_str()).collect();
    assert!(msgs.iter().any(|m| m.contains("expected `Num`, found `P`")), "foreign doc type in mismatches: {msgs:?}");
    assert!(msgs.iter().any(|m| m.contains("no member `z` on `P`")), "foreign member table consulted: {msgs:?}");
    assert_eq!(r.diags.len(), 2, "{msgs:?}");

    // Staleness: retype the field in A; B's member flow flips.
    let a2 = IncSession::new(&lexer, &out.def.sg, &tables, "struct P { x: P }\nlet p: P = new P;\n").unwrap();
    db.set_tree("a", a2.tree().unwrap().clone());
    let r = db.types("b");
    assert!(
        r.diags.iter().any(|d| d.msg.contains("expected `Num`, found `P`")
            && r.diags.iter().filter(|d2| d2.msg.contains("expected `Num`, found `P`")).count() == 2),
        "p.x now yields P, so `let m: Num = p.x` mismatches too: {:?}",
        r.diags
    );
}

/// Foreign FUNCTIONS: arrows are global, so a fn defined in one file is
/// callable from another with full arity/argument checking — directly
/// and through a first-class binding.
#[test]
fn foreign_functions_flow_with_checking() {
    let tc = QanaToolchain::new();
    let out = compile_source(&tc, APP_LANG);
    let (lexer, tables) = certify(&out.def).unwrap();
    let mut db = SemDb::new(out.def.binding.clone());
    db.set_types(out.def.types.clone());

    let a = IncSession::new(&lexer, &out.def.sg, &tables,
        "fn add(a: Num, b: Num) -> Num { return a + b; }\n").unwrap();
    db.set_tree("a", a.tree().unwrap().clone());
    let b_doc = "let x = add(1, 2);\nlet g = add;\nlet y = g(3, 4);\nlet bad = add(1);\nlet worse = g(1, \"s\");\n";
    let b = IncSession::new(&lexer, &out.def.sg, &tables, b_doc).unwrap();
    db.set_tree("b", b.tree().unwrap().clone());
    let r = db.types("b");
    let defs = def_types(&r, b_doc);
    assert_eq!(defs.iter().find(|(n, _)| n == "x").unwrap().1, "Num", "foreign call produces the return");
    assert_eq!(defs.iter().find(|(n, _)| n == "g").unwrap().1, "fn(Num, Num) -> Num", "foreign arrow flows first-class");
    assert_eq!(defs.iter().find(|(n, _)| n == "y").unwrap().1, "Num");
    let msgs: Vec<&str> = r.diags.iter().map(|d| d.msg.as_str()).collect();
    assert!(msgs.iter().any(|m| m.contains("expected 2 argument(s), found 1")), "{msgs:?}");
    assert!(msgs.iter().any(|m| m.contains("expected `Num`, found `Str`")), "{msgs:?}");
    assert_eq!(r.diags.len(), 2, "{msgs:?}");
}
