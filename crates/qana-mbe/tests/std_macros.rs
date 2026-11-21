//! std's macros expand to what rustc expands them to.
//!
//! `tests/fixtures/std/` holds the definitions, copied from the standard library's
//! source for nightly 1.94.0 (f52090008 2025-12-10), programs that invoke
//! them, and what `rustc +nightly -Ztrace-macros` printed for those
//! programs: each `macro_rules!` (and `macro`) expansion it made, the
//! tokens it was given and the tokens it wrote. Each program is expanded
//! here, every invocation in turn, and every expansion made must be one
//! rustc made, token for token; every one rustc made must be made, but for
//! those inside a built-in macro's expansion (which no `macro_rules!`
//! defines).

mod common;

use std::collections::HashMap;

use common::{check, definition, trace};
use qana_mbe::Form;

#[test]
fn vec_assert_eq_and_thread_local_expand_as_rustc_expands_them() {
    let vec_rs = include_str!("fixtures/std/vec.rs");
    let assert_rs = include_str!("fixtures/std/assert_eq.rs");
    let tl_rs = include_str!("fixtures/std/thread_local.rs");
    let mut defs = HashMap::new();
    defs.insert("vec", definition(vec_rs, 10, "macro_rules!", "vec", Form::MacroRules));
    defs.insert(
        "assert_eq",
        definition(assert_rs, 11, "macro_rules!", "assert_eq", Form::MacroRules),
    );
    defs.insert(
        "thread_local",
        definition(tl_rs, 12, "macro_rules!", "thread_local", Form::MacroRules),
    );
    defs.insert(
        "thread_local_process_attrs",
        definition(tl_rs, 12, "pub macro", "thread_local_process_attrs", Form::Macro),
    );
    defs.insert(
        "thread_local_inner",
        definition(tl_rs, 12, "pub macro", "thread_local_inner", Form::Macro),
    );
    let made = check(
        include_str!("fixtures/std/probe_std.rs"),
        &defs,
        trace(include_str!("fixtures/std/probe_std.trace")),
        &[],
        &["format_args"],
    );
    assert_eq!(made, 18);
}

#[test]
fn uint_impl_expands_as_rustc_expands_it() {
    let program =
        format!("{}{}", include_str!("fixtures/std/uint_impl.rs"), include_str!("fixtures/std/probe_uint.rs"));
    let made = check(
        &program,
        &HashMap::new(),
        trace(include_str!("fixtures/std/probe_uint.trace")),
        // `panic_2021!` is what the built-in `assert!` expands to.
        &["panic_2021"],
        // `stringify!` stands in `concat!`'s arguments and `cfg!` in doc
        // comments: neither is where rustc reads an invocation.
        &["assert", "assert_unsafe_precondition", "concat", "try_opt"],
    );
    assert_eq!(made, 7);
}
