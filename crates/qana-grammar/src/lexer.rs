//! The compiled, table-driven line lexer: `lex_line(text, entry_state)`
//! is a pure, total function — the same contract the P0 hand lexer
//! satisfied, now generated from a grammar value and certified by the
//! envelope lints instead of by inspection.

use crate::dfa::{build_mode_dfa, CompileError, ModeDfa, DEAD};
use crate::lints::{check_envelope, EnvelopeReport, LintError};
use crate::model::{Action, LexGrammar, TokenId, Vocab};
use crate::pat::classify;
use std::collections::HashMap;

/// Engine capacity for the mode stack. Grammar bounds are L2-checked
/// against this; the state stays `Copy + Eq` and small.
pub const MAX_STACK: usize = 8;

/// The finite line-start state: a bounded mode stack. Base mode (0) is
/// implicit at the bottom and never stored.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct MStack {
    stack: [u16; MAX_STACK],
    len: u8,
}

impl MStack {
    pub fn current_mode(&self) -> u16 {
        if self.len == 0 {
            0
        } else {
            self.stack[self.len as usize - 1]
        }
    }
    pub fn depth(&self) -> usize {
        self.len as usize
    }
    /// Saturating push (L2 semantics: silently drop beyond the bound —
    /// matching the P0 spike's nested-comment cap behavior).
    fn push(&mut self, mode: u16, bound: u8) {
        if self.len < bound.min(MAX_STACK as u8) {
            self.stack[self.len as usize] = mode;
            self.len += 1;
        }
    }
    fn pop(&mut self) {
        if self.len > 0 {
            self.len -= 1;
            // Clear the vacated slot: `MStack` derives PartialEq over
            // the WHOLE array, so residue above `len` would make two
            // semantically identical states compare unequal — breaking
            // the relex reconvergence test and turning (for example)
            // "delete a block comment" or any EOL-popped directive line
            // into a full-file relex.
            self.stack[self.len as usize] = 0;
        }
    }
    /// Build a state from a mode sequence (bottom to top) — test helper.
    pub fn of(modes: &[u16]) -> MStack {
        let mut s = MStack::default();
        for &m in modes {
            s.push(m, MAX_STACK as u8);
        }
        s
    }
}

/// A lexed token: id into the vocab + byte length. Positions are always
/// derived (prefix sums), never stored — the relocatability invariant.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Token {
    pub id: TokenId,
    pub len: u32,
}

#[derive(Clone, Debug)]
pub enum BuildError {
    Compile(CompileError),
    Lint(LintError),
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuildError::Compile(e) => write!(f, "{e}"),
            BuildError::Lint(e) => write!(f, "{e}"),
        }
    }
}

/// A grammar compiled to runnable tables + its envelope certification.
#[derive(Debug)]
pub struct CompiledLexer {
    pub vocab: Vocab,
    pub report: EnvelopeReport,
    modes: Vec<ModeDfa>,
    actions: Vec<Action>,
    trivia: Vec<bool>,
    specialize: Vec<bool>,
    eol_pop: Vec<bool>,
    keywords: HashMap<(TokenId, String), TokenId>,
    stack_bound: u8,
    unknown: TokenId,
}

impl CompiledLexer {
    /// Compile + lint. This is the envelope gate: an out-of-envelope
    /// grammar does not produce a lexer, it produces a counterexample.
    pub fn build(g: &LexGrammar) -> Result<CompiledLexer, BuildError> {
        let mut modes = Vec::with_capacity(g.mode_names.len());
        for m in 0..g.mode_names.len() as u16 {
            modes.push(build_mode_dfa(g, m).map_err(BuildError::Compile)?);
        }
        let report = check_envelope(g, &modes).map_err(BuildError::Lint)?;
        Ok(CompiledLexer {
            vocab: Vocab::of(g),
            stack_bound: report.stack_bound,
            report,
            modes,
            actions: g.tokens.iter().map(|t| t.action).collect(),
            trivia: g.tokens.iter().map(|t| t.trivia).collect(),
            specialize: g.tokens.iter().map(|t| t.specialize).collect(),
            eol_pop: g.eol_pop.clone(),
            // Specialization is PER-OWNER: a keyword only re-tags the
            // token it was declared for (composed languages keep their
            // keyword spaces separate).
            keywords: g
                .keywords
                .iter()
                .map(|(w, kw, owner)| ((*owner, w.clone()), *kw))
                .collect(),
            unknown: g.unknown_id(),
        })
    }

    /// Lex one line: pure, total, line-local. Every byte is covered
    /// (unknown bytes become the synthetic Unknown trivia token).
    pub fn lex_line(&self, text: &str, entry: MStack) -> (Vec<Token>, MStack) {
        let mut out = Vec::new();
        let mut state = entry;
        let n = text.len();
        let mut i = 0usize;
        while i < n {
            let dfa = &self.modes[state.current_mode() as usize];
            let mut s = dfa.start;
            let mut best: Option<(usize, TokenId)> = None;
            let mut j = i;
            for c in text[i..].chars() {
                let nxt = dfa.trans[s as usize][classify(c)];
                if nxt == DEAD {
                    break;
                }
                s = nxt;
                j += c.len_utf8();
                if let Some(tok) = dfa.accept[s as usize] {
                    best = Some((j, tok));
                }
            }
            match best {
                None => {
                    // Total coverage: consume one char as Unknown trivia.
                    let ch = text[i..].chars().next().unwrap();
                    out.push(Token { id: self.unknown, len: ch.len_utf8() as u32 });
                    i += ch.len_utf8();
                }
                Some((end, mut tok)) => {
                    if self.specialize[tok as usize] {
                        if let Some(&kw) = self.keywords.get(&(tok, text[i..end].to_string())) {
                            tok = kw;
                        }
                    }
                    out.push(Token { id: tok, len: (end - i) as u32 });
                    match self.actions[tok as usize] {
                        Action::None => {}
                        Action::Push(m) => state.push(m, self.stack_bound),
                        Action::Pop => state.pop(),
                    }
                    i = end;
                }
            }
        }
        debug_assert_eq!(
            out.iter().map(|t| t.len as usize).sum::<usize>(),
            n,
            "lossless invariant: tokens must cover the line exactly"
        );
        // Line-bounded modes (`@push(M, eol)`) end with the line: pop
        // them from the top so the EXIT state never carries them —
        // directive edits stay line-local by construction.
        while state.depth() > 0 && self.eol_pop[state.current_mode() as usize] {
            state.pop();
        }
        (out, state)
    }

    pub fn is_trivia(&self, id: TokenId) -> bool {
        if id as usize == self.trivia.len() {
            true // Unknown
        } else {
            self.trivia[id as usize]
        }
    }
}
