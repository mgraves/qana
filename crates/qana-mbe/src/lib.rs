//! Rust's macros by example (`macro_rules!`, and `macro` as std defines
//! its own), expanded as rustc expands them.
//!
//! - **A definition** is read from its tokens ([`MacroRules::read`]): its
//!   arms, each a matcher and a transcriber, checked as rustc checks them
//!   (balanced matchers, the follow-set rules, repetitions that cannot match
//!   nothing, one binding per name).
//! - **An invocation** is matched against the arms in order and the first
//!   that matches is transcribed ([`expand`]). The result is a
//!   [`TokenStream`] in which every token knows where it was written
//!   ([`Token::origin`]: the invocation's text, or the definition's) and
//!   which expansion wrote it ([`Token::mark`]).
//! - **An expansion is parsed** where its invocation stands, at one of the
//!   grammar's entries (a module's items, statements, an expression…:
//!   [`TokenStream::parse`]), and the invocations inside it are expanded in
//!   turn, to rustc's recursion limit ([`expand_nested`]).
//!
//! It follows rustc's `rustc_expand::mbe` and the parser around it as of
//! nightly 1.94 (2025-12-10): the matcher is the same NFA over matcher
//! positions, with no lookahead; where one fragment (`$e:expr`) is all that
//! can go on, it is parsed by the grammar at that fragment's rule (the
//! grammar's `fragment` declarations), greedily, as rustc's parser parses
//! it; a token rustc's parser splits (`>>` closing two generic lists) is
//! split where the grammar ends. A fragment a transcriber puts in stays one
//! node of its kind, as rustc's invisible delimiters keep it: forwarded to
//! another macro, it is matched whole; in an expansion, it is wrapped in
//! parentheses where the grammar would otherwise read it otherwise
//! (`$e * 2` with `$e` = `1 + 1`).
//!
//! What is the language's (keywords, which tokens can begin a fragment, how
//! punctuation glues) is rustc's, here; what is the grammar's (how a
//! fragment is parsed) comes from the grammar, which must be the one
//! `examples/rust/rust.qana` declares, or one with the same tokens and
//! entries.

mod driver;
mod input;
mod matcher;
mod parsed;
mod render;
mod rules;
mod token;
mod transcribe;

pub use driver::{expand_all, expand_nested, Expansion, Nested, Outcome, RECURSION_LIMIT};
pub use parsed::{Parsed, Spanned};
pub use rules::{Form, MacroRules};
pub use token::{Token, TokenKind, TokenStream};

use qana_lang::EmbeddedLang;

/// A Rust edition, which decides what some fragments match (`expr`, `pat`)
/// and which names are keywords.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Edition {
    E2015,
    E2018,
    E2021,
    E2024,
}

/// Where a token was written: a source the caller numbers (a file, the text
/// of a definition) and the byte span in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Origin {
    pub source: u32,
    pub start: u32,
    pub end: u32,
}

/// Why a definition could not be read, an invocation matched or
/// transcribed, or an expansion parsed: rustc's message, and where.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub message: String,
    pub at: Option<Origin>,
}

impl Error {
    pub(crate) fn new(message: impl Into<String>, at: Option<Origin>) -> Error {
        Error { message: message.into(), at }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.at {
            Some(o) => {
                write!(f, "{} (source {} at {}..{})", self.message, o.source, o.start, o.end)
            }
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for Error {}

/// Expand `input` (an invocation's tokens, inside its delimiters) by
/// `rules`: the first arm whose matcher matches it, transcribed. The tokens
/// the transcriber writes are marked `mark` ([`Token::mark`]); those of the
/// fragments it fills in keep theirs.
pub fn expand(
    lang: &EmbeddedLang,
    rules: &MacroRules,
    input: &TokenStream,
    mark: u32,
) -> Result<TokenStream, Error> {
    let (arm, matches) = matcher::match_arms(lang, rules, input)?;
    transcribe::transcribe(rules, arm, &matches, mark)
}
