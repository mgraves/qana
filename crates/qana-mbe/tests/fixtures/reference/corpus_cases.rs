// What expanding the corpus taught, each case a crate's pattern in small.
#![feature(stmt_expr_attributes)]

// An expression macro's expansion and an expression fragment may begin
// with attributes (portable-simd, nalgebra).
macro_rules! with_attr { ($e:expr) => { #[allow(unused_parens)] ($e) }; }
macro_rules! take_expr { ($e:expr) => { $e }; }
fn with_attrs() -> i32 { with_attr!(1) + take_expr!(#[allow(unused_parens)] (2)) }

// An invocation that ends a statement macro's expansion is a statement
// macro too (glam).
macro_rules! inner_stmts { ($x:ident) => { let $x = 1; let _ = $x; }; }
macro_rules! outer_stmts { ($x:ident) => { inner_stmts!($x) }; }
fn trailing() { outer_stmts!(y); }

// `$($s:stmt)*` takes each `;` as a statement of its own (axum).
macro_rules! stmts { ($($s:stmt)*) => { fn stmts() { $($s)* } }; }
stmts! { let a = 1; let _b = a; }

// A visibility in a tuple struct's field (zerocopy).
macro_rules! tuple_struct { ($v:vis $t:ty) => { pub struct TupleStruct($v $t); }; }
tuple_struct!(pub [u8; 2]);

// Attributes written before a forwarded item are its own (core's tuples).
macro_rules! doc_item { (#[$m:meta] $i:item) => { #[doc = "x"] #[$m] $i }; }
doc_item!(#[derive(Clone)] #[allow(dead_code)] struct DocItem;);

// A type before `<<` is one node (rand).
macro_rules! shift { ($t:ty) => { const SHIFT: u32 = 1 as $t << 3; }; }
shift!(u32);

fn main() {
    let _ = with_attrs();
    trailing();
    stmts();
}
