// A macro exported with `local_inner_macros` names its helpers alone,
// and rustc resolves each as `$crate::name!`: the crate's exported
// `helper!`, which a `helper!` in the caller's textual scope does not
// shadow. A name the caller passes in is resolved where the caller is.

#[macro_export(local_inner_macros)]
macro_rules! outer {
    ($($t:tt)*) => { helper!($($t)*) };
}

#[macro_export(local_inner_macros)]
macro_rules! calls {
    ($m:ident) => { ($m!(2), helper!(3)) };
}

#[macro_export]
macro_rules! helper {
    ($x:expr) => { $x + 1 };
}

fn main() {
    macro_rules! helper {
        ($x:expr) => { $x * 100 };
    }
    let _ = outer!(1);
    let _ = calls!(helper);
    let _ = helper!(4);
}
