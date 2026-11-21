//! The compiler's `format_args!`, as what it reads: its arguments and the
//! names its format string captures ([`crate::MacroRules::format_args`]).
//!
//! rustc parses the format string (the literal's value, its escapes
//! decoded) and maps each piece back to the literal's text; so does this,
//! for the names it captures: an implicit argument (`{x}`, `{x:?}`) and a
//! width or precision parameter (`{:w$}`, `{:.p$}`) that no named argument
//! is.

use std::collections::HashSet;
use std::sync::Arc;

use crate::token::{is_reserved, Delim, Group, Tree};
use crate::{Error, Origin, Token, TokenKind, TokenStream};

/// The origin of a token a built-in writes, which no text holds.
const WRITTEN: Origin = Origin { source: u32::MAX, start: 0, end: 0 };

/// What `format_args!` reads of `input`, expansion `mark`'s: a parenthesized
/// list of its arguments' expressions, as written (a named argument's
/// value without its name), then the names its format string captures, each
/// a token written where the string writes it.
pub(crate) fn read(input: &TokenStream, mark: u32) -> Result<TokenStream, Error> {
    let trees = &input.trees;
    // The arguments, by the commas between them (one inside a group is the
    // group's).
    let mut segments: Vec<&[Tree]> = Vec::new();
    let mut from = 0;
    for (k, tree) in trees.iter().enumerate() {
        if let Tree::Token(t) = tree {
            if t.is_punct(",") {
                segments.push(&trees[from..k]);
                from = k + 1;
            }
        }
    }
    segments.push(&trees[from..]);
    let Some((format, args)) = segments.split_first() else {
        return Err(Error::new("requires at least a format string argument", None));
    };
    let by_name: HashSet<String> =
        args.iter().filter_map(|s| named(s).map(|(name, _)| name)).collect();
    let comma = |edition| Tree::Token(Token::new(TokenKind::Punct, ",", WRITTEN, edition, mark));
    let edition = trees.first().map_or(crate::Edition::E2021, |t| match t {
        Tree::Token(t) => t.edition,
        Tree::Group(g) => g.edition,
    });
    let mut out: Vec<Tree> = Vec::new();
    let captured = match literal(format) {
        Some(lit) => captures(&lit.text)
            .into_iter()
            .filter(|(name, _)| !by_name.contains(name) && !is_reserved(name, lit.edition))
            .map(|(name, (s, e))| {
                let origin = Origin {
                    source: lit.origin.source,
                    start: lit.origin.start + s,
                    end: lit.origin.start + e,
                };
                Tree::Token(Token::new(TokenKind::Ident, &name, origin, lit.edition, lit.mark))
            })
            .collect(),
        None => {
            // A format no literal writes: an expression of its own.
            if !format.is_empty() {
                out.extend(format.iter().cloned());
                out.push(comma(edition));
            }
            Vec::new()
        }
    };
    for segment in args {
        let value = match named(segment) {
            Some((_, value)) => value,
            None => segment,
        };
        if value.is_empty() {
            continue;
        }
        out.extend(value.iter().cloned());
        out.push(comma(edition));
    }
    for name in captured {
        out.push(name);
        out.push(comma(edition));
    }
    let group = Group {
        delim: Delim::Paren,
        open: WRITTEN,
        close: WRITTEN,
        edition,
        mark,
        trees: Arc::new(out),
    };
    Ok(TokenStream::new(vec![Tree::Group(group)]))
}

/// A named argument's name and value (`name = value`).
fn named(segment: &[Tree]) -> Option<(String, &[Tree])> {
    match segment {
        [Tree::Token(name), Tree::Token(eq), value @ ..]
            if name.kind == TokenKind::Ident && eq.is_punct("=") =>
        {
            let (name, _) = name.ident()?;
            Some((name.to_string(), value))
        }
        _ => None,
    }
}

/// The string literal a format is, through a fragment's invisible group
/// (`$fmt:expr` forwarded); `None` for anything else.
fn literal(format: &[Tree]) -> Option<&Token> {
    match format {
        [Tree::Token(t)] if t.kind == TokenKind::Literal => {
            let text = &*t.text;
            (text.starts_with('"') || text.starts_with("r\"") || text.starts_with("r#"))
                .then_some(t)
        }
        [Tree::Group(g)] if matches!(g.delim, Delim::Invisible(_)) => literal(&g.trees),
        _ => None,
    }
}

/// The names the format string `literal` (a string literal's text, quotes
/// included) captures, each with its byte span in that text: in the order
/// it writes them, each place once.
pub(crate) fn captures(literal: &str) -> Vec<(String, (u32, u32))> {
    let Some(chars) = cooked(literal) else { return Vec::new() };
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i].0 {
            '{' if chars.get(i + 1).is_some_and(|c| c.0 == '{') => i += 2,
            '{' => {
                let Some(close) = (i + 1..chars.len()).find(|&k| chars[k].0 == '}') else { break };
                let inside = &chars[i + 1..close];
                let colon = inside.iter().position(|c| c.0 == ':').unwrap_or(inside.len());
                // The argument: a name, an index, or nothing.
                if let Some(name) = identifier(&inside[..colon]) {
                    out.push(name);
                }
                // The spec: a width or a precision a name gives (`w$`).
                let spec = inside.get(colon + 1..).unwrap_or(&[]);
                let mut k = 0;
                while k < spec.len() {
                    if is_start(spec[k].0) {
                        let end = (k..spec.len())
                            .find(|&j| !is_continue(spec[j].0))
                            .unwrap_or(spec.len());
                        if spec.get(end).is_some_and(|c| c.0 == '$') {
                            if let Some(name) = identifier(&spec[k..end]) {
                                out.push(name);
                            }
                        }
                        k = end;
                    } else {
                        k += 1;
                    }
                }
                i = close + 1;
            }
            _ => i += 1,
        }
    }
    out
}

/// `chars` as one identifier (not `_` alone), with its span.
fn identifier(chars: &[(char, (u32, u32))]) -> Option<(String, (u32, u32))> {
    let (first, rest) = chars.split_first()?;
    if !is_start(first.0) || !rest.iter().all(|c| is_continue(c.0)) {
        return None;
    }
    let name: String = chars.iter().map(|c| c.0).collect();
    if name == "_" {
        return None;
    }
    Some((name, (first.1 .0, chars.last()?.1 .1)))
}

fn is_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_continue(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The value of string literal `literal` (its text, quotes included), each
/// char with the span of the text that writes it; `None` for one that is no
/// string literal.
fn cooked(literal: &str) -> Option<Vec<(char, (u32, u32))>> {
    if let Some(raw) = literal.strip_prefix('r') {
        let hashes = raw.len() - raw.trim_start_matches('#').len();
        let open = 1 + hashes + 1;
        let close = literal.len().checked_sub(1 + hashes)?;
        if close < open || !literal[open - 1..].starts_with('"') {
            return None;
        }
        return Some(
            literal[open..close]
                .char_indices()
                .map(|(k, c)| (c, ((open + k) as u32, (open + k + c.len_utf8()) as u32)))
                .collect(),
        );
    }
    let body = literal.strip_prefix('"')?.strip_suffix('"')?;
    let mut out = Vec::new();
    let mut it = body.char_indices().peekable();
    while let Some((k, c)) = it.next() {
        let at = (k + 1) as u32;
        if c != '\\' {
            out.push((c, (at, at + c.len_utf8() as u32)));
            continue;
        }
        let Some((_, e)) = it.next() else { break };
        let value = match e {
            'n' => Some('\n'),
            'r' => Some('\r'),
            't' => Some('\t'),
            '\\' => Some('\\'),
            '0' => Some('\0'),
            '\'' => Some('\''),
            '"' => Some('"'),
            'x' => {
                let hex: String = (0..2).filter_map(|_| it.next().map(|(_, h)| h)).collect();
                u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32)
            }
            'u' => {
                let mut hex = String::new();
                for (_, h) in it.by_ref() {
                    match h {
                        '{' | '_' => {}
                        '}' => break,
                        h => hex.push(h),
                    }
                }
                u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32)
            }
            // A line continuation: the newline and the whitespace after it.
            '\n' | '\r' => {
                while it.peek().is_some_and(|(_, w)| w.is_whitespace()) {
                    it.next();
                }
                None
            }
            _ => None,
        };
        if let Some(value) = value {
            let end = it.peek().map_or(body.len() + 1, |(j, _)| j + 1) as u32;
            out.push((value, (at, end)));
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::captures;

    /// The names `literal` captures, as its text writes them at their spans.
    fn read(literal: &str) -> Vec<String> {
        captures(literal)
            .into_iter()
            .map(|(name, (s, e))| {
                assert_eq!(&literal[s as usize..e as usize], name, "its span in {literal}");
                name
            })
            .collect()
    }

    #[test]
    fn implicit_arguments_and_parameters_are_captured_where_the_string_writes_them() {
        assert_eq!(read(r#""{x} and {y:?}""#), ["x", "y"]);
        assert_eq!(read(r#""{:w$} {:.p$} {:>1$} {0:x}""#), ["w", "p"]);
        assert_eq!(read(r#""{v:>width$.prec$}""#), ["v", "width", "prec"]);
        assert_eq!(read(r#""{:a<5} {:x?} {:#X}""#), Vec::<String>::new(), "a fill, a type");
        assert_eq!(read(r#""{{x}} {} {0} {_}""#), Vec::<String>::new(), "escaped, positional");
        assert_eq!(read(r#""{données}""#), ["données"]);
    }

    #[test]
    fn escapes_and_raw_strings_are_read_by_their_values() {
        assert_eq!(read(r#""\u{1F600} {a}\n{b}""#), ["a", "b"], "a unicode escape is no argument");
        assert_eq!(
            read(r#""\x7bc}""#),
            ["c"],
            "a hex escape is the brace it writes, as rustc reads it"
        );
        assert_eq!(read("r#\"{a} \"{b}\"\"#"), ["a", "b"]);
        assert_eq!(read("\"line \\\n    {c}\""), ["c"], "after a continuation");
    }
}
