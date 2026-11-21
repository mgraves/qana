//! Tokens and token trees, as rustc's lexer and token-tree reader make
//! them, read from text by the grammar's lexer.
//!
//! The grammar cuts punctuation one character at a time (each lexed as what
//! it begins) and a string into its opener, contents and closer; rustc's
//! tokens are glued punctuation (`::`, `>>=`) and whole literals. A token
//! here is rustc's: the grammar's tokens it is made of are read back from
//! its text whenever it is parsed.

use std::sync::Arc;

use qana_lang::EmbeddedLang;
use serde::{Deserialize, Serialize};

use crate::{Edition, Error, Origin};

/// What kind of token, as rustc's lexer has it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TokenKind {
    /// A name or a keyword (`foo`, `fn`, `r#type`, `_`), or `$crate`.
    Ident,
    /// `'a`, `'static`, `'r#a`.
    Lifetime,
    /// A literal, with its suffix: `"a"`, `r#"a"#`, `b'a'`, `1u8`, `1.5`.
    Literal,
    /// Punctuation, glued as rustc glues it: `::`, `>>=`, `=>`.
    Punct,
    /// A doc comment a definition writes (`/// …`, `//! …`, `/** … */`). In
    /// an invocation's tokens a doc comment is read as the attribute it
    /// stands for (`#[doc = r" …"]`), as rustc reads it.
    DocComment,
    /// A bracket, in a flattened stream ([`crate::Parsed::tokens`]).
    Delimiter,
}

/// A token, with where it was written.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Token {
    pub kind: TokenKind,
    pub text: Arc<str>,
    pub origin: Origin,
    /// The edition of the text it was written in, which decides whether a
    /// name is a keyword.
    pub edition: Edition,
    /// The expansion whose transcriber wrote it (its
    /// [`crate::Expansion::mark`]), or 0 for text no macro wrote.
    pub mark: u32,
    /// An identifier or lifetime a transcriber put in for a fragment
    /// (`$i:ident`, `$l:lifetime`): rustc's `NtIdent` and `NtLifetime`. A
    /// block fragment may begin with such a lifetime, and not with one
    /// written out.
    pub(crate) interpolated: bool,
}

impl Token {
    pub(crate) fn new(
        kind: TokenKind,
        text: &str,
        origin: Origin,
        edition: Edition,
        mark: u32,
    ) -> Token {
        Token { kind, text: Arc::from(text), origin, edition, mark, interpolated: false }
    }

    /// The name of an identifier and whether it is raw (`r#type` is `type`,
    /// raw).
    pub(crate) fn ident(&self) -> Option<(&str, bool)> {
        if self.kind != TokenKind::Ident {
            return None;
        }
        Some(match self.text.strip_prefix("r#") {
            Some(name) => (name, true),
            None => (&self.text, false),
        })
    }

    /// The name of a lifetime, quote included, and whether it is raw
    /// (`'r#a` is `'a`, raw).
    pub(crate) fn lifetime(&self) -> Option<(String, bool)> {
        if self.kind != TokenKind::Lifetime {
            return None;
        }
        Some(match self.text.strip_prefix("'r#") {
            Some(name) => (format!("'{name}"), true),
            None => (self.text.to_string(), false),
        })
    }

    /// Whether it is the keyword `kw`, not raw.
    pub(crate) fn is_keyword(&self, kw: &str) -> bool {
        matches!(self.ident(), Some((name, false)) if name == kw)
    }

    pub(crate) fn is_punct(&self, p: &str) -> bool {
        self.kind == TokenKind::Punct && &*self.text == p
    }

    /// The same token, now written by expansion `mark`, at `origin`.
    pub(crate) fn remarked(&self, origin: Origin, mark: u32) -> Token {
        Token { origin, mark, ..self.clone() }
    }
}

/// rustc's equality of tokens in a matcher and an invocation, unhygienic:
/// identifiers and lifetimes by name and rawness, everything else by kind
/// and text.
pub(crate) fn token_name_eq(a: &Token, b: &Token) -> bool {
    if let (Some(x), Some(y)) = (a.ident(), b.ident()) {
        return x == y;
    }
    if let (Some(x), Some(y)) = (a.lifetime(), b.lifetime()) {
        return x == y;
    }
    a.kind == b.kind && a.text == b.text
}

// ---- keywords, as rustc_span's symbols class them ----

const USED_KEYWORDS: &[&str] = &[
    "as", "break", "const", "continue", "crate", "else", "enum", "extern", "false", "fn", "for",
    "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return",
    "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where",
    "while",
];
const USED_KEYWORDS_2018: &[&str] = &["async", "await", "dyn"];
const UNUSED_KEYWORDS: &[&str] = &[
    "abstract", "become", "box", "do", "final", "macro", "override", "priv", "typeof", "unsized",
    "virtual", "yield",
];
const UNUSED_KEYWORDS_2018: &[&str] = &["try"];
const UNUSED_KEYWORDS_2024: &[&str] = &["gen"];

/// rustc's reserved identifiers: the special ones (`_`, `$crate`) and the
/// keywords, used and unused, of `edition`.
pub(crate) fn is_reserved(name: &str, edition: Edition) -> bool {
    name == "_"
        || name == "$crate"
        || USED_KEYWORDS.contains(&name)
        || UNUSED_KEYWORDS.contains(&name)
        || (edition >= Edition::E2018
            && (USED_KEYWORDS_2018.contains(&name) || UNUSED_KEYWORDS_2018.contains(&name)))
        || (edition >= Edition::E2024 && UNUSED_KEYWORDS_2024.contains(&name))
}

/// `super`, `self`, `Self`, `crate`, `$crate`: the keywords a path segment
/// may be.
pub(crate) fn is_path_segment_keyword(name: &str) -> bool {
    matches!(name, "super" | "self" | "Self" | "crate" | "$crate")
}

// ---- token trees ----

/// What a fragment a transcriber put in is, as rustc's invisible
/// delimiters record it (`MetaVarKind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum MetaVarKind {
    Item,
    Block,
    /// A statement: whole in rustc's tree, which needs no `;` after it,
    /// where the grammar needs one written (`semi`).
    Stmt {
        semi: Semi,
    },
    Pat(PatKind),
    Expr {
        kind: ExprKind,
        can_begin_literal_maybe_minus: bool,
        can_begin_string_literal: bool,
    },
    Ty {
        is_path: bool,
    },
    Meta {
        has_meta_form: bool,
    },
    Path,
    Vis,
    Literal,
}

/// Whether the grammar needs a `;` after a statement fragment, which
/// rustc's tree does not: a `let` always, an expression unless it ends its
/// block, an item, a block or an empty statement never. It is what the
/// statement's tokens are, so it tells no two of rustc's kinds apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum Semi {
    Never,
    Always,
    UnlessTail,
}

/// Which patterns a `pat` fragment matches: or-patterns (`pat` from 2021),
/// or not (`pat_param`, and `pat` before 2021: `inferred`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum PatKind {
    WithOr,
    Param { inferred: bool },
}

/// Which expressions an `expr` fragment matches: from 2024 a leading `_`
/// or `const` too; `expr_2021`, and `expr` before 2024 (`inferred`), not.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum ExprKind {
    Expr,
    Expr2021 { inferred: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum Delim {
    Paren,
    Bracket,
    Brace,
    /// A fragment a transcriber put in: its tokens, one node of its kind.
    Invisible(MetaVarKind),
}

impl Delim {
    pub(crate) fn open_text(self) -> &'static str {
        match self {
            Delim::Paren => "(",
            Delim::Bracket => "[",
            Delim::Brace => "{",
            Delim::Invisible(_) => "",
        }
    }

    pub(crate) fn close_text(self) -> &'static str {
        match self {
            Delim::Paren => ")",
            Delim::Bracket => "]",
            Delim::Brace => "}",
            Delim::Invisible(_) => "",
        }
    }
}

/// A token tree: a token, or a delimited group of trees.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum Tree {
    Token(Token),
    Group(Group),
}

impl Tree {
    /// Where it begins: a token's origin, a group's opening bracket's.
    pub(crate) fn origin(&self) -> Origin {
        match self {
            Tree::Token(t) => t.origin,
            Tree::Group(g) => g.open,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct Group {
    pub delim: Delim,
    pub open: Origin,
    pub close: Origin,
    pub edition: Edition,
    pub mark: u32,
    pub trees: Arc<Vec<Tree>>,
}

/// Tokens with their origins: an invocation's, a definition's, an
/// expansion's.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TokenStream {
    pub(crate) trees: Vec<Tree>,
}

impl TokenStream {
    pub(crate) fn new(trees: Vec<Tree>) -> TokenStream {
        TokenStream { trees }
    }

    /// The tokens of `text`, which stands at byte `offset` of source
    /// `source` and is written in `edition`: rustc's tokens, glued and in
    /// their trees. An error where rustc's lexer or token-tree reader would
    /// stop: a character no token begins, an unterminated literal, an
    /// unbalanced delimiter.
    pub fn lex(
        lang: &EmbeddedLang,
        text: &str,
        source: u32,
        offset: u32,
        edition: Edition,
    ) -> Result<TokenStream, Error> {
        lex_trees(lang, text, source, offset, edition).map(TokenStream::new)
    }

    pub fn is_empty(&self) -> bool {
        self.trees.is_empty()
    }

    /// The streams one after another, as one.
    pub fn concat(streams: impl IntoIterator<Item = TokenStream>) -> TokenStream {
        TokenStream::new(streams.into_iter().flat_map(|s| s.trees).collect())
    }

    /// An invocation's parts, the stream being one (`path ! ( input )`,
    /// with `[…]` or `{…}`, and a `;` after it or not): the tokens of the
    /// macro's path, a fragment's among them (`$p!(…)`), and what is passed
    /// to the macro, the tokens inside its delimiters. `None` for a stream
    /// that is no invocation.
    pub fn invocation(&self) -> Option<(Vec<Token>, TokenStream)> {
        fn tokens_of(trees: &[Tree], out: &mut Vec<Token>) -> bool {
            for tree in trees {
                match tree {
                    Tree::Token(t) => out.push(t.clone()),
                    Tree::Group(g) if matches!(g.delim, Delim::Invisible(_)) => {
                        if !tokens_of(&g.trees, out) {
                            return false;
                        }
                    }
                    Tree::Group(_) => return false,
                }
            }
            true
        }
        let bang =
            self.trees.iter().position(|t| matches!(t, Tree::Token(t) if t.is_punct("!")))?;
        let mut path = Vec::new();
        if bang == 0 || !tokens_of(&self.trees[..bang], &mut path) {
            return None;
        }
        let Some(Tree::Group(group)) = self.trees.get(bang + 1) else { return None };
        if matches!(group.delim, Delim::Invisible(_)) {
            return None;
        }
        let rest = &self.trees[bang + 2..];
        let ends = match rest {
            [] => true,
            [Tree::Token(t)] => t.is_punct(";"),
            _ => false,
        };
        ends.then(|| (path, TokenStream::new(group.trees.to_vec())))
    }

    /// The arms of rustc's built-in `cfg_select!`, the stream being what
    /// an invocation of it is passed (`predicate => { items }`, `_ => { …
    /// }`, a `,` after each or not): each arm's predicate, `None` for `_`,
    /// and the tokens inside its braces. An error where rustc's parser of
    /// it would stop.
    pub fn cfg_select_arms(&self) -> Result<Vec<(Option<TokenStream>, TokenStream)>, Error> {
        let mut arms = Vec::new();
        let mut i = 0usize;
        let trees = &self.trees;
        while i < trees.len() {
            let start = i;
            while i < trees.len() && !matches!(&trees[i], Tree::Token(t) if t.is_punct("=>")) {
                i += 1;
            }
            let predicate = &trees[start..i];
            let at = predicate.first().map(Tree::origin);
            if predicate.is_empty() {
                return Err(Error::new("expected a `cfg_select!` predicate", at));
            }
            if i == trees.len() {
                return Err(Error::new("expected `=>` after a `cfg_select!` predicate", at));
            }
            i += 1;
            let body = match trees.get(i) {
                Some(Tree::Group(g)) if g.delim == Delim::Brace => {
                    TokenStream::new(g.trees.to_vec())
                }
                other => {
                    return Err(Error::new(
                        "expected `{` after `=>` in `cfg_select!`",
                        other.map(Tree::origin).or(at),
                    ))
                }
            };
            i += 1;
            if matches!(trees.get(i), Some(Tree::Token(t)) if t.is_punct(",")) {
                i += 1;
            }
            let wildcard = matches!(predicate, [Tree::Token(t)] if t.is_keyword("_"));
            let predicate = (!wildcard).then(|| TokenStream::new(predicate.to_vec()));
            arms.push((predicate, body));
        }
        Ok(arms)
    }

    /// The texts of its tokens and brackets in order, a fragment's tokens
    /// without the invisible group around them, as rustc's `trace_macros`
    /// prints a stream.
    pub fn token_texts(&self) -> Vec<String> {
        fn go(trees: &[Tree], out: &mut Vec<String>) {
            for tree in trees {
                match tree {
                    Tree::Token(t) => out.push(t.text.to_string()),
                    Tree::Group(g) => {
                        if !matches!(g.delim, Delim::Invisible(_)) {
                            out.push(g.delim.open_text().to_string());
                        }
                        go(&g.trees, out);
                        if !matches!(g.delim, Delim::Invisible(_)) {
                            out.push(g.delim.close_text().to_string());
                        }
                    }
                }
            }
        }
        let mut out = Vec::new();
        go(&self.trees, &mut out);
        out
    }
}

// ---- lexing ----

/// rustc's single-character punctuation, brackets aside.
const PUNCT: &str = ";,.@#~?:$=!<>-&|+*/^%";

/// rustc's gluing of a punctuation token and the next character joined to
/// it (`Token::glue`).
pub(crate) fn glue(a: &str, b: &str) -> Option<&'static str> {
    Some(match (a, b) {
        ("=", "=") => "==",
        ("=", ">") => "=>",
        ("<", "=") => "<=",
        ("<", "<") => "<<",
        ("<", "<=") => "<<=",
        ("<", "-") => "<-",
        (">", "=") => ">=",
        (">", ">") => ">>",
        (">", ">=") => ">>=",
        ("!", "=") => "!=",
        ("+", "=") => "+=",
        ("-", "=") => "-=",
        ("-", ">") => "->",
        ("*", "=") => "*=",
        ("/", "=") => "/=",
        ("%", "=") => "%=",
        ("^", "=") => "^=",
        ("&", "=") => "&=",
        ("&", "&") => "&&",
        ("|", "=") => "|=",
        ("|", "|") => "||",
        ("<<", "=") => "<<=",
        (">>", "=") => ">>=",
        (".", ".") => "..",
        (".", "..") => "...",
        ("..", ".") => "...",
        ("..", "=") => "..=",
        (":", ":") => "::",
        _ => return None,
    })
}

/// A line comment's doc style, as rustc_lexer tells it: `///` (not
/// `////`) is outer, `//!` inner.
fn line_doc(text: &str) -> bool {
    (text.starts_with("///") && !text.starts_with("////")) || text.starts_with("//!")
}

/// A block comment's: `/*!` is inner, `/**` outer unless `*` or `/` follows
/// (`/***`, `/**/`).
fn block_doc(text: &str) -> bool {
    text.starts_with("/*!")
        || (text.starts_with("/**") && !text.starts_with("/***") && !text.starts_with("/**/"))
}

/// A string opener: `"`, `r"`, `r#"`…, with a `b` or `c` prefix.
fn string_opener(text: &str) -> Option<usize> {
    let rest = text.strip_prefix('b').or_else(|| text.strip_prefix('c')).unwrap_or(text);
    let (raw, rest) = match rest.strip_prefix('r') {
        Some(r) => (true, r),
        None => (false, rest),
    };
    let hashes = rest.len() - rest.trim_start_matches('#').len();
    if &rest[hashes..] != "\"" || (!raw && hashes > 0) {
        return None;
    }
    Some(hashes)
}

fn lex_trees(
    lang: &EmbeddedLang,
    text: &str,
    source: u32,
    offset: u32,
    edition: Edition,
) -> Result<Vec<Tree>, Error> {
    let toks = lang.tokens(text);
    let vocab = &lang.lexer.vocab;
    // A token's mode; the line terminators the lexer adds are in none.
    let mode_of = |id: qana_grammar::TokenId| vocab.modes.get(id as usize).copied().unwrap_or(0);
    // How many tokens from `i` on a construct of other modes still takes:
    // its tokens, and the line terminators between them (a string or a
    // block comment over several lines), not those after it.
    let continues = |i: usize| -> usize {
        let mut j = i;
        while j < toks.len() && toks[j].id == qana_grammar::NEWLINE {
            j += 1;
        }
        if j < toks.len() && mode_of(toks[j].id) != 0 {
            j - i + 1
        } else {
            0
        }
    };
    let at = |start: usize, end: usize| Origin {
        source,
        start: offset + start as u32,
        end: offset + end as u32,
    };
    // Open groups: their delimiter, the opener's origin, the trees so far.
    let mut stack: Vec<(Delim, Origin, Vec<Tree>)> = Vec::new();
    let mut out: Vec<Tree> = Vec::new();
    // Whether whitespace or a comment stands before the next token: rustc
    // glues punctuation only where nothing does.
    let mut spaced = true;
    let mut pos = 0usize;
    let mut i = 0usize;
    while i < toks.len() {
        let tok = &toks[i];
        let start = pos;
        let mode = mode_of(tok.id);
        if tok.id != qana_grammar::NEWLINE
            && (tok.id == vocab.unknown
                || vocab.error.get(tok.id as usize).copied().unwrap_or(false))
        {
            return Err(Error::new(
                format!("unknown start of token: `{}`", tok.text),
                Some(at(start, start + tok.text.len())),
            ));
        }
        if tok.trivia {
            let mut end = pos + tok.text.len();
            i += 1;
            if mode == 0 && tok.text.starts_with("/*") {
                // A block comment: its body's tokens are of its own modes.
                loop {
                    let n = continues(i);
                    if n == 0 {
                        break;
                    }
                    for t in &toks[i..i + n] {
                        end += t.text.len();
                    }
                    i += n;
                }
                let comment = &text[start..end];
                if !comment.ends_with("*/") || comment.len() < 4 {
                    return Err(Error::new("unterminated block comment", Some(at(start, end))));
                }
                if block_doc(comment) {
                    push(&mut stack, &mut out, doc_token(comment, at(start, end), edition));
                }
            } else if tok.text.starts_with("//") && line_doc(&tok.text) {
                push(&mut stack, &mut out, doc_token(&tok.text, at(start, end), edition));
            }
            pos = end;
            spaced = true;
            continue;
        }
        if let Some((kind, is_open)) = vocab.brackets.get(tok.id as usize).copied().flatten() {
            let delim = match kind {
                qana_grammar::BracketKind::Paren => Delim::Paren,
                qana_grammar::BracketKind::Bracket => Delim::Bracket,
                qana_grammar::BracketKind::Brace => Delim::Brace,
            };
            let origin = at(start, start + tok.text.len());
            if is_open {
                stack.push((delim, origin, Vec::new()));
            } else {
                let Some((open_delim, open, trees)) = stack.pop() else {
                    return Err(Error::new(
                        format!("unexpected closing delimiter: `{}`", tok.text),
                        Some(origin),
                    ));
                };
                if open_delim != delim {
                    return Err(Error::new(
                        format!("mismatched closing delimiter: `{}`", tok.text),
                        Some(origin),
                    ));
                }
                let group = Tree::Group(Group {
                    delim,
                    open,
                    close: origin,
                    edition,
                    mark: 0,
                    trees: Arc::new(trees),
                });
                match stack.last_mut() {
                    Some((_, _, trees)) => trees.push(group),
                    None => out.push(group),
                }
            }
            pos += tok.text.len();
            i += 1;
            spaced = false;
            continue;
        }
        // A token, with what its modes read after it (a string's contents
        // and closer).
        let mut end = pos + tok.text.len();
        let mut last: &str = &tok.text;
        let mut pieces = 1usize;
        i += 1;
        let opener = string_opener(&tok.text);
        loop {
            let n = continues(i);
            if n == 0 {
                break;
            }
            for t in &toks[i..i + n] {
                end += t.text.len();
                last = &t.text;
                pieces += 1;
            }
            i += n;
        }
        let whole = &text[start..end];
        let origin = at(start, end);
        // A string is closed by its quote and as many `#` as it opened with:
        // the grammar's closer, the one token of its modes that begins so.
        if let Some(hashes) = opener {
            let closer = format!("\"{}", "#".repeat(hashes));
            if pieces < 2 || !last.starts_with(&closer) {
                return Err(Error::new("unterminated string literal", Some(origin)));
            }
        }
        let kind = classify(whole, opener.is_some()).ok_or_else(|| {
            Error::new(format!("unknown start of token: `{whole}`"), Some(origin))
        })?;
        if kind == TokenKind::Literal && whole.starts_with('\'') && !char_closed(whole) {
            return Err(Error::new("unterminated character literal", Some(origin)));
        }
        let token = Token::new(kind, whole, origin, edition, 0);
        if kind == TokenKind::Punct && !spaced {
            let trees = match stack.last_mut() {
                Some((_, _, trees)) => trees,
                None => &mut out,
            };
            if let Some(Tree::Token(prev)) = trees.last_mut() {
                if prev.kind == TokenKind::Punct {
                    if let Some(glued) = glue(&prev.text, &token.text) {
                        prev.text = Arc::from(glued);
                        prev.origin.end = origin.end;
                        pos = end;
                        spaced = false;
                        continue;
                    }
                }
            }
        }
        push(&mut stack, &mut out, token);
        pos = end;
        spaced = false;
    }
    if let Some((_, open, _)) = stack.last() {
        return Err(Error::new("unclosed delimiter", Some(*open)));
    }
    Ok(out)
}

/// Whether a character or byte literal is closed: a quote after its first.
fn char_closed(text: &str) -> bool {
    let body = text.strip_prefix("b'").or_else(|| text.strip_prefix('\'')).unwrap_or(text);
    let bytes = body.as_bytes();
    let mut k = 0usize;
    while k < bytes.len() {
        match bytes[k] {
            b'\\' => k += 2,
            b'\'' => return k > 0,
            _ => k += 1,
        }
    }
    false
}

fn push(stack: &mut [(Delim, Origin, Vec<Tree>)], out: &mut Vec<Tree>, token: Token) {
    match stack.last_mut() {
        Some((_, _, trees)) => trees.push(Tree::Token(token)),
        None => out.push(Tree::Token(token)),
    }
}

fn doc_token(text: &str, origin: Origin, edition: Edition) -> Token {
    Token::new(TokenKind::DocComment, text, origin, edition, 0)
}

/// What rustc's lexer makes of a token's text.
fn classify(text: &str, string: bool) -> Option<TokenKind> {
    let first = text.chars().next()?;
    if string {
        return Some(TokenKind::Literal);
    }
    if first == '\'' {
        // A lifetime is a quote and a name (`'a`, `'r#a`, `'1` rustc reports
        // later); anything else after a quote is a character literal.
        let name = text[1..].strip_prefix("r#").unwrap_or(&text[1..]);
        let lifetime = name.chars().next().is_some_and(|c| c == '_' || c.is_alphanumeric())
            && name.chars().all(|c| c == '_' || c.is_alphanumeric());
        return Some(if lifetime { TokenKind::Lifetime } else { TokenKind::Literal });
    }
    if first.is_ascii_digit() || text.starts_with("b'") {
        return Some(TokenKind::Literal);
    }
    if first == '_' || first.is_alphabetic() {
        return Some(TokenKind::Ident);
    }
    if text.len() == 1 && PUNCT.contains(first) {
        return Some(TokenKind::Punct);
    }
    None
}

/// The tokens a doc comment stands for in an invocation, as rustc's
/// `desugar_doc_comments` writes them: `#[doc = r"…"]` (`#![…]` for an
/// inner one), the raw string with as many `#` as its text needs.
pub(crate) fn desugar_docs(trees: &[Tree]) -> Result<Vec<Tree>, Error> {
    let mut out = Vec::with_capacity(trees.len());
    for tree in trees {
        match tree {
            Tree::Token(t) if t.kind == TokenKind::DocComment => {
                let (inner, data) = doc_parts(&t.text);
                let mut hashes = 0usize;
                let mut count = 0usize;
                for c in data.chars() {
                    count = match c {
                        '"' => 1,
                        '#' if count > 0 => count + 1,
                        _ => 0,
                    };
                    hashes = hashes.max(count);
                }
                if hashes > 3 {
                    return Err(Error::new(
                        "a doc comment that needs a raw string of more than three `#`, \
                         which the grammar does not read",
                        Some(t.origin),
                    ));
                }
                let mk = |kind, text: &str| Token {
                    kind,
                    text: Arc::from(text),
                    origin: t.origin,
                    edition: t.edition,
                    mark: t.mark,
                    interpolated: false,
                };
                out.push(Tree::Token(mk(TokenKind::Punct, "#")));
                if inner {
                    out.push(Tree::Token(mk(TokenKind::Punct, "!")));
                }
                let h = "#".repeat(hashes);
                let lit = format!("r{h}\"{data}\"{h}");
                out.push(Tree::Group(Group {
                    delim: Delim::Bracket,
                    open: t.origin,
                    close: t.origin,
                    edition: t.edition,
                    mark: t.mark,
                    trees: Arc::new(vec![
                        Tree::Token(mk(TokenKind::Ident, "doc")),
                        Tree::Token(mk(TokenKind::Punct, "=")),
                        Tree::Token(mk(TokenKind::Literal, &lit)),
                    ]),
                }));
            }
            Tree::Token(t) => out.push(Tree::Token(t.clone())),
            Tree::Group(g) => out
                .push(Tree::Group(Group { trees: Arc::new(desugar_docs(&g.trees)?), ..g.clone() })),
        }
    }
    Ok(out)
}

/// A doc comment's style (inner or not) and its text: what follows `///`
/// or `//!`, or what stands between `/**` or `/*!` and `*/`.
fn doc_parts(text: &str) -> (bool, &str) {
    if let Some(rest) = text.strip_prefix("//!") {
        return (true, rest);
    }
    if let Some(rest) = text.strip_prefix("///") {
        return (false, rest);
    }
    let inner = text.starts_with("/*!");
    let body = &text[3..text.len() - 2];
    (inner, body)
}
