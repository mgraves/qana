// Copied verbatim from the Rust standard library's source (MIT OR Apache-2.0),
// as rustup's `rust-src` has it for nightly 1.94.0 (f52090008 2025-12-10):
// - library/std/src/thread/local.rs lines 140-348
// - library/std/src/thread/local.rs lines 395-405
// - library/std/src/sys/thread_local/native/mod.rs lines 41-111

#[doc(hidden)]
#[allow_internal_unstable(thread_local_internals)]
#[unstable(feature = "thread_local_internals", issue = "none")]
#[rustc_macro_transparency = "semitransparent"]
pub macro thread_local_process_attrs {

    // Parse `cfg_attr` to figure out whether it's a `rustc_align_static`.
    // Each `cfg_attr` can have zero or more attributes on the RHS, and can be nested.

    // finished parsing the `cfg_attr`, it had no `rustc_align_static`
    (
        [] [$(#[$($prev_other_attrs:tt)*])*];
        @processing_cfg_attr { pred: ($($predicate:tt)*), rhs: [] };
        [$($prev_align_attrs_ret:tt)*] [$($prev_other_attrs_ret:tt)*];
        $($rest:tt)*
    ) => (
        $crate::thread::local_impl::thread_local_process_attrs!(
            [$($prev_align_attrs_ret)*] [$($prev_other_attrs_ret)* #[cfg_attr($($predicate)*, $($($prev_other_attrs)*),*)]];
            $($rest)*
        );
    ),

    // finished parsing the `cfg_attr`, it had nothing but `rustc_align_static`
    (
        [$(#[$($prev_align_attrs:tt)*])+] [];
        @processing_cfg_attr { pred: ($($predicate:tt)*), rhs: [] };
        [$($prev_align_attrs_ret:tt)*] [$($prev_other_attrs_ret:tt)*];
        $($rest:tt)*
    ) => (
        $crate::thread::local_impl::thread_local_process_attrs!(
            [$($prev_align_attrs_ret)*  #[cfg_attr($($predicate)*, $($($prev_align_attrs)*),+)]] [$($prev_other_attrs_ret)*];
            $($rest)*
        );
    ),

    // finished parsing the `cfg_attr`, it had a mix of `rustc_align_static` and other attrs
    (
        [$(#[$($prev_align_attrs:tt)*])+] [$(#[$($prev_other_attrs:tt)*])+];
        @processing_cfg_attr { pred: ($($predicate:tt)*), rhs: [] };
        [$($prev_align_attrs_ret:tt)*] [$($prev_other_attrs_ret:tt)*];
        $($rest:tt)*
    ) => (
        $crate::thread::local_impl::thread_local_process_attrs!(
            [$($prev_align_attrs_ret)*  #[cfg_attr($($predicate)*, $($($prev_align_attrs)*),+)]] [$($prev_other_attrs_ret)* #[cfg_attr($($predicate)*, $($($prev_other_attrs)*),+)]];
            $($rest)*
        );
    ),

    // it's a `rustc_align_static`
    (
        [$($prev_align_attrs:tt)*] [$($prev_other_attrs:tt)*];
        @processing_cfg_attr { pred: ($($predicate:tt)*), rhs: [rustc_align_static($($align_static_args:tt)*) $(, $($attr_rhs:tt)*)?] };
        $($rest:tt)*
    ) => (
        $crate::thread::local_impl::thread_local_process_attrs!(
            [$($prev_align_attrs)* #[rustc_align_static($($align_static_args)*)]] [$($prev_other_attrs)*];
            @processing_cfg_attr { pred: ($($predicate)*), rhs: [$($($attr_rhs)*)?] };
            $($rest)*
        );
    ),

    // it's a nested `cfg_attr(true, ...)`; recurse into RHS
    (
        [$($prev_align_attrs:tt)*] [$($prev_other_attrs:tt)*];
        @processing_cfg_attr { pred: ($($predicate:tt)*), rhs: [cfg_attr(true, $($cfg_rhs:tt)*) $(, $($attr_rhs:tt)*)?] };
        $($rest:tt)*
    ) => (
        $crate::thread::local_impl::thread_local_process_attrs!(
            [] [];
            @processing_cfg_attr { pred: (true), rhs: [$($cfg_rhs)*] };
            [$($prev_align_attrs)*] [$($prev_other_attrs)*];
            @processing_cfg_attr { pred: ($($predicate)*), rhs: [$($($attr_rhs)*)?] };
            $($rest)*
        );
    ),

    // it's a nested `cfg_attr(false, ...)`; recurse into RHS
    (
        [$($prev_align_attrs:tt)*] [$($prev_other_attrs:tt)*];
        @processing_cfg_attr { pred: ($($predicate:tt)*), rhs: [cfg_attr(false, $($cfg_rhs:tt)*) $(, $($attr_rhs:tt)*)?] };
        $($rest:tt)*
    ) => (
        $crate::thread::local_impl::thread_local_process_attrs!(
            [] [];
            @processing_cfg_attr { pred: (false), rhs: [$($cfg_rhs)*] };
            [$($prev_align_attrs)*] [$($prev_other_attrs)*];
            @processing_cfg_attr { pred: ($($predicate)*), rhs: [$($($attr_rhs)*)?] };
            $($rest)*
        );
    ),


    // it's a nested `cfg_attr(..., ...)`; recurse into RHS
    (
        [$($prev_align_attrs:tt)*] [$($prev_other_attrs:tt)*];
        @processing_cfg_attr { pred: ($($predicate:tt)*), rhs: [cfg_attr($cfg_lhs:meta, $($cfg_rhs:tt)*) $(, $($attr_rhs:tt)*)?] };
        $($rest:tt)*
    ) => (
        $crate::thread::local_impl::thread_local_process_attrs!(
            [] [];
            @processing_cfg_attr { pred: ($cfg_lhs), rhs: [$($cfg_rhs)*] };
            [$($prev_align_attrs)*] [$($prev_other_attrs)*];
            @processing_cfg_attr { pred: ($($predicate)*), rhs: [$($($attr_rhs)*)?] };
            $($rest)*
        );
    ),

    // it's some other attribute
    (
        [$($prev_align_attrs:tt)*] [$($prev_other_attrs:tt)*];
        @processing_cfg_attr { pred: ($($predicate:tt)*), rhs: [$meta:meta $(, $($attr_rhs:tt)*)?] };
        $($rest:tt)*
    ) => (
        $crate::thread::local_impl::thread_local_process_attrs!(
            [$($prev_align_attrs)*] [$($prev_other_attrs)* #[$meta]];
            @processing_cfg_attr { pred: ($($predicate)*), rhs: [$($($attr_rhs)*)?] };
            $($rest)*
        );
    ),


    // Separate attributes into `rustc_align_static` and everything else:

    // `rustc_align_static` attribute
    ([$($prev_align_attrs:tt)*] [$($prev_other_attrs:tt)*]; #[rustc_align_static $($attr_rest:tt)*] $($rest:tt)*) => (
        $crate::thread::local_impl::thread_local_process_attrs!(
            [$($prev_align_attrs)* #[rustc_align_static $($attr_rest)*]] [$($prev_other_attrs)*];
            $($rest)*
        );
    ),

    // `cfg_attr(true, ...)` attribute; parse it
    ([$($prev_align_attrs:tt)*] [$($prev_other_attrs:tt)*]; #[cfg_attr(true, $($cfg_rhs:tt)*)] $($rest:tt)*) => (
        $crate::thread::local_impl::thread_local_process_attrs!(
            [] [];
            @processing_cfg_attr { pred: (true), rhs: [$($cfg_rhs)*] };
            [$($prev_align_attrs)*] [$($prev_other_attrs)*];
            $($rest)*
        );
    ),

    // `cfg_attr(false, ...)` attribute; parse it
    ([$($prev_align_attrs:tt)*] [$($prev_other_attrs:tt)*]; #[cfg_attr(false, $($cfg_rhs:tt)*)] $($rest:tt)*) => (
        $crate::thread::local_impl::thread_local_process_attrs!(
            [] [];
            @processing_cfg_attr { pred: (false), rhs: [$($cfg_rhs)*] };
            [$($prev_align_attrs)*] [$($prev_other_attrs)*];
            $($rest)*
        );
    ),

    // `cfg_attr(..., ...)` attribute; parse it
    ([$($prev_align_attrs:tt)*] [$($prev_other_attrs:tt)*]; #[cfg_attr($cfg_pred:meta, $($cfg_rhs:tt)*)] $($rest:tt)*) => (
        $crate::thread::local_impl::thread_local_process_attrs!(
            [] [];
            @processing_cfg_attr { pred: ($cfg_pred), rhs: [$($cfg_rhs)*] };
            [$($prev_align_attrs)*] [$($prev_other_attrs)*];
            $($rest)*
        );
    ),

    // doc comment not followed by any other attributes; process it all at once to avoid blowing recursion limit
    ([$($prev_align_attrs:tt)*] [$($prev_other_attrs:tt)*]; $(#[doc $($doc_rhs:tt)*])+ $vis:vis static $($rest:tt)*) => (
        $crate::thread::local_impl::thread_local_process_attrs!(
            [$($prev_align_attrs)*] [$($prev_other_attrs)* $(#[doc $($doc_rhs)*])+];
            $vis static $($rest)*
        );
    ),

    // 8 lines of doc comment; process them all at once to avoid blowing recursion limit
    ([$($prev_align_attrs:tt)*] [$($prev_other_attrs:tt)*];
     #[doc $($doc_rhs_1:tt)*] #[doc $($doc_rhs_2:tt)*] #[doc $($doc_rhs_3:tt)*] #[doc $($doc_rhs_4:tt)*]
     #[doc $($doc_rhs_5:tt)*] #[doc $($doc_rhs_6:tt)*] #[doc $($doc_rhs_7:tt)*] #[doc $($doc_rhs_8:tt)*]
     $($rest:tt)*) => (
        $crate::thread::local_impl::thread_local_process_attrs!(
            [$($prev_align_attrs)*] [$($prev_other_attrs)*
            #[doc $($doc_rhs_1)*] #[doc $($doc_rhs_2)*] #[doc $($doc_rhs_3)*] #[doc $($doc_rhs_4)*]
            #[doc $($doc_rhs_5)*] #[doc $($doc_rhs_6)*] #[doc $($doc_rhs_7)*] #[doc $($doc_rhs_8)*]];
            $($rest)*
        );
    ),

    // other attribute
    ([$($prev_align_attrs:tt)*] [$($prev_other_attrs:tt)*]; #[$($attr:tt)*] $($rest:tt)*) => (
        $crate::thread::local_impl::thread_local_process_attrs!(
            [$($prev_align_attrs)*] [$($prev_other_attrs)* #[$($attr)*]];
            $($rest)*
        );
    ),


    // Delegate to `thread_local_inner` once attributes are fully categorized:

    // process `const` declaration and recurse
    ([$($align_attrs:tt)*] [$($other_attrs:tt)*]; $vis:vis static $name:ident: $t:ty = const $init:block $(; $($($rest:tt)+)?)?) => (
        $($other_attrs)* $vis const $name: $crate::thread::LocalKey<$t> =
            $crate::thread::local_impl::thread_local_inner!(@key $t, $($align_attrs)*, const $init);

        $($($crate::thread::local_impl::thread_local_process_attrs!([] []; $($rest)+);)?)?
    ),

    // process non-`const` declaration and recurse
    ([$($align_attrs:tt)*] [$($other_attrs:tt)*]; $vis:vis static $name:ident: $t:ty = $init:expr $(; $($($rest:tt)+)?)?) => (
        $($other_attrs)* $vis const $name: $crate::thread::LocalKey<$t> =
            $crate::thread::local_impl::thread_local_inner!(@key $t, $($align_attrs)*, $init);

        $($($crate::thread::local_impl::thread_local_process_attrs!([] []; $($rest)+);)?)?
    ),
}

#[macro_export]
#[stable(feature = "rust1", since = "1.0.0")]
#[cfg_attr(not(test), rustc_diagnostic_item = "thread_local_macro")]
#[allow_internal_unstable(thread_local_internals)]
macro_rules! thread_local {
    () => {};

    ($($tt:tt)+) => {
        $crate::thread::local_impl::thread_local_process_attrs!([] []; $($tt)+);
    };
}

#[doc(hidden)]
#[allow_internal_unstable(
    thread_local_internals,
    cfg_target_thread_local,
    thread_local,
    never_type
)]
#[allow_internal_unsafe]
#[unstable(feature = "thread_local_internals", issue = "none")]
#[rustc_macro_transparency = "semitransparent"]
pub macro thread_local_inner {
    // NOTE: we cannot import `LocalKey`, `LazyStorage` or `EagerStorage` with a `use` because that
    // can shadow user provided type or type alias with a matching name. Please update the shadowing
    // test in `tests/thread.rs` if these types are renamed.

    // Used to generate the `LocalKey` value for const-initialized thread locals.
    (@key $t:ty, $(#[$align_attr:meta])*, const $init:expr) => {{
        const __RUST_STD_INTERNAL_INIT: $t = $init;

        unsafe {
            $crate::thread::LocalKey::new(const {
                if $crate::mem::needs_drop::<$t>() {
                    |_| {
                        #[thread_local]
                        $(#[$align_attr])*
                        static __RUST_STD_INTERNAL_VAL: $crate::thread::local_impl::EagerStorage<$t>
                            = $crate::thread::local_impl::EagerStorage::new(__RUST_STD_INTERNAL_INIT);
                        __RUST_STD_INTERNAL_VAL.get()
                    }
                } else {
                    |_| {
                        #[thread_local]
                        $(#[$align_attr])*
                        static __RUST_STD_INTERNAL_VAL: $t = __RUST_STD_INTERNAL_INIT;
                        &__RUST_STD_INTERNAL_VAL
                    }
                }
            })
        }
    }},

    // used to generate the `LocalKey` value for `thread_local!`
    (@key $t:ty, $(#[$align_attr:meta])*, $init:expr) => {{
        #[inline]
        fn __rust_std_internal_init_fn() -> $t {
            $init
        }

        unsafe {
            $crate::thread::LocalKey::new(const {
                if $crate::mem::needs_drop::<$t>() {
                    |__rust_std_internal_init| {
                        #[thread_local]
                        $(#[$align_attr])*
                        static __RUST_STD_INTERNAL_VAL: $crate::thread::local_impl::LazyStorage<$t, ()>
                            = $crate::thread::local_impl::LazyStorage::new();
                        __RUST_STD_INTERNAL_VAL.get_or_init(__rust_std_internal_init, __rust_std_internal_init_fn)
                    }
                } else {
                    |__rust_std_internal_init| {
                        #[thread_local]
                        $(#[$align_attr])*
                        static __RUST_STD_INTERNAL_VAL: $crate::thread::local_impl::LazyStorage<$t, !>
                            = $crate::thread::local_impl::LazyStorage::new();
                        __RUST_STD_INTERNAL_VAL.get_or_init(__rust_std_internal_init, __rust_std_internal_init_fn)
                    }
                }
            })
        }
    }},
}
