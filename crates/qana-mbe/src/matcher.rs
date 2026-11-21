//! Matching an invocation against a definition's arms: rustc's
//! `mbe::macro_parser` (`TtParser`), an NFA over matcher positions that
//! calls out to the parser where exactly one fragment can go on.

use std::collections::HashMap;
use std::rc::Rc;

use qana_lang::EmbeddedLang;

use crate::input::{may_begin_with, token_descr, Cur, Cursor, Input, Nt};
use crate::rules::{ArmKind, Kleene, MacroRules, MatcherLoc};
use crate::token::{desugar_docs, token_name_eq, Delim, TokenKind, TokenStream};
use crate::Error;

/// What a meta-variable matched: one fragment, or one per repetition.
#[derive(Clone, Debug)]
pub(crate) enum NamedMatch {
    Seq(Vec<NamedMatch>),
    Single(Rc<Nt>),
}

pub(crate) type Matches = HashMap<String, NamedMatch>;

#[derive(Clone)]
struct MatcherPos {
    idx: usize,
    matches: Rc<Vec<NamedMatch>>,
}

impl MatcherPos {
    fn push_match(&mut self, metavar_idx: usize, seq_depth: usize, m: NamedMatch) {
        let matches = Rc::make_mut(&mut self.matches);
        if seq_depth == 0 {
            debug_assert_eq!(metavar_idx, matches.len());
            matches.push(m);
            return;
        }
        let mut curr = &mut matches[metavar_idx];
        for _ in 0..seq_depth - 1 {
            match curr {
                NamedMatch::Seq(seq) => curr = seq.last_mut().expect("an open repetition"),
                NamedMatch::Single(_) => unreachable!("a repetition's match"),
            }
        }
        match curr {
            NamedMatch::Seq(seq) => seq.push(m),
            NamedMatch::Single(_) => unreachable!("a repetition's match"),
        }
    }
}

enum Outcome {
    Success(Vec<NamedMatch>),
    /// The arm does not match at a token. Another arm is tried.
    Failure(Cursor),
    /// The invocation is an error: no other arm is tried.
    Error(Error),
}

/// Match `input` against `rules`' function-like arms in order: the first
/// that matches, and its matches by name.
pub(crate) fn match_arms(
    lang: &EmbeddedLang,
    rules: &MacroRules,
    input: &TokenStream,
) -> Result<(usize, Matches), Error> {
    let trees = desugar_docs(&input.trees)?;
    let mut input = Input::new(lang, &trees);
    let mut best: Option<Cursor> = None;
    for (i, arm) in rules.arms.iter().enumerate() {
        if arm.kind != ArmKind::Func {
            continue;
        }
        match parse_tt(&mut input, &arm.locs) {
            Outcome::Success(matches) => return Ok((i, nameize(&arm.locs, matches)?)),
            Outcome::Failure(at) => {
                if best.is_none_or(|b| (at.flat, at.at) > (b.flat, b.at)) {
                    best = Some(at);
                }
            }
            Outcome::Error(e) => return Err(e),
        }
    }
    // rustc's message, at the token the arm that got furthest failed at.
    let at = best.unwrap_or(input.start());
    let message = match input.cur(at) {
        Cur::Eof => "unexpected end of macro invocation".to_string(),
        cur => format!("no rules expected {}", token_descr(&cur)),
    };
    Err(Error::new(message, input.origin(at)))
}

fn matches_token(loc: &MatcherLoc, cur: &Cur) -> bool {
    match (loc, cur) {
        (MatcherLoc::Token(t), Cur::Token(c)) => token_name_eq(t, c),
        (MatcherLoc::Open(d), Cur::Open(c)) | (MatcherLoc::Close(d), Cur::Close(c)) => {
            d == c && !matches!(d, Delim::Invisible(_))
        }
        _ => false,
    }
}

fn parse_tt(input: &mut Input, locs: &[MatcherLoc]) -> Outcome {
    let mut cur_mps = vec![MatcherPos { idx: 0, matches: Rc::new(Vec::new()) }];
    let mut next_mps: Vec<MatcherPos> = Vec::new();
    let mut bb_mps: Vec<MatcherPos> = Vec::new();
    let mut cursor = input.start();
    loop {
        next_mps.clear();
        bb_mps.clear();
        let token = input.cur(cursor);
        let at_eof = matches!(token, Cur::Eof);
        // Positions that would be done if the invocation ended here.
        let mut eof_mps: (usize, Option<MatcherPos>) = (0, None);
        while let Some(mut mp) = cur_mps.pop() {
            match &locs[mp.idx] {
                loc @ (MatcherLoc::Token(_) | MatcherLoc::Open(_) | MatcherLoc::Close(_)) => {
                    // A doc comment in a matcher matches nothing and is
                    // passed over, as rustc does.
                    if matches!(loc, MatcherLoc::Token(t) if t.kind == TokenKind::DocComment) {
                        mp.idx += 1;
                        cur_mps.push(mp);
                    } else if matches_token(loc, &token) {
                        mp.idx += 1;
                        next_mps.push(mp);
                    }
                }
                MatcherLoc::Delimited => {
                    mp.idx += 1;
                    cur_mps.push(mp);
                }
                &MatcherLoc::Sequence {
                    op,
                    num_metavar_decls,
                    idx_first_after,
                    next_metavar,
                    seq_depth,
                } => {
                    for metavar_idx in next_metavar..next_metavar + num_metavar_decls {
                        mp.push_match(metavar_idx, seq_depth, NamedMatch::Seq(Vec::new()));
                    }
                    if matches!(op, Kleene::ZeroOrMore | Kleene::ZeroOrOne) {
                        cur_mps.push(MatcherPos {
                            idx: idx_first_after,
                            matches: Rc::clone(&mp.matches),
                        });
                    }
                    mp.idx += 1;
                    cur_mps.push(mp);
                }
                &MatcherLoc::SequenceKleeneOpNoSep { op, idx_first } => {
                    cur_mps.push(MatcherPos { idx: mp.idx + 1, matches: Rc::clone(&mp.matches) });
                    if op != Kleene::ZeroOrOne {
                        mp.idx = idx_first;
                        cur_mps.push(mp);
                    }
                }
                MatcherLoc::SequenceSep { separator } => {
                    cur_mps.push(MatcherPos { idx: mp.idx + 2, matches: Rc::clone(&mp.matches) });
                    if matches!(&token, Cur::Token(t) if token_name_eq(t, separator)) {
                        mp.idx += 1;
                        next_mps.push(mp);
                    }
                }
                &MatcherLoc::SequenceKleeneOpAfterSep { idx_first } => {
                    mp.idx = idx_first;
                    cur_mps.push(mp);
                }
                &MatcherLoc::MetaVarDecl { kind, .. } => {
                    if may_begin_with(kind, &token) {
                        bb_mps.push(mp);
                    }
                }
                MatcherLoc::Eof => {
                    if at_eof {
                        eof_mps = match eof_mps {
                            (0, _) => (1, Some(mp)),
                            (n, _) => (n + 1, None),
                        };
                    }
                }
            }
        }
        if at_eof {
            return match eof_mps {
                (1, Some(mp)) => {
                    let matches = Rc::try_unwrap(mp.matches).unwrap_or_else(|rc| (*rc).clone());
                    Outcome::Success(matches)
                }
                (0, _) => Outcome::Failure(cursor),
                _ => Outcome::Error(Error::new(
                    "ambiguity: multiple successful parses",
                    input.origin(cursor),
                )),
            };
        }
        match (next_mps.len(), bb_mps.len()) {
            (0, 0) => return Outcome::Failure(cursor),
            (_, 0) => {
                cur_mps.append(&mut next_mps);
                cursor = input.bump(cursor);
            }
            (0, 1) => {
                let mut mp = bb_mps.pop().expect("one position");
                let &MatcherLoc::MetaVarDecl { kind, next_metavar, seq_depth, .. } = &locs[mp.idx]
                else {
                    unreachable!("a fragment's position")
                };
                match input.parse_nonterminal(kind, cursor) {
                    Ok((nt, next)) => {
                        mp.push_match(next_metavar, seq_depth, NamedMatch::Single(Rc::new(nt)));
                        mp.idx += 1;
                        cursor = next;
                    }
                    Err(e) => return Outcome::Error(e),
                }
                cur_mps.push(mp);
            }
            _ => {
                let nts: Vec<String> = bb_mps
                    .iter()
                    .map(|mp| match &locs[mp.idx] {
                        MatcherLoc::MetaVarDecl { name, kind, .. } => {
                            format!("{} ('{name}')", kind.name())
                        }
                        _ => unreachable!("a fragment's position"),
                    })
                    .collect();
                let others = match next_mps.len() {
                    0 => String::new(),
                    1 => " or 1 other option".to_string(),
                    n => format!(" or {n} other options"),
                };
                return Outcome::Error(Error::new(
                    format!(
                        "local ambiguity when calling macro: multiple parsing options: built-in \
                         NTs {}{others}.",
                        nts.join(" or ")
                    ),
                    input.origin(cursor),
                ));
            }
        }
    }
}

/// The matches by name, each name bound once.
fn nameize(locs: &[MatcherLoc], matches: Vec<NamedMatch>) -> Result<Matches, Error> {
    let mut out = HashMap::new();
    let mut it = matches.into_iter();
    for (name, origin) in crate::rules::decl_names(locs) {
        let m = it.next().expect("a match per declaration");
        if out.insert(name.clone(), m).is_some() {
            return Err(Error::new(format!("duplicated bind name: {name}"), Some(origin)));
        }
    }
    Ok(out)
}
