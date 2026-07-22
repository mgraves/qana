//! The P0 spike's demo language, re-expressed as a grammar VALUE.
//! This is the equivalence subject: the lexer generated from this value
//! must be observationally identical to the hand-written P0 lexer.

use crate::model::{BracketKind, LexGrammar, TokenDef, TokenId};
use crate::pat::{ClassSet, Pat};

pub struct DemoIds {
    pub ws: TokenId,
    pub line_comment: TokenId,
    pub block_open: TokenId,
    pub string: TokenId,
    pub string_unterm: TokenId,
    pub number: TokenId,
    pub ident: TokenId,
    pub lparen: TokenId,
    pub rparen: TokenId,
    pub lbracket: TokenId,
    pub rbracket: TokenId,
    pub lbrace: TokenId,
    pub rbrace: TokenId,
    pub punct: TokenId,
    pub keyword: TokenId,
    pub b_open: TokenId,
    pub b_close: TokenId,
    pub b_content: TokenId,
    pub b_misc: TokenId,
    pub unknown: TokenId,
    pub block_mode: u16,
}

pub const KEYWORDS: &[&str] = &[
    "fn", "let", "if", "else", "while", "for", "return", "struct", "enum",
    "impl", "match", "true", "false", "mut", "pub", "use",
];

pub fn demo_grammar() -> (LexGrammar, DemoIds) {
    let mut g = LexGrammar::new("Demo", &["DEFAULT", "BLOCK"]);
    const DEFAULT: u16 = 0;
    const BLOCK: u16 = 1;
    g.keywords = KEYWORDS.iter().map(|s| s.to_string()).collect();
    g.max_stack = Some(8); // BLOCK pushes itself (nested comments): L2 bound.

    // String pieces: escape = '\' followed by any non-terminator;
    // safe = anything except quote, backslash, terminators.
    let esc = Pat::seq([Pat::lit("\\"), Pat::Class(ClassSet::any())]);
    let safe = Pat::Class(ClassSet::not_chars(&['"', '\\', '\r', '\n']));
    let body = Pat::star(Pat::alt([esc.clone(), safe.clone()]));

    // Declaration order = tie-break priority (earlier wins at equal length).
    let ws = g.add(TokenDef::new("WS", DEFAULT, Pat::plus(Pat::Class(ClassSet::line_ws()))).trivia());
    let line_comment = g.add(
        TokenDef::new(
            "LINE_COMMENT",
            DEFAULT,
            Pat::seq([Pat::lit("//"), Pat::star(Pat::Class(ClassSet::any()))]),
        )
        .trivia(),
    );
    let block_open = g.add(TokenDef::new("BLOCK_OPEN", DEFAULT, Pat::lit("/*")).trivia().push(BLOCK));
    let string = g.add(TokenDef::new(
        "STRING",
        DEFAULT,
        Pat::seq([Pat::lit("\""), body.clone(), Pat::lit("\"")]),
    ));
    // Unterminated string: a prefix (optionally ending in a dangling
    // backslash, which the hand lexer also consumes). Loses ties to
    // STRING at equal length; loses to STRING by maximal munch otherwise.
    let string_unterm = g.add(
        TokenDef::new(
            "STRING_UNTERM",
            DEFAULT,
            Pat::seq([Pat::lit("\""), body, Pat::opt(Pat::lit("\\"))]),
        )
        .error(),
    );
    let number = g.add(TokenDef::new(
        "NUMBER",
        DEFAULT,
        Pat::seq([
            Pat::plus(Pat::Class(ClassSet::digit())),
            Pat::opt(Pat::seq([Pat::lit("."), Pat::plus(Pat::Class(ClassSet::digit()))])),
        ]),
    ));
    let ident = g.add(TokenDef::new(
        "IDENT",
        DEFAULT,
        Pat::seq([
            Pat::Class(ClassSet::ident_start()),
            Pat::star(Pat::Class(ClassSet::ident_cont())),
        ]),
    ));
    let lparen = g.add(TokenDef::new("LPAREN", DEFAULT, Pat::lit("(")).bracket(BracketKind::Paren, true));
    let rparen =
        g.add(TokenDef::new("RPAREN", DEFAULT, Pat::lit(")")).bracket(BracketKind::Paren, false));
    let lbracket = g
        .add(TokenDef::new("LBRACKET", DEFAULT, Pat::lit("[")).bracket(BracketKind::Bracket, true));
    let rbracket = g
        .add(TokenDef::new("RBRACKET", DEFAULT, Pat::lit("]")).bracket(BracketKind::Bracket, false));
    let lbrace =
        g.add(TokenDef::new("LBRACE", DEFAULT, Pat::lit("{")).bracket(BracketKind::Brace, true));
    let rbrace =
        g.add(TokenDef::new("RBRACE", DEFAULT, Pat::lit("}")).bracket(BracketKind::Brace, false));
    // Generic single-char ASCII punctuation (brackets/quotes already win
    // their ties above by declaration order).
    let punct = g.add(TokenDef::new(
        "PUNCT",
        DEFAULT,
        Pat::Class(ClassSet::ranges(&[('!', '/'), (':', '@'), ('[', '`'), ('{', '~')])),
    ));
    let keyword = g.add(TokenDef::new("KW", DEFAULT, Pat::Never));

    // BLOCK mode: nested comment interior. All trivia.
    let b_open = g.add(TokenDef::new("B_OPEN", BLOCK, Pat::lit("/*")).trivia().push(BLOCK));
    let b_close = g.add(TokenDef::new("B_CLOSE", BLOCK, Pat::lit("*/")).trivia().pop());
    let b_content = g.add(
        TokenDef::new(
            "B_CONTENT",
            BLOCK,
            Pat::plus(Pat::Class(ClassSet::not_chars(&['*', '/', '\r', '\n']))),
        )
        .trivia(),
    );
    let b_misc = g.add(TokenDef::new("B_MISC", BLOCK, Pat::Class(ClassSet::chars(&['*', '/']))).trivia());

    // Wire specialization after KW exists.
    g.tokens[ident as usize].specialize_to = Some(keyword);

    let unknown = g.unknown_id();
    (
        g,
        DemoIds {
            ws,
            line_comment,
            block_open,
            string,
            string_unterm,
            number,
            ident,
            lparen,
            rparen,
            lbracket,
            rbracket,
            lbrace,
            rbrace,
            punct,
            keyword,
            b_open,
            b_close,
            b_content,
            b_misc,
            unknown,
            block_mode: BLOCK,
        },
    )
}
