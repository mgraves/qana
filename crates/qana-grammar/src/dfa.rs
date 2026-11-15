//! Pattern → NFA (Thompson) → DFA (subset construction) over the
//! 132-column compressed alphabet. One DFA per lexer mode, with accepting
//! states tagged by token id; ties at equal match length resolve to the
//! earliest-declared token (maximal munch, then declaration priority).
//!
//! # Trailing context
//!
//! A token may end in a trailing context ([`Pat::Ahead`]): it is accepted
//! only if the rest of the line does, or does not, begin with a match of
//! a second pattern. The mode DFA itself is unchanged by this: a context
//! matches no characters, so it adds no transition. What changes is what
//! an accepting state says. A state whose winning token is unconditional
//! says so in [`ModeDfa::accept`], as before. A state where a conditional
//! token outranks every unconditional one points at a row of
//! [`ModeDfa::rows`]: the tokens it may accept, in declaration priority,
//! each with the context it needs. The lexer decides such a row by
//! running the context's own small DFA ([`ContextDfa`]) over the rest of
//! the line, and it does that only for the longest candidate it actually
//! needs (see `CompiledLexer::lex_line`).

use crate::model::{LexGrammar, TokenId};
use crate::pat::{ClassSet, ContextError, Pat, NCOLS};
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub enum CompileError {
    /// Explicit non-ASCII literal/range chars are out of scope for P1.
    NonAsciiLiteral { token: String, ch: char },
    /// A token pattern can match the empty string (would loop forever).
    EmptyMatch { token: String },
    /// A trailing context is somewhere it cannot mean anything.
    Context { token: String, error: ContextError },
    /// A trailing context can match the empty string: it would hold (or,
    /// negated, fail) everywhere, so it says nothing.
    EmptyContext { token: String },
    /// More DFA states than a `u16` state id can name.
    TooManyStates { token: String },
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
            CompileError::Context { token, error } => {
                let why = match error {
                    ContextError::NotAtEnd => {
                        "a trailing context must END the token: nothing may follow it"
                    }
                    ContextError::Repeated => {
                        "a trailing context cannot be repeated or optional: it matches no characters"
                    }
                    ContextError::Nested => "a trailing context cannot contain another",
                };
                write!(f, "token `{token}`: {why}")
            }
            CompileError::EmptyContext { token } => write!(
                f,
                "token `{token}`: its trailing context can match the empty string, so it holds \
                 everywhere and says nothing (for \"end of line\", write `(?!.)`)"
            ),
            CompileError::TooManyStates { token } => write!(
                f,
                "token `{token}`: the lexer for its mode needs more than {} states",
                DEAD
            ),
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
    /// asserts[i] = (context, target): state `i` reaches `target` without
    /// consuming, PROVIDED the context holds on the rest of the line. The
    /// subset construction does not follow these; they are read when a
    /// DFA state's accepts are worked out. A context only ends a token
    /// (`Pat::check_contexts`), so `target` leads to an accept by ε alone.
    asserts: Vec<Vec<(u16, u32)>>,
    accept: Vec<Option<TokenId>>,
    preds: Vec<PredKind>,
    /// The trailing contexts met while compiling, by index.
    contexts: Vec<(Pat, bool)>,
}

impl Nfa {
    fn new() -> Self {
        Nfa {
            eps: vec![],
            edges: vec![],
            asserts: vec![],
            accept: vec![],
            preds: vec![],
            contexts: vec![],
        }
    }
    fn state(&mut self) -> u32 {
        self.eps.push(vec![]);
        self.edges.push(vec![]);
        self.asserts.push(vec![]);
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
            Pat::Ahead { context, negated } => {
                // Consumes nothing: one conditional step from `s` to `e`.
                let s = self.state();
                let e = self.state();
                let id = self.contexts.len() as u16;
                self.contexts.push(((**context).clone(), *negated));
                self.asserts[s as usize].push((id, e));
                Some((s, e))
            }
        })
    }

    /// Column membership of every predicate.
    fn pred_cols(&self) -> Vec<[bool; NCOLS]> {
        self.preds
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
            .collect()
    }

    /// ε-closure of `set`, in place, sorted. Conditional steps
    /// (`asserts`) are not followed.
    fn closure(&self, set: &mut Vec<u32>) {
        let mut stack: Vec<u32> = set.clone();
        while let Some(s) = stack.pop() {
            for &t in &self.eps[s as usize] {
                if !set.contains(&t) {
                    set.push(t);
                    stack.push(t);
                }
            }
        }
        set.sort_unstable();
        set.dedup();
    }

    /// Subset construction from `start`. `None` when there are more
    /// states than a `u16` id can name.
    fn determinize(&self, start: u32) -> Option<Subsets> {
        let pred_cols = self.pred_cols();

        let mut start_set = vec![start];
        self.closure(&mut start_set);

        let mut dfa_states: Vec<Vec<u32>> = vec![start_set.clone()];
        let mut index: HashMap<Vec<u32>, u16> = HashMap::new();
        index.insert(start_set, 0);
        let mut trans: Vec<[u16; NCOLS]> = vec![];

        let mut i = 0usize;
        while i < dfa_states.len() {
            let cur = dfa_states[i].clone();
            let mut row = [DEAD; NCOLS];
            for (col, slot) in row.iter_mut().enumerate() {
                let mut nxt: Vec<u32> = Vec::new();
                for &s in &cur {
                    for &(p, t) in &self.edges[s as usize] {
                        if pred_cols[p as usize][col] && !nxt.contains(&t) {
                            nxt.push(t);
                        }
                    }
                }
                if nxt.is_empty() {
                    continue;
                }
                self.closure(&mut nxt);
                let id = match index.get(&nxt) {
                    Some(&id) => id,
                    None => {
                        if dfa_states.len() >= DEAD as usize {
                            return None;
                        }
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
        Some(Subsets { sets: dfa_states, trans })
    }
}

/// What the subset construction yields: for every DFA state (state 0 is
/// the start) the NFA states it stands for, and the transition table.
struct Subsets {
    sets: Vec<Vec<u32>>,
    trans: Vec<[u16; NCOLS]>,
}

// ---------------- DFA ----------------

pub const DEAD: u16 = u16::MAX;

/// "This state has no conditional accepts" in [`ModeDfa::conditional`].
pub const NO_ROW: u32 = u32::MAX;

/// One way a state may accept: `token`, if `context` (an index into
/// [`ModeDfa::contexts`]) holds on the rest of the line. `None` is
/// unconditional.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ConditionalAccept {
    pub token: TokenId,
    pub context: Option<u16>,
}

/// A trailing context, compiled: does the rest of the line begin with a
/// match of its pattern?
#[derive(Clone, Debug)]
pub struct ContextDfa {
    /// A negated context holds when the pattern does NOT match.
    pub negated: bool,
    /// trans[state][col] → state or DEAD; state 0 is the start.
    trans: Vec<[u16; NCOLS]>,
    accepting: Vec<bool>,
}

impl ContextDfa {
    /// Does the context hold for a token that is followed, on its line,
    /// by `rest`? It reads `rest` only as far as the first match (or the
    /// first character that rules one out), never the whole line unless
    /// the pattern needs it. An empty `rest` is the end of the line:
    /// nothing follows, so a plain context fails and a negated one holds.
    pub fn holds(&self, rest: &str) -> bool {
        let mut state = 0u16;
        let mut matched = false;
        for c in rest.chars() {
            state = self.trans[state as usize][crate::pat::classify(c)];
            if state == DEAD {
                break;
            }
            if self.accepting[state as usize] {
                matched = true;
                break;
            }
        }
        matched != self.negated
    }
}

/// DFA for one lexer mode.
#[derive(Clone, Debug)]
pub struct ModeDfa {
    pub start: u16,
    /// trans[state][col] → state or DEAD
    pub trans: Vec<[u16; NCOLS]>,
    /// accept[state] → the token this state accepts whatever follows, if
    /// that token outranks every conditional one here. `None` for a state
    /// that accepts nothing, and for one with a row in `conditional`.
    pub accept: Vec<Option<TokenId>>,
    /// conditional[state] → index into `rows`, or [`NO_ROW`]. Set exactly
    /// where a token with a trailing context outranks every unconditional
    /// one, so the answer depends on the rest of the line.
    pub conditional: Vec<u32>,
    /// The conditional accepts, in declaration priority: the first entry
    /// whose context holds is the token. A row ends at its first
    /// unconditional entry, if it has one.
    pub rows: Vec<Vec<ConditionalAccept>>,
    /// The trailing contexts the rows refer to.
    pub contexts: Vec<ContextDfa>,
}

impl ModeDfa {
    /// The token a conditional `row` accepts when the token would be
    /// followed, on its line, by `rest`: the first entry whose context
    /// holds. `None` when none does.
    pub fn resolve(&self, row: u32, rest: &str) -> Option<TokenId> {
        self.rows[row as usize]
            .iter()
            .find(|entry| match entry.context {
                None => true,
                Some(context) => self.contexts[context as usize].holds(rest),
            })
            .map(|entry| entry.token)
    }

    /// A token `state` accepts for SOME continuation of the line, if any:
    /// what the envelope lints mean by "an accepting state".
    pub fn may_accept(&self, state: u16) -> Option<TokenId> {
        self.accept[state as usize].or_else(|| {
            let row = self.conditional[state as usize];
            (row != NO_ROW).then(|| self.rows[row as usize][0].token)
        })
    }
}

/// Compile one trailing context to its DFA.
fn build_context(pat: &Pat, negated: bool, token: &str) -> Result<ContextDfa, CompileError> {
    let mut nfa = Nfa::new();
    let Some((start, end)) = nfa.frag(pat, token)? else {
        // A pattern that matches nothing (`Pat::Never`): one state, no
        // way out of it. A plain context never holds; a negated one
        // always does.
        return Ok(ContextDfa { negated, trans: vec![[DEAD; NCOLS]], accepting: vec![false] });
    };
    let Subsets { sets, trans } = nfa
        .determinize(start)
        .ok_or_else(|| CompileError::TooManyStates { token: token.into() })?;
    let accepting: Vec<bool> = sets.iter().map(|set| set.contains(&end)).collect();
    if accepting[0] {
        return Err(CompileError::EmptyContext { token: token.into() });
    }
    Ok(ContextDfa { negated, trans, accepting })
}

/// Build the DFA for one mode of the grammar.
pub fn build_mode_dfa(g: &LexGrammar, mode: u16) -> Result<ModeDfa, CompileError> {
    let mut nfa = Nfa::new();
    let start = nfa.state();
    // The token each context belongs to, for error messages.
    let mut context_owner: Vec<&str> = Vec::new();
    for (idx, def) in g.tokens.iter().enumerate() {
        if def.mode != mode {
            continue;
        }
        def.pat
            .check_contexts()
            .map_err(|error| CompileError::Context { token: def.name.clone(), error })?;
        if let Some((fs, fe)) = nfa.frag(&def.pat, &def.name)? {
            nfa.eps[start as usize].push(fs);
            nfa.accept[fe as usize] = Some(idx as TokenId);
        }
        context_owner.resize(nfa.contexts.len(), &def.name);
    }

    let mode_token = || {
        g.tokens
            .iter()
            .find(|def| def.mode == mode)
            .map(|def| def.name.clone())
            .unwrap_or_default()
    };
    let Subsets { sets: dfa_states, trans } = nfa
        .determinize(start)
        .ok_or_else(|| CompileError::TooManyStates { token: mode_token() })?;

    // What each DFA state accepts: its NFA states' own accepts, and the
    // accepts reached across a conditional step, tagged with the context.
    let mut accept: Vec<Option<TokenId>> = Vec::with_capacity(dfa_states.len());
    let mut conditional: Vec<u32> = Vec::with_capacity(dfa_states.len());
    let mut rows: Vec<Vec<ConditionalAccept>> = Vec::new();
    let mut row_index: HashMap<Vec<ConditionalAccept>, u32> = HashMap::new();
    for set in &dfa_states {
        let mut entries: Vec<ConditionalAccept> = Vec::new();
        for &n in set {
            if let Some(token) = nfa.accept[n as usize] {
                entries.push(ConditionalAccept { token, context: None });
            }
            for &(context, target) in &nfa.asserts[n as usize] {
                let mut beyond = vec![target];
                nfa.closure(&mut beyond);
                for &m in &beyond {
                    // Nothing after a context may consume or assert
                    // again. `check_contexts` guarantees it; a pattern
                    // that slipped past is refused, not miscompiled.
                    if !nfa.edges[m as usize].is_empty() || !nfa.asserts[m as usize].is_empty() {
                        return Err(CompileError::Context {
                            token: context_owner[context as usize].to_string(),
                            error: ContextError::NotAtEnd,
                        });
                    }
                    if let Some(token) = nfa.accept[m as usize] {
                        entries.push(ConditionalAccept { token, context: Some(context) });
                    }
                }
            }
        }
        // Declaration priority; for one token, its unconditional accept
        // makes its conditional ones redundant, so it sorts first.
        entries.sort_unstable_by_key(|e| (e.token, e.context.is_some(), e.context));
        entries.dedup();
        // Nothing after the first unconditional entry can ever win.
        if let Some(first) = entries.iter().position(|e| e.context.is_none()) {
            entries.truncate(first + 1);
        }
        match entries.first() {
            None => {
                accept.push(None);
                conditional.push(NO_ROW);
            }
            Some(ConditionalAccept { token, context: None }) => {
                accept.push(Some(*token));
                conditional.push(NO_ROW);
            }
            Some(_) => {
                accept.push(None);
                let next = rows.len() as u32;
                let row = *row_index.entry(entries.clone()).or_insert_with(|| {
                    rows.push(entries);
                    next
                });
                conditional.push(row);
            }
        }
    }

    let contexts = nfa
        .contexts
        .iter()
        .zip(&context_owner)
        .map(|((pat, negated), token)| build_context(pat, *negated, token))
        .collect::<Result<Vec<_>, _>>()?;

    let dfa = ModeDfa { start: 0, trans, accept, conditional, rows, contexts };

    // Empty-match check: the start state must not accept, with or
    // without a context (a context consumes nothing).
    if let Some(tok) = dfa.may_accept(0) {
        return Err(CompileError::EmptyMatch { token: g.tokens[tok as usize].name.clone() });
    }

    Ok(dfa)
}
