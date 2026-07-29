//! The envelope lints, lexical tier. The tool refuses grammars outside
//! the envelope **with counterexamples** — that promise is implemented
//! here, on the compiled automata (not on the pattern syntax), so there
//! is no gap between what's checked and what runs.
//!
//! * **L1 — line-local tokens:** no token may match a string containing a
//!   line terminator. Checked by reachability over the DFA's `\n`/`\r`
//!   columns; violations come with a witness string.
//! * **L2 — finite line-start state:** the mode-push graph must be
//!   acyclic (natural static bound) or the grammar must declare
//!   `max_stack ≤ MAX_STACK`; violations name the push cycle.

use crate::dfa::{ModeDfa, DEAD};
use crate::lexer::MAX_STACK;
use crate::model::{Action, LexGrammar};
use crate::pat::{representative, NCOLS};

#[derive(Clone, Debug)]
pub enum LintError {
    /// L1: `token` can match `witness` (which contains a line terminator).
    TokenSpansLines { token: String, witness: String },
    /// L2: mode-push cycle with no declared stack bound.
    UnboundedModeStack { cycle: Vec<String> },
    /// L2: declared bound exceeds engine capacity.
    StackBoundTooLarge { declared: u8, max: u8 },
    /// `@continues` on a token whose mode is not eol-popped: the splice
    /// suppresses an eol pop that would never happen — dead config is
    /// refused, not ignored.
    ContinuationOutsideLineBoundedMode { token: String, mode: String },
}

impl std::fmt::Display for LintError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LintError::TokenSpansLines { token, witness } => write!(
                f,
                "L1: token `{token}` can span a line break — e.g. it matches {witness:?}. \
                 Multi-line constructs must be expressed via mode state, not tokens."
            ),
            LintError::UnboundedModeStack { cycle } => write!(
                f,
                "L2: mode push cycle {} makes the line-start state unbounded; \
                 declare `max_stack` (≤ {MAX_STACK}) to bound it explicitly",
                cycle.join(" → ")
            ),
            LintError::StackBoundTooLarge { declared, max } => {
                write!(f, "L2: declared max_stack {declared} exceeds engine capacity {max}")
            }
            LintError::ContinuationOutsideLineBoundedMode { token, mode } => write!(
                f,
                "`@continues` on token `{token}`: its mode `{mode}` is not line-bounded \
                 (`@push({mode}, eol)`), so there is no end-of-line pop to suppress. \
                 A splice token belongs in the mode it splices."
            ),
        }
    }
}

/// Structural facts the lints certify, reported back to the author.
#[derive(Clone, Debug)]
pub struct EnvelopeReport {
    /// Static bound on the mode stack depth.
    pub stack_bound: u8,
    /// Number of distinct line-start states (upper bound):
    /// Σ_{d=0..=bound} pushable_modes^d.
    pub line_state_space: u64,
    /// Per-mode DFA state counts (a size sanity metric).
    pub dfa_states: Vec<usize>,
}

/// L1: for each mode DFA, no reachable path may traverse a `\n` or `\r`
/// column and still reach an accepting state. Returns a witness on
/// violation: shortest `prefix + terminator + suffix` accepted.
pub fn check_l1(g: &LexGrammar, dfas: &[ModeDfa]) -> Result<(), LintError> {
    for dfa in dfas {
        // BFS shortest path strings from start to every state.
        let n = dfa.trans.len();
        let mut path: Vec<Option<String>> = vec![None; n];
        path[dfa.start as usize] = Some(String::new());
        let mut queue = std::collections::VecDeque::from([dfa.start]);
        while let Some(s) = queue.pop_front() {
            let prefix = path[s as usize].clone().unwrap();
            for (col, &t) in dfa.trans[s as usize].iter().enumerate() {
                if t != DEAD && path[t as usize].is_none() {
                    let mut p = prefix.clone();
                    p.push(representative(col));
                    path[t as usize] = Some(p);
                    queue.push_back(t);
                }
            }
        }
        // From every state reached via \n or \r, can we reach acceptance?
        for term in ['\n', '\r'] {
            let col = term as usize;
            for (s, row) in dfa.trans.iter().enumerate() {
                if path[s].is_none() || row[col] == DEAD {
                    continue;
                }
                let after = row[col];
                if let Some((tok, suffix)) = reach_accept(dfa, after) {
                    let mut witness = path[s].clone().unwrap();
                    witness.push(term);
                    witness.push_str(&suffix);
                    return Err(LintError::TokenSpansLines {
                        token: g.tokens[tok as usize].name.clone(),
                        witness,
                    });
                }
            }
        }
    }
    Ok(())
}

/// Shortest suffix from `from` to any accepting state, if one exists.
fn reach_accept(dfa: &ModeDfa, from: u16) -> Option<(u16, String)> {
    let n = dfa.trans.len();
    let mut path: Vec<Option<String>> = vec![None; n];
    path[from as usize] = Some(String::new());
    let mut queue = std::collections::VecDeque::from([from]);
    while let Some(s) = queue.pop_front() {
        if let Some(tok) = dfa.accept[s as usize] {
            return Some((tok, path[s as usize].clone().unwrap()));
        }
        let prefix = path[s as usize].clone().unwrap();
        for col in 0..NCOLS {
            let t = dfa.trans[s as usize][col];
            if t != DEAD && path[t as usize].is_none() {
                let mut p = prefix.clone();
                p.push(representative(col));
                path[t as usize] = Some(p);
                queue.push_back(t);
            }
        }
    }
    None
}

/// L2: bound the mode stack. Returns the certified bound.
pub fn check_l2(g: &LexGrammar) -> Result<u8, LintError> {
    let m = g.mode_names.len();
    // push adjacency: mode -> modes it can push
    let mut adj: Vec<Vec<usize>> = vec![vec![]; m];
    for def in &g.tokens {
        if let Action::Push(target) = def.action {
            adj[def.mode as usize].push(target as usize);
        }
    }
    // Cycle detection (DFS with colors), recording one cycle if found.
    let mut color = vec![0u8; m]; // 0 white, 1 gray, 2 black
    let mut stack: Vec<usize> = vec![];
    fn dfs(
        u: usize,
        adj: &[Vec<usize>],
        color: &mut [u8],
        stack: &mut Vec<usize>,
    ) -> Option<Vec<usize>> {
        color[u] = 1;
        stack.push(u);
        for &v in &adj[u] {
            if color[v] == 1 {
                let pos = stack.iter().position(|&x| x == v).unwrap();
                let mut cycle = stack[pos..].to_vec();
                cycle.push(v);
                return Some(cycle);
            }
            if color[v] == 0 {
                if let Some(c) = dfs(v, adj, color, stack) {
                    return Some(c);
                }
            }
        }
        stack.pop();
        color[u] = 2;
        None
    }
    let mut cycle: Option<Vec<usize>> = None;
    for u in 0..m {
        if color[u] == 0 {
            if let Some(c) = dfs(u, &adj, &mut color, &mut stack, ) {
                cycle = Some(c);
                break;
            }
        }
    }

    let bound = match (cycle, g.max_stack) {
        (Some(c), None) => {
            return Err(LintError::UnboundedModeStack {
                cycle: c.into_iter().map(|i| g.mode_names[i].clone()).collect(),
            })
        }
        (_, Some(b)) => b,
        (None, None) => {
            // DAG: longest push chain is the natural bound.
            fn longest(u: usize, adj: &[Vec<usize>], memo: &mut [i32]) -> u8 {
                if memo[u] >= 0 {
                    return memo[u] as u8;
                }
                let mut best = 0u8;
                for &v in &adj[u] {
                    best = best.max(1 + longest(v, adj, memo));
                }
                memo[u] = best as i32;
                best
            }
            let mut memo = vec![-1i32; m];
            (0..m).map(|u| longest(u, &adj, &mut memo)).max().unwrap_or(0)
        }
    };
    if bound as usize > MAX_STACK {
        return Err(LintError::StackBoundTooLarge { declared: bound, max: MAX_STACK as u8 });
    }
    Ok(bound)
}

/// Run all lexical-tier lints; produce the envelope report.
pub fn check_envelope(g: &LexGrammar, dfas: &[ModeDfa]) -> Result<EnvelopeReport, LintError> {
    check_l1(g, dfas)?;
    let bound = check_l2(g)?;
    // A `@continues` splice only means something where an eol pop
    // exists to suppress. Anywhere else it is dead config — refused
    // with the fix, not silently ignored (the @scope(unordered)
    // lesson: attributes that do nothing hide real intent).
    for t in &g.tokens {
        if t.continues && !g.eol_pop.get(t.mode as usize).copied().unwrap_or(false) {
            return Err(LintError::ContinuationOutsideLineBoundedMode {
                token: t.name.clone(),
                mode: g.mode_names[t.mode as usize].clone(),
            });
        }
    }
    let pushable = g
        .tokens
        .iter()
        .filter_map(|t| match t.action {
            Action::Push(m) => Some(m),
            _ => None,
        })
        .collect::<std::collections::HashSet<_>>()
        .len() as u64;
    let mut space = 0u64;
    let mut pow = 1u64;
    for _ in 0..=bound {
        space = space.saturating_add(pow);
        pow = pow.saturating_mul(pushable.max(1));
    }
    Ok(EnvelopeReport {
        stack_bound: bound,
        line_state_space: space,
        dfa_states: dfas.iter().map(|d| d.trans.len()).collect(),
    })
}
