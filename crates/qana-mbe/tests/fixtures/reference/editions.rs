macro_rules! e { ($e:expr) => { 1 }; ($($t:tt)*) => { 2 }; }
macro_rules! e21 { ($e:expr_2021) => { 1 }; ($($t:tt)*) => { 2 }; }
macro_rules! p { ($p:pat) => { 1 }; ($($t:tt)*) => { 2 }; }
fn main() {
    let _ = e!(_);
    let _ = e!(const { 1 });
    let _ = e21!(_);
    let _ = p!(1 | 2);
}
