//! What the tests share: the grammar, and rustc's traces read back.
//!
//! A trace is what `rustc +nightly -Ztrace-macros` printed: each
//! `macro_rules!` (and `macro`) expansion it made, the tokens it was given
//! and the tokens it wrote. [`check`] expands a program, every invocation
//! in turn, and asserts that every expansion made is one rustc made, token
//! for token, and every one rustc made is made.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use qana_lang::EmbeddedLang;
use qana_mbe::{expand_all, Edition, Form, MacroRules, Nested, Outcome, Token, TokenStream};

const GRAMMAR: &str = include_str!("../../../../examples/rust/rust.qana");

pub fn lang() -> &'static EmbeddedLang {
    static LANG: OnceLock<EmbeddedLang> = OnceLock::new();
    LANG.get_or_init(|| EmbeddedLang::from_qana_source(GRAMMAR).unwrap_or_else(|d| panic!("{d:?}")))
}

pub fn lex(text: &str, source: u32) -> TokenStream {
    lex_in(text, source, Edition::E2021)
}

pub fn lex_in(text: &str, source: u32, edition: Edition) -> TokenStream {
    TokenStream::lex(lang(), text, source, 0, edition)
        .unwrap_or_else(|e| panic!("lexing source {source}: {e}"))
}

/// A stream's tokens' texts, `$crate` one token, as rustc prints it.
pub fn texts(stream: &TokenStream) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in stream.token_texts() {
        if t == "crate" && out.last().is_some_and(|l| l == "$") {
            out.pop();
            out.push("$crate".to_string());
        } else {
            out.push(t);
        }
    }
    out
}

/// The definition `name` in a fixture: the text after its name up to its
/// closing `}`, which in these files (std's style) is the first line that
/// is `}` alone.
pub fn definition(
    text: &str,
    source: u32,
    keyword: &str,
    name: &str,
    form: Form,
) -> Arc<MacroRules> {
    let head = format!("{keyword} {name}");
    let at = text.find(&head).unwrap_or_else(|| panic!("no `{head}`")) + head.len();
    let end = at + text[at..].find("\n}").expect("a closing line") + 2;
    let body = TokenStream::lex(lang(), &text[at..end], source, at as u32, Edition::E2021)
        .unwrap_or_else(|e| panic!("lexing `{name}`: {e}"));
    Arc::new(MacroRules::read(&body, form).unwrap_or_else(|e| panic!("reading `{name}`: {e}")))
}

/// One expansion rustc printed: the macro, the tokens it was given, the
/// tokens it wrote.
#[derive(Debug)]
pub struct Step {
    pub name: String,
    pub input: Vec<String>,
    pub output: Vec<String>,
}

/// The steps of a trace: each note `expanding `name! { input }`` and the
/// `to `output`` after it, their continuation lines unindented by the
/// note's own indent (which rustc's renderer adds to every line, those
/// inside a literal too).
pub fn trace(text: &str) -> Vec<Step> {
    let lines: Vec<&str> = text.lines().filter(|l| !l.starts_with("//")).collect();
    let mut items: Vec<(bool, String)> = Vec::new();
    let mut i = 0usize;
    while i < lines.len() {
        let line = lines[i];
        let (kind, marker) = if let Some(k) = line.find("= note: expanding `") {
            (true, k + "= note: ".len())
        } else if let Some(k) = line.find("= note: to `") {
            (false, k + "= note: ".len())
        } else {
            i += 1;
            continue;
        };
        let indent = marker;
        let first = &line[marker..];
        let first = first.strip_prefix(if kind { "expanding `" } else { "to `" }).expect("a note");
        let mut body = first.to_string();
        i += 1;
        while i < lines.len()
            && lines[i].len() >= indent
            && lines[i][..indent].bytes().all(|b| b == b' ')
            && !lines[i].contains("= note: ")
        {
            body.push('\n');
            body.push_str(&lines[i][indent..]);
            i += 1;
        }
        let body = body.strip_suffix('`').expect("a note ends in a backtick").to_string();
        items.push((kind, body));
    }
    let mut steps = Vec::new();
    let mut it = items.into_iter();
    while let Some((true, expanding)) = it.next() {
        let (false, to) = it.next().expect("each expansion's result") else {
            panic!("an expansion without its result")
        };
        let bang = expanding.find("! {").expect("`name! {`");
        let name = expanding[..bang].to_string();
        let input = expanding[bang + 3..].strip_suffix('}').expect("`}`").to_string();
        steps.push(Step { name, input: texts(&lex(&input, 90)), output: texts(&lex(&to, 91)) });
    }
    steps
}

/// Every expansion made, recursively: (macro, input, output); the macros
/// no definition was found for; the invocations that failed, (macro,
/// message).
pub fn made(
    nested: &[Nested],
    out: &mut Vec<(String, Vec<String>, Vec<String>)>,
    open: &mut Vec<String>,
    failed: &mut Vec<(String, String)>,
) {
    for n in nested {
        let name = n.path.last().map(|t| t.text.to_string()).unwrap_or_default();
        match &n.outcome {
            Outcome::Expanded(e) => {
                out.push((name, texts(&n.input), texts(&e.tokens)));
                made(&e.nested, out, open, failed);
            }
            Outcome::Unresolved => open.push(name),
            Outcome::Failed(err) => failed.push((name, err.message.clone())),
        }
    }
}

/// Where two token sequences first differ, with some context.
pub fn first_difference(ours: &[String], theirs: &[String]) -> String {
    let k = ours.iter().zip(theirs).take_while(|(a, b)| a == b).count();
    let ctx = |v: &[String]| v[k.saturating_sub(8)..(k + 8).min(v.len())].join(" ");
    format!(
        "differ at token {k} of {} / {}:\n  ours:   {}\n  rustc's: {}",
        ours.len(),
        theirs.len(),
        ctx(ours),
        ctx(theirs)
    )
}

/// Expand `program`, resolving macros by their last name in `defs`, and
/// check each expansion against `steps`. `inside_builtins` names the
/// macros rustc expanded inside a built-in's expansion, which are not
/// made here; `unresolved` the macros left unexpanded (built-ins, and
/// macros the fixtures do not define).
pub fn check(
    program: &str,
    defs: &HashMap<&str, Arc<MacroRules>>,
    steps: Vec<Step>,
    inside_builtins: &[&str],
    unresolved: &[&str],
) -> usize {
    check_in(program, Edition::E2021, defs, steps, inside_builtins, unresolved, &[]).0
}

/// [`check`] for a program of `edition`, in which the invocations of
/// `failures` (macro, rustc's message) fail; also the expansions made, as
/// [`expand_all`] returns them.
pub fn check_in(
    program: &str,
    edition: Edition,
    defs: &HashMap<&str, Arc<MacroRules>>,
    steps: Vec<Step>,
    inside_builtins: &[&str],
    unresolved: &[&str],
    failures: &[(&str, &str)],
) -> (usize, Vec<Nested>) {
    let mut resolve = |path: &[Token]| path.last().and_then(|t| defs.get(&*t.text).cloned());
    let (_, nested) = expand_all(lang(), &lex_in(program, 1, edition), "file", &mut resolve)
        .unwrap_or_else(|e| panic!("expanding the program: {e}"));
    let mut ours = Vec::new();
    let mut open = Vec::new();
    let mut failed = Vec::new();
    made(&nested, &mut ours, &mut open, &mut failed);
    let failed: Vec<(&str, &str)> = failed.iter().map(|(n, m)| (n.as_str(), m.as_str())).collect();
    assert_eq!(failed, failures, "invocations that failed");
    let mut steps: Vec<Option<Step>> = steps.into_iter().map(Some).collect();
    for (name, input, output) in &ours {
        let found = steps
            .iter_mut()
            .find(|s| s.as_ref().is_some_and(|s| &s.name == name && &s.input == input));
        let Some(slot) = found else {
            panic!("rustc made no expansion of `{name}!` with input {}", input.join(" "))
        };
        let step = slot.take().expect("a step");
        assert!(
            output == &step.output,
            "`{name}!` {}: {}",
            input.join(" "),
            first_difference(output, &step.output)
        );
    }
    for step in steps.into_iter().flatten() {
        assert!(
            inside_builtins.contains(&step.name.as_str()),
            "rustc expanded `{}!` {}, which was not",
            step.name,
            step.input.join(" ")
        );
    }
    open.sort();
    open.dedup();
    assert_eq!(open, unresolved, "macros left unexpanded");
    (ours.len(), nested)
}
