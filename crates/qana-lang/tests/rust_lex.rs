//! Rust's lexical grammar, written as a `.qana` file, reads Rust the way
//! rustc's own lexer does.
//!
//! `examples/rust-lex/rust_lex.qana` is the grammar. The reference is
//! rustc_lexer, which this workspace does not depend on (its toolchain
//! crates have no external dependency). So the comparison is in two
//! halves:
//!
//! * `tools/rust-fit`, a crate outside the workspace, links rustc_lexer.
//!   It compares the two lexers over any source tree, and it writes
//!   `examples/rust-lex/sample.classes`: rustc_lexer's reading of
//!   `sample.rs`, byte by byte.
//! * These tests hold qana to that record, with nothing but the workspace.
//!
//! What is compared is the class of every byte (see `classes.rs`), which
//! is what highlighting shows and what a parser's tokens are cut from.

#[path = "../../../tools/rust-fit/src/classes.rs"]
mod classes;

use classes::{encode, line_classes, token_classes, Class};
use qana_engine::LexedBuffer;
use qana_grammar::CompiledLexer;
use qana_lang::EmbeddedLang;
use std::path::{Path, PathBuf};

const GRAMMAR: &str = include_str!("../../../examples/rust-lex/rust_lex.qana");
const SAMPLE: &str = include_str!("../../../examples/rust-lex/sample.rs");
const RECORD: &str = include_str!("../../../examples/rust-lex/sample.classes");

/// The one place the grammar asks what follows a token: `1.` is a float
/// unless a `.`, a `_` or a letter comes next.
const TRAILING_CONTEXT: &str = r"\d[\d_]*\.(?![._\a])";

fn lexer_of(grammar: &str) -> &'static CompiledLexer {
    match EmbeddedLang::from_qana_source(grammar) {
        Ok(lang) => lang.lexer,
        Err(diags) => {
            let messages: Vec<String> =
                diags.iter().map(|d| format!("at {:?}: {}", d.span, d.msg)).collect();
            panic!("the grammar certifies, but: {messages:#?}");
        }
    }
}

/// qana's reading of `src`: the classes of each line's bytes.
fn read(lexer: &CompiledLexer, src: &str) -> Vec<Vec<Class>> {
    let classes = token_classes(lexer);
    let buffer = LexedBuffer::new(lexer, src);
    assert!(buffer.verify_coverage(), "every byte of every line is in a token");
    buffer.lexed.iter().map(|line| line_classes(&line.tokens, &classes)).collect()
}

/// The record's lines, without its header.
fn recorded() -> Vec<&'static str> {
    RECORD.lines().skip_while(|line| line.starts_with('#')).collect()
}

#[test]
fn the_grammar_is_in_the_envelope() {
    lexer_of(GRAMMAR);
    assert_eq!(
        GRAMMAR.matches(TRAILING_CONTEXT).count(),
        1,
        "the grammar asks what follows a token in one place, the float's bare dot"
    );
}

/// Every byte of the sample has the class rustc_lexer gives it.
#[test]
fn qana_reads_the_sample_as_rustc_lexer_does() {
    let ours = read(lexer_of(GRAMMAR), SAMPLE);
    let theirs = recorded();
    let source: Vec<&str> = SAMPLE.split('\n').collect();
    assert_eq!(
        ours.len(),
        theirs.len(),
        "the record has a line for each line of the sample; write it again with \
         `cargo run --manifest-path tools/rust-fit/Cargo.toml -- record`"
    );
    assert!(ours.len() > 150, "the sample is the whole file, got {} lines", ours.len());

    let mut wrong = Vec::new();
    for (n, (ours, theirs)) in ours.iter().zip(&theirs).enumerate() {
        let ours = encode(ours);
        if ours != *theirs {
            wrong.push(format!(
                "line {}: {}\n    rustc_lexer: {theirs}\n    qana:        {ours}",
                n + 1,
                source[n]
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} line(s) of the sample are not read as rustc_lexer reads them:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

/// The lines of `src` where a number is followed by a `.` that is not
/// part of it, and then by a `.`, a `_` or a letter: `0..10`, `1.max(2)`.
fn lines_where_a_dot_is_left_to_what_follows(src: &str, reading: &[Vec<Class>]) -> Vec<usize> {
    src.split('\n')
        .zip(reading)
        .enumerate()
        .filter(|(_, (text, classes))| {
            let bytes = text.as_bytes();
            (1..bytes.len().saturating_sub(1)).any(|i| {
                bytes[i] == b'.'
                    && classes[i] == Class::Punct
                    && classes[i - 1] == Class::Number
                    && (matches!(bytes[i + 1], b'.' | b'_') || !bytes[i + 1].is_ascii()
                        || bytes[i + 1].is_ascii_alphabetic())
            })
        })
        .map(|(n, _)| n)
        .collect()
}

/// The record can tell a wrong reading from a right one, and trailing
/// context is what the right one needs: take it out of the grammar, and
/// the lines that go wrong are exactly the ones where it decided.
#[test]
fn without_trailing_context_a_number_swallows_the_dot_after_it() {
    let right = read(lexer_of(GRAMMAR), SAMPLE);

    let without = GRAMMAR.replace(TRAILING_CONTEXT, r"\d[\d_]*\.");
    assert_ne!(without, GRAMMAR, "the trailing context was there to take out");
    let wrong = read(lexer_of(&without), SAMPLE);

    let differing: Vec<usize> =
        (0..right.len()).filter(|&n| right[n] != wrong[n]).collect();
    let decided = lines_where_a_dot_is_left_to_what_follows(SAMPLE, &right);
    assert!(
        decided.len() >= 8,
        "the sample has ranges and method calls on numbers, found {}",
        decided.len()
    );
    assert_eq!(
        differing, decided,
        "the lines a grammar without trailing context reads wrong are the lines where \
         a number is followed by a dot it must not take"
    );

    let range = SAMPLE.split('\n').position(|l| l.contains("let range = 0..10;")).unwrap();
    assert_eq!(encode(&right[range]), "_4 w3 _1 w5 _1 p1 _1 n1 p2 n2 p1", "0..10 is 0, .., 10");
    assert_eq!(encode(&wrong[range]), "_4 w3 _1 w5 _1 p1 _1 n2 p1 n2 p1", "0. then . then 10");
}

/// A raw string is a mode, and the mode is what one line hands the next:
/// every line inside `r#"…"#` is string text, however many there are, and
/// the line that closes it leaves the lexer where it started.
#[test]
fn a_raw_string_holds_its_mode_from_line_to_line() {
    let lexer = lexer_of(GRAMMAR);
    let classes = token_classes(lexer);
    let buffer = LexedBuffer::new(lexer, SAMPLE);
    let source: Vec<&str> = SAMPLE.split('\n').collect();

    let open = source.iter().position(|l| l.contains("let raw_across = r#\"")).unwrap();
    let close = source.iter().position(|l| l.contains("A quote and a hash close it: \"#;")).unwrap();
    assert!(close >= open + 2, "the literal spans lines");

    assert_eq!(buffer.entry_state(open).depth(), 0, "code before the literal");
    for line in open..close {
        assert_eq!(
            buffer.lexed[line].exit.depth(),
            1,
            "line {} ends inside the raw string",
            line + 1
        );
    }
    for line in open + 1..close {
        let read = line_classes(&buffer.lexed[line].tokens, &classes);
        assert!(
            read.iter().all(|&c| c == Class::Text),
            "line {} is string text throughout: {}",
            line + 1,
            encode(&read)
        );
    }
    assert_eq!(buffer.lexed[close].exit.depth(), 0, "the closing line is back in code");
    // The line after it is code again: `let after = 1..2;`.
    let after = line_classes(&buffer.lexed[close + 1].tokens, &classes);
    assert_eq!(encode(&after), "_4 w3 _1 w5 _1 p1 _1 n1 p2 n1 p1");
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
    for entry in entries {
        let path = entry.expect("a directory entry").path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// Real code, without the reference: this workspace's own Rust. The
/// grammar gives every byte to a token it knows, and every string and
/// comment that opens is closed by the end of its file. (`rust-fit
/// compare crates` checks the same files against rustc_lexer.)
#[test]
fn the_workspaces_own_rust_lexes_clean() {
    let lexer = lexer_of(GRAMMAR);
    let classes = token_classes(lexer);
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    rust_files(&crates, &mut files);
    files.sort();
    assert!(files.len() >= 50, "the workspace's sources, found {} files", files.len());

    let mut lines = 0usize;
    for path in &files {
        let src = std::fs::read_to_string(path).expect("Rust source is UTF-8");
        let buffer = LexedBuffer::new(lexer, &src);
        assert!(buffer.verify_coverage(), "{}: every byte is in a token", path.display());
        for (n, line) in buffer.lexed.iter().enumerate() {
            let read = line_classes(&line.tokens, &classes);
            assert!(
                !read.contains(&Class::Unknown),
                "{}:{}: a byte no token takes: {}",
                path.display(),
                n + 1,
                encode(&read)
            );
        }
        let end = buffer.lexed.last().map(|line| line.exit.depth()).unwrap_or(0);
        assert_eq!(end, 0, "{}: a string or comment is still open at the end", path.display());
        lines += buffer.lexed.len();
    }
    assert!(lines > 20_000, "the workspace's sources, found {lines} lines");
}
