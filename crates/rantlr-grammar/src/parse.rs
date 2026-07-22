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
