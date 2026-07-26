//! QANA — a language-building toolchain.
//!
//! A language designed inside qana's **language-shape envelope** gets
//! incremental parsing, editor intelligence, error containment, lossless
//! trees, and parallel cold parsing **by construction**. Grammars outside
//! the envelope are refused, with counterexamples — the refusal is the
//! feature, because it is what lets everything downstream be guaranteed
//! rather than attempted.
//!
//! # This crate is a facade
//!
//! It contains no logic. Every item below is a re-export of one of the
//! family crates, gathered so that `qana::` is a single coherent
//! namespace and a release moves one version number instead of six. The
//! individual crates remain independently usable; depend on them
//! directly if you want a narrower tree than even the features below
//! give you.
//!
//! | module | crate | what it is |
//! |---|---|---|
//! | [`grammar`] | `qana-grammar` | grammar-as-value, pattern→DFA, LR tables, the envelope lints |
//! | [`engine`] | `qana-engine` | incremental line-lexing and parsing over compiled grammars |
//! | [`sem`] | `qana-sem` | binding, types, macros; revisioned queries with signature firewalls |
//! | [`services`] | `qana-services` | semantic tokens, folding, outline, completion, diagnostics |
//! | [`lang`] | `qana-lang` | the `.qana` grammar surface, self-hosted on the engine above |
//! | [`linework`] | `linework` | the editor protocol: line-keyed paint and facts |
//!
//! # Features
//!
//! The layers nest, so name only the highest one you need:
//!
//! ```toml
//! qana = "0.0.2"                                             # everything (default: "lang")
//! qana = { version = "0.0.2", default-features = false, features = ["engine"] }   # just parsing
//! ```
//!
//! `grammar` ⊂ `engine` ⊂ `sem` ⊂ `services` ⊂ `lang`.
//!
//! # The command-line tool
//!
//! The `qana` binary lives in a separate crate so that the library does
//! not drag an argument parser into every consumer's dependency tree:
//!
//! ```sh
//! cargo install qana-cli   # installs the `qana` command
//! ```

#![cfg_attr(docsrs, feature(doc_cfg))]

#[cfg(feature = "grammar")]
#[cfg_attr(docsrs, doc(cfg(feature = "grammar")))]
pub use qana_grammar as grammar;

#[cfg(feature = "engine")]
#[cfg_attr(docsrs, doc(cfg(feature = "engine")))]
pub use qana_engine as engine;

#[cfg(feature = "sem")]
#[cfg_attr(docsrs, doc(cfg(feature = "sem")))]
pub use qana_sem as sem;

#[cfg(feature = "services")]
#[cfg_attr(docsrs, doc(cfg(feature = "services")))]
pub use qana_services as services;

#[cfg(feature = "lang")]
#[cfg_attr(docsrs, doc(cfg(feature = "lang")))]
pub use qana_lang as lang;

/// The editor protocol, re-exported for convenience.
///
/// This crate has NO dependency on qana and never will — an editor
/// depends on `linework` alone and stays ignorant of whatever engine is
/// behind the [`Limner`](linework::Limner) it holds. It is re-exported
/// here only so that consumers already depending on `qana` do not need a
/// second entry in their manifest.
#[cfg(feature = "services")]
#[cfg_attr(docsrs, doc(cfg(feature = "services")))]
pub use ::linework;

/// The types you reach for first.
///
/// Everything here is also available at its full path; this is a
/// convenience, not a second API. Each item is gated by the feature that
/// provides it, so a narrowed build gets a narrowed prelude rather than
/// a compile error.
pub mod prelude {
    #[cfg(feature = "grammar")]
    pub use qana_grammar::{CompiledLexer, GreenChild, GreenNode, LrTables, SynGrammar};

    #[cfg(feature = "engine")]
    pub use qana_engine::{IncSession, LexedBuffer, Line, LineEdit};

    #[cfg(feature = "sem")]
    pub use qana_sem::SemDb;

    #[cfg(feature = "services")]
    pub use ::linework::Limner;

    #[cfg(feature = "lang")]
    pub use qana_lang::{
        compile::{certify, compile, LangDef, QanaDiag},
        compile_source, EmbeddedLang, QanaOutcome, QanaToolchain,
    };
}
