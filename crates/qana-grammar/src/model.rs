//! Grammar-as-value: the core data model (Part III commitment).
//!
//! A [`LexGrammar`] is an ordinary immutable Rust value — `Clone`, `Eq`,
//! `Debug` — buildable programmatically, comparable, and (later) parseable
//! from the self-hosted grammar language. Macros generating grammars for
//! other macros manipulate exactly this type.

use crate::pat::Pat;

/// Shared token identity across the toolchain: index into the grammar's
/// token-def list; synthetic ids (Unknown) come after.
pub type TokenId = u16;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BracketKind {
    Paren,
    Bracket,
    Brace,
}

/// Lexer command attached to a token.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    None,
    /// Push the given mode. Saturates silently at the grammar's stack
    /// bound (the L2-linted, spike-compatible semantics).
    Push(u16),
    /// Pop to the previous mode; no-op in the base mode.
    Pop,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenDef {
    pub name: String,
    /// Mode this token is active in (0 = base mode).
    pub mode: u16,
    pub pat: Pat,
    pub trivia: bool,
    /// Error-flavored token (e.g. an unterminated string): a real token,
    /// lossless, but diagnostics-worthy.
    pub error: bool,
    pub bracket: Option<(BracketKind, bool /* is_open */)>,
    pub action: Action,
    /// If set, matched text is looked up in the grammar's keyword map and
    /// re-tagged to that keyword's own token id on a hit.
    pub specialize: bool,
}

impl TokenDef {
    pub fn new(name: &str, mode: u16, pat: Pat) -> Self {
        TokenDef {
            name: name.to_string(),
            mode,
            pat,
            trivia: false,
            error: false,
            bracket: None,
            action: Action::None,
            specialize: false,
        }
    }
    pub fn trivia(mut self) -> Self {
        self.trivia = true;
        self
    }
    pub fn error(mut self) -> Self {
        self.error = true;
        self
    }
    pub fn bracket(mut self, kind: BracketKind, open: bool) -> Self {
        self.bracket = Some((kind, open));
        self
    }
    pub fn push(mut self, mode: u16) -> Self {
        self.action = Action::Push(mode);
        self
    }
    pub fn pop(mut self) -> Self {
        self.action = Action::Pop;
        self
    }
    pub fn specialize(mut self) -> Self {
        self.specialize = true;
        self
    }
}

/// The lexical half of an envelope grammar, as a first-class value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LexGrammar {
    pub name: String,
    pub mode_names: Vec<String>,
    pub tokens: Vec<TokenDef>,
    /// Keyword specializations: (text, keyword token id, OWNER token id).
    /// Each keyword owns a distinct id (a `Pat::Never` token def) so
    /// parsers can reference individual keywords as terminals, and each
    /// entry names the @specialize token it applies to — specialization
    /// is per-owner, which is what keeps COMPOSED languages' keyword
    /// spaces separate (a host keyword is an ordinary identifier inside
    /// a guest island).
    pub keywords: Vec<(String, TokenId, TokenId)>,
    /// Maximum mode-stack depth. Required (and L2-checked) when the mode
    /// push graph is cyclic; must be ≤ [`crate::lexer::MAX_STACK`].
    pub max_stack: Option<u8>,
    /// Per-mode LINE-BOUNDED flag (`@push(MODE, eol)`): the mode pops
    /// automatically at end of line, so it can never appear in a line's
    /// ENTRY state — the L2 space shrinks, and edits inside such a mode
    /// (preprocessor directives, say) can never damage neighboring
    /// lines. At EOL, flagged modes pop from the top of the stack until
    /// an unflagged one (or the base) is exposed.
    pub eol_pop: Vec<bool>,
}

impl LexGrammar {
    pub fn new(name: &str, mode_names: &[&str]) -> Self {
        LexGrammar {
            name: name.to_string(),
            mode_names: mode_names.iter().map(|s| s.to_string()).collect(),
            tokens: Vec::new(),
            keywords: Vec::new(),
            max_stack: None,
            eol_pop: vec![false; mode_names.len()],
        }
    }

    pub fn add(&mut self, def: TokenDef) -> TokenId {
        assert!(
            (def.mode as usize) < self.mode_names.len(),
            "token {} references undeclared mode {}",
            def.name,
            def.mode
        );
        self.tokens.push(def);
        (self.tokens.len() - 1) as TokenId
    }

    /// Synthetic id for the total-coverage fallback token.
    pub fn unknown_id(&self) -> TokenId {
        self.tokens.len() as TokenId
    }

    pub fn token_count_with_synthetic(&self) -> usize {
        self.tokens.len() + 1
    }
}

/// Downstream-facing token metadata (what the incremental engine and the
/// services layer consult). Derived from a grammar; independent of it.
#[derive(Clone, Debug)]
pub struct Vocab {
    pub names: Vec<String>,
    pub trivia: Vec<bool>,
    pub error: Vec<bool>,
    pub brackets: Vec<Option<(BracketKind, bool)>>,
    pub unknown: TokenId,
}

impl Vocab {
    pub fn of(g: &LexGrammar) -> Vocab {
        let mut names: Vec<String> = g.tokens.iter().map(|t| t.name.clone()).collect();
        let mut trivia: Vec<bool> = g.tokens.iter().map(|t| t.trivia).collect();
        let mut error: Vec<bool> = g.tokens.iter().map(|t| t.error).collect();
        let mut brackets: Vec<Option<(BracketKind, bool)>> =
            g.tokens.iter().map(|t| t.bracket).collect();
        names.push("UNKNOWN".to_string());
        trivia.push(true);
        error.push(false);
        brackets.push(None);
        Vocab { names, trivia, error, brackets, unknown: g.unknown_id() }
    }
    pub fn is_trivia(&self, id: TokenId) -> bool {
        self.trivia[id as usize]
    }
}
