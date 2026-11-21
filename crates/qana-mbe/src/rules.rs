//! A definition read from its tokens: its arms, each matcher and
//! transcriber read as rustc's `mbe::quoted` reads them and checked as
//! `mbe::macro_rules` checks them.

use std::collections::HashMap;

use crate::token::{Delim, ExprKind, PatKind, Token, TokenKind, TokenStream, Tree};
use crate::{Edition, Error, Origin};

/// The syntax a definition is written in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Form {
    /// `macro_rules! name { arms }`: arms separated by `;`. Its names are
    /// hygienic as `macro_rules!`'s are (mixed site).
    MacroRules,
    /// `macro name { arms }` (arms separated by `,`) or `macro name(params)
    /// { body }`, as std writes its internal macros.
    Macro,
}

/// A macro by example: its arms, in order.
#[derive(Clone, Debug)]
pub struct MacroRules {
    pub(crate) arms: Vec<Arm>,
    pub(crate) form: Form,
}

impl MacroRules {
    /// Read a definition from the tokens after its name: for
    /// `macro_rules!`, its group (`{ … }`, or `( … )` or `[ … ]` and a
    /// `;`); for `macro`, `{ … }` or `( … ) { … }`. An error where rustc
    /// refuses the definition.
    pub fn read(def: &TokenStream, form: Form) -> Result<MacroRules, Error> {
        let trees = &def.trees;
        let first_origin = trees.first().map(tree_origin);
        let body: Vec<Tree> = match form {
            Form::MacroRules => match trees.as_slice() {
                [Tree::Group(g)] if !matches!(g.delim, Delim::Invisible(_)) => g.trees.to_vec(),
                [Tree::Group(g), Tree::Token(semi)]
                    if semi.is_punct(";") && matches!(g.delim, Delim::Paren | Delim::Bracket) =>
                {
                    g.trees.to_vec()
                }
                _ => {
                    return Err(Error::new(
                        "a `macro_rules!` definition is one delimited group",
                        first_origin,
                    ))
                }
            },
            Form::Macro => match trees.as_slice() {
                [Tree::Group(g)] if g.delim == Delim::Brace => g.trees.to_vec(),
                // `macro m(params) { body }` is the one arm `(params) =>
                // { body }`, as rustc's parser rewrites it.
                [params @ Tree::Group(p), body @ Tree::Group(b)]
                    if p.delim == Delim::Paren && b.delim == Delim::Brace =>
                {
                    let arrow = Token::new(TokenKind::Punct, "=>", b.open, b.edition, b.mark);
                    vec![params.clone(), Tree::Token(arrow), body.clone()]
                }
                _ => {
                    return Err(Error::new(
                        "a `macro` definition is `{ arms }` or `(params) { body }`",
                        first_origin,
                    ))
                }
            },
        };
        let sep = match form {
            Form::MacroRules => ";",
            Form::Macro => ",",
        };
        let mut arms = Vec::new();
        let mut i = 0usize;
        while i < body.len() {
            let mut kind = ArmKind::Func;
            if let Tree::Token(t) = &body[i] {
                if t.is_keyword("unsafe") {
                    i += 1;
                }
            }
            if let Some(Tree::Token(t)) = body.get(i) {
                if t.is_keyword("attr") {
                    kind = ArmKind::Attr;
                    i += 1;
                    // The attribute's arguments, a matcher of their own.
                    let Some(args) = body.get(i) else {
                        return Err(Error::new("expected macro attr args", Some(t.origin)));
                    };
                    parse_lhs(std::slice::from_ref(args))?;
                    i += 1;
                } else if t.is_keyword("derive") {
                    kind = ArmKind::Derive;
                    i += 1;
                    match body.get(i) {
                        Some(Tree::Group(g)) if g.trees.is_empty() => i += 1,
                        _ => {
                            return Err(Error::new(
                                "`derive` rules do not accept arguments; `derive` must be \
                                 followed by `()`",
                                Some(t.origin),
                            ))
                        }
                    }
                }
            }
            let Some(lhs_tree) = body.get(i) else {
                return Err(Error::new("macro definition ended unexpectedly", first_origin));
            };
            let lhs = parse_lhs(std::slice::from_ref(lhs_tree))?;
            i += 1;
            match body.get(i) {
                Some(Tree::Token(t)) if t.is_punct("=>") => i += 1,
                other => {
                    return Err(Error::new(
                        "expected `=>`",
                        other.map(tree_origin).or(Some(tree_origin(lhs_tree))),
                    ))
                }
            }
            let Some(rhs_tree) = body.get(i) else {
                return Err(Error::new(
                    "expected right-hand side of macro rule",
                    Some(tree_origin(lhs_tree)),
                ));
            };
            i += 1;
            let rhs = parse_tts(std::slice::from_ref(rhs_tree), Part::Body)?
                .pop()
                .expect("one tree read as one");
            if !matches!(rhs, Mtt::Delimited { .. }) {
                return Err(Error::new("macro rhs must be delimited", Some(tree_origin(rhs_tree))));
            }
            let Mtt::Delimited { tts: matcher, .. } = &lhs else {
                unreachable!("parse_lhs returns a delimited matcher")
            };
            arms.push(Arm { kind, locs: compute_locs(matcher), rhs });
            if i == body.len() {
                break;
            }
            match &body[i] {
                Tree::Token(t) if t.is_punct(sep) => i += 1,
                other => {
                    return Err(Error::new(format!("expected `{sep}`"), Some(tree_origin(other))))
                }
            }
        }
        if arms.is_empty() {
            return Err(Error::new("macros must contain at least one rule", first_origin));
        }
        Ok(MacroRules { arms, form })
    }

    /// The syntax it was written in.
    pub fn form(&self) -> Form {
        self.form
    }
}

/// The span of `$name`, from the `$` to the name's end where they are in
/// one source.
pub(crate) fn metavar_origin(dollar: &Token, ident: &Token) -> Origin {
    if dollar.origin.source == ident.origin.source && ident.origin.end >= dollar.origin.start {
        Origin { end: ident.origin.end, ..dollar.origin }
    } else {
        dollar.origin
    }
}

pub(crate) fn tree_origin(tree: &Tree) -> Origin {
    match tree {
        Tree::Token(t) => t.origin,
        Tree::Group(g) => Origin { source: g.open.source, start: g.open.start, end: g.close.end },
    }
}

/// Which invocations an arm is for: `name!(…)`, or (unstable) an attribute
/// or a derive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ArmKind {
    Func,
    Attr,
    Derive,
}

#[derive(Clone, Debug)]
pub(crate) struct Arm {
    pub kind: ArmKind,
    pub locs: Vec<MatcherLoc>,
    pub rhs: Mtt,
}

/// What a fragment specifier names (rustc's `NonterminalKind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Fragment {
    Item,
    Block,
    Stmt,
    Pat(PatKind),
    Expr(ExprKind),
    Ty,
    Ident,
    Lifetime,
    Literal,
    Meta,
    Path,
    Vis,
    Tt,
}

impl Fragment {
    /// The fragment `name` names in a macro written in `edition`: `pat` and
    /// `expr` match by the edition.
    fn from_name(name: &str, edition: Edition) -> Option<Fragment> {
        Some(match name {
            "item" => Fragment::Item,
            "block" => Fragment::Block,
            "stmt" => Fragment::Stmt,
            "pat" => {
                if edition >= Edition::E2021 {
                    Fragment::Pat(PatKind::WithOr)
                } else {
                    Fragment::Pat(PatKind::Param { inferred: true })
                }
            }
            "pat_param" => Fragment::Pat(PatKind::Param { inferred: false }),
            "expr" => {
                if edition >= Edition::E2024 {
                    Fragment::Expr(ExprKind::Expr)
                } else {
                    Fragment::Expr(ExprKind::Expr2021 { inferred: true })
                }
            }
            "expr_2021" => Fragment::Expr(ExprKind::Expr2021 { inferred: false }),
            "ty" => Fragment::Ty,
            "ident" => Fragment::Ident,
            "lifetime" => Fragment::Lifetime,
            "literal" => Fragment::Literal,
            "meta" => Fragment::Meta,
            "path" => Fragment::Path,
            "vis" => Fragment::Vis,
            "tt" => Fragment::Tt,
            _ => return None,
        })
    }

    /// The grammar's `fragment` that parses it; `None` for those the engine
    /// reads itself (one identifier, one lifetime, one token tree).
    pub(crate) fn rule(self) -> Option<&'static str> {
        Some(match self {
            Fragment::Item => "item",
            Fragment::Block => "block",
            Fragment::Stmt => "stmt",
            Fragment::Pat(PatKind::WithOr) => "pat",
            Fragment::Pat(PatKind::Param { .. }) => "pat_param",
            Fragment::Expr(ExprKind::Expr) => "expr",
            Fragment::Expr(ExprKind::Expr2021 { .. }) => "expr_2021",
            Fragment::Ty => "ty",
            Fragment::Literal => "literal",
            Fragment::Meta => "meta",
            Fragment::Path => "path",
            Fragment::Vis => "vis",
            Fragment::Ident | Fragment::Lifetime | Fragment::Tt => return None,
        })
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Fragment::Ident => "ident",
            Fragment::Lifetime => "lifetime",
            Fragment::Tt => "tt",
            Fragment::Pat(PatKind::Param { inferred: true }) => "pat",
            Fragment::Expr(ExprKind::Expr2021 { inferred: true }) => "expr",
            other => other.rule().expect("a grammar fragment"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kleene {
    ZeroOrMore,
    OneOrMore,
    ZeroOrOne,
}

/// A matcher's or a transcriber's token tree (rustc's `mbe::TokenTree`).
#[derive(Clone, Debug)]
pub(crate) enum Mtt {
    Token(Token),
    Delimited {
        delim: Delim,
        open: Origin,
        close: Origin,
        edition: Edition,
        tts: Vec<Mtt>,
    },
    Sequence {
        tts: Vec<Mtt>,
        sep: Option<Token>,
        op: Kleene,
        num_captures: usize,
        origin: Origin,
    },
    /// `$name` in a transcriber: the name, and its two tokens as written.
    MetaVar {
        name: String,
        dollar: Token,
        ident: Token,
    },
    /// `$name:kind` in a matcher.
    MetaVarDecl {
        name: String,
        kind: Fragment,
        origin: Origin,
    },
    /// `${count(x)}` and the rest, in a transcriber.
    MetaVarExpr {
        expr: MetaVarExpr,
        origin: Origin,
        edition: Edition,
    },
}

/// rustc's meta-variable expressions (`macro_metavar_expr`, which core and
/// std use).
#[derive(Clone, Debug)]
pub(crate) enum MetaVarExpr {
    Concat(Vec<ConcatElem>),
    Count(String, usize),
    Ignore(String),
    Index(usize),
    Len(usize),
}

#[derive(Clone, Debug)]
pub(crate) enum ConcatElem {
    Ident(String),
    Literal(String),
    Var(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Part {
    Pattern,
    Body,
}

/// A matcher (one delimited group), read and checked: the follow-set
/// rules, no repetition that can match nothing, one binding per name.
fn parse_lhs(tree: &[Tree]) -> Result<Mtt, Error> {
    let lhs = parse_tts(tree, Part::Pattern)?.pop().expect("one tree read as one");
    let Mtt::Delimited { tts, .. } = &lhs else {
        return Err(Error::new(
            "invalid macro matcher; matchers must be contained in balanced delimiters",
            Some(tree_origin(&tree[0])),
        ));
    };
    check_matcher(tts)?;
    check_no_empty_seq(std::slice::from_ref(&lhs))?;
    let mut seen = HashMap::new();
    check_binders(tts, &mut seen)?;
    Ok(lhs)
}

fn check_binders(tts: &[Mtt], seen: &mut HashMap<String, Origin>) -> Result<(), Error> {
    for tt in tts {
        match tt {
            Mtt::MetaVarDecl { name, origin, .. } => {
                if seen.insert(name.clone(), *origin).is_some() {
                    return Err(Error::new(
                        format!("duplicate matcher binding (`${name}`)"),
                        Some(*origin),
                    ));
                }
            }
            Mtt::Delimited { tts, .. } | Mtt::Sequence { tts, .. } => check_binders(tts, seen)?,
            Mtt::Token(_) | Mtt::MetaVar { .. } | Mtt::MetaVarExpr { .. } => {}
        }
    }
    Ok(())
}

/// The trees of a matcher or a transcriber, `$`-forms read (rustc's
/// `quoted::parse`).
fn parse_tts(trees: &[Tree], part: Part) -> Result<Vec<Mtt>, Error> {
    let mut result = Vec::new();
    let mut i = 0usize;
    while i < trees.len() {
        let tree = parse_tree(trees, &mut i, part)?;
        if part == Part::Body {
            result.push(tree);
            continue;
        }
        let Mtt::MetaVar { name, dollar, ident } = tree else {
            result.push(tree);
            continue;
        };
        let origin = metavar_origin(&dollar, &ident);
        // A matcher's `$name` declares a fragment: `:` and its kind.
        let missing =
            |at: Origin| Error::new(format!("missing fragment specifier (`${name}`)"), Some(at));
        match trees.get(i) {
            Some(Tree::Token(colon)) if colon.is_punct(":") => {
                i += 1;
                let Some(Tree::Token(spec)) = trees.get(i) else {
                    return Err(missing(colon.origin));
                };
                i += 1;
                let Some((fragment, _)) = spec.ident() else {
                    return Err(missing(spec.origin));
                };
                let span = Origin { end: spec.origin.end, ..origin };
                let kind = Fragment::from_name(fragment, spec.edition).ok_or_else(|| {
                    Error::new(format!("invalid fragment specifier `{fragment}`"), Some(span))
                })?;
                result.push(Mtt::MetaVarDecl { name, kind, origin: span });
            }
            _ => return Err(missing(origin)),
        }
    }
    Ok(result)
}

/// One tree at `trees[*i]`, with what a `$` takes after it (rustc's
/// `quoted::parse_tree`).
fn parse_tree(trees: &[Tree], i: &mut usize, part: Part) -> Result<Mtt, Error> {
    let tree = &trees[*i];
    *i += 1;
    match tree {
        Tree::Token(dollar) if dollar.is_punct("$") => {
            // A `$` before an invisible group reads into it (a fragment a
            // macro wrote into a definition): what the `$` takes comes from
            // the group, and the rest of the group is dropped, as rustc
            // drops it.
            if let Some(Tree::Group(g)) = trees.get(*i) {
                if matches!(g.delim, Delim::Invisible(_)) {
                    *i += 1;
                    let mut inner: Vec<Tree> = Vec::with_capacity(g.trees.len() + 1);
                    inner.push(Tree::Token(dollar.clone()));
                    inner.extend(g.trees.iter().cloned());
                    let mut k = 0usize;
                    return parse_tree(&inner, &mut k, part);
                }
            }
            let (rest, at): (&[Tree], &mut usize) = (trees, i);
            let Some(next) = rest.get(*at) else {
                return Ok(Mtt::Token(dollar.clone()));
            };
            match next {
                Tree::Group(g) => {
                    *at += 1;
                    if part == Part::Pattern && g.delim != Delim::Paren {
                        return Err(Error::new(
                            "`$$` and meta-variable expressions are not allowed inside macro \
                             parameter definitions",
                            Some(g.open),
                        ));
                    }
                    if part == Part::Body {
                        match g.delim {
                            Delim::Brace => {
                                let origin = Origin { end: g.close.end, ..dollar.origin };
                                let expr = parse_metavar_expr(&g.trees, origin)?;
                                return Ok(Mtt::MetaVarExpr {
                                    expr,
                                    origin,
                                    edition: dollar.edition,
                                });
                            }
                            Delim::Paren => {}
                            _ => {
                                return Err(Error::new(
                                    "expected `(` or `{`, found `[`",
                                    Some(g.open),
                                ))
                            }
                        }
                    }
                    let tts = parse_tts(&g.trees, part)?;
                    let (sep, op) = parse_sep_and_kleene_op(rest, at, g.close)?;
                    let num_captures =
                        if part == Part::Pattern { count_metavar_decls(&tts) } else { 0 };
                    let origin = Origin { end: g.close.end, ..dollar.origin };
                    Ok(Mtt::Sequence { tts, sep, op, num_captures, origin })
                }
                Tree::Token(t) if t.kind == TokenKind::Ident => {
                    *at += 1;
                    let (name, raw) = t.ident().expect("an identifier");
                    let origin = Origin { end: t.origin.end, ..dollar.origin };
                    if name == "crate" && !raw {
                        let mut token = t.clone();
                        token.text = "$crate".into();
                        token.origin = origin;
                        Ok(Mtt::Token(token))
                    } else {
                        Ok(Mtt::MetaVar {
                            name: name.to_string(),
                            dollar: dollar.clone(),
                            ident: t.clone(),
                        })
                    }
                }
                Tree::Token(t) if t.is_punct("$") => {
                    *at += 1;
                    if part == Part::Pattern {
                        return Err(Error::new(
                            "`$$` and meta-variable expressions are not allowed inside macro \
                             parameter definitions",
                            Some(t.origin),
                        ));
                    }
                    Ok(Mtt::Token(t.clone()))
                }
                Tree::Token(t) => Err(Error::new(
                    format!("expected identifier, found `{}`", t.text),
                    Some(t.origin),
                )),
            }
        }
        Tree::Token(t) => Ok(Mtt::Token(t.clone())),
        Tree::Group(g) => Ok(Mtt::Delimited {
            delim: g.delim,
            open: g.open,
            close: g.close,
            edition: g.edition,
            tts: parse_tts(&g.trees, part)?,
        }),
    }
}

fn kleene(t: &Token) -> Option<Kleene> {
    if t.kind != TokenKind::Punct {
        return None;
    }
    match &*t.text {
        "*" => Some(Kleene::ZeroOrMore),
        "+" => Some(Kleene::OneOrMore),
        "?" => Some(Kleene::ZeroOrOne),
        _ => None,
    }
}

/// A repetition's separator, if any, and its operator.
fn parse_sep_and_kleene_op(
    trees: &[Tree],
    i: &mut usize,
    after: Origin,
) -> Result<(Option<Token>, Kleene), Error> {
    let expected = |at: Origin| Error::new("expected one of: `*`, `+`, or `?`", Some(at));
    match trees.get(*i) {
        Some(Tree::Token(t)) => {
            *i += 1;
            if let Some(op) = kleene(t) {
                return Ok((None, op));
            }
            match trees.get(*i) {
                Some(Tree::Token(op_token)) => {
                    *i += 1;
                    match kleene(op_token) {
                        Some(Kleene::ZeroOrOne) => Err(Error::new(
                            "the `?` macro repetition operator does not take a separator",
                            Some(t.origin),
                        )),
                        Some(op) => Ok((Some(t.clone()), op)),
                        None => Err(expected(op_token.origin)),
                    }
                }
                Some(other) => Err(expected(tree_origin(other))),
                None => Err(expected(t.origin)),
            }
        }
        Some(other) => Err(expected(tree_origin(other))),
        None => Err(expected(after)),
    }
}

// ---- meta-variable expressions (rustc's `mbe::metavar_expr`) ----

fn parse_metavar_expr(trees: &[Tree], origin: Origin) -> Result<MetaVarExpr, Error> {
    let err = |msg: &str, at: Origin| Error::new(msg.to_string(), Some(at));
    let Some(Tree::Token(func)) = trees.first() else {
        return Err(err("expected identifier", origin));
    };
    let Some((name, false)) = func.ident() else {
        return Err(err("expected identifier", func.origin));
    };
    let Some(Tree::Group(args)) = trees.get(1) else {
        return Err(err("expected `(`", func.origin));
    };
    if args.delim != Delim::Paren {
        return Err(err("expected `(`", args.open));
    }
    if trees.len() > 2 {
        return Err(err("unexpected trailing tokens", tree_origin(&trees[2])));
    }
    let args = &args.trees;
    let mut k = 0usize;
    let ident = |k: &mut usize| -> Result<String, Error> {
        match args.get(*k) {
            Some(Tree::Token(t)) => match t.ident() {
                Some((name, false)) => {
                    *k += 1;
                    Ok(name.to_string())
                }
                Some((_, true)) => {
                    Err(err("`${concat(..)}` currently does not support raw identifiers", t.origin))
                }
                None => Err(err(&format!("expected identifier, found `{}`", t.text), t.origin)),
            },
            Some(other) => Err(err("expected identifier or string literal", tree_origin(other))),
            None => Err(err("expected identifier or string literal", origin)),
        }
    };
    let dollar = |k: &mut usize| -> bool {
        if let Some(Tree::Token(t)) = args.get(*k) {
            if t.is_punct("$") {
                *k += 1;
                return true;
            }
        }
        false
    };
    let comma = |k: &mut usize| -> bool {
        if let Some(Tree::Token(t)) = args.get(*k) {
            if t.is_punct(",") {
                *k += 1;
                return true;
            }
        }
        false
    };
    let depth = |k: &mut usize| -> Result<usize, Error> {
        match args.get(*k) {
            None => Ok(0),
            Some(Tree::Token(t)) if t.kind == TokenKind::Literal => {
                *k += 1;
                parse_unsuffixed_int(&t.text).and_then(|n| usize::try_from(n).ok()).ok_or_else(
                    || {
                        err(
                            "only unsuffixes integer literals are supported in meta-variable \
                             expressions",
                            t.origin,
                        )
                    },
                )
            }
            Some(other) => {
                Err(err("meta-variable expression depth must be a literal", tree_origin(other)))
            }
        }
    };
    let need_dollar = |k: &mut usize| -> Result<(), Error> {
        if dollar(k) {
            Ok(())
        } else {
            Err(err(
                "meta-variables within meta-variable expressions must be referenced using a \
                 dollar sign",
                func.origin,
            ))
        }
    };
    let expr = match name {
        "concat" => {
            let mut elems = Vec::new();
            loop {
                let is_var = dollar(&mut k);
                let elem = match args.get(k) {
                    Some(Tree::Token(_)) if is_var => ConcatElem::Var(ident(&mut k)?),
                    Some(Tree::Token(t))
                        if t.kind == TokenKind::Literal
                            && t.text.starts_with('"')
                            && t.text.ends_with('"')
                            && t.text.len() >= 2 =>
                    {
                        k += 1;
                        ConcatElem::Literal(t.text[1..t.text.len() - 1].to_string())
                    }
                    Some(Tree::Token(t)) if t.kind == TokenKind::Ident => {
                        ConcatElem::Ident(ident(&mut k)?)
                    }
                    Some(other) => {
                        return Err(err(
                            "expected identifier or string literal",
                            tree_origin(other),
                        ))
                    }
                    None => return Err(err("expected identifier or string literal", origin)),
                };
                elems.push(elem);
                if k == args.len() {
                    break;
                }
                if !comma(&mut k) {
                    return Err(err("expected comma", origin));
                }
            }
            if elems.len() < 2 {
                return Err(err("`concat` must have at least two elements", func.origin));
            }
            MetaVarExpr::Concat(elems)
        }
        "count" => {
            need_dollar(&mut k)?;
            let var = ident(&mut k)?;
            let d = if comma(&mut k) {
                if k == args.len() {
                    return Err(err(
                        "`count` followed by a comma must have an associated index indicating \
                         its depth",
                        func.origin,
                    ));
                }
                depth(&mut k)?
            } else {
                0
            };
            MetaVarExpr::Count(var, d)
        }
        "ignore" => {
            need_dollar(&mut k)?;
            MetaVarExpr::Ignore(ident(&mut k)?)
        }
        "index" => MetaVarExpr::Index(depth(&mut k)?),
        "len" => MetaVarExpr::Len(depth(&mut k)?),
        _ => {
            return Err(err(
                "unrecognized meta-variable expression: valid are `count`, `ignore`, `index`, \
                 `len`, and `concat`",
                func.origin,
            ))
        }
    };
    if k < args.len() {
        return Err(err("unexpected trailing tokens", tree_origin(&args[k])));
    }
    Ok(expr)
}

/// An integer literal without a suffix, as rustc reads one (`10`, `0x1f`,
/// `1_000`).
fn parse_unsuffixed_int(text: &str) -> Option<u128> {
    let (radix, digits) = if let Some(d) = text.strip_prefix("0x") {
        (16, d)
    } else if let Some(d) = text.strip_prefix("0o") {
        (8, d)
    } else if let Some(d) = text.strip_prefix("0b") {
        (2, d)
    } else {
        (10, text)
    };
    let digits: String = digits.chars().filter(|&c| c != '_').collect();
    if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
        return None;
    }
    u128::from_str_radix(&digits, radix).ok()
}

// ---- checks (rustc's `macro_rules::check_lhs`) ----

/// A repetition that can match nothing would make the matcher loop.
fn check_no_empty_seq(tts: &[Mtt]) -> Result<(), Error> {
    for tt in tts {
        match tt {
            Mtt::Delimited { tts, .. } => check_no_empty_seq(tts)?,
            Mtt::Sequence { tts, sep, origin, .. } => {
                let empty = sep.is_none()
                    && tts.iter().all(|tt| match tt {
                        Mtt::MetaVarDecl { kind: Fragment::Vis, .. } => true,
                        Mtt::Token(t) => t.kind == TokenKind::DocComment,
                        Mtt::Sequence { op, .. } => {
                            matches!(op, Kleene::ZeroOrMore | Kleene::ZeroOrOne)
                        }
                        _ => false,
                    });
                if empty {
                    return Err(Error::new("repetition matches empty token tree", Some(*origin)));
                }
                check_no_empty_seq(tts)?;
            }
            _ => {}
        }
    }
    Ok(())
}

/// A member of a FIRST or FOLLOW set: a matcher tree, or a token the
/// matcher implies (a delimiter, a separator).
#[derive(Clone, Debug)]
enum Follow<'a> {
    Tt(&'a Mtt),
    Open(Delim),
    Close,
    Token(Token),
}

#[derive(Clone, Debug)]
struct TokenSet<'a> {
    tokens: Vec<Follow<'a>>,
    maybe_empty: bool,
}

impl<'a> TokenSet<'a> {
    fn empty() -> Self {
        TokenSet { tokens: Vec::new(), maybe_empty: true }
    }
    fn singleton(f: Follow<'a>) -> Self {
        TokenSet { tokens: vec![f], maybe_empty: false }
    }
    fn replace_with(&mut self, f: Follow<'a>) {
        self.tokens.clear();
        self.tokens.push(f);
        self.maybe_empty = false;
    }
    fn replace_with_irrelevant(&mut self) {
        self.tokens.clear();
        self.maybe_empty = false;
    }
    fn add_one(&mut self, f: Follow<'a>) {
        self.tokens.push(f);
        self.maybe_empty = false;
    }
    fn add_one_maybe(&mut self, f: Follow<'a>) {
        self.tokens.push(f);
    }
    fn add_all(&mut self, other: &Self) {
        self.tokens.extend(other.tokens.iter().cloned());
        if !other.maybe_empty {
            self.maybe_empty = false;
        }
    }
}

/// Each repetition's FIRST set, by the address of its tree.
struct FirstSets<'a> {
    first: HashMap<usize, TokenSet<'a>>,
}

fn key(tt: &Mtt) -> usize {
    tt as *const Mtt as usize
}

impl<'a> FirstSets<'a> {
    fn new(tts: &'a [Mtt]) -> Self {
        let mut sets = FirstSets { first: HashMap::new() };
        build(&mut sets, tts);
        return sets;

        fn build<'a>(sets: &mut FirstSets<'a>, tts: &'a [Mtt]) -> TokenSet<'a> {
            let mut first = TokenSet::empty();
            for tt in tts.iter().rev() {
                match tt {
                    Mtt::Token(_)
                    | Mtt::MetaVar { .. }
                    | Mtt::MetaVarDecl { .. }
                    | Mtt::MetaVarExpr { .. } => first.replace_with(Follow::Tt(tt)),
                    Mtt::Delimited { delim, tts, .. } => {
                        build(sets, tts);
                        first.replace_with(Follow::Open(*delim));
                    }
                    Mtt::Sequence { tts, sep, op, .. } => {
                        let subfirst = build(sets, tts);
                        sets.first.insert(key(tt), subfirst.clone());
                        if let (Some(sep), true) = (sep, subfirst.maybe_empty) {
                            first.add_one_maybe(Follow::Token(sep.clone()));
                        }
                        if subfirst.maybe_empty
                            || matches!(op, Kleene::ZeroOrMore | Kleene::ZeroOrOne)
                        {
                            first.add_all(&TokenSet { maybe_empty: true, ..subfirst });
                        } else {
                            first = subfirst;
                        }
                    }
                }
            }
            first
        }
    }

    fn first(&self, tts: &'a [Mtt]) -> TokenSet<'a> {
        let mut first = TokenSet::empty();
        for tt in tts {
            match tt {
                Mtt::Token(_)
                | Mtt::MetaVar { .. }
                | Mtt::MetaVarDecl { .. }
                | Mtt::MetaVarExpr { .. } => {
                    first.add_one(Follow::Tt(tt));
                    return first;
                }
                Mtt::Delimited { delim, .. } => {
                    first.add_one(Follow::Open(*delim));
                    return first;
                }
                Mtt::Sequence { sep, op, .. } => {
                    let subfirst = &self.first[&key(tt)];
                    if let (Some(sep), true) = (sep, subfirst.maybe_empty) {
                        first.add_one_maybe(Follow::Token(sep.clone()));
                    }
                    first.add_all(subfirst);
                    if subfirst.maybe_empty || matches!(op, Kleene::ZeroOrMore | Kleene::ZeroOrOne)
                    {
                        first.maybe_empty = true;
                        continue;
                    } else {
                        return first;
                    }
                }
            }
        }
        first
    }
}

fn check_matcher(matcher: &[Mtt]) -> Result<(), Error> {
    let first_sets = FirstSets::new(matcher);
    check_matcher_core(&first_sets, matcher, &TokenSet::empty()).map(|_| ())
}

/// That `matcher` can be followed by each token of `follow`, and each
/// fragment in it by what can follow it: rustc's `check_matcher_core`.
fn check_matcher_core<'a>(
    first_sets: &FirstSets<'a>,
    matcher: &'a [Mtt],
    follow: &TokenSet<'a>,
) -> Result<TokenSet<'a>, Error> {
    let mut last = TokenSet::empty();
    for (i, token) in matcher.iter().enumerate() {
        let suffix = &matcher[i + 1..];
        let build_suffix_first = || {
            let mut s = first_sets.first(suffix);
            if s.maybe_empty {
                s.add_all(follow);
            }
            s
        };
        let suffix_first = match token {
            Mtt::Token(_)
            | Mtt::MetaVar { .. }
            | Mtt::MetaVarDecl { .. }
            | Mtt::MetaVarExpr { .. } => {
                if can_be_followed_by_any(token) {
                    last.replace_with_irrelevant();
                    continue;
                }
                last.replace_with(Follow::Tt(token));
                build_suffix_first()
            }
            Mtt::Delimited { tts, .. } => {
                let my_suffix = TokenSet::singleton(Follow::Close);
                check_matcher_core(first_sets, tts, &my_suffix)?;
                last.replace_with_irrelevant();
                continue;
            }
            Mtt::Sequence { tts, sep, .. } => {
                let suffix_first = build_suffix_first();
                let mut with_sep;
                let my_suffix = if let Some(sep) = sep {
                    with_sep = suffix_first.clone();
                    with_sep.add_one_maybe(Follow::Token(sep.clone()));
                    &with_sep
                } else {
                    &suffix_first
                };
                let next = check_matcher_core(first_sets, tts, my_suffix)?;
                if next.maybe_empty {
                    last.add_all(&next);
                } else {
                    last = next;
                }
                continue;
            }
        };
        let may_be =
            if last.tokens.len() == 1 && suffix_first.tokens.len() == 1 { "is" } else { "may be" };
        for tt in &last.tokens {
            if let Follow::Tt(Mtt::MetaVarDecl { name, kind, .. }) = tt {
                for next in &suffix_first.tokens {
                    if !is_in_follow(next, *kind) {
                        return Err(Error::new(
                            format!(
                                "`${name}:{}` {may_be} followed by `{}`, which is not allowed \
                                 for `{}` fragments",
                                kind.name(),
                                follow_text(next),
                                kind.name()
                            ),
                            follow_origin(next),
                        ));
                    }
                }
            }
        }
    }
    Ok(last)
}

fn follow_text(f: &Follow) -> String {
    match f {
        Follow::Tt(Mtt::Token(t)) | Follow::Token(t) => t.text.to_string(),
        Follow::Tt(Mtt::MetaVarDecl { name, kind, .. }) => format!("${name}:{}", kind.name()),
        Follow::Tt(Mtt::MetaVar { name, .. }) => format!("${name}"),
        Follow::Open(d) => d.open_text().to_string(),
        Follow::Close => "a closing delimiter".to_string(),
        Follow::Tt(_) => "a token tree".to_string(),
    }
}

fn follow_origin(f: &Follow) -> Option<Origin> {
    match f {
        Follow::Tt(Mtt::Token(t)) | Follow::Token(t) => Some(t.origin),
        Follow::Tt(Mtt::MetaVarDecl { origin, .. }) => Some(*origin),
        _ => None,
    }
}

fn can_be_followed_by_any(tt: &Mtt) -> bool {
    match tt {
        Mtt::MetaVarDecl { kind, .. } => matches!(
            kind,
            Fragment::Item
                | Fragment::Block
                | Fragment::Ident
                | Fragment::Literal
                | Fragment::Meta
                | Fragment::Lifetime
                | Fragment::Tt
        ),
        _ => true,
    }
}

/// Whether a fragment of `kind` may be followed by `f`: rustc's
/// `is_in_follow`.
fn is_in_follow(f: &Follow, kind: Fragment) -> bool {
    if matches!(f, Follow::Close) {
        return true;
    }
    // The token `f` is, if it is one.
    let token: Option<&Token> = match f {
        Follow::Tt(Mtt::Token(t)) | Follow::Token(t) => Some(t),
        _ => None,
    };
    let punct = |p: &str| token.is_some_and(|t| t.is_punct(p));
    let keyword = |k: &str| token.is_some_and(|t| t.is_keyword(k));
    match kind {
        Fragment::Item | Fragment::Block => true,
        Fragment::Stmt | Fragment::Expr(_) => punct("=>") || punct(",") || punct(";"),
        Fragment::Pat(PatKind::Param { .. }) => {
            punct("=>") || punct(",") || punct("=") || punct("|") || keyword("if") || keyword("in")
        }
        Fragment::Pat(PatKind::WithOr) => {
            punct("=>") || punct(",") || punct("=") || keyword("if") || keyword("in")
        }
        Fragment::Path | Fragment::Ty => {
            matches!(f, Follow::Open(Delim::Brace | Delim::Bracket))
                || [",", "=>", ":", "=", ">", ">>", ";", "|"].iter().any(|p| punct(p))
                || keyword("as")
                || keyword("where")
                || matches!(f, Follow::Tt(Mtt::MetaVarDecl { kind: Fragment::Block, .. }))
        }
        Fragment::Ident
        | Fragment::Lifetime
        | Fragment::Literal
        | Fragment::Meta
        | Fragment::Tt => true,
        Fragment::Vis => match f {
            Follow::Tt(Mtt::MetaVarDecl {
                kind: Fragment::Ident | Fragment::Ty | Fragment::Path,
                ..
            }) => true,
            Follow::Open(d) => matches!(d, Delim::Paren | Delim::Bracket),
            _ => match token {
                Some(t) => {
                    t.is_punct(",")
                        || match t.ident() {
                            Some((_, true)) => true,
                            Some((name, false)) => name != "priv",
                            None => crate::input::token_can_begin_type(t),
                        }
                }
                None => false,
            },
        },
    }
}

// ---- matcher positions (rustc's `macro_parser::MatcherLoc`) ----

#[derive(Clone, Debug)]
pub(crate) enum MatcherLoc {
    Token(Token),
    Open(Delim),
    Close(Delim),
    Delimited,
    Sequence {
        op: Kleene,
        num_metavar_decls: usize,
        idx_first_after: usize,
        next_metavar: usize,
        seq_depth: usize,
    },
    SequenceKleeneOpNoSep {
        op: Kleene,
        idx_first: usize,
    },
    SequenceSep {
        separator: Token,
    },
    SequenceKleeneOpAfterSep {
        idx_first: usize,
    },
    MetaVarDecl {
        origin: Origin,
        name: String,
        kind: Fragment,
        next_metavar: usize,
        seq_depth: usize,
    },
    Eof,
}

fn count_metavar_decls(tts: &[Mtt]) -> usize {
    tts.iter()
        .map(|tt| match tt {
            Mtt::MetaVarDecl { .. } => 1,
            Mtt::Sequence { num_captures, .. } => *num_captures,
            Mtt::Delimited { tts, .. } => count_metavar_decls(tts),
            Mtt::Token(_) | Mtt::MetaVar { .. } | Mtt::MetaVarExpr { .. } => 0,
        })
        .sum()
}

fn compute_locs(matcher: &[Mtt]) -> Vec<MatcherLoc> {
    fn inner(tts: &[Mtt], locs: &mut Vec<MatcherLoc>, next_metavar: &mut usize, seq_depth: usize) {
        for tt in tts {
            match tt {
                Mtt::Token(t) => locs.push(MatcherLoc::Token(t.clone())),
                Mtt::Delimited { delim, tts, .. } => {
                    locs.push(MatcherLoc::Delimited);
                    locs.push(MatcherLoc::Open(*delim));
                    inner(tts, locs, next_metavar, seq_depth);
                    locs.push(MatcherLoc::Close(*delim));
                }
                Mtt::Sequence { tts, sep, op, num_captures, .. } => {
                    let idx_seq = locs.len();
                    locs.push(MatcherLoc::Eof);
                    let next_metavar_orig = *next_metavar;
                    let idx_first = locs.len();
                    inner(tts, locs, next_metavar, seq_depth + 1);
                    if let Some(separator) = sep {
                        locs.push(MatcherLoc::SequenceSep { separator: separator.clone() });
                        locs.push(MatcherLoc::SequenceKleeneOpAfterSep { idx_first });
                    } else {
                        locs.push(MatcherLoc::SequenceKleeneOpNoSep { op: *op, idx_first });
                    }
                    locs[idx_seq] = MatcherLoc::Sequence {
                        op: *op,
                        num_metavar_decls: *num_captures,
                        idx_first_after: locs.len(),
                        next_metavar: next_metavar_orig,
                        seq_depth,
                    };
                }
                Mtt::MetaVarDecl { name, kind, origin } => {
                    locs.push(MatcherLoc::MetaVarDecl {
                        origin: *origin,
                        name: name.clone(),
                        kind: *kind,
                        next_metavar: *next_metavar,
                        seq_depth,
                    });
                    *next_metavar += 1;
                }
                Mtt::MetaVar { .. } | Mtt::MetaVarExpr { .. } => {
                    unreachable!("a matcher declares its meta-variables")
                }
            }
        }
    }
    let mut locs = Vec::new();
    let mut next_metavar = 0usize;
    inner(matcher, &mut locs, &mut next_metavar, 0);
    locs.push(MatcherLoc::Eof);
    locs
}

/// The names a matcher declares, in the order of its metavariable
/// declarations.
pub(crate) fn decl_names(locs: &[MatcherLoc]) -> Vec<(String, Origin)> {
    locs.iter()
        .filter_map(|loc| match loc {
            MatcherLoc::MetaVarDecl { name, origin, .. } => Some((name.clone(), *origin)),
            _ => None,
        })
        .collect()
}
