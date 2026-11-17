//! The folds channel of a live document (`Limner::folds_shared`): what
//! it finds, that it nests, and that the folds kept across edits are
//! the folds of the document opened afresh.

use linework::{Limner, LineEdit, Mark};
use qana_lang::live::LiveDoc;
use qana_lang::EmbeddedLang;
use std::sync::Arc;

const RUST: &str = include_str!("../../../examples/rust/rust.qana");
const C: &str = include_str!("../../../examples/c/c.qana");
const C_DEMO: &str = include_str!("../../../examples/c/demo.c");
const RUST_SAMPLE: &str = include_str!("../../../examples/rust-lex/sample.rs");

fn lang(grammar: &str) -> Arc<EmbeddedLang> {
    Arc::new(EmbeddedLang::from_qana_source(grammar).unwrap_or_else(|d| panic!("{d:?}")))
}

/// A fold as `(category name, the text it spans)`.
fn spelled(doc: &mut LiveDoc, text: &str) -> Vec<(String, String)> {
    let legend = doc.fold_legend();
    doc.folds_shared()
        .iter()
        .map(|f| {
            (
                legend[f.category as usize].clone(),
                text[f.start as usize..f.end as usize].to_string(),
            )
        })
        .collect()
}

/// Document order, and containment: a fold that starts inside another
/// ends inside it.
fn assert_nested(folds: &[Mark], what: &str) {
    let mut open_ends: Vec<u32> = Vec::new();
    for (i, f) in folds.iter().enumerate() {
        assert!(f.start < f.end, "{what}: fold {i} is not empty");
        assert!(f.name_start == f.start && f.name_start < f.name_end && f.name_end <= f.end);
        if i > 0 {
            assert!(folds[i - 1].start <= f.start, "{what}: fold {i} is in document order");
        }
        while open_ends.last().is_some_and(|&e| f.start >= e) {
            open_ends.pop();
        }
        if let Some(&outer) = open_ends.last() {
            assert!(f.end <= outer, "{what}: fold {i} ends inside the fold it starts in");
        }
        open_ends.push(f.end);
    }
}

#[test]
fn rust_folds_blocks_comments_strings_and_comment_lines() {
    let src = "\
//! Two lines
//! of module docs.

/// One line of docs.
fn main() {
    let v = [
        1, 2,
    ];
    let s = \"over
lines\";
    /* a block
       /* nested */
       comment */
    call(a,
         b);
    let one = { 1 };
}
";
    let mut doc = LiveDoc::open(lang(RUST), "t.rs", src);
    let folds = spelled(&mut doc, src);
    let want: Vec<(&str, &str)> = vec![
        ("comment lines", "//! Two lines\n//! of module docs."),
        ("brace", &src[src.find("{\n    let v").unwrap()..src.rfind('}').unwrap() + 1]),
        ("bracket", "[\n        1, 2,\n    ]"),
        ("string", "\"over\nlines\""),
        ("comment", "/* a block\n       /* nested */\n       comment */"),
        ("paren", "(a,\n         b)"),
    ];
    let want: Vec<(String, String)> =
        want.into_iter().map(|(c, t)| (c.to_string(), t.to_string())).collect();
    assert_eq!(folds, want, "a single doc line, `{{ 1 }}` and `[` … `]` on a line fold nothing");
    assert_nested(&doc.folds_shared(), "the example");
}

#[test]
fn a_close_takes_the_nearest_open_of_its_kind() {
    // The `(` is never closed: the `}` passes over it to its `{`.
    let src = "fn f() {\n    g(1,\n}\nfn h() {\n    x\n}\n";
    let mut doc = LiveDoc::open(lang(RUST), "t.rs", src);
    let folds = spelled(&mut doc, src);
    assert_eq!(
        folds,
        vec![
            ("brace".to_string(), "{\n    g(1,\n}".to_string()),
            ("brace".to_string(), "{\n    x\n}".to_string()),
        ]
    );

    // A stray close is ignored, and the blocks around it still pair.
    let src = "fn f() {\n    ]\n}\n";
    let mut doc = LiveDoc::open(lang(RUST), "t.rs", src);
    assert_eq!(spelled(&mut doc, src), vec![("brace".to_string(), "{\n    ]\n}".to_string())]);

    // An open never closed folds nothing.
    let src = "fn f() {\n    x\n";
    let mut doc = LiveDoc::open(lang(RUST), "t.rs", src);
    assert!(doc.folds_shared().is_empty());
}

#[test]
fn the_sample_and_the_c_demo_nest() {
    let mut rust = LiveDoc::open(lang(RUST), "sample.rs", RUST_SAMPLE);
    let folds = rust.folds_shared();
    assert!(folds.len() >= 10, "the sample has functions, strings and comments: {}", folds.len());
    assert_nested(&folds, "sample.rs");

    let mut c = LiveDoc::open(lang(C), "demo.c", C_DEMO);
    let folds = c.folds_shared();
    assert!(folds.len() >= 5, "the demo has functions and block comments: {}", folds.len());
    assert_nested(&folds, "demo.c");
}

#[test]
fn the_same_revision_hands_out_the_same_folds() {
    let mut doc = LiveDoc::open(lang(RUST), "sample.rs", RUST_SAMPLE);
    let (a, b) = (doc.folds_shared(), doc.folds_shared());
    assert!(Arc::ptr_eq(&a, &b), "asked twice in one revision, the same Arc");
    let rev = doc.folds_rev();
    doc.edit(&LineEdit { start: 0, end: 1, lines: vec!["// x".to_string()] });
    assert_ne!(doc.folds_rev(), rev, "an edit moves the revision");
}

/// Little pieces of Rust that change folds when typed anywhere: brackets,
/// quotes, comment openers and closers, line breaks.
const PIECES: &[&str] = &[
    "{",
    "}",
    "(",
    ")",
    "[",
    "]",
    "\"",
    "r#\"",
    "\"#",
    "/*",
    "*/",
    "//",
    "///",
    "\n",
    "\n\n",
    "x",
    "fn f() {\n",
    "\n}",
    "// c\n",
    "'a'",
    "\\",
    " ",
];

fn rng(mut seed: u64) -> impl FnMut(usize) -> usize {
    move |n: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % n.max(1) as u64) as usize
    }
}

/// The lines `start..end` of `text`, replaced by `lines`, as the editor
/// sends them.
fn apply(text: &str, start: usize, end: usize, lines: &[String]) -> String {
    let old: Vec<&str> = text.split('\n').collect();
    let mut new: Vec<String> = old[..start].iter().map(|s| s.to_string()).collect();
    new.extend(lines.iter().cloned());
    new.extend(old[end..].iter().map(|s| s.to_string()));
    new.join("\n")
}

/// After every edit of a random script, the folds kept across edits are
/// the folds of the same text opened afresh, and they nest.
fn kept_equals_fresh(grammar: &str, start_text: &str, seed: u64, steps: usize) {
    let lang = lang(grammar);
    let mut text = start_text.to_string();
    let mut doc = LiveDoc::open(lang.clone(), "t", &text);
    let mut pick = rng(seed);
    for step in 0..steps {
        let lines: Vec<&str> = text.split('\n').collect();
        let line = pick(lines.len());
        let old = lines[line];
        // A char boundary in the line, and maybe a span after it to cut.
        let bounds: Vec<usize> =
            old.char_indices().map(|(i, _)| i).chain(std::iter::once(old.len())).collect();
        let at = bounds[pick(bounds.len())];
        let cut_to = bounds.iter().copied().filter(|&b| b >= at).nth(pick(3)).unwrap_or(at);
        let edited = if pick(4) == 0 {
            format!("{}{}", &old[..at], &old[cut_to..])
        } else {
            format!("{}{}{}", &old[..at], PIECES[pick(PIECES.len())], &old[at..])
        };
        // Sometimes the edit also takes the next line with it: a join.
        let end = if pick(6) == 0 && line + 1 < lines.len() { line + 2 } else { line + 1 };
        let joined = if end == line + 2 { format!("{edited}{}", lines[line + 1]) } else { edited };
        let new_lines: Vec<String> = joined.split('\n').map(str::to_string).collect();

        text = apply(&text, line, end, &new_lines);
        doc.edit(&LineEdit { start: line as u32, end: end as u32, lines: new_lines });
        assert_eq!(doc.text(), text, "step {step}: the document follows the script");

        let kept = doc.folds_shared();
        let fresh = LiveDoc::open(lang.clone(), "t", &text).folds_shared();
        assert_eq!(*kept, *fresh, "step {step} (seed {seed}): kept folds are fresh folds");
        assert_nested(&kept, &format!("step {step}"));
    }
}

#[test]
fn kept_folds_are_fresh_folds_rust() {
    for seed in [1, 2, 3, 0x5eed] {
        kept_equals_fresh(RUST, RUST_SAMPLE, seed, 300);
    }
}

/// C's directives are a line-bounded mode: one ends with its line and
/// folds nothing, unless a backslash carries it on, and then the whole
/// directive is one fold, named for the style of the `#` that opens it.
#[test]
fn a_c_directive_folds_when_a_backslash_carries_it_over_lines() {
    let src = "\
#define ONE 1
#define TWO(a, b) \\
    ((a) + \\
     (b))
/* a block
   comment */
int f(void) {
    return TWO(ONE, 2);
}
";
    let mut doc = LiveDoc::open(lang(C), "t.c", src);
    let folds = spelled(&mut doc, src);
    let directive = &src[src.find("#define TWO").unwrap()..src.find("(b))").unwrap() + 4];
    let want: Vec<(String, String)> = vec![
        ("regexp".to_string(), directive.to_string()),
        ("comment".to_string(), "/* a block\n   comment */".to_string()),
        ("brace".to_string(), "{\n    return TWO(ONE, 2);\n}".to_string()),
    ];
    assert_eq!(folds, want);
    assert_nested(&doc.folds_shared(), "the C example");
}
