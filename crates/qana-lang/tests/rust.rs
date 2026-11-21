//! `examples/rust/rust.qana` reads Rust's syntax without lexing it any
//! differently from `examples/rust-lex/rust_lex.qana`, the grammar that is
//! measured against rustc's own lexer (see `rust_lex.rs` and
//! `tools/rust-fit`).
//!
//! The syntax grammar cuts every line into the same tokens and leaves
//! every line in the same mode; what it changes is only which token a
//! piece of text is (a keyword rather than a name, one kind of
//! punctuation rather than another). So what was measured holds for it,
//! and these tests check exactly that, then what it adds. Its syntax is
//! tested in `rust_syntax.rs`.

#[path = "../../../tools/rust-fit/src/classes.rs"]
mod classes;

use classes::{encode, line_classes, token_classes, Class};
use qana_engine::{IncSession, LexedBuffer};
use qana_grammar::CompiledLexer;
use qana_lang::compile::certify;
use qana_lang::{compile_source, EmbeddedLang, QanaToolchain};
use qana_services::completion_at;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const GRAMMAR: &str = include_str!("../../../examples/rust/rust.qana");
const MEASURED: &str = include_str!("../../../examples/rust-lex/rust_lex.qana");
const SAMPLE: &str = include_str!("../../../examples/rust-lex/sample.rs");
const RECORD: &str = include_str!("../../../examples/rust-lex/sample.classes");

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

/// The class of each of the colouring grammar's tokens, read through the
/// measured grammar's: a token both grammars have keeps its class, and
/// what the colouring grammar adds is a word (a keyword) or punctuation
/// (an operator or a bracket), as rustc_lexer has them.
fn colour_classes(colour: &CompiledLexer, measured: &CompiledLexer) -> Vec<Class> {
    let by_name: HashMap<&str, Class> =
        measured.vocab.names.iter().map(String::as_str).zip(token_classes(measured)).collect();
    colour
        .vocab
        .names
        .iter()
        .enumerate()
        .map(|(id, name)| {
            if colour.keyword_text(id as qana_grammar::TokenId).is_some() {
                return Class::Word;
            }
            match name.as_str() {
                // Brackets, and each punctuation character lexed as what
                // it begins: one character each, as rustc_lexer has them.
                "LPAREN" | "RPAREN" | "LBRACK" | "RBRACK" | "LBRACE" | "RBRACE" | "EQ_EQ"
                | "EQ_ARROW" | "EQ" | "BANG_EQ" | "BANG" | "LT_SHLEQ" | "LT_SHL" | "LT_LE"
                | "LT" | "GT_SHREQ" | "GT_SHR" | "GT_GE" | "GT" | "AMP_AMP" | "AMP_EQ" | "AMP"
                | "PIPE_PIPE" | "PIPE_EQ" | "PIPE" | "PLUS_EQ" | "PLUS" | "MINUS_EQ"
                | "MINUS_ARROW" | "MINUS" | "STAR_EQ" | "STAR" | "SLASH_EQ" | "SLASH"
                | "PERCENT_EQ" | "PERCENT" | "CARET_EQ" | "CARET" | "DOT_DOT" | "DOT_EQ"
                | "DOT" | "COLON_COLON" | "COLON" | "SEMI" | "COMMA" | "AT" | "HASH_BANG"
                | "HASH" | "TILDE" | "QMARK" | "DOLLAR" => Class::Punct,
                // The weak keywords, where they act as keywords, and
                // `cfg_attr` and `derive` before `(`: words.
                "KW_UNION" | "KW_AUTO" | "KW_DEFAULT" | "KW_RAW" | "KW_SAFE" | "KW_CFG_ATTR"
                | "KW_DERIVE" => Class::Word,
                // `1.` alone, a float the parser may split, as rustc_lexer
                // cuts it.
                "FLOAT_DOT" => by_name["FLOAT"],
                // Doc comments are comments, cut where rust_lex.qana cuts
                // its comments, in a mode that reads as its BLOCK.
                "DOC_LINE" => by_name["LINE_COMMENT"],
                "BLOCK_OPEN_EOL" | "DOC_OPEN" => by_name["BLOCK_OPEN"],
                "D_OPEN" => by_name["B_OPEN"],
                "D_CLOSE" => by_name["B_CLOSE"],
                "D_CONTENT" => by_name["B_CONTENT"],
                "D_MISC" => by_name["B_MISC"],
                // A byte string's or a C string's opening, a token of its
                // own here: a string's, as rustc_lexer has it.
                "PSTR_OPEN" => by_name["STR_OPEN"],
                "PRAW0_OPEN" => by_name["RAW0_OPEN"],
                "PRAW1_OPEN" => by_name["RAW1_OPEN"],
                "PRAW2_OPEN" => by_name["RAW2_OPEN"],
                "PRAW3_OPEN" => by_name["RAW3_OPEN"],
                other => *by_name.get(other).unwrap_or_else(|| {
                    panic!("rust.qana has a token rust_lex.qana does not, with no class: {other}")
                }),
            }
        })
        .collect()
}

#[test]
fn the_grammar_is_in_the_envelope() {
    let tc = QanaToolchain::new();
    let out = compile_source(&tc, GRAMMAR);
    assert!(out.diags.is_empty(), "rust.qana compiles: {:?}", out.diags);
    certify(&out.def).expect("rust.qana is in the envelope");
}

/// The mode a token named `name` is read in.
fn mode_of(lexer: &CompiledLexer, name: &str) -> u16 {
    let id = lexer.vocab.names.iter().position(|n| n == name).unwrap_or_else(|| panic!("{name}"));
    lexer.vocab.modes[id]
}

/// Every line is cut where the measured grammar cuts it, and hands the
/// next line the mode the measured grammar hands it: the same depth of
/// modes, the innermost the same. A block doc comment's mode is a block
/// comment's (the measured grammar does not tell doc comments apart), and
/// one holds only itself, as a block comment's does.
fn assert_same_cuts(colour: &CompiledLexer, measured: &CompiledLexer, src: &str, what: &str) {
    let (doc, block) = (mode_of(colour, "D_OPEN"), mode_of(colour, "B_OPEN"));
    assert_eq!(block, mode_of(measured, "B_OPEN"), "BLOCK is the same mode in both");
    let exit = |m: qana_grammar::MStack| {
        let current = m.current_mode();
        (m.depth(), if current == doc { block } else { current })
    };
    let ours = LexedBuffer::new(colour, src);
    let theirs = LexedBuffer::new(measured, src);
    assert!(ours.verify_coverage(), "{what}: every byte is in a token");
    assert_eq!(ours.lexed.len(), theirs.lexed.len(), "{what}: the same lines");
    for (n, (a, b)) in ours.lexed.iter().zip(&theirs.lexed).enumerate() {
        let cuts =
            |tokens: &[qana_grammar::Token]| tokens.iter().map(|t| t.len).collect::<Vec<_>>();
        assert_eq!(cuts(&a.tokens), cuts(&b.tokens), "{what}:{}: the same tokens", n + 1);
        assert_eq!(exit(a.exit), exit(b.exit), "{what}:{}: the same mode after the line", n + 1);
    }
}

/// On the sample, against the record of rustc_lexer's reading: every
/// byte has rustc_lexer's class.
#[test]
fn the_sample_reads_as_rustc_lexer_reads_it() {
    let colour = lexer_of(GRAMMAR);
    let measured = lexer_of(MEASURED);
    assert_same_cuts(colour, measured, SAMPLE, "sample.rs");

    let classes = colour_classes(colour, measured);
    let buffer = LexedBuffer::new(colour, SAMPLE);
    let theirs: Vec<&str> = RECORD.lines().skip_while(|line| line.starts_with('#')).collect();
    assert_eq!(buffer.lexed.len(), theirs.len(), "the record has a line for each line");
    let source: Vec<&str> = SAMPLE.split('\n').collect();
    let wrong: Vec<String> = buffer
        .lexed
        .iter()
        .zip(&theirs)
        .enumerate()
        .filter_map(|(n, (line, theirs))| {
            let ours = encode(&line_classes(&line.tokens, &classes));
            (ours != *theirs).then(|| {
                format!(
                    "line {}: {}\n    rustc_lexer: {theirs}\n    qana: {ours}",
                    n + 1,
                    source[n]
                )
            })
        })
        .collect();
    assert!(wrong.is_empty(), "{} line(s) read differently:\n{}", wrong.len(), wrong.join("\n"));
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
    for entry in entries {
        let path = entry.expect("a directory entry").path();
        // A test's fixtures are other code's Rust (std's, copied), which
        // may be what this grammar leaves out (`macro` items).
        if path.is_dir() && path.file_name().is_some_and(|n| n == "fixtures") {
            continue;
        }
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// On this workspace's own Rust: the same cuts and modes as the measured
/// grammar, and the flat rule takes every file whole, with no repair.
#[test]
fn the_workspaces_own_rust_reads_as_the_measured_grammar_reads_it() {
    let colour = lexer_of(GRAMMAR);
    let measured = lexer_of(MEASURED);
    let tc = QanaToolchain::new();
    let out = compile_source(&tc, GRAMMAR);
    let (lexer, tables) = certify(&out.def).expect("rust.qana is in the envelope");

    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    rust_files(&crates, &mut files);
    files.sort();
    assert!(files.len() >= 50, "the workspace's sources, found {} files", files.len());
    for path in &files {
        let src = std::fs::read_to_string(path).expect("Rust source is UTF-8");
        let what = path.display().to_string();
        assert_same_cuts(colour, measured, &src, &what);
        let session = IncSession::new(&lexer, &out.def.sg, &tables, &src).unwrap();
        assert!(session.last_repairs.is_empty(), "{what}: parses with no repair");
    }
}

/// The name of the token each piece of `line` is lexed as, trivia left out.
fn token_names(lexer: &CompiledLexer, line: &str) -> Vec<(String, String)> {
    let buffer = LexedBuffer::new(lexer, line);
    let mut at = 0usize;
    let mut out = Vec::new();
    for token in &buffer.lexed[0].tokens {
        let text = &line[at..at + token.len as usize];
        at += token.len as usize;
        if !lexer.is_trivia(token.id) {
            out.push((text.to_string(), lexer.vocab.names[token.id as usize].clone()));
        }
    }
    out
}

#[test]
fn keywords_are_keywords_and_names_stay_names() {
    let lexer = lexer_of(GRAMMAR);
    let read = token_names(
        lexer,
        "pub fn new(self) -> Self { let r#fn = union; gen; 'static; macro_rules! }",
    );
    let name_of = |text: &str| -> &str {
        &read.iter().find(|(t, _)| t == text).unwrap_or_else(|| panic!("`{text}` in {read:?}")).1
    };
    assert_eq!(name_of("pub"), "KW_PUB");
    assert_eq!(name_of("fn"), "KW_FN");
    assert_eq!(name_of("self"), "KW_SELF");
    assert_eq!(name_of("Self"), "KW_Self", "`Self` is its own keyword, not `self`'s");
    assert_eq!(name_of("let"), "KW_LET");
    assert_eq!(name_of("r#fn"), "RAW_IDENT", "a raw identifier is a name");
    assert_eq!(name_of("union"), "IDENT", "a weak keyword is a name");
    assert_eq!(name_of("gen"), "IDENT", "only the 2024 edition reserves `gen`");
    assert_eq!(name_of("'static"), "LIFETIME");
    assert_eq!(name_of("macro_rules"), "IDENT");
    assert!(
        read.windows(2).any(
            |w| w[0] == ("-".into(), "MINUS_ARROW".into()) && w[1] == (">".into(), "GT".into())
        ),
        "`->` is two tokens, as rustc_lexer has it, the first lexed as what it begins: {read:?}"
    );
}

#[test]
fn operators_punctuation_and_brackets_are_told_apart() {
    let lexer = lexer_of(GRAMMAR);
    let read = token_names(lexer, "a.b(c)[0] += {x}; y::z");
    let names: Vec<&str> = read.iter().map(|(_, n)| n.as_str()).collect();
    assert_eq!(
        names,
        [
            "IDENT",
            "DOT",
            "IDENT",
            "LPAREN",
            "IDENT",
            "RPAREN",
            "LBRACK",
            "INT",
            "RBRACK",
            "PLUS_EQ",
            "EQ",
            "LBRACE",
            "IDENT",
            "RBRACE",
            "SEMI",
            "IDENT",
            "COLON_COLON",
            "COLON",
            "IDENT",
        ]
    );
}

/// Completion offers a keyword by its text. `Self` and `self` are two
/// keywords, and each is offered as written, where an expression can
/// begin.
#[test]
fn completion_offers_self_and_self_type_as_written() {
    let tc = QanaToolchain::new();
    let out = compile_source(&tc, GRAMMAR);
    let (lexer, tables) = certify(&out.def).expect("rust.qana is in the envelope");
    let src = "fn f() { ";
    let buffer = LexedBuffer::new(&lexer, src);
    let items = completion_at(&lexer, &buffer, &out.def.sg, &tables, src.len() as u32);
    let keyword = |label: &str| items.iter().any(|i| i.is_keyword && i.label == label);
    assert!(keyword("self"), "`self` is offered");
    assert!(keyword("Self"), "`Self` is offered as written, not as `self`");
    assert!(keyword("fn"), "an item can begin a statement");
    assert!(!items.iter().any(|i| i.is_keyword && i.label.starts_with("KW_")));
}
