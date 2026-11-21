//! An expansion parsed where its invocation stands.

use std::collections::HashSet;
use std::sync::Arc;

use qana_grammar::{batch_parse_from, GreenNode};
use qana_lang::EmbeddedLang;

use crate::render::{flatten, render, split_groups, wrappable, Flat};
use crate::token::{Delim, MetaVarKind, Token, TokenKind, TokenStream, Tree};
use crate::Error;

/// A token as written into a parsed expansion's text, with its span there.
#[derive(Clone, Debug)]
pub struct Spanned {
    pub span: (u32, u32),
    pub token: Token,
}

/// A token stream parsed at one of the grammar's entries: the text it was
/// written as, the tree, and where in the text each token and bracket is.
/// A fragment the grammar would otherwise read otherwise is written in
/// parentheses ([`TokenKind::Delimiter`] tokens at the fragment's origin).
#[derive(Clone, Debug)]
pub struct Parsed {
    pub text: String,
    pub tree: Arc<GreenNode>,
    pub tokens: Vec<Spanned>,
    /// The flats the text was written from, and the token of `tokens` each
    /// begins at.
    pub(crate) flats: Vec<Flat>,
    pub(crate) flat_spans: Vec<(u32, u32)>,
}

impl TokenStream {
    /// Parse the stream as the grammar's rule `entry` (one it can be parsed
    /// from: `items_opt`, `block_stmts`, `pat`, `type`…). An expression (the
    /// rule the `expr` fragment parses at) may end in a `;`, which rustc
    /// lets an expression macro's expansion end in. An error where the
    /// grammar does not read it as `entry`.
    pub fn parse(&self, lang: &EmbeddedLang, entry: &str) -> Result<Parsed, Error> {
        let start = lang
            .entry_state(entry)
            .ok_or_else(|| Error::new(format!("`{entry}` is no entry of the grammar"), None))?;
        let flats = flatten(&self.trees);
        let mut skip = HashSet::new();
        let expression =
            lang.sg.fragment("expr").is_some_and(|nt| lang.sg.nt_names[nt as usize] == entry);
        if expression {
            if let Some(Flat::Token(t)) = flats.last() {
                if t.is_punct(";") {
                    skip.insert(flats.len() - 1);
                }
            }
        }
        let mut wrapped = HashSet::new();
        loop {
            let r = render(lang, &flats, &wrapped, &skip);
            let (tree, _, repairs) = batch_parse_from(lang.sg, lang.tables, start, &r.full)
                .map_err(|e| Error::new(format!("parse failed: {e:?}"), None))?;
            let split =
                split_groups(lang, &flats, 0, flats.len(), &r, &wrapped, &tree, 0, &repairs, 0)?;
            if split.is_empty() {
                if let Some(rep) = repairs.first() {
                    let at = r.sig_byte(rep.at_terminal);
                    // A fragment before the error may be what the grammar
                    // read otherwise (rustc keeps it one node): those not
                    // wrapped yet are, and the parse is done again.
                    let retry: Vec<usize> = (0..flats.len())
                        .filter(|&i| {
                            matches!(flats[i], Flat::Open { .. })
                                && flats[i].invisible().is_some_and(wrappable)
                                && !wrapped.contains(&i)
                                && r.spans[i].0 <= at
                        })
                        .collect();
                    if !retry.is_empty() {
                        wrapped.extend(retry);
                        continue;
                    }
                    let origin =
                        flats.iter().zip(&r.spans).find(|(_, &(s, e))| e > s && e > at).map(
                            |(f, _)| match f {
                                Flat::Token(t) => t.origin,
                                Flat::Open { group, .. } => group.open,
                                Flat::Close { origin, .. } => *origin,
                            },
                        );
                    return Err(Error::new(
                        format!("the expansion does not parse as `{entry}`"),
                        origin,
                    ));
                }
                let mut tokens = Vec::new();
                for (i, flat) in flats.iter().enumerate() {
                    let span = r.spans[i];
                    if span.1 == span.0 {
                        continue;
                    }
                    let token = match flat {
                        Flat::Token(t) => t.clone(),
                        Flat::Open { group, .. } => Token::new(
                            TokenKind::Delimiter,
                            match group.delim {
                                Delim::Invisible(_) => "(",
                                d => d.open_text(),
                            },
                            group.open,
                            group.edition,
                            group.mark,
                        ),
                        Flat::Close {
                            delim: Delim::Invisible(MetaVarKind::Stmt { .. }),
                            origin,
                            edition,
                            mark,
                            ..
                        } => Token::new(TokenKind::Punct, ";", *origin, *edition, *mark),
                        Flat::Close { delim, origin, edition, mark, .. } => Token::new(
                            TokenKind::Delimiter,
                            match delim {
                                Delim::Invisible(_) => ")",
                                d => d.close_text(),
                            },
                            *origin,
                            *edition,
                            *mark,
                        ),
                    };
                    tokens.push(Spanned { span, token });
                }
                return Ok(Parsed { text: r.text, tree, tokens, flat_spans: r.spans, flats });
            }
            wrapped.extend(split);
        }
    }
}

impl Parsed {
    /// The token written at byte `at` of the text.
    pub fn token_at(&self, at: u32) -> Option<&Token> {
        let k = self.tokens.partition_point(|s| s.span.1 <= at);
        self.tokens.get(k).filter(|s| s.span.0 <= at).map(|s| &s.token)
    }

    /// The token trees written within byte span `span` of the text, as
    /// they are in the stream: a group whose tokens all are within, whole
    /// (a fragment a transcriber put in stays one, of its kind); one only
    /// partly within, by what of it is. What a nested invocation is passed,
    /// and what a `macro_rules!` an expansion writes is defined by.
    pub fn stream(&self, span: (u32, u32)) -> TokenStream {
        // The span of the flats from `a` to `b` that the text writes.
        let content = |a: usize, b: usize| -> Option<(u32, u32)> {
            let written = || (a..=b).map(|i| self.flat_spans[i]).filter(|&(s, e)| e > s);
            let first = written().next()?;
            let last = written().next_back()?;
            Some((first.0, last.1))
        };
        let inside = |(s, e): (u32, u32)| s >= span.0 && e <= span.1;
        let mut out = Vec::new();
        let mut i = 0usize;
        while i < self.flats.len() {
            match &self.flats[i] {
                Flat::Token(t) => {
                    let s = self.flat_spans[i];
                    if s.1 > s.0 && inside(s) {
                        out.push(Tree::Token(t.clone()));
                    }
                    i += 1;
                }
                Flat::Open { group, close } => match content(i, *close) {
                    Some(written) if inside(written) => {
                        out.push(Tree::Group(group.clone()));
                        i = close + 1;
                    }
                    Some((s, e)) if e <= span.0 || s >= span.1 => i = close + 1,
                    // Nothing of it written: an empty fragment, nowhere.
                    None => i = close + 1,
                    // Partly within: what of it is.
                    Some(_) => i += 1,
                },
                Flat::Close { .. } => i += 1,
            }
        }
        TokenStream::new(out)
    }
}
