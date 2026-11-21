// Fragments: each kind, matched and transcribed.
macro_rules! k_item { ($i:item) => { $i }; }
macro_rules! k_block { ($b:block) => { fn from_block() $b }; }
macro_rules! k_stmt { ($s:stmt) => { fn from_stmt() { $s; } }; }
macro_rules! k_pat { ($p:pat) => { fn from_pat(x: Option<u8>) { if let $p = x {} } }; }
macro_rules! k_pat_param { ($p:pat_param | $q:pat_param) => { fn from_pat_param(x: u8) { match x { $p | $q => {} _ => {} } } }; }
macro_rules! k_expr { ($e:expr) => { const FROM_EXPR: i32 = $e * 2; }; }
macro_rules! k_ty { ($t:ty) => { type FromTy = $t; }; }
macro_rules! k_ident { ($i:ident) => { fn $i() {} }; }
macro_rules! k_path { ($p:path) => { type FromPath = $p; }; }
macro_rules! k_tt { ($($t:tt)*) => { $($t)* }; }
macro_rules! k_meta { ($m:meta) => { #[$m] fn from_meta() {} }; }
macro_rules! k_lifetime { ($l:lifetime) => { fn from_lifetime<$l>(x: &$l u8) -> &$l u8 { x } }; }
macro_rules! k_vis { ($v:vis $n:ident) => { $v fn $n() {} }; }
macro_rules! k_literal { ($l:literal) => { const FROM_LITERAL: i32 = $l; }; }

// A statement is whole: it needs no `;`, as a `let`, an expression that
// ends its block (its tail), or one before another statement.
macro_rules! s_tail { ($s:stmt) => { fn s_tail() -> i32 { $s } }; }
macro_rules! s_let_end { ($s:stmt) => { fn s_let_end() { $s } }; }
macro_rules! s_then { ($s:stmt) => { fn s_then() { $s let _y = 2; } }; }
macro_rules! s_empty { ($s:stmt) => { fn s_empty() { $s } }; }
s_tail!(1 + 1);
s_let_end!(let _x = 1);
s_then!(let _x = 1);
s_empty!(;);

k_item!(struct FromItem;);
k_block!({ let _ = 1; });
k_stmt!(let _x = 3);
k_pat!(Some(1 | 2) | None);
k_pat_param!(1 | 2);
k_expr!(1 + 1);
k_ty!(Vec<Vec<u8>>);
k_ident!(from_ident);
k_path!(std::collections::HashMap<u8, u8>);
k_tt!(fn from_tt() -> u8 { 1 });
k_meta!(inline(always));
k_lifetime!('a);
k_vis!(pub(crate) from_vis);
k_vis!(no_vis);
k_literal!(-1);

// Repetitions: `*`, `+`, `?`, separators, nesting, lockstep.
macro_rules! reps {
    ($($a:ident = $b:expr),* ; $($c:ident)+ ; $(# $d:literal)?) => {
        $(const $a: i32 = $b;)*
        $(fn $c() {})+
        $(const D: i32 = $d;)?
    };
}
reps!(A = 1, B = 2 ; c1 c2 ; # 3);
reps!( ; c3 ; );
macro_rules! nested {
    ($($name:ident : $($v:literal),*);*) => {
        $(const $name: &[i32] = &[$($v),*];)*
    };
}
nested!(N1: 1, 2; N2: ; N3: 4);
macro_rules! sep_arrow { ($($x:ident)=>*) => { $(fn $x() {})* }; }
sep_arrow!(s1 => s2 => s3);

// `$crate`, and a metavariable the matcher does not bind, kept as written.
macro_rules! with_crate { () => { $crate::helper() }; }
fn helper() -> u8 { 0 }
macro_rules! dollar_kept { ($x:ident) => { stringify!($y $x) }; }

// Forwarding: an `expr` is one node to the macro it is passed to; a `tt`
// is its tokens.
macro_rules! inner_lit { ($l:literal) => { $l }; }
macro_rules! outer_expr { ($e:expr) => { inner_lit!($e) }; }
macro_rules! inner_three { (3) => { 3 }; }
macro_rules! outer_tt { ($t:tt) => { inner_three!($t) }; }
macro_rules! double { ($e:expr) => { $e * 2 }; }
// Written bare, `x as u8 < 3` would read `u8<3` as a type's generics: the
// cast is one node, before `<` in an expansion and in what a macro is
// passed.
macro_rules! lt3 { ($e:expr) => { $e < 3 }; }
macro_rules! take { ($x:expr) => { $x }; }
macro_rules! fwd_lt { ($e:expr) => { take!($e < 3) }; }

// Splitting `>>` where a type ends, and a type followed by `>`.
macro_rules! ty_then_gt { ($t:ty >) => { type Split = $t; }; }
ty_then_gt!(Vec<u8>>);

// Recursion, an incremental muncher.
macro_rules! count {
    () => { 0usize };
    ($head:tt $($tail:tt)*) => { 1usize + count!($($tail)*) };
}

// A macro that defines one, seen after it.
macro_rules! make { ($n:ident) => { macro_rules! $n { () => { 7 } } }; }
make!(seven);

// Doc comments in an invocation are attributes; in a matcher, nothing.
macro_rules! docs { ($(#[$m:meta])* fn $n:ident) => { $(#[$m])* fn $n() {} }; }
docs! {
    /// One.
    /** Two. */
    fn documented
}
macro_rules! doc_in_matcher { (/// ignored
    $x:ident) => { fn $x() {} }; }
doc_in_matcher!(undocumented);

fn main() {
    let _ = with_crate!();
    let _ = dollar_kept!(x);
    let _ = outer_expr!(3);
    let _ = outer_tt!(3);
    let _ = double!(1 + 1);
    let x = 1u32;
    let _ = lt3!(x as u8);
    let _ = fwd_lt!(x as u8);
    let _ = count!(a b c);
    let _ = seven!();
    from_block();
    from_stmt();
    from_pat(None);
    from_pat_param(1);
}
