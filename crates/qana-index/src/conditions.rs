//! Conditions: what code is under where it may not be compiled (Rust's
//! `#[cfg(…)]`), and whether a configuration compiles it.
//!
//! A grammar says a condition with `@condition(pred)` on an attribute, and
//! how its predicate reads with `@predicate(name[, value])`,
//! `@predicates(name, list)` and `@predicate_literal(literal)`. A summary
//! keeps each construct's ([`Locations::condition_regions`](crate::Locations::condition_regions),
//! and each offered item's own, in [`Offers::conditions`](crate::Offers::conditions)).
//! A project model says each crate's [`Configuration`].
//!
//! The answer is three-valued ([`Holds`]): a configuration decides the
//! options it knows of, and any other may be set where the index cannot
//! see it (a build script, a flag), so may hold.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// A configuration predicate, as a summary keeps it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Condition {
    /// An option is set (`test`, `unix`).
    Option(String),
    /// An option is set with a value (`feature = "std"`); one name may be
    /// set with several values.
    Value(String, String),
    /// Every one holds (`all(…)`; none: holds).
    All(Vec<Condition>),
    /// One of them holds (`any(…)`; none: does not hold).
    Any(Vec<Condition>),
    /// It does not hold (`not(…)`).
    Not(Box<Condition>),
    /// A literal (`true`, `false`).
    Literal(bool),
    /// A predicate the index does not read (`version("1.80")`, one written
    /// in a way it does not know): it may hold.
    Unread,
}

/// The configuration a crate is compiled in: the options that are set,
/// and the names whose options it decides.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Configuration {
    /// The options set: a name alone (`test`), or a name and a value
    /// (`feature`, `"std"`).
    pub options: BTreeSet<(String, Option<String>)>,
    /// The names it decides: an option of one of them that is not set
    /// does not hold. An option of any other name may hold.
    pub decided: BTreeSet<String>,
}

/// Whether a condition holds in a configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Holds {
    /// It holds: what it is said of is compiled.
    Yes,
    /// It does not: what it is said of is not there.
    No,
    /// It may: the configuration does not decide it.
    Maybe,
}

impl Holds {
    fn not(self) -> Holds {
        match self {
            Holds::Yes => Holds::No,
            Holds::No => Holds::Yes,
            Holds::Maybe => Holds::Maybe,
        }
    }
}

impl Configuration {
    /// Whether the option `name` (with `value`, where it has one) holds.
    pub fn option(&self, name: &str, value: Option<&str>) -> Holds {
        let key = (name.to_string(), value.map(str::to_string));
        if self.options.contains(&key) {
            Holds::Yes
        } else if self.decided.contains(name) {
            Holds::No
        } else {
            Holds::Maybe
        }
    }
}

impl Condition {
    /// Whether it holds in `configuration`: an `all` holds where each
    /// does and not where one does not, an `any` holds where one does and
    /// not where none does, and otherwise each may.
    pub fn holds(&self, configuration: &Configuration) -> Holds {
        match self {
            Condition::Option(name) => configuration.option(name, None),
            Condition::Value(name, value) => configuration.option(name, Some(value)),
            Condition::All(all) => {
                let mut answer = Holds::Yes;
                for c in all {
                    match c.holds(configuration) {
                        Holds::No => return Holds::No,
                        Holds::Maybe => answer = Holds::Maybe,
                        Holds::Yes => {}
                    }
                }
                answer
            }
            Condition::Any(any) => {
                let mut answer = Holds::No;
                for c in any {
                    match c.holds(configuration) {
                        Holds::Yes => return Holds::Yes,
                        Holds::Maybe => answer = Holds::Maybe,
                        Holds::No => {}
                    }
                }
                answer
            }
            Condition::Not(c) => c.holds(configuration).not(),
            Condition::Literal(true) => Holds::Yes,
            Condition::Literal(false) => Holds::No,
            Condition::Unread => Holds::Maybe,
        }
    }

    /// `conditions`, all of which a construct is under, as one: none is no
    /// condition; one is itself; several, their `all`.
    pub fn all_of(mut conditions: Vec<Condition>) -> Option<Condition> {
        match conditions.len() {
            0 => None,
            1 => conditions.pop(),
            _ => Some(Condition::All(conditions)),
        }
    }

    /// How a predicate written `name(list)` reads, by its name, as
    /// configuration predicates are written: `all`, `any`, `not` (of one),
    /// and Rust's `target(os = "…", …)`, each of whose options is a
    /// `target_`-prefixed one. Any other is [`Unread`](Condition::Unread).
    pub fn list(name: &str, mut list: Vec<Condition>) -> Condition {
        match name {
            "all" => Condition::All(list),
            "any" => Condition::Any(list),
            "not" if list.len() == 1 => Condition::Not(Box::new(list.pop().expect("one"))),
            "target" => {
                let mut options = Vec::with_capacity(list.len());
                for c in list {
                    match c {
                        Condition::Value(n, v) => {
                            options.push(Condition::Value(format!("target_{n}"), v))
                        }
                        _ => return Condition::Unread,
                    }
                }
                Condition::All(options)
            }
            _ => Condition::Unread,
        }
    }

    /// A predicate written `name = value`, where `value` is a literal as
    /// written: a string's text (plain, with its escapes read, or raw).
    /// Any other literal is [`Unread`](Condition::Unread).
    pub fn value(name: &str, literal: &str) -> Condition {
        match string_text(literal) {
            Some(value) => Condition::Value(name.to_string(), value),
            None => Condition::Unread,
        }
    }

    /// A literal predicate: `true` or `false`; any other is
    /// [`Unread`](Condition::Unread).
    pub fn literal(literal: &str) -> Condition {
        match literal {
            "true" => Condition::Literal(true),
            "false" => Condition::Literal(false),
            _ => Condition::Unread,
        }
    }
}

/// The text of the string literal `literal` (`"std"`, `r"std"`,
/// `r#"std"#`), its escapes read; `None` for any other literal.
fn string_text(literal: &str) -> Option<String> {
    if let Some(raw) = literal.strip_prefix('r') {
        let hashes = raw.len() - raw.trim_start_matches('#').len();
        let fence = "#".repeat(hashes);
        return raw
            .strip_prefix(fence.as_str())?
            .strip_prefix('"')?
            .strip_suffix(fence.as_str())?
            .strip_suffix('"')
            .map(str::to_string);
    }
    let inner = literal.strip_prefix('"')?.strip_suffix('"')?;
    let mut text = String::with_capacity(inner.len());
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            text.push(c);
            continue;
        }
        match chars.next()? {
            'n' => text.push('\n'),
            'r' => text.push('\r'),
            't' => text.push('\t'),
            '0' => text.push('\0'),
            '\\' => text.push('\\'),
            '\'' => text.push('\''),
            '"' => text.push('"'),
            'x' => {
                let digits: String = [chars.next()?, chars.next()?].iter().collect();
                let value = u8::from_str_radix(&digits, 16)
                    .ok()
                    .filter(|v| *v <= 0x7f)?;
                text.push(char::from(value));
            }
            'u' => {
                if chars.next()? != '{' {
                    return None;
                }
                let mut digits = String::new();
                loop {
                    match chars.next()? {
                        '}' => break,
                        '_' => {}
                        d => digits.push(d),
                    }
                }
                text.push(char::from_u32(u32::from_str_radix(&digits, 16).ok()?)?);
            }
            '\n' => {
                while chars.peek().is_some_and(|c| c.is_whitespace()) {
                    chars.next();
                }
            }
            _ => return None,
        }
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configuration(options: &[(&str, Option<&str>)], decided: &[&str]) -> Configuration {
        Configuration {
            options: options
                .iter()
                .map(|(n, v)| (n.to_string(), v.map(str::to_string)))
                .collect(),
            decided: decided.iter().map(|n| n.to_string()).collect(),
        }
    }

    fn option(name: &str) -> Condition {
        Condition::Option(name.into())
    }

    #[test]
    fn an_option_holds_where_set_does_not_where_decided_and_may_elsewhere() {
        let c = configuration(
            &[("unix", None), ("feature", Some("std"))],
            &["unix", "windows", "feature"],
        );
        assert_eq!(option("unix").holds(&c), Holds::Yes);
        assert_eq!(option("windows").holds(&c), Holds::No);
        assert_eq!(
            option("tokio_unstable").holds(&c),
            Holds::Maybe,
            "a build script may set it"
        );
        assert_eq!(Condition::value("feature", "\"std\"").holds(&c), Holds::Yes);
        assert_eq!(
            Condition::value("feature", "\"serde\"").holds(&c),
            Holds::No
        );
        assert_eq!(
            Condition::value("feature", "r#\"std\"#").holds(&c),
            Holds::Yes
        );
    }

    #[test]
    fn all_any_and_not_are_three_valued() {
        let c = configuration(&[("unix", None)], &["unix", "windows"]);
        let maybe = option("loom");
        let all = |v: Vec<Condition>| Condition::All(v);
        let any = |v: Vec<Condition>| Condition::Any(v);
        assert_eq!(
            all(vec![option("unix"), maybe.clone()]).holds(&c),
            Holds::Maybe
        );
        assert_eq!(
            all(vec![option("windows"), maybe.clone()]).holds(&c),
            Holds::No
        );
        assert_eq!(
            any(vec![option("unix"), maybe.clone()]).holds(&c),
            Holds::Yes
        );
        assert_eq!(
            any(vec![option("windows"), maybe.clone()]).holds(&c),
            Holds::Maybe
        );
        assert_eq!(all(Vec::new()).holds(&c), Holds::Yes);
        assert_eq!(any(Vec::new()).holds(&c), Holds::No);
        assert_eq!(
            Condition::list("not", vec![option("windows")]).holds(&c),
            Holds::Yes
        );
        assert_eq!(Condition::list("not", vec![maybe]).holds(&c), Holds::Maybe);
        assert_eq!(Condition::list("not", Vec::new()), Condition::Unread);
        assert_eq!(Condition::Unread.holds(&c), Holds::Maybe);
        assert_eq!(Condition::literal("false").holds(&c), Holds::No);
    }

    #[test]
    fn a_target_list_is_its_target_options() {
        let list = Condition::list(
            "target",
            vec![
                Condition::value("os", "\"macos\""),
                Condition::value("arch", "\"aarch64\""),
            ],
        );
        assert_eq!(
            list,
            Condition::All(vec![
                Condition::Value("target_os".into(), "macos".into()),
                Condition::Value("target_arch".into(), "aarch64".into()),
            ])
        );
        assert_eq!(
            Condition::list("version", vec![Condition::value("x", "\"1\"")]),
            Condition::Unread
        );
        assert_eq!(
            Condition::value("feature", "1"),
            Condition::Unread,
            "no string"
        );
    }
}
