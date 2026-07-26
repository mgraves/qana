//! Terminal rendering: source-anchored diagnostics and green-tree dumps.
//!
//! The toolchain's refusals all carry byte spans into the `.rg` text (or
//! into a target document). This module is the only thing that turns a
//! span into something a person reads: `file:line:col`, the source line,
//! and a caret under the offending run.

use qana_grammar::green::{ERROR_NT, ERROR_PROD, LIST_PROD, NEWLINE, RUN_PROD};
use qana_grammar::{GreenChild, GreenNode, SynGrammar};
use std::io::IsTerminal;

// ---------------------------------------------------------------------------
// Color
// ---------------------------------------------------------------------------

pub fn color_on() -> bool {
    std::env::var_os("NO_COLOR").is_none() && std::io::stdout().is_terminal()
}

/// Wrap `s` in an SGR code when color is enabled.
pub fn paint(code: &str, s: &str) -> String {
    if color_on() {
        format!("\x1b[{code}m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

pub fn red(s: &str) -> String {
    paint("31;1", s)
}
pub fn yellow(s: &str) -> String {
    paint("33;1", s)
}
pub fn green(s: &str) -> String {
    paint("32;1", s)
}
pub fn cyan(s: &str) -> String {
    paint("36", s)
}
pub fn dim(s: &str) -> String {
    paint("2", s)
}
pub fn bold(s: &str) -> String {
    paint("1", s)
}

// ---------------------------------------------------------------------------
// Source positions
// ---------------------------------------------------------------------------

/// A source file with a line index, for turning byte spans into
/// human coordinates.
pub struct Src<'a> {
    pub path: &'a str,
    pub text: &'a str,
    starts: Vec<u32>,
}

impl<'a> Src<'a> {
    pub fn new(path: &'a str, text: &'a str) -> Src<'a> {
        let mut starts = vec![0u32];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                starts.push(i as u32 + 1);
            }
        }
        Src { path, text, starts }
    }

    /// 0-based line index containing `off`.
    pub fn line_of(&self, off: u32) -> usize {
        match self.starts.binary_search(&off) {
            Ok(i) => i,
            Err(i) => i - 1,
        }
    }

    pub fn line_text(&self, line: usize) -> &'a str {
        let start = self.starts[line] as usize;
        let end = self
            .starts
            .get(line + 1)
            .map(|&e| e as usize)
            .unwrap_or(self.text.len());
        self.text[start..end].trim_end_matches(['\n', '\r'])
    }

    /// 1-based (line, column) in CHARACTERS, which is what an editor
    /// shows and what a caret has to line up with.
    pub fn line_col(&self, off: u32) -> (usize, usize) {
        let line = self.line_of(off);
        let start = self.starts[line] as usize;
        let col = self.text[start..(off as usize).min(self.text.len())].chars().count();
        (line + 1, col + 1)
    }
}

/// One diagnostic, rustc-style:
///
/// ```text
/// error: no rule named `stmts`
///   --> mylang.rg:14:12
///    |
/// 14 | rule file = File: stmts
///    |                   ^^^^^
/// ```
pub fn diagnostic(src: &Src, severity: u8, span: (u32, u32), msg: &str) {
    let (label, tint): (&str, fn(&str) -> String) = match severity {
        2 => ("warning", yellow as fn(&str) -> String),
        _ => ("error", red as fn(&str) -> String),
    };
    let (line, col) = src.line_col(span.0);
    let text = src.line_text(line - 1);
    let gutter = line.to_string();
    let pad = " ".repeat(gutter.len());

    // Caret width in characters, clamped to the line (spans may cover
    // a whole multi-line construct; underline just the first line).
    let line_start = src.starts[line - 1] as usize;
    let line_end = line_start + text.len();
    let stop = (span.1 as usize).clamp(span.0 as usize, line_end);
    let width = src.text[span.0 as usize..stop].chars().count().max(1);

    eprintln!("{}: {}", tint(label), bold(msg));
    eprintln!("{pad}{} {}:{line}:{col}", cyan("-->"), src.path);
    eprintln!("{pad} {}", cyan("|"));
    eprintln!("{} {} {text}", cyan(&gutter), cyan("|"));
    eprintln!(
        "{pad} {} {}{}",
        cyan("|"),
        " ".repeat(col - 1),
        tint(&"^".repeat(width))
    );
}

// ---------------------------------------------------------------------------
// Green-tree dump
// ---------------------------------------------------------------------------

pub struct TreeOpts {
    /// Show trivia tokens (whitespace/comments) as well as symbols.
    pub trivia: bool,
    /// Stop descending past this depth (0 = unlimited).
    pub max_depth: usize,
}

/// Print the lossless parse tree. Node lines carry the production name
/// (which is exactly the typed-AST variant name) and the byte span;
/// token lines carry the terminal name and its text.
pub fn tree(node: &GreenNode, sg: &SynGrammar, opts: &TreeOpts) {
    println!("{}", node_label(node, sg, 0));
    walk(node, sg, 0, &mut String::new(), opts, 1);
}

/// Nonterminal name, tolerant of the synthetic ids the engine uses for
/// balanced-list and error nodes.
fn nt_name(sg: &SynGrammar, nt: u16) -> &str {
    sg.nt_names.get(nt as usize).map(|s| s.as_str()).unwrap_or("?")
}

/// Node kinds a reader meets: ordinary productions (named exactly as the
/// typed-AST variant), the LIST/RUN nodes that give L4 sequences their
/// balanced shape, and error nodes from recovery.
fn node_label(n: &GreenNode, sg: &SynGrammar, base: u32) -> String {
    let span = dim(&format!("@{}..{}", base, base + n.width));
    if n.nt == ERROR_NT || n.prod == ERROR_PROD {
        return format!("{} {span}", red("ERROR"));
    }
    let name = match n.prod {
        LIST_PROD => format!("{} {}", bold(nt_name(sg, n.nt)), dim("(balanced list)")),
        RUN_PROD => format!("{} {}", bold(nt_name(sg, n.nt)), dim("(run)")),
        p if (p as usize) < sg.prods.len() => bold(&sg.prod_name(p as usize)),
        _ => bold(nt_name(sg, n.nt)),
    };
    if n.has_err {
        format!("{name} {span} {}", red("[error]"))
    } else {
        format!("{name} {span}")
    }
}

fn walk(
    n: &GreenNode,
    sg: &SynGrammar,
    base: u32,
    prefix: &mut String,
    opts: &TreeOpts,
    depth: usize,
) {
    if opts.max_depth != 0 && depth > opts.max_depth {
        return;
    }
    let shown: Vec<(usize, u32)> = {
        let mut off = base;
        let mut v = Vec::new();
        for (i, c) in n.children.iter().enumerate() {
            let keep = match c {
                GreenChild::Token(t) => opts.trivia || !t.trivia,
                GreenChild::Node(_) => true,
            };
            if keep {
                v.push((i, off));
            }
            off += c.width();
        }
        v
    };
    for (k, &(i, off)) in shown.iter().enumerate() {
        let last = k + 1 == shown.len();
        let (branch, cont) = if last { ("└─ ", "   ") } else { ("├─ ", "│  ") };
        match &n.children[i] {
            GreenChild::Node(child) => {
                println!("{prefix}{branch}{}", node_label(child, sg, off));
                let saved = prefix.len();
                prefix.push_str(cont);
                walk(child, sg, off, prefix, opts, depth + 1);
                prefix.truncate(saved);
            }
            GreenChild::Token(t) => {
                // NEWLINE is synthesized by the engine as line-terminator
                // trivia; it has no entry in the grammar's vocabulary.
                let name = if t.id == NEWLINE { "NEWLINE" } else { sg.term_name(t.id) };
                let body = if t.is_missing() {
                    red("(missing — inserted by recovery)")
                } else {
                    let s = escape(&t.text);
                    if t.trivia {
                        dim(&format!("{s} (trivia)"))
                    } else {
                        green(&s)
                    }
                };
                println!("{prefix}{branch}{} {body} {}", cyan(name), dim(&format!("@{off}")));
            }
        }
    }
}

/// Token text as a quoted, escaped, length-capped literal.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars().take(40) {
        match c {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    if s.chars().count() > 40 {
        out.push('…');
    }
    out.push('"');
    out
}
