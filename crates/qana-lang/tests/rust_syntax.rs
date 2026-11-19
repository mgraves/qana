//! `examples/rust/rust.qana` reads Rust as rustc does: each test pins the
//! tree rustc's reading gives a construct the grammar had to decide (see
//! the grammar's header), or the error rustc reports.
//!
//! Trees are written compactly: a token is its text, a node with no
//! token (an absent optional part) is left out, a node with one child is
//! that child, and any other node is `(Label children…)`, with lists
//! flattened into their parent.

use qana_grammar::green::{flat_children, is_seq_prod, GreenChild, GreenNode};
use qana_lang::EmbeddedLang;
use std::sync::OnceLock;

const GRAMMAR: &str = include_str!("../../../examples/rust/rust.qana");

fn lang() -> &'static EmbeddedLang {
    static LANG: OnceLock<EmbeddedLang> = OnceLock::new();
    LANG.get_or_init(|| EmbeddedLang::from_qana_source(GRAMMAR).unwrap_or_else(|d| panic!("{d:?}")))
}

/// The children of a node with its lists flattened in, trivia dropped.
fn children(n: &GreenNode, out: &mut Vec<GreenChild>) {
    let kids: Vec<&GreenChild> =
        if is_seq_prod(n.prod) { flat_children(n) } else { n.children.iter().collect() };
    for c in kids {
        match c {
            GreenChild::Token(t) if t.trivia => {}
            GreenChild::Node(m) if m.n_toks == 0 => {}
            GreenChild::Node(m) if is_seq_prod(m.prod) => children(m, out),
            other => out.push(other.clone()),
        }
    }
}

fn shape(c: &GreenChild) -> String {
    match c {
        GreenChild::Token(t) => t.text.clone(),
        GreenChild::Node(n) => {
            let mut kids = Vec::new();
            children(n, &mut kids);
            match kids.as_slice() {
                [] => format!("({})", lang().sg.prod_name(n.prod as usize)),
                [only] => shape(only),
                _ => {
                    let inner: Vec<String> = kids.iter().map(shape).collect();
                    format!("({} {})", lang().sg.prod_name(n.prod as usize), inner.join(" "))
                }
            }
        }
    }
}

/// The shape of a whole source, which must parse without a repair.
fn parses(src: &str) -> String {
    let s = lang().session(src);
    assert!(s.last_repairs.is_empty(), "`{src}` parses without a repair: {:?}", s.last_repairs);
    shape(&GreenChild::Node(s.tree().expect("total").clone()))
}

/// The shape of the statement-level expression `e;` in a function body.
fn stmt(e: &str) -> String {
    let all = parses(&format!("fn f() {{ {e} }}"));
    let open = all.find("{ ").map(|i| i + 2).expect("a body");
    let close = all.rfind(" }").expect("a body");
    all[open..close].to_string()
}

fn refused(src: &str) {
    let s = lang().session(src);
    assert!(
        !s.last_repairs.is_empty(),
        "`{src}` is a syntax error, as rustc has it, but parses as {}",
        shape(&GreenChild::Node(s.tree().expect("total").clone()))
    );
}

#[test]
fn binary_operators_bind_as_the_reference_tables_them() {
    assert_eq!(stmt("x = a + b * c;"), "(ExprStmt (SAssign x = (Add a + (Mul b * c))) ;)");
    assert_eq!(stmt("a - b - c;"), "(ExprStmt (SSub (SSub a - b) - c) ;)");
    assert_eq!(stmt("a || b && c;"), "(ExprStmt (SOr a | | (And b & & c)) ;)");
    assert_eq!(stmt("x = y = z;"), "(ExprStmt (SAssign x = (Assign y = z)) ;)");
    assert_eq!(stmt("-x as u8;"), "(ExprStmt (SCast (SNeg - x) as u8) ;)");
    assert_eq!(
        stmt("*x.y()?;"),
        "(ExprStmt (SDeref * (Try (Member x . (MemberMethod y ( ))) ?)) ;)"
    );
    assert_eq!(stmt("a as u32 as u64;"), "(ExprStmt (SCast (SCast a as u32) as u64) ;)");
}

/// `>>`, `>=` and `>>=` are told apart at their first character, and in
/// a type each `>` closes a generic list.
#[test]
fn joined_punctuation_is_one_operator_and_closes_generics_one_by_one() {
    assert_eq!(stmt("a >> b >= c;"), "(ExprStmt (SGe (SShr a > > b) > = c) ;)");
    assert_eq!(stmt("x >>= 1;"), "(ExprStmt (SShrAssign x > > = 1) ;)");
    assert_eq!(stmt("a << b <= c;"), "(ExprStmt (SLe (SShl a < < b) < = c) ;)");
    assert_eq!(
        stmt("let v: Vec<Vec<u8>>= w;"),
        "(LetStmt let v (TypeAnn : (TArgsName Vec (GenericArgs < (TArgsName Vec (GenericArgs < u8 >)) >))) (LetInit = w) ;)"
    );
    assert_eq!(stmt("f::<Vec<u8>>();"), "(ExprStmt (SCall (Turbofish f (PathSep : :) (GenericArgs < (TArgsName Vec (GenericArgs < u8 >)) >)) ( )) ;)");
    // `&&` in an operand's place is two references.
    assert_eq!(stmt("&&x;"), "(ExprStmt (SRef & (Ref & x)) ;)");
    assert_eq!(stmt("a && &b;"), "(ExprStmt (SAnd a & & (Ref & b)) ;)");
    assert_eq!(stmt("a..=b;"), "(ExprStmt (SRangeIncl a . . = b) ;)");
}

/// Comparisons and ranges do not chain; a type after `&` takes one bound.
#[test]
fn what_rustc_refuses_is_refused() {
    refused("fn f() { a < b < c; }");
    refused("fn f() { a == b == c; }");
    refused("fn f() { a..b..c; }");
    refused("fn f(x: &dyn A + B) {}");
    // `as` then `<` begins generic arguments, as rustc reads it.
    refused("fn f() { a as u8 < b; }");
}

/// In a condition a path before `{` is the path, and the brace begins the
/// block (no struct literal); elsewhere it is a struct literal.
#[test]
fn a_condition_takes_no_struct_literal() {
    assert_eq!(
        stmt("if x == S {} else {}"),
        "(IfElse if (IfThen (CEq x = = S) (Block { })) else (Block { }))"
    );
    assert_eq!(
        stmt("let s = S { a: 1 };"),
        "(LetStmt let s (LetInit = (StructLit S { (FieldInit a : 1) })) ;)"
    );
    assert_eq!(stmt("for i in 0.. {}"), "(For for i in (CRangeFrom 0 . .) (Block { }))");
    assert_eq!(
        stmt("let r = 0..{5};"),
        "(LetStmt let r (LetInit = (Range 0 . . (Block { 5 }))) ;)"
    );
}

/// A block-like expression that begins a statement ends it, unless `.`
/// or `?` continues it.
#[test]
fn a_block_like_expression_ends_its_statement() {
    assert_eq!(stmt("{ 1 } - 1;"), "(Block { 1 }) (ExprStmt (SNeg - 1) ;)");
    assert_eq!(
        stmt("match x {}.len();"),
        "(ExprStmt (SBlockMember (Match match x { }) . (MemberMethod len ( ))) ;)"
    );
    assert_eq!(
        stmt("if a {} else {}?;"),
        "(ExprStmt (SBlockTry (IfElse if (IfThen a (Block { })) else (Block { })) ?) ;)"
    );
    // In an operand's place it is an operand.
    assert_eq!(
        stmt("let x = if a { 1 } else { 2 } + 1;"),
        "(LetStmt let x (LetInit = (Add (IfElse if (IfThen a (Block { 1 })) else (Block { 2 })) + 1)) ;)"
    );
}

#[test]
fn let_chains_and_let_else() {
    assert_eq!(
        stmt("if let Some(a) = x && a > 0 {}"),
        "(IfNoElse if (IfThen (CAnd (CLet let (TupleStructPat Some ( a )) = x) & & (CGt a > 0)) (Block { })))"
    );
    assert_eq!(
        stmt("let Some(x) = y else { return };"),
        "(LetStmt let (TupleStructPat Some ( x )) (LetInitElse = y else (Block { return })) ;)"
    );
    // `else` after `if` belongs to the `if`.
    assert_eq!(
        stmt("let x = if a { b } else { c };"),
        "(LetStmt let x (LetInit = (IfElse if (IfThen a (Block { b })) else (Block { c }))) ;)"
    );
}

#[test]
fn closures_and_jumps_reach_as_far_right_as_they_can() {
    assert_eq!(
        stmt("let f = |x| x + 1;"),
        "(LetStmt let f (LetInit = (Closure (ClosureParams | x |) (Add x + 1))) ;)"
    );
    assert_eq!(stmt("let f = move || -> u8 { 1 };"), "(LetStmt let f (LetInit = (ClosureRet (MoveClosure move (ClosureNoParams | |)) - > u8 (Block { 1 }))) ;)");
    assert_eq!(stmt("return a - 1;"), "(ExprStmt (SReturnValue return (Sub a - 1)) ;)");
    assert_eq!(stmt("break 'outer;"), "(ExprStmt (SBreakLabel break 'outer) ;)");
}

#[test]
fn paths_patterns_and_fields() {
    assert_eq!(
        stmt("<T as Tr>::f();"),
        "(ExprStmt (SCall (QualPath (QSelfAs < T as Tr >) (PathSep : :) f) ( )) ;)"
    );
    assert_eq!(stmt("x.0.1;"), "(ExprStmt (SMember x . 0.1) ;)");
    assert_eq!(stmt("let &mut x = y;"), "(LetStmt let (RefMutPat & mut x) (LetInit = y) ;)");
    assert_eq!(
        stmt("match v { A | B => 1, 0..=9 => 2, _ => 3 }"),
        "(Match match v { (ArmsLast (ArmExpr (Pat A | B) = > 1 ,) (ArmExpr (RangeInclPat 0 . . = 9) = > 2 ,) (ArmLast _ = > 3)) })"
    );
}

/// `union`, `auto`, `default`, `raw` and `safe` are keywords only where
/// they act as one.
#[test]
fn weak_keywords_are_keywords_only_where_they_act_as_one() {
    let p = parses("union U { a: u8 }\nauto trait T {}\nfn f() { a.union(b); Default::default(); let p = &raw const x; }");
    assert!(p.contains("(Union union U"), "{p}");
    assert!(p.contains("(AutoTrait auto trait)"), "{p}");
    assert!(p.contains("(SMember a . (MemberMethod union ( b )))"), "{p}");
    assert!(p.contains("(RawConst & raw const x)"), "{p}");
}

#[test]
fn items() {
    let p = parses("pub(crate) struct S(pub (u8, u8), pub(crate) u8);");
    assert!(p.contains("(TupleFieldPubParen pub ( (TupleType"), "{p}");
    assert!(p.contains("(TupleFieldPubCrate pub ( crate ) u8)"), "{p}");
    parses("impl<T> Tr for Vec<T> where T: Clone {}");
    parses("unsafe extern \"C\" { safe fn f(); pub static X: u8; }");
    parses("macro_rules! m { ($x:expr) => { $x }; }");
    parses("#![allow(x)]\n#[derive(Debug)]\npub enum E { A, B(u8), C { x: u8 } = 3 }");
    parses("fn f<'a, T: ?Sized + 'a, const N: usize>(x: &'a T) -> impl Fn(u8) -> u8 + 'a where for<'b> &'b T: Tr {}");
}

/// What third-party crates showed the first draft missing.
#[test]
fn what_the_crates_registry_found() {
    // After `&` any pattern but a range; after `&mut` one beginning `mut`.
    assert_eq!(stmt("let f = |&mut mut x| x;"), "(LetStmt let f (LetInit = (Closure (ClosureParams | (RefMutPat & mut (MutIdentPat mut x)) |) x)) ;)");
    assert!(stmt("if let Some(&c @ b'0'..=b'9') = s {}")
        .contains("(RefPat & (BindPat c @ (RangeInclPat b'0' . . = b'9')))"));
    assert!(stmt("match t { (11, ..26) => {} _ => {} }").contains("(RangeToExclPat . . 26)"));
    // Outer attributes on call arguments, array and tuple elements.
    assert_eq!(stmt("f(#[cfg(unix)] a, b);"), "(ExprStmt (SCall f ( (Args (AttrElem (OuterAttr # [ (Attr cfg (ParenTt ( unix ))) ]) a) , b) )) ;)");
    // An inherent impl on a trait object with bounds.
    parses("impl dyn Io + Send {}");
    // `impl <` begins generics; an impl on a qualified path with none is
    // written with an empty list (the grammar's header says why).
    refused("impl <T as A>::B {}");
    parses("impl<> <T as A>::B {}");
    parses("extern crate self as me;");
}

/// What rust-analyzer's parser tests of valid syntax showed missing.
#[test]
fn what_rust_analyzers_tests_found() {
    // `0.` after `x.` is a tuple index and a `.`, split as rustc splits it.
    assert_eq!(stmt("x.0. await;"), "(ExprStmt (SSplitMember x . 0. await) ;)");
    // A struct expression ending in `..`, for destructuring assignment.
    assert_eq!(
        stmt("S { a, .. } = s;"),
        "(ExprStmt (SAssign (StructLit S { (StructFieldsRest a , . .) }) = s) ;)"
    );
    // A `|` before the first alternative, inside a pattern.
    assert_eq!(
        stmt("let [| a, ..] = s;"),
        "(LetStmt let (SlicePat [ (Pats (LeadPipePat | a) , (RestPat . .)) ]) (LetInit = s) ;)"
    );
    assert!(stmt("let S { #[cfg(x)] .. } = s;").contains("(PatFieldsOnlyRest (OuterAttr"));
    // `::` before generic or parenthesized arguments in a type.
    parses("type T = Foo<Item::<T> = U, Item::<T>: Bound>;");
    parses("type F = Box<dyn Fn::(u8) -> u8>;");
}
