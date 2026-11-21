//! Table-driven batch LR parse over a terminal stream, producing a plain
//! tree (green trees arrive in the next increment) plus syntax errors
//! carrying EXPECTED-TOKEN SETS — the same table row that will later
//! drive completion, exercised from day one.

use crate::lr::{LrAct, LrTables};
use crate::model::TokenId;
use crate::syn::{SynGrammar, EOF};

/// A terminal fed to the parser: id + slice of source text (for display
/// and tree rendering; positions come with the green-tree increment).
#[derive(Clone, Debug)]
pub struct TermTok {
    pub id: TokenId,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PNode {
    Tok { id: TokenId, text: String },
    Rule { prod: u16, nt: u16, children: Vec<PNode> },
}

#[derive(Clone, Debug)]
pub struct ParseError {
    /// Index into the token stream (== stream.len() for unexpected EOF).
    pub at: usize,
    pub found: String,
    pub expected: Vec<String>,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "syntax error at token #{} ({}): expected one of {}",
            self.at,
            self.found,
            self.expected.join(", ")
        )
    }
}

pub fn parse(
    g: &SynGrammar,
    t: &LrTables,
    tokens: &[TermTok],
) -> Result<PNode, ParseError> {
    let mut states: Vec<u16> = vec![0];
    let mut nodes: Vec<PNode> = Vec::new();
    let mut pos = 0usize;

    loop {
        let state = *states.last().unwrap();
        let (la, la_disp) = if pos < tokens.len() {
            (tokens[pos].id, format!("`{}`", tokens[pos].text))
        } else {
            (EOF, "<eof>".to_string())
        };
        match t.action[state as usize].get(&la).copied() {
            Some(LrAct::Shift(next)) => {
                nodes.push(PNode::Tok { id: la, text: tokens[pos].text.clone() });
                states.push(next);
                pos += 1;
            }
            Some(LrAct::Reduce(pidx)) => {
                let prod = &g.prods[pidx as usize];
                let k = prod.rhs.len();
                let children = nodes.split_off(nodes.len() - k);
                for _ in 0..k {
                    states.pop();
                }
                let top = *states.last().unwrap();
                let next = *t.goto_[top as usize]
                    .get(&prod.lhs)
                    .expect("goto must exist after reduce");
                nodes.push(PNode::Rule { prod: pidx, nt: prod.lhs, children });
                states.push(next);
            }
            Some(LrAct::Accept) => {
                debug_assert_eq!(nodes.len(), 1);
                return Ok(nodes.pop().unwrap());
            }
            Some(LrAct::Error) | None => {
                return Err(ParseError {
                    at: pos,
                    found: la_disp,
                    expected: t.expected_tokens(state, g),
                });
            }
        }
    }
}

/// Render a tree as an s-expression over nonterminal names and terminal
/// names — the golden-test format (tree-sitter corpus style).
pub fn sexpr(g: &SynGrammar, node: &PNode) -> String {
    match node {
        PNode::Tok { id, .. } => g.term_name(*id).to_string(),
        PNode::Rule { nt, children, .. } => {
            let mut s = format!("({}", g.nt_names[*nt as usize]);
            for c in children {
                s.push(' ');
                s.push_str(&sexpr(g, c));
            }
            s.push(')');
            s
        }
    }
}

/// How many of `tokens` the rule whose start state is `start` (state 0, the
/// start rule's, or an entry's: [`LrTables::entry_states`]) parses, read
/// greedily as a macro's fragment is: tokens are taken while the rule can
/// go on, and where the next one can't continue it, the rule must end there
/// (`Err` with where that is if it can't: there is no shorter reading to
/// fall back to). At the end of `tokens`, the rule must end too (`Err` with
/// their length if it can't).
pub fn parse_prefix(
    g: &SynGrammar,
    t: &LrTables,
    start: u16,
    tokens: &[TokenId],
) -> Result<usize, usize> {
    let mut states: Vec<u16> = vec![start];
    let mut pos = 0usize;
    loop {
        if pos == tokens.len() {
            return if accepts_at_eof(g, t, states) { Ok(pos) } else { Err(pos) };
        }
        // Reduce on the token as far as it takes, then shift it; if it
        // can't be shifted, the rule ends before it, from where it stood.
        let before = states.clone();
        loop {
            let state = *states.last().expect("a state");
            match t.action[state as usize].get(&tokens[pos]).copied() {
                Some(LrAct::Shift(next)) => {
                    states.push(next);
                    pos += 1;
                    break;
                }
                Some(LrAct::Reduce(pidx)) => reduce(g, t, &mut states, pidx),
                Some(LrAct::Accept) | Some(LrAct::Error) | None => {
                    return if accepts_at_eof(g, t, before) { Ok(pos) } else { Err(pos) };
                }
            }
        }
    }
}

/// Whether the parse whose states are `states` accepts if its input ends
/// here.
fn accepts_at_eof(g: &SynGrammar, t: &LrTables, mut states: Vec<u16>) -> bool {
    loop {
        let state = *states.last().expect("a state");
        match t.action[state as usize].get(&EOF).copied() {
            Some(LrAct::Accept) => return true,
            Some(LrAct::Reduce(pidx)) => reduce(g, t, &mut states, pidx),
            Some(LrAct::Shift(_)) | Some(LrAct::Error) | None => return false,
        }
    }
}

/// Reduce by production `pidx` on `states`.
fn reduce(g: &SynGrammar, t: &LrTables, states: &mut Vec<u16>, pidx: u16) {
    let prod = &g.prods[pidx as usize];
    for _ in 0..prod.rhs.len() {
        states.pop();
    }
    let top = *states.last().expect("a state under a reduction");
    let next = *t.goto_[top as usize].get(&prod.lhs).expect("goto must exist after reduce");
    states.push(next);
}
