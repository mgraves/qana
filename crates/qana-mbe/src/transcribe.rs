//! Filling an arm's transcriber in with what its matcher matched: rustc's
//! `mbe::transcribe`. What the transcriber writes is marked with the
//! expansion; a fragment keeps the tokens it matched, an identifier or a
//! lifetime as itself, anything else as one invisible group of its kind.

use std::sync::Arc;

use crate::input::Nt;
use crate::matcher::{Matches, NamedMatch};
use crate::rules::{metavar_origin, ConcatElem, Kleene, MacroRules, MetaVarExpr, Mtt};
use crate::token::{Delim, Group, MetaVarKind, Token, TokenKind, TokenStream, Tree};
use crate::{Error, Origin};

struct Cx<'a> {
    interp: &'a Matches,
    mark: u32,
    /// The repetitions being transcribed: (this one's index, how many).
    repeats: Vec<(usize, usize)>,
}

pub(crate) fn transcribe(
    rules: &MacroRules,
    arm: usize,
    interp: &Matches,
    mark: u32,
) -> Result<TokenStream, Error> {
    let Mtt::Delimited { tts, .. } = &rules.arms[arm].rhs else {
        unreachable!("a transcriber is delimited")
    };
    let mut cx = Cx { interp, mark, repeats: Vec::new() };
    let mut out = Vec::new();
    transcribe_tts(tts, &mut cx, &mut out)?;
    Ok(TokenStream::new(out))
}

fn transcribe_tts(tts: &[Mtt], cx: &mut Cx, out: &mut Vec<Tree>) -> Result<(), Error> {
    for tt in tts {
        match tt {
            Mtt::Sequence { tts: seq, sep, op, origin, .. } => {
                match lockstep_iter_size(tt, cx.interp, &cx.repeats) {
                    Lockstep::Unconstrained => {
                        return Err(Error::new(
                            "attempted to repeat an expression containing no syntax variables \
                             matched as repeating at this depth",
                            Some(*origin),
                        ))
                    }
                    Lockstep::Contradiction(msg) => return Err(Error::new(msg, Some(*origin))),
                    Lockstep::Constraint(len, _) => {
                        if len == 0 {
                            if *op == Kleene::OneOrMore {
                                return Err(Error::new(
                                    "this must repeat at least once",
                                    Some(*origin),
                                ));
                            }
                            continue;
                        }
                        cx.repeats.push((0, len));
                        for idx in 0..len {
                            cx.repeats.last_mut().expect("a repetition").0 = idx;
                            if idx > 0 {
                                // A separator keeps its span unmarked, as in
                                // rustc.
                                if let Some(sep) = sep {
                                    out.push(Tree::Token(sep.clone()));
                                }
                            }
                            transcribe_tts(seq, cx, out)?;
                        }
                        cx.repeats.pop();
                    }
                }
            }
            Mtt::MetaVar { name, dollar, ident } => {
                transcribe_metavar(name, dollar, ident, cx, out)?;
            }
            Mtt::MetaVarExpr { expr, origin, edition, .. } => {
                transcribe_metavar_expr(expr, *origin, *edition, cx, out)?;
            }
            Mtt::Delimited { delim, open, close, edition, tts, .. } => {
                let mut inner = Vec::new();
                transcribe_tts(tts, cx, &mut inner)?;
                out.push(Tree::Group(Group {
                    delim: *delim,
                    open: *open,
                    close: *close,
                    edition: *edition,
                    mark: cx.mark,
                    trees: Arc::new(inner),
                }));
            }
            Mtt::Token(t) => out.push(Tree::Token(t.remarked(t.origin, cx.mark))),
            Mtt::MetaVarDecl { .. } => unreachable!("a transcriber declares no fragments"),
        }
    }
    Ok(())
}

fn lookup_cur_matched<'a>(
    name: &str,
    interp: &'a Matches,
    repeats: &[(usize, usize)],
) -> Option<&'a NamedMatch> {
    interp.get(name).map(|mut matched| {
        for &(idx, _) in repeats {
            match matched {
                NamedMatch::Single(_) => break,
                NamedMatch::Seq(ads) => matched = &ads[idx],
            }
        }
        matched
    })
}

fn transcribe_metavar(
    name: &str,
    dollar: &Token,
    ident: &Token,
    cx: &mut Cx,
    out: &mut Vec<Tree>,
) -> Result<(), Error> {
    let sp = metavar_origin(dollar, ident);
    let Some(matched) = lookup_cur_matched(name, cx.interp, &cx.repeats) else {
        // Not a meta-variable of the matcher: `$` and the name, as written.
        out.push(Tree::Token(dollar.remarked(dollar.origin, cx.mark)));
        out.push(Tree::Token(ident.remarked(ident.origin, cx.mark)));
        return Ok(());
    };
    let NamedMatch::Single(nt) = matched else {
        return Err(Error::new(
            format!("variable `{name}` is still repeating at this depth"),
            Some(sp),
        ));
    };
    out.push(match &**nt {
        Nt::Tt(tree) => tree.clone(),
        Nt::Ident(t) | Nt::Lifetime(t) => {
            let mut t = t.clone();
            t.interpolated = true;
            Tree::Token(t)
        }
        Nt::Node { kind, trees, .. } => {
            // A fragment that is one group of its own kind is not grouped
            // again.
            let stream = match trees.as_slice() {
                [Tree::Group(g)] if g.delim == Delim::Invisible(*kind) => g.trees.clone(),
                _ => Arc::new(trees.clone()),
            };
            Tree::Group(Group {
                delim: Delim::Invisible(*kind),
                open: sp,
                close: sp,
                edition: dollar.edition,
                mark: cx.mark,
                trees: stream,
            })
        }
    });
    Ok(())
}

fn matched_from_ident<'a>(
    name: &str,
    interp: &'a Matches,
    at: Origin,
) -> Result<&'a NamedMatch, Error> {
    interp.get(name).ok_or_else(|| {
        Error::new(
            format!("variable `{name}` is not recognized in meta-variable expression"),
            Some(at),
        )
    })
}

fn int_token(n: usize, origin: Origin, edition: crate::Edition, mark: u32) -> Tree {
    Tree::Token(Token::new(TokenKind::Literal, &n.to_string(), origin, edition, mark))
}

fn transcribe_metavar_expr(
    expr: &MetaVarExpr,
    origin: Origin,
    edition: crate::Edition,
    cx: &mut Cx,
    out: &mut Vec<Tree>,
) -> Result<(), Error> {
    match expr {
        MetaVarExpr::Concat(elems) => {
            let mut s = String::new();
            for elem in elems {
                match elem {
                    ConcatElem::Ident(name) | ConcatElem::Literal(name) => s.push_str(name),
                    ConcatElem::Var(name) => {
                        let nt = match matched_from_ident(name, cx.interp, origin)? {
                            NamedMatch::Seq(seq) => {
                                let Some(&(idx, _)) = cx.repeats.last() else {
                                    return Err(Error::new("invalid syntax", Some(origin)));
                                };
                                match &seq[idx] {
                                    NamedMatch::Seq(_) => {
                                        return Err(Error::new(
                                            "nested repetitions with `${concat(...)}` \
                                             metavariable expressions are not yet supported",
                                            Some(origin),
                                        ))
                                    }
                                    NamedMatch::Single(nt) => nt.clone(),
                                }
                            }
                            NamedMatch::Single(nt) => nt.clone(),
                        };
                        s.push_str(&concat_symbol(&nt, origin)?);
                    }
                }
            }
            let valid = s.chars().next().is_some_and(|c| c == '_' || c.is_alphabetic())
                && s.chars().all(|c| c == '_' || c.is_alphanumeric());
            if !valid {
                return Err(Error::new(
                    "`${concat(..)}` is not generating a valid identifier",
                    Some(origin),
                ));
            }
            out.push(Tree::Token(Token::new(TokenKind::Ident, &s, origin, edition, cx.mark)));
        }
        MetaVarExpr::Count(name, depth) => {
            let matched = matched_from_ident(name, cx.interp, origin)?;
            let n = count_repetitions(*depth, matched, &cx.repeats, origin)?;
            out.push(int_token(n, origin, edition, cx.mark));
        }
        MetaVarExpr::Ignore(name) => {
            matched_from_ident(name, cx.interp, origin)?;
        }
        MetaVarExpr::Index(depth) => match cx.repeats.iter().nth_back(*depth) {
            Some(&(index, _)) => out.push(int_token(index, origin, edition, cx.mark)),
            None => return Err(out_of_bounds(cx.repeats.len(), origin, "index")),
        },
        MetaVarExpr::Len(depth) => match cx.repeats.iter().nth_back(*depth) {
            Some(&(_, len)) => out.push(int_token(len, origin, edition, cx.mark)),
            None => return Err(out_of_bounds(cx.repeats.len(), origin, "len")),
        },
    }
    Ok(())
}

fn out_of_bounds(max: usize, origin: Origin, ty: &str) -> Error {
    let msg = if max == 0 {
        format!(
            "meta-variable expression `{ty}` with depth parameter must be called inside of a \
             macro repetition"
        )
    } else {
        format!("depth parameter of meta-variable expression `{ty}` must be less than {max}")
    };
    Error::new(msg, Some(origin))
}

/// What a fragment adds to a `${concat(..)}`, as rustc's
/// `extract_symbol_from_pnr` says: an identifier's name, a string
/// literal's text, an unsuffixed integer's digits.
fn concat_symbol(nt: &Nt, origin: Origin) -> Result<String, Error> {
    // A plain string without a suffix (rustc's `LitKind::Str`): its text
    // between the quotes, escapes as written.
    let string = |t: &Token| -> Option<String> {
        (t.kind == TokenKind::Literal
            && t.text.len() >= 2
            && t.text.starts_with('"')
            && t.text.ends_with('"'))
        .then(|| t.text[1..t.text.len() - 1].to_string())
    };
    match nt {
        Nt::Ident(t) | Nt::Tt(Tree::Token(t)) if t.kind == TokenKind::Ident => match t.ident() {
            Some((name, false)) => Ok(name.to_string()),
            _ => Err(Error::new(
                "`${concat(..)}` currently does not support raw identifiers",
                Some(origin),
            )),
        },
        Nt::Tt(Tree::Token(t)) if string(t).is_some() => Ok(string(t).expect("a string")),
        Nt::Node { kind: MetaVarKind::Literal, trees } => {
            // A forwarded literal is the literal it holds.
            let mut trees = trees.as_slice();
            while let [Tree::Group(g)] = trees {
                trees = g.trees.as_slice();
            }
            match trees {
                [Tree::Token(t)] if string(t).is_some() => Ok(string(t).expect("a string")),
                [Tree::Token(t)] if t.kind == TokenKind::Literal => match integer(&t.text) {
                    Some((_, "")) => Ok(t.text.to_string()),
                    Some((_, "f16" | "f32" | "f64" | "f128")) => Err(Error::new(
                        "floats are not supported as metavariables of `${concat(..)}`",
                        Some(origin),
                    )),
                    Some(_) => Err(Error::new(
                        "integer metavariables of `${concat(..)}` must not be suffixed",
                        Some(origin),
                    )),
                    None => Err(concat_kind_err(origin)),
                },
                _ => Err(concat_kind_err(origin)),
            }
        }
        _ => Err(concat_kind_err(origin)),
    }
}

fn concat_kind_err(origin: Origin) -> Error {
    Error::new(
        "metavariables of `${concat(..)}` must be of type `ident`, `literal` or `tt` (currently \
         only string and integer literals are supported)",
        Some(origin),
    )
}

/// An integer literal's digits and suffix, as rustc's lexer reads its
/// text; `None` for any other literal (a float: a decimal with a `.` or an
/// exponent after its digits).
fn integer(text: &str) -> Option<(&str, &str)> {
    if !text.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let (prefix, hex) = if text.starts_with("0x") {
        (2, true)
    } else if text.starts_with("0o") || text.starts_with("0b") {
        (2, false)
    } else {
        (0, false)
    };
    let digit = |c: char| c == '_' || if hex { c.is_ascii_hexdigit() } else { c.is_ascii_digit() };
    let end = prefix + text[prefix..].find(|c: char| !digit(c)).unwrap_or(text.len() - prefix);
    let rest = &text[end..];
    if prefix == 0 {
        let mut chars = rest.chars();
        let float = match chars.next() {
            Some('.') => true,
            Some('e' | 'E') => {
                matches!(chars.next(), Some(c) if c.is_ascii_digit() || matches!(c, '+' | '-' | '_'))
            }
            _ => false,
        };
        if float {
            return None;
        }
    }
    Some((&text[..end], rest))
}

fn count_repetitions(
    depth_user: usize,
    matched: &NamedMatch,
    repeats: &[(usize, usize)],
    origin: Origin,
) -> Result<usize, Error> {
    fn count(depth_curr: usize, depth_max: usize, matched: &NamedMatch) -> usize {
        match matched {
            NamedMatch::Single(_) => 1,
            NamedMatch::Seq(ms) => {
                if depth_curr == depth_max {
                    ms.len()
                } else {
                    ms.iter().map(|m| count(depth_curr + 1, depth_max, m)).sum()
                }
            }
        }
    }
    fn depth(counter: usize, matched: &NamedMatch) -> usize {
        match matched {
            NamedMatch::Single(_) => counter,
            NamedMatch::Seq(ms) => match ms.first() {
                Some(m) => depth(counter + 1, m),
                None => counter + 1,
            },
        }
    }
    let depth_max = depth(0, matched)
        .checked_sub(1)
        .and_then(|d| d.checked_sub(repeats.len()))
        .unwrap_or_default();
    if depth_user > depth_max {
        return Err(out_of_bounds(depth_max + 1, origin, "count"));
    }
    let mut matched = matched;
    for &(idx, _) in repeats {
        if let NamedMatch::Seq(ms) = matched {
            matched = &ms[idx];
        }
    }
    if let NamedMatch::Single(_) = matched {
        return Err(Error::new(
            "`count` can not be placed inside the innermost repetition",
            Some(origin),
        ));
    }
    Ok(count(depth_user, depth_max, matched))
}

enum Lockstep {
    Unconstrained,
    Constraint(usize, String),
    Contradiction(String),
}

impl Lockstep {
    fn with(self, other: Lockstep) -> Lockstep {
        match self {
            Lockstep::Unconstrained => other,
            Lockstep::Contradiction(_) => self,
            Lockstep::Constraint(l_len, l_id) => match other {
                Lockstep::Unconstrained => Lockstep::Constraint(l_len, l_id),
                Lockstep::Contradiction(_) => other,
                Lockstep::Constraint(r_len, _) if l_len == r_len => Lockstep::Constraint(l_len, l_id),
                Lockstep::Constraint(r_len, r_id) => Lockstep::Contradiction(format!(
                    "meta-variable `{l_id}` repeats {l_len} time{}, but `{r_id}` repeats {r_len} time{}",
                    if l_len == 1 { "" } else { "s" },
                    if r_len == 1 { "" } else { "s" },
                )),
            },
        }
    }
}

fn lockstep_iter_size(tt: &Mtt, interp: &Matches, repeats: &[(usize, usize)]) -> Lockstep {
    let of_name = |name: &str| match lookup_cur_matched(name, interp, repeats) {
        Some(NamedMatch::Seq(ms)) => Lockstep::Constraint(ms.len(), name.to_string()),
        _ => Lockstep::Unconstrained,
    };
    match tt {
        Mtt::Delimited { tts, .. } | Mtt::Sequence { tts, .. } => {
            tts.iter().fold(Lockstep::Unconstrained, |size, tt| {
                size.with(lockstep_iter_size(tt, interp, repeats))
            })
        }
        Mtt::MetaVar { name, .. } | Mtt::MetaVarDecl { name, .. } => of_name(name),
        Mtt::MetaVarExpr { expr, .. } => match expr {
            MetaVarExpr::Concat(elems) => {
                elems.iter().fold(Lockstep::Unconstrained, |size, e| match e {
                    ConcatElem::Var(name) => size.with(of_name(name)),
                    _ => size,
                })
            }
            MetaVarExpr::Count(name, _) | MetaVarExpr::Ignore(name) => of_name(name),
            MetaVarExpr::Index(_) | MetaVarExpr::Len(_) => Lockstep::Unconstrained,
        },
        Mtt::Token(_) => Lockstep::Unconstrained,
    }
}
