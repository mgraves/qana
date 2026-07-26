//! The syntactic half of an envelope grammar, as a first-class value —
//! same philosophy as [`crate::model::LexGrammar`]: a plain `Clone + Eq`
//! structure that builders, tests, and (later) macros can construct and
//! transform.
//!
//! Disambiguation is declarative only: token precedence/associativity
//! declarations plus per-production overrides (envelope commitment L3 —
//! conflicts that survive them are compile errors with traces, never
//! runtime nondeterminism).

use crate::model::TokenId;

/// Terminal id used for end-of-input in lookaheads and ACTION tables.
pub const EOF: TokenId = u16::MAX;

pub type NtId = u16;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Sym {
    T(TokenId),
    N(NtId),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Assoc {
    Left,
    Right,
    NonAssoc,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prod {
    pub lhs: NtId,
    pub rhs: Vec<Sym>,
    /// Explicit precedence override; defaults to the precedence of the
    /// last terminal in `rhs` (yacc semantics).
    pub prec: Option<u8>,
    /// CamelCase variant name for the typed AST (e.g. "LetStmt").
    /// Unnamed productions get positional names in generated code.
    pub name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct SynGrammar {
    pub name: String,
    /// Terminal display names, indexed by TokenId (from the lex vocab).
    pub term_names: Vec<String>,
    pub nt_names: Vec<String>,
    pub prods: Vec<Prod>,
    pub start: NtId,
    /// Token precedence: (level, assoc). Higher level binds tighter.
    pub token_prec: Vec<Option<(u8, Assoc)>>,
}

pub(crate) fn camel(s: &str) -> String {
    let mut out = String::new();
    let mut up = true;
    for c in s.chars() {
        if c == '_' || c == '-' {
            up = true;
        } else if up {
            out.extend(c.to_uppercase());
            up = false;
        } else {
            out.extend(c.to_lowercase());
        }
    }
    out
}

impl SynGrammar {
    pub fn new(name: &str, term_names: Vec<String>) -> Self {
        let n = term_names.len();
        SynGrammar {
            name: name.to_string(),
            term_names,
            nt_names: Vec::new(),
            prods: Vec::new(),
            start: 0,
            token_prec: vec![None; n],
        }
    }

    pub fn nt(&mut self, name: &str) -> NtId {
        self.nt_names.push(name.to_string());
        (self.nt_names.len() - 1) as NtId
    }

    pub fn prod(&mut self, lhs: NtId, rhs: Vec<Sym>) -> usize {
        self.prods.push(Prod { lhs, rhs, prec: None, name: None });
        self.prods.len() - 1
    }

    pub fn prod_named(&mut self, lhs: NtId, name: &str, rhs: Vec<Sym>) -> usize {
        self.prods.push(Prod { lhs, rhs, prec: None, name: Some(name.to_string()) });
        self.prods.len() - 1
    }

    pub fn prod_prec(&mut self, lhs: NtId, rhs: Vec<Sym>, prec: u8) -> usize {
        self.prods.push(Prod { lhs, rhs, prec: Some(prec), name: None });
        self.prods.len() - 1
    }

    /// Generated-code name for a production: declared name or positional.
    pub fn prod_name(&self, idx: usize) -> String {
        match &self.prods[idx].name {
            Some(n) => n.clone(),
            None => format!(
                "{}V{}",
                camel(&self.nt_names[self.prods[idx].lhs as usize]),
                idx
            ),
        }
    }

    pub fn set_token_prec(&mut self, t: TokenId, level: u8, assoc: Assoc) {
        self.token_prec[t as usize] = Some((level, assoc));
    }

    pub fn term_name(&self, t: TokenId) -> &str {
        if t == EOF {
            "<eof>"
        } else {
            &self.term_names[t as usize]
        }
    }

    /// Effective precedence of a production (yacc default: last terminal).
    pub fn prod_precedence(&self, p: &Prod) -> Option<(u8, Assoc)> {
        if let Some(level) = p.prec {
            return Some((level, Assoc::Left));
        }
        p.rhs
            .iter()
            .rev()
            .find_map(|s| match s {
                Sym::T(t) => self.token_prec[*t as usize],
                Sym::N(_) => None,
            })
    }

    /// CamelCase a snake/upper name: `args_ne` → `ArgsNe`.
    pub fn camel_name(s: &str) -> String {
        camel(s)
    }

    /// Render a production like `expr → expr PLUS expr`.
    pub fn prod_display(&self, idx: usize) -> String {
        let p = &self.prods[idx];
        let rhs: Vec<&str> = p
            .rhs
            .iter()
            .map(|s| match s {
                Sym::T(t) => self.term_name(*t),
                Sym::N(n) => self.nt_names[*n as usize].as_str(),
            })
            .collect();
        format!(
            "{} → {}",
            self.nt_names[p.lhs as usize],
            if rhs.is_empty() { "ε".to_string() } else { rhs.join(" ") }
        )
    }
}
