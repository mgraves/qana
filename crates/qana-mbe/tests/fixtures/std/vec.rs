// Copied verbatim from the Rust standard library's source (MIT OR Apache-2.0),
// as rustup's `rust-src` has it for nightly 1.94.0 (f52090008 2025-12-10):
// - library/alloc/src/macros.rs lines 37-56

#[cfg(not(no_global_oom_handling))]
#[macro_export]
#[stable(feature = "rust1", since = "1.0.0")]
#[rustc_diagnostic_item = "vec_macro"]
#[allow_internal_unstable(rustc_attrs, liballoc_internals)]
macro_rules! vec {
    () => (
        $crate::vec::Vec::new()
    );
    ($elem:expr; $n:expr) => (
        $crate::vec::from_elem($elem, $n)
    );
    ($($x:expr),+ $(,)?) => (
        <[_]>::into_vec(
            // Using the intrinsic produces a dramatic improvement in stack usage for
            // unoptimized programs using this code path to construct large Vecs.
            $crate::boxed::box_new([$($x),+])
        )
    );
}
