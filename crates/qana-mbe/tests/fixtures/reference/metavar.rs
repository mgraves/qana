#![feature(macro_metavar_expr, macro_metavar_expr_concat)]

macro_rules! counts {
    ($($x:ident),*) => {
        const N: usize = ${count($x)};
        $(const ${concat(C_, $x)}: usize = ${index()};)*
    };
}
counts!(a, b, c);

macro_rules! lens {
    ($($x:ident)*) => { $(const ${concat($x, _LEN)}: usize = ${len()}; ${ignore($x)})* };
}
lens!(p q);

macro_rules! nested_count {
    ($($($x:ident)*);*) => {
        const NC0: usize = ${count($x)};
        const NC1: usize = ${count($x, 1)};
    };
}
nested_count!(a b; c);

macro_rules! from_literal { ($l:literal) => { const ${concat(V, $l)}: u8 = 0; }; }
from_literal!(5);
from_literal!("x");

macro_rules! dollar_dollar { () => { macro_rules! made { ($$x:ident) => { $$x } } }; }
dollar_dollar!();

fn main() {}
