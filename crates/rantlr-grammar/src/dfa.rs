//! Pattern → NFA (Thompson) → DFA (subset construction) over the
//! 132-column compressed alphabet. One DFA per lexer mode, with accepting
//! states tagged by token id; ties at equal match length resolve to the
//! earliest-declared token (maximal munch, then declaration priority).

use crate::model::{LexGrammar, TokenId};
use crate::pat::{ClassSet, Pat, NCOLS};
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub enum CompileError {
    /// Explicit non-ASCII literal/range chars are out of scope for P1.
    NonAsciiLiteral { token: String, ch: char },
    /// A token pattern can match the empty string (would loop forever).
    EmptyMatch { token: String },
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompileError::NonAsciiLiteral { token, ch } => write!(
                f,
                "token `{token}`: non-ASCII literal char {ch:?} (use builtin classes for unicode in P1)"
            ),
            CompileError::EmptyMatch { token } => {
                write!(f, "token `{token}` can match the empty string — a lexer must always consume")
            }
        }
    }
}

// ---------------- NFA ----------------

#[derive(Clone, Debug)]
enum PredKind {
    Char(char),
    Class(ClassSet),
}

struct Nfa {
    /// eps[i] = ε-successors of state i
    eps: Vec<Vec<u32>>,
    /// edges[i] = (predicate, target)
    edges: Vec<Vec<(u32, u32)>>,
    accept: Vec<Option<TokenId>>,
    preds: Vec<PredKind>,
}

impl Nfa {
    fn new() -> Self {
        Nfa { eps: vec![], edges: vec![], accept: vec![], preds: vec![] }
    }
    fn state(&mut self) -> u32 {
        self.eps.push(vec![]);
        self.edges.push(vec![]);
        self.accept.push(None);
        (self.eps.len() - 1) as u32
    }
    fn pred(&mut self, p: PredKind) -> u32 {
        self.preds.push(p);
        (self.preds.len() - 1) as u32
    }

    /// Compile `pat` into a fragment (start, end). `end` has no out-edges.
    fn frag(&mut self, pat: &Pat, token: &str) -> Result<Option<(u32, u32)>, CompileError> {
        Ok(match pat {
            Pat::Never => None,
            Pat::Lit(s) => {
                let start = self.state();
                let mut cur = start;
                for ch in s.chars() {
                    if (ch as u32) >= 128 {
                        return Err(CompileError::NonAsciiLiteral { token: token.into(), ch });
                    }
                    let nxt = self.state();
                    let p = self.pred(PredKind::Char(ch));
                    self.edges[cur as usize].push((p, nxt));
                    cur = nxt;
                }
                Some((start, cur))
            }
            Pat::Class(cs) => {
                for &c in &cs.chars {
                    if (c as u32) >= 128 {
                        return Err(CompileError::NonAsciiLiteral { token: token.into(), ch: c });
                    }
                }
                for &(a, b) in &cs.ranges {
                    if (a as u32) >= 128 || (b as u32) >= 128 {
                        return Err(CompileError::NonAsciiLiteral { token: token.into(), ch: b });
                    }
                }
                let s = self.state();
                let e = self.state();
                let p = self.pred(PredKind::Class(cs.clone()));
                self.edges[s as usize].push((p, e));
                Some((s, e))
            }
            Pat::Seq(ps) => {
                let mut cur: Option<(u32, u32)> = None;
                for p in ps {
                    let f = match self.frag(p, token)? {
                        Some(f) => f,
                        None => return Ok(None),
                    };
                    cur = Some(match cur {
                        None => f,
                        Some((s, e)) => {
                            self.eps[e as usize].push(f.0);
                            (s, f.1)
                        }
                    });
                }
                match cur {
                    Some(f) => Some(f),
                    None => {
                        // Empty Seq matches ε; represent explicitly.
                        let s = self.state();
                        Some((s, s))
                    }
                }
            }
            Pat::Alt(ps) => {
                let s = self.state();
                let e = self.state();
                let mut any = false;
                for p in ps {
                    if let Some((fs, fe)) = self.frag(p, token)? {
                        self.eps[s as usize].push(fs);
                        self.eps[fe as usize].push(e);
                        any = true;
                    }
                }
                if !any {
                    return Ok(None);
                }
                Some((s, e))
            }
            Pat::Star(p) => {
                let s = self.state();
                let e = self.state();
                self.eps[s as usize].push(e);
                if let Some((fs, fe)) = self.frag(p, token)? {
                    self.eps[s as usize].push(fs);
                    self.eps[fe as usize].push(e);
                    self.eps[fe as usize].push(fs);
                }
                Some((s, e))
            }
            Pat::Plus(p) => {
                let (fs, fe) = match self.frag(p, token)? {
                    Some(f) => f,
                    None => return Ok(None),
                };
                self.eps[fe as usize].push(fs);
                Some((fs, fe))
            }
            Pat::Opt(p) => {
                let s = self.state();
                let e = self.state();
                self.eps[s as usize].push(e);
                if let Some((fs, fe)) = self.frag(p, token)? {
                    self.eps[s as usize].push(fs);
                    self.eps[fe as usize].push(e);
                }
                Some((s, e))
            }
        })
    }
}

// ---------------- DFA ----------------

pub const DEAD: u16 = u16::MAX;

/// DFA for one lexer mode.
#[derive(Clone, Debug)]
pub struct ModeDfa {
    pub start: u16,
    /// trans[state][col] → state or DEAD
    pub trans: Vec<[u16; NCOLS]>,
    /// accept[state] → winning token id at this state, if accepting
    pub accept: Vec<Option<TokenId>>,
}

/// Build the DFA for one mode of the grammar.
pub fn build_mode_dfa(g: &LexGrammar, mode: u16) -> Result<ModeDfa, CompileError> {
    let mut nfa = Nfa::new();
    let start = nfa.state();
    for (idx, def) in g.tokens.iter().enumerate() {
        if def.mode != mode {
            continue;
        }
        if let Some((fs, fe)) = nfa.frag(&def.pat, &def.name)? {
            nfa.eps[start as usize].push(fs);
            nfa.accept[fe as usize] = Some(idx as TokenId);
        }
    }

    // Precompute pred → column membership.
    let pred_cols: Vec<[bool; NCOLS]> = nfa
        .preds
        .iter()
        .map(|p| {
            let mut row = [false; NCOLS];
            for (col, slot) in row.iter_mut().enumerate() {
                *slot = match p {
                    PredKind::Char(c) => col == *c as usize,
                    PredKind::Class(cs) => cs.matches_col(col),
                };
            }
            row
        })
        .collect();

    let closure = |set: &mut Vec<u32>| {
        let mut stack: Vec<u32> = set.clone();
        while let Some(s) = stack.pop() {
            for &t in &nfa.eps[s as usize] {
                if !set.contains(&t) {
                    set.push(t);
                    stack.push(t);
                }
            }
        }
        set.sort_unstable();
        set.dedup();
    };

    let accept_of = |set: &[u32]| -> Option<TokenId> {
        set.iter().filter_map(|&s| nfa.accept[s as usize]).min()
    };

    let mut start_set = vec![start];
    closure(&mut start_set);

    let mut dfa_states: Vec<Vec<u32>> = vec![start_set.clone()];
    let mut index: HashMap<Vec<u32>, u16> = HashMap::new();
    index.insert(start_set, 0);
    let mut trans: Vec<[u16; NCOLS]> = vec![];
    let mut accept: Vec<Option<TokenId>> = vec![];

    let mut i = 0usize;
    while i < dfa_states.len() {
        let cur = dfa_states[i].clone();
        accept.push(accept_of(&cur));
        let mut row = [DEAD; NCOLS];
        for (col, slot) in row.iter_mut().enumerate() {
            let mut nxt: Vec<u32> = Vec::new();
            for &s in &cur {
                for &(p, t) in &nfa.edges[s as usize] {
                    if pred_cols[p as usize][col] && !nxt.contains(&t) {
                        nxt.push(t);
                    }
                }
            }
            if nxt.is_empty() {
                continue;
            }
            closure(&mut nxt);
            let id = match index.get(&nxt) {
                Some(&id) => id,
                None => {
                    let id = dfa_states.len() as u16;
                    index.insert(nxt.clone(), id);
                    dfa_states.push(nxt);
                    id
                }
            };
            *slot = id;
        }
        trans.push(row);
        i += 1;
    }

    // Empty-match check: the start state must not accept.
    if let Some(tok) = accept[0] {
        return Err(CompileError::EmptyMatch { token: g.tokens[tok as usize].name.clone() });
    }

    Ok(ModeDfa { start: 0, trans, accept })
}
