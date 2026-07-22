//! Token patterns: a small regular language over *lines*.
//!
//! Design constraints baked into the type, per envelope commitment L1:
//! there is no way to write "newline" except through an explicit `'\n'`
//! character/range, and the built-in classes (`lws`, `any` via negation)
//! exclude line terminators by definition. The L1 lint then proves the
//! property on the compiled automaton and reports a witness if an author
//! smuggles a terminator in anyway.

/// A character class. Explicit chars/ranges are ASCII-only in P1 (a
/// compile-time error otherwise); unicode is reached through the builtin
/// flags, which are *class-uniform* over the compressed alphabet:
/// `alpha` ⇒ all unicode alphabetics, `alnum` ⇒ alphabetics + numerics,
/// `lws` ⇒ unicode whitespace excluding `\r`/`\n`.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ClassSet {
    pub negated: bool,
    pub chars: Vec<char>,
    pub ranges: Vec<(char, char)>,
    pub alpha: bool,
    pub alnum: bool,
    pub digit: bool,
    pub lws: bool,
}

impl ClassSet {
    pub fn chars(cs: &[char]) -> Self {
        ClassSet { chars: cs.to_vec(), ..Default::default() }
    }
    pub fn not_chars(cs: &[char]) -> Self {
        ClassSet { negated: true, chars: cs.to_vec(), ..Default::default() }
    }
    pub fn digit() -> Self {
        ClassSet { digit: true, ..Default::default() }
    }
    pub fn ident_start() -> Self {
        ClassSet { alpha: true, chars: vec!['_'], ..Default::default() }
    }
    pub fn ident_cont() -> Self {
        ClassSet { alnum: true, chars: vec!['_'], ..Default::default() }
    }
    pub fn line_ws() -> Self {
        ClassSet { lws: true, ..Default::default() }
    }
    /// Any character except line terminators — the only "any" there is.
    pub fn any() -> Self {
        ClassSet::not_chars(&['\r', '\n'])
    }
    pub fn ranges(rs: &[(char, char)]) -> Self {
        ClassSet { ranges: rs.to_vec(), ..Default::default() }
    }
}

/// Token pattern AST. `Never` is a pattern that matches nothing — used for
/// tokens that only arise by specialization (keywords) so they own an id
/// and a name without participating in the DFA.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pat {
    Lit(String),
    Class(ClassSet),
    Seq(Vec<Pat>),
    Alt(Vec<Pat>),
    Star(Box<Pat>),
    Plus(Box<Pat>),
    Opt(Box<Pat>),
    Never,
}

impl Pat {
    pub fn lit(s: &str) -> Pat {
        Pat::Lit(s.to_string())
    }
    pub fn seq(ps: impl IntoIterator<Item = Pat>) -> Pat {
        Pat::Seq(ps.into_iter().collect())
    }
    pub fn alt(ps: impl IntoIterator<Item = Pat>) -> Pat {
        Pat::Alt(ps.into_iter().collect())
    }
    pub fn star(p: Pat) -> Pat {
        Pat::Star(Box::new(p))
    }
    pub fn plus(p: Pat) -> Pat {
        Pat::Plus(Box::new(p))
    }
    pub fn opt(p: Pat) -> Pat {
        Pat::Opt(Box::new(p))
    }
}

// ---------------------------------------------------------------------------
// The compressed alphabet: 128 ASCII columns + 4 unicode class columns.
// ---------------------------------------------------------------------------

pub const NCOLS: usize = 132;
pub const COL_UALPHA: usize = 128;
pub const COL_UNUM: usize = 129;
pub const COL_UWS: usize = 130;
pub const COL_UOTHER: usize = 131;

/// Map a char to its alphabet column.
#[inline]
pub fn classify(c: char) -> usize {
    if (c as u32) < 128 {
        c as usize
    } else if c.is_alphabetic() {
        COL_UALPHA
    } else if c.is_numeric() {
        COL_UNUM
    } else if c.is_whitespace() {
        COL_UWS
    } else {
        COL_UOTHER
    }
}

/// A printable representative character for each column (used to build
/// human-readable lint counterexamples).
pub fn representative(col: usize) -> char {
    match col {
        COL_UALPHA => 'α',
        COL_UNUM => '٣',
        COL_UWS => '\u{00A0}',
        COL_UOTHER => '§',
        c => c as u8 as char,
    }
}

impl ClassSet {
    /// Does this class match the given alphabet column? Exact for ASCII
    /// columns; exact for unicode columns because builtins are
    /// class-uniform by construction.
    pub fn matches_col(&self, col: usize) -> bool {
        let member = match col {
            0..=127 => {
                let c = col as u8 as char;
                self.chars.contains(&c)
                    || self.ranges.iter().any(|&(a, b)| a <= c && c <= b)
                    || (self.alpha && c.is_ascii_alphabetic())
                    || (self.alnum && c.is_ascii_alphanumeric())
                    || (self.digit && c.is_ascii_digit())
                    || (self.lws && c.is_whitespace() && c != '\r' && c != '\n')
            }
            COL_UALPHA => self.alpha || self.alnum,
            COL_UNUM => self.alnum,
            COL_UWS => self.lws,
            _ => false,
        };
        member ^ self.negated
    }
}
