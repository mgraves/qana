//! The measure `rust-fit` compares two lexers by, and the form the record
//! of it is written in.
//!
//! This file is compiled twice: here, into the tool, and into
//! `crates/qana-lang/tests/rust_lex.rs`, which names it by path. The test
//! holds qana to a committed record of rustc_lexer's reading; the tool
//! writes that record. Both must read qana's tokens the same way, so both
//! read them with this code.

use qana_grammar::{CompiledLexer, Token};

/// What a byte of source is, coarsely. Both lexers are reduced to this.
///
/// Classes, not token boundaries, because qana splits a string or a
/// comment that spans lines into tokens line by line, by design. A word,
/// a number or a lifetime is one token in both lexers, so a boundary in
/// the wrong place there shows as a byte in the wrong class.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Class {
    Whitespace,
    Comment,
    Word,
    Lifetime,
    Number,
    /// Strings of every kind, char literals and byte literals.
    Text,
    Punct,
    Unknown,
}

impl Class {
    /// The class's letter in a record.
    pub fn letter(self) -> char {
        match self {
            Class::Whitespace => '_',
            Class::Comment => 'c',
            Class::Word => 'w',
            Class::Lifetime => 'l',
            Class::Number => 'n',
            Class::Text => 's',
            Class::Punct => 'p',
            Class::Unknown => '?',
        }
    }
}

/// The class of each token of `rust_lex.qana`, indexed by token id.
///
/// By name: the grammar names its tokens for what they are, and a token
/// this does not know is a change to the grammar that has to be given a
/// class here before anything can be measured.
pub fn token_classes(lexer: &CompiledLexer) -> Vec<Class> {
    lexer
        .vocab
        .names
        .iter()
        .map(|name| match name.as_str() {
            "WS" => Class::Whitespace,
            "LINE_COMMENT" | "BLOCK_OPEN" | "B_OPEN" | "B_CLOSE" | "B_CONTENT" | "B_MISC" => {
                Class::Comment
            }
            "IDENT" | "RAW_IDENT" => Class::Word,
            "LIFETIME" | "RAW_LIFETIME" => Class::Lifetime,
            "INT" | "FLOAT" => Class::Number,
            "PUNCT" => Class::Punct,
            "CHAR" | "BYTE" | "STR_OPEN" | "S_CLOSE" | "S_ESC" | "S_BACKSLASH" | "S_TEXT" => {
                Class::Text
            }
            "RAW0_OPEN" | "R0_CLOSE" | "R0_TEXT" => Class::Text,
            "RAW1_OPEN" | "R1_CLOSE" | "R1_QUOTE" | "R1_TEXT" => Class::Text,
            "RAW2_OPEN" | "R2_CLOSE" | "R2_QUOTE" | "R2_TEXT" => Class::Text,
            "RAW3_OPEN" | "R3_CLOSE" | "R3_QUOTE" | "R3_TEXT" => Class::Text,
            // The token the lexer gives a character no pattern takes.
            "UNKNOWN" => Class::Unknown,
            other => panic!("rust_lex.qana has a token with no class here: {other}"),
        })
        .collect()
}

/// The class of each byte of a line, from the tokens qana lexed it into.
pub fn line_classes(tokens: &[Token], classes: &[Class]) -> Vec<Class> {
    let mut out = Vec::new();
    for token in tokens {
        let class = classes[token.id as usize];
        out.extend(std::iter::repeat_n(class, token.len as usize));
    }
    out
}

/// A line's classes as a record writes them: runs of one class, each a
/// letter and a byte count, separated by spaces. `w3 _1 p1` is a word of
/// three bytes, a space, and one punctuation character. A line of no
/// bytes is an empty string.
pub fn encode(classes: &[Class]) -> String {
    let mut out = String::new();
    let mut at = 0;
    while at < classes.len() {
        let class = classes[at];
        let run = classes[at..].iter().take_while(|&&c| c == class).count();
        if !out.is_empty() {
            out.push(' ');
        }
        out.push(class.letter());
        out.push_str(&run.to_string());
        at += run;
    }
    out
}
