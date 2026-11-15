//! The `/.../` pattern sub-language: a compact regular-expression
//! surface compiled to [`Pat`] values at grammar-compile time. This is
//! data-inside-a-token (like digits inside a number literal), so parsing
//! it here — not in the `.qana` syntax grammar — keeps the parse pure
//! (envelope L5) while the L1 lint still certifies the compiled result.
//!
//! Envelope by construction: there is NO way to write a line terminator.
//! `\n`/`\r` are refused with an explanation, `.` and negated classes
//! exclude terminators by definition — mirroring [`ClassSet::any`].
//!
//! Trailing context: a pattern, or any branch of it, may END with
//! `(?=…)` or `(?!…)`. The token is then accepted only if the rest of
//! its line does, or does not, begin with a match of the inner pattern.
//! It is the one form of lookaround there is, and it only looks ahead on
//! the same line, so L1 is untouched. Nothing may follow it, and it may
//! not be repeated or nested: those are refused here, with a position.
//!
//! Canonical-form contract (what the self-host fixed point relies on):
//! adjacent unpostfixed literal characters merge into one `Pat::Lit`,
//! singleton sequences/alternations collapse to their element, and
//! groups add no wrapper — so a pattern parses to the same value a
//! careful author would build with the combinators.

use qana_grammar::pat::{ClassSet, Pat};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatError {
    /// Byte offset within the pattern INTERIOR (caller adds the span of
    /// the opening delimiter).
    pub pos: usize,
    pub msg: String,
}

fn err<T>(pos: usize, msg: impl Into<String>) -> Result<T, PatError> {
    Err(PatError { pos, msg: msg.into() })
}

struct P<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> P<'a> {
    fn peek(&self) -> Option<char> {
        self.src.get(self.pos).map(|&b| b as char)
    }
    fn bump(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }
}

/// Parse a pattern interior (delimiters already stripped).
pub fn parse_pattern(src: &str) -> Result<Pat, PatError> {
    if !src.is_ascii() {
        // Explicit chars are ASCII-only (P1 alphabet contract); unicode
        // is reached through the class shorthands.
        let pos = src.bytes().position(|b| !b.is_ascii()).unwrap_or(0);
        return err(pos, "non-ASCII characters: use \\a (alpha), \\w (alnum), \\s (space) classes");
    }
    let mut p = P { src: src.as_bytes(), pos: 0 };
    let pat = parse_alt(&mut p)?;
    match p.peek() {
        None => Ok(pat),
        Some(')') => err(p.pos, "unmatched `)`"),
        Some(c) => err(p.pos, format!("unexpected `{c}`")),
    }
}

fn parse_alt(p: &mut P) -> Result<Pat, PatError> {
    let mut branches = vec![parse_seq(p)?];
    while p.peek() == Some('|') {
        p.bump();
        branches.push(parse_seq(p)?);
    }
    Ok(if branches.len() == 1 { branches.pop().unwrap() } else { Pat::Alt(branches) })
}

fn parse_seq(p: &mut P) -> Result<Pat, PatError> {
    let mut items: Vec<Pat> = Vec::new();
    let mut lit = String::new(); // pending mergeable literal run
    let start = p.pos;
    // Set once an item ends in a trailing context: it must be the last.
    let mut ended_by_context = false;
    loop {
        match p.peek() {
            None | Some('|') | Some(')') => break,
            Some(c @ ('*' | '+' | '?')) => return err(p.pos, format!("`{c}` needs an operand")),
            _ => {}
        }
        if ended_by_context {
            return err(
                p.pos,
                "nothing may follow a trailing context — `(?=…)` / `(?!…)` must end the token",
            );
        }
        let at = p.pos;
        let atom = parse_atom(p)?;
        let postfix = match p.peek() {
            Some('*') => {
                p.bump();
                Some('*')
            }
            Some('+') => {
                p.bump();
                Some('+')
            }
            Some('?') => {
                p.bump();
                Some('?')
            }
            _ => None,
        };
        if atom.has_context() {
            // A context matches no characters: repeating it, or making
            // the context itself optional, says nothing. A group that
            // merely ENDS in one may be optional (`(\.(?!\.))?`).
            let bare = matches!(atom, Pat::Ahead { .. });
            match postfix {
                Some(op @ ('*' | '+')) => {
                    return err(at, format!("a trailing context cannot be repeated with `{op}`"));
                }
                Some('?') if bare => {
                    return err(at, "a trailing context cannot be optional");
                }
                _ => {}
            }
            ended_by_context = true;
        }
        match (postfix, &atom) {
            (None, Pat::Lit(s)) => lit.push_str(s),
            _ => {
                if !lit.is_empty() {
                    items.push(Pat::Lit(std::mem::take(&mut lit)));
                }
                let wrapped = match postfix {
                    Some('*') => Pat::star(atom),
                    Some('+') => Pat::plus(atom),
                    Some('?') => Pat::opt(atom),
                    _ => atom,
                };
                items.push(wrapped);
            }
        }
        let _ = at;
    }
    if !lit.is_empty() {
        items.push(Pat::Lit(lit));
    }
    match items.len() {
        0 => err(start, "empty pattern branch"),
        1 => Ok(items.pop().unwrap()),
        _ => Ok(Pat::Seq(items)),
    }
}

fn parse_atom(p: &mut P) -> Result<Pat, PatError> {
    let at = p.pos;
    match p.bump().expect("caller checked") {
        '.' => Ok(Pat::Class(ClassSet::any())),
        '(' if p.peek() == Some('?') => {
            // `(?=…)` / `(?!…)`: trailing context.
            p.bump();
            let negated = match p.bump() {
                Some('=') => false,
                Some('!') => true,
                _ => {
                    return err(
                        at,
                        "unknown group `(?…` — the only forms are `(?=…)` and `(?!…)`, \
                         a trailing context that ends a token",
                    )
                }
            };
            let context = parse_alt(p)?;
            if p.peek() != Some(')') {
                return err(p.pos, "unclosed `(`");
            }
            p.bump();
            if context.has_context() {
                return err(at, "a trailing context cannot contain another");
            }
            Ok(Pat::Ahead { context: Box::new(context), negated })
        }
        '(' => {
            let inner = parse_alt(p)?;
            if p.peek() != Some(')') {
                return err(p.pos, "unclosed `(`");
            }
            p.bump();
            Ok(inner)
        }
        '[' => parse_class(p, at),
        '\\' => match escape(p, at)? {
            Esc::Lit(c) => Ok(Pat::Lit(c.to_string())),
            Esc::Short(f) => Ok(Pat::Class(f.into_class())),
        },
        ']' => err(at, "unmatched `]`"),
        c => Ok(Pat::Lit(c.to_string())),
    }
}

enum Esc {
    Lit(char),
    Short(Shorthand),
}

#[derive(Clone, Copy)]
enum Shorthand {
    Digit,
    Alpha,
    Alnum,
    Space,
}

impl Shorthand {
    fn into_class(self) -> ClassSet {
        match self {
            Shorthand::Digit => ClassSet::digit(),
            Shorthand::Alpha => ClassSet { alpha: true, ..Default::default() },
            Shorthand::Alnum => ClassSet { alnum: true, ..Default::default() },
            Shorthand::Space => ClassSet::line_ws(),
        }
    }
}

fn escape(p: &mut P, at: usize) -> Result<Esc, PatError> {
    match p.bump() {
        None => err(at, "dangling `\\`"),
        Some('n') | Some('r') => err(
            at,
            "line terminators cannot appear in tokens (envelope L1) — express multi-line constructs via modes",
        ),
        Some('t') => Ok(Esc::Lit('\t')),
        Some('d') => Ok(Esc::Short(Shorthand::Digit)),
        Some('a') => Ok(Esc::Short(Shorthand::Alpha)),
        Some('w') => Ok(Esc::Short(Shorthand::Alnum)),
        Some('s') => Ok(Esc::Short(Shorthand::Space)),
        // A letter that is not one of the shorthands above is almost
        // always someone reaching for a PCRE class qana does not have
        // (`\S`, `\D`, `\b`, `\p{…}`). Treating it as a literal letter —
        // the old behaviour — silently produces a token that matches the
        // wrong thing, so refuse it and name the alternatives.
        Some(c) if c.is_ascii_alphabetic() => err(
            at,
            &format!(
                "unknown escape `\\{c}` — qana patterns support \\d \\a \\w \\s \\t; \
                 use `.` for any character (newlines are excluded by L1) \
                 or a class like `[^\"\\\\]`"
            ),
        ),
        Some(c) => Ok(Esc::Lit(c)),
    }
}

fn parse_class(p: &mut P, open: usize) -> Result<Pat, PatError> {
    let mut set = ClassSet::default();
    if p.peek() == Some('^') {
        p.bump();
        set.negated = true;
    }
    let mut any = false;
    loop {
        match p.peek() {
            None => return err(open, "unclosed `[`"),
            Some(']') => {
                p.bump();
                break;
            }
            _ => {}
        }
        let at = p.pos;
        let lhs = class_element(p, at)?;
        // Range `a-b` (a `-` right before `]` is a literal dash).
        if p.peek() == Some('-') && p.src.get(p.pos + 1) != Some(&b']') {
            p.bump();
            let rat = p.pos;
            let lo = match lhs {
                ClassElem::Char(c) => c,
                ClassElem::Short(_) => return err(at, "class shorthand cannot start a range"),
            };
            let hi = match class_element(p, rat)? {
                ClassElem::Char(c) => c,
                ClassElem::Short(_) => return err(rat, "class shorthand cannot end a range"),
            };
            if hi < lo {
                return err(at, format!("empty range `{lo}-{hi}`"));
            }
            set.ranges.push((lo, hi));
        } else {
            match lhs {
                ClassElem::Char(c) => set.chars.push(c),
                ClassElem::Short(s) => match s {
                    Shorthand::Digit => set.digit = true,
                    Shorthand::Alpha => set.alpha = true,
                    Shorthand::Alnum => set.alnum = true,
                    Shorthand::Space => set.lws = true,
                },
            }
        }
        any = true;
    }
    if !any {
        return err(open, "empty class");
    }
    if set.negated {
        // Negation is over LINE content only (envelope L1): terminators
        // are always excluded, mirroring `ClassSet::any()`.
        set.chars.push('\r');
        set.chars.push('\n');
    }
    Ok(Pat::Class(set))
}

enum ClassElem {
    Char(char),
    Short(Shorthand),
}

fn class_element(p: &mut P, at: usize) -> Result<ClassElem, PatError> {
    match p.bump() {
        None => err(at, "unclosed `[`"),
        Some('\\') => match escape(p, at)? {
            Esc::Lit(c) => Ok(ClassElem::Char(c)),
            Esc::Short(s) => Ok(ClassElem::Short(s)),
        },
        Some(c) => Ok(ClassElem::Char(c)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qana_grammar::pat::{ClassSet, Pat};

    #[test]
    fn canonical_forms_match_combinators() {
        // The exact demo-grammar patterns, surface vs combinators.
        assert_eq!(parse_pattern(r"\s+").unwrap(), Pat::plus(Pat::Class(ClassSet::line_ws())));
        assert_eq!(
            parse_pattern(r"\/\/.*").unwrap(),
            Pat::seq([Pat::lit("//"), Pat::star(Pat::Class(ClassSet::any()))])
        );
        assert_eq!(
            parse_pattern(r"[\a_][\w_]*").unwrap(),
            Pat::seq([
                Pat::Class(ClassSet::ident_start()),
                Pat::star(Pat::Class(ClassSet::ident_cont())),
            ])
        );
        assert_eq!(
            parse_pattern(r"\d+(\.\d+)?").unwrap(),
            Pat::seq([
                Pat::plus(Pat::Class(ClassSet::digit())),
                Pat::opt(Pat::seq([Pat::lit("."), Pat::plus(Pat::Class(ClassSet::digit()))])),
            ])
        );
        let esc = Pat::seq([Pat::lit("\\"), Pat::Class(ClassSet::any())]);
        let safe = Pat::Class(ClassSet::not_chars(&['"', '\\', '\r', '\n']));
        let body = Pat::star(Pat::alt([esc.clone(), safe.clone()]));
        assert_eq!(
            parse_pattern(r#""(\\.|[^"\\])*""#).unwrap(),
            Pat::seq([Pat::lit("\""), body.clone(), Pat::lit("\"")])
        );
        assert_eq!(
            parse_pattern(r#""(\\.|[^"\\])*\\?"#).unwrap(),
            Pat::seq([Pat::lit("\""), body, Pat::opt(Pat::lit("\\"))])
        );
        assert_eq!(
            parse_pattern(r"[!-\/:-@\[-`{-~]").unwrap(),
            Pat::Class(ClassSet::ranges(&[('!', '/'), (':', '@'), ('[', '`'), ('{', '~')]))
        );
        assert_eq!(
            parse_pattern(r"[^*\/]+").unwrap(),
            Pat::plus(Pat::Class(ClassSet::not_chars(&['*', '/', '\r', '\n'])))
        );
        assert_eq!(
            parse_pattern(r"[*\/]").unwrap(),
            Pat::Class(ClassSet::chars(&['*', '/']))
        );
    }

    #[test]
    fn terminators_are_unwritable() {
        assert!(parse_pattern(r"a\nb").unwrap_err().msg.contains("L1"));
        assert!(parse_pattern(r"[\r]").unwrap_err().msg.contains("L1"));
    }

    #[test]
    fn errors_carry_positions() {
        assert_eq!(parse_pattern("ab(cd").unwrap_err().pos, 5);
        assert_eq!(parse_pattern("*a").unwrap_err().pos, 0);
        assert_eq!(parse_pattern("a||b").unwrap_err().pos, 2);
        assert!(parse_pattern("[]").is_err());
        assert!(parse_pattern("[z-a]").is_err());
    }

    #[test]
    fn trailing_context_parses_to_the_combinator_form() {
        let digits = || Pat::plus(Pat::Class(ClassSet::digit()));
        // `1.` is a float unless a `.`, `_` or a letter follows.
        assert_eq!(
            parse_pattern(r"\d+\.(?![._\a])").unwrap(),
            Pat::seq([
                digits(),
                Pat::lit("."),
                Pat::not_before(Pat::Class(ClassSet {
                    chars: vec!['.', '_'],
                    alpha: true,
                    ..Default::default()
                })),
            ])
        );
        // One branch of an alternation may end in a context.
        assert_eq!(
            parse_pattern(r"\d+\.\d+|\d+\.(?!\.)").unwrap(),
            Pat::alt([
                Pat::seq([digits(), Pat::lit("."), digits()]),
                Pat::seq([digits(), Pat::lit("."), Pat::not_before(Pat::lit("."))]),
            ])
        );
        // A context is a whole pattern: here, spaces and then a colon.
        assert_eq!(
            parse_pattern(r"\a+(?=\s*:)").unwrap(),
            Pat::seq([
                Pat::plus(Pat::Class(ClassSet { alpha: true, ..Default::default() })),
                Pat::before(Pat::seq([
                    Pat::star(Pat::Class(ClassSet::line_ws())),
                    Pat::lit(":"),
                ])),
            ])
        );
        // A group that ends in a context may be optional.
        assert_eq!(
            parse_pattern(r"a(b(?=c))?").unwrap(),
            Pat::seq([
                Pat::lit("a"),
                Pat::opt(Pat::seq([Pat::lit("b"), Pat::before(Pat::lit("c"))])),
            ])
        );
        for ok in [r"\d+\.(?![._\a])", r"\d+\.\d+|\d+\.(?!\.)", r"\a+(?=\s*:)", r"a(b(?=c))?"] {
            assert_eq!(parse_pattern(ok).unwrap().check_contexts(), Ok(()), "{ok}");
        }
    }

    #[test]
    fn a_trailing_context_only_ends_a_token() {
        // Something after it.
        let e = parse_pattern("a(?=b)c").unwrap_err();
        assert_eq!(e.pos, 6);
        assert!(e.msg.contains("must end the token"), "{}", e.msg);
        // Something after a group that ends in one.
        let e = parse_pattern("(a(?=b))c").unwrap_err();
        assert_eq!(e.pos, 8);
        assert!(e.msg.contains("must end the token"), "{}", e.msg);
        // Repeated, optional, nested.
        assert!(parse_pattern("a(?=b)*").unwrap_err().msg.contains("repeated"));
        assert!(parse_pattern("(a(?=b))+").unwrap_err().msg.contains("repeated"));
        assert!(parse_pattern("a(?=b)?").unwrap_err().msg.contains("optional"));
        assert!(parse_pattern("a(?=b(?!c))").unwrap_err().msg.contains("contain another"));
        // Not a form qana has.
        let e = parse_pattern("a(?:b)").unwrap_err();
        assert_eq!(e.pos, 1);
        assert!(e.msg.contains("(?=…)"), "{}", e.msg);
        assert!(parse_pattern("a(?<=b)").is_err(), "no lookbehind");
        // An empty context, and an unclosed one.
        assert!(parse_pattern("a(?=)").is_err());
        assert_eq!(parse_pattern("a(?=b").unwrap_err().msg, "unclosed `(`");
        // Each branch is judged by itself.
        assert!(parse_pattern("a(?=b)|c(?!d)").is_ok());
        assert!(parse_pattern("a(?=b)|cd").is_ok());
    }

    #[test]
    fn merge_respects_postfix() {
        assert_eq!(
            parse_pattern("ab*").unwrap(),
            Pat::seq([Pat::lit("a"), Pat::star(Pat::lit("b"))])
        );
        assert_eq!(parse_pattern("abc").unwrap(), Pat::lit("abc"));
    }
}
