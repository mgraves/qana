//! Type-tier infrastructure numbers: memoized per-item checking.
//!
//!   cargo run --release -p qana-lang --bin p8bench
//!
//! Scenarios over an N-item document: cold derivation, a BODY edit
//! (types unchanged — the keystroke case), and a SIGNATURE edit (a
//! def's type changes — the documented ripple), with the walk/pass
//! counters that prove which path ran.

use qana_engine::{IncSession, Line, LineEdit};
use qana_lang::compile::certify;
use qana_lang::{compile_source, QanaToolchain};
use qana_sem::SemDb;
use std::time::Instant;

const LANG: &str = r#"
language Bench

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
rule ty = | TyNum: "Num" @type(Num) | TyStr: "Str" @type(Str)
rule block = Block: "{" stmts "}" @scope
rule stmts = stmt*
rule stmt = | RetStmt: "return" e:expr ";" @type(returns, e) | ExprStmt: expr ";"
rule expr =
  | AddExpr:  expr "+" expr @type(sig, Num, Num, Num)
  | CallExpr: callee:IDENT "(" a:args ")" @ref(callee, call) @type(apply, a)
  | NumLit:   NUMBER @type(Num)
  | StrLit:   STRING @type(Str)
  | NameRef:  name:IDENT @ref(name) @type(ref)
rule args = expr* % ","
"#;

fn main() {
    let n: usize = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(2000);
    let tc = QanaToolchain::new();
    let out = compile_source(&tc, LANG);
    assert!(out.diags.is_empty(), "{:?}", out.diags);
    let (lexer, tables) = certify(&out.def).expect("in envelope");

    let mut doc = String::from("fn add(a: Num, b: Num) -> Num { return a + b; }\n");
    for i in 0..n {
        doc.push_str(&format!("let x{i} = add({i}, {i}) + {i};\n"));
    }
    let t0 = Instant::now();
    let mut session = IncSession::new(&lexer, &out.def.sg, &tables, &doc).unwrap();
    let parse_cold = t0.elapsed();

    let mut db = SemDb::new(out.def.binding.clone());
    db.set_types(out.def.types.clone());
    db.set_tree("d", session.tree().unwrap().clone());

    let t0 = Instant::now();
    let cold = db.types("d");
    let types_cold = t0.elapsed();
    assert!(cold.diags.is_empty());
    println!("items {} | parse cold {:?} | types cold {:?}", n + 1, parse_cold, types_cold);
    println!(
        "  cold counters: walks {} passes {}",
        db.stats.type_item_walks, db.stats.type_passes
    );

    // BODY edit: value changes, type doesn't.
    let li = session.buf.lines.iter().position(|l| l.text.contains("let x1000 ")).unwrap();
    let term = session.buf.lines[li].term;
    let t0 = Instant::now();
    session
        .edit(&out.def.sg, &tables, &[LineEdit {
            start: li,
            end: li + 1,
            replacement: vec![Line::new("let x1000 = add(9, 9) + 9;", term)],
        }])
        .unwrap();
    db.set_tree("d", session.tree().unwrap().clone());
    let (w0, p0) = (db.stats.type_item_walks, db.stats.type_passes);
    let r = db.types("d");
    let body = t0.elapsed();
    assert!(r.diags.is_empty());
    println!(
        "body edit (types unchanged): {:?} | walks {} passes {}",
        body,
        db.stats.type_item_walks - w0,
        db.stats.type_passes - p0
    );

    // SIGNATURE edit: a def's type changes.
    let t0 = Instant::now();
    session
        .edit(&out.def.sg, &tables, &[LineEdit {
            start: li,
            end: li + 1,
            replacement: vec![Line::new("let x1000 = \"now a string\";", term)],
        }])
        .unwrap();
    db.set_tree("d", session.tree().unwrap().clone());
    let (w0, p0) = (db.stats.type_item_walks, db.stats.type_passes);
    let r = db.types("d");
    let sig = t0.elapsed();
    assert!(r.diags.is_empty());
    println!(
        "signature edit (type changed): {:?} | walks {} passes {}",
        sig,
        db.stats.type_item_walks - w0,
        db.stats.type_passes - p0
    );

    // Steady state: query again with no edit at all.
    let (w0, p0) = (db.stats.type_item_walks, db.stats.type_passes);
    let t0 = Instant::now();
    let _ = db.types("d");
    let idle = t0.elapsed();
    println!(
        "no-op query: {:?} | walks {} passes {}",
        idle,
        db.stats.type_item_walks - w0,
        db.stats.type_passes - p0
    );
}
