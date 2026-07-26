//! The P0 spike's demo language, re-expressed as a grammar VALUE.
//! This is the equivalence subject: the lexer generated from this value
//! must be observationally equivalent to the hand-written P0 lexer.
//!
//! P1 increment 2 refines token granularity for the parser's sake:
//! each keyword owns a distinct token id (specialized from IDENT via the
//! keyword map), and the operators the syntax grammar references get
//! dedicated ids ahead of the generic PUNCT class. The hand lexer sees
//! these as `Keyword`/`Punct` classes — the equivalence gate compares at
//! that class level (the generated lexer refines, never disagrees).

use crate::model::{BracketKind, LexGrammar, TokenDef, TokenId, Vocab};
use crate::pat::{ClassSet, Pat};
use crate::syn::{Assoc, Sym, SynGrammar};

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
    pub plus: TokenId,
    pub minus: TokenId,
    pub star: TokenId,
    pub slash: TokenId,
    pub semi: TokenId,
    pub comma: TokenId,
    pub eq: TokenId,
    pub punct: TokenId,
    /// Keyword token ids, aligned with [`DemoIds::kw_names`].
    pub kw: Vec<TokenId>,
    /// The keyword spellings this grammar instance was built with
    /// (defaults to [`KEYWORDS`]; hot-reload builds custom sets).
    pub kw_names: Vec<String>,
    pub b_open: TokenId,
    pub b_close: TokenId,
    pub b_content: TokenId,
    pub b_misc: TokenId,
    pub unknown: TokenId,
    pub block_mode: u16,
}

impl DemoIds {
    pub fn kw_id(&self, word: &str) -> TokenId {
        let idx = self.kw_names.iter().position(|k| k == word).expect("known keyword");
        self.kw[idx]
    }
    pub fn is_kw(&self, id: TokenId) -> bool {
        self.kw.contains(&id)
    }
    pub fn is_punct_class(&self, id: TokenId) -> bool {
        id == self.plus
            || id == self.minus
            || id == self.star
            || id == self.slash
            || id == self.semi
            || id == self.comma
            || id == self.eq
            || id == self.punct
    }
}

pub const KEYWORDS: &[&str] = &[
    "fn", "let", "if", "else", "while", "for", "return", "struct", "enum",
    "impl", "match", "true", "false", "mut", "pub", "use",
];

pub fn demo_grammar() -> (LexGrammar, DemoIds) {
    demo_grammar_with_keywords(&KEYWORDS.iter().map(|s| s.to_string()).collect::<Vec<_>>())
}

/// Hot-reload entry point: the same certified pipeline with a custom
/// keyword set. (The syntax grammar requires `let`, `if`, `else` — the
/// config loader validates that before calling.)
pub fn demo_grammar_with_keywords(keywords: &[String]) -> (LexGrammar, DemoIds) {
    let mut g = LexGrammar::new("Demo", &["DEFAULT", "BLOCK"]);
    const DEFAULT: u16 = 0;
    const BLOCK: u16 = 1;
    g.max_stack = Some(8); // BLOCK pushes itself (nested comments): L2 bound.

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
    let ident = g.add(
        TokenDef::new(
            "IDENT",
            DEFAULT,
            Pat::seq([
                Pat::Class(ClassSet::ident_start()),
                Pat::star(Pat::Class(ClassSet::ident_cont())),
            ]),
        )
        .specialize(),
    );
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
    // Parser-relevant operators, each with its own id.
    let plus = g.add(TokenDef::new("PLUS", DEFAULT, Pat::lit("+")));
    let minus = g.add(TokenDef::new("MINUS", DEFAULT, Pat::lit("-")));
    let star = g.add(TokenDef::new("STAR", DEFAULT, Pat::lit("*")));
    let slash = g.add(TokenDef::new("SLASH", DEFAULT, Pat::lit("/")));
    let semi = g.add(TokenDef::new("SEMI", DEFAULT, Pat::lit(";")));
    let comma = g.add(TokenDef::new("COMMA", DEFAULT, Pat::lit(",")));
    let eq = g.add(TokenDef::new("EQ", DEFAULT, Pat::lit("=")));
    // Everything else single-char ASCII punctuation.
    let punct = g.add(TokenDef::new(
        "PUNCT",
        DEFAULT,
        Pat::Class(ClassSet::ranges(&[('!', '/'), (':', '@'), ('[', '`'), ('{', '~')])),
    ));

    // One distinct token per keyword (specialization targets).
    let kw: Vec<TokenId> = keywords
        .iter()
        .map(|w| g.add(TokenDef::new(&format!("KW_{}", w.to_uppercase()), DEFAULT, Pat::Never)))
        .collect();
    g.keywords = keywords
        .iter()
        .zip(&kw)
        .map(|(w, id)| (w.clone(), *id, ident))
        .collect();

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
            plus,
            minus,
            star,
            slash,
            semi,
            comma,
            eq,
            punct,
            kw,
            kw_names: keywords.to_vec(),
            b_open,
            b_close,
            b_content,
            b_misc,
            unknown,
            block_mode: BLOCK,
        },
    )
}

/// The demo language's SYNTAX grammar (P1 increment 2): statements and
/// precedence-resolved expressions over the lexical grammar above. Uses
/// braces-required `if`/`else` (no dangling-else ambiguity under
/// canonical LR(1)) and declarative operator precedence (envelope L3).
pub fn demo_syn_grammar(ids: &DemoIds, vocab: &Vocab) -> SynGrammar {
    demo_syn_grammar_prec(ids, vocab, &default_prec(ids))
}

/// Default operator precedence: `+ -` level 1 left, `* /` level 2 left.
pub fn default_prec(ids: &DemoIds) -> Vec<(TokenId, u8, Assoc)> {
    vec![
        (ids.plus, 1, Assoc::Left),
        (ids.minus, 1, Assoc::Left),
        (ids.star, 2, Assoc::Left),
        (ids.slash, 2, Assoc::Left),
    ]
}

/// Hot-reload entry point: same syntax grammar, custom operator
/// precedence/associativity. Conflicting configurations are refused by
/// `build_lr` with counterexamples — the envelope, live.
pub fn demo_syn_grammar_prec(
    ids: &DemoIds,
    vocab: &Vocab,
    prec: &[(TokenId, u8, Assoc)],
) -> SynGrammar {
    let mut sg = SynGrammar::new("DemoSyn", vocab.names.clone());
    let t = |id: TokenId| Sym::T(id);

    let file = sg.nt("file");
    let stmts = sg.nt("stmts");
    let stmt = sg.nt("stmt");
    let block = sg.nt("block");
    let expr = sg.nt("expr");
    let args = sg.nt("args");
    let args_ne = sg.nt("args_ne");
    sg.start = file;

    for &(tok, level, assoc) in prec {
        sg.set_token_prec(tok, level, assoc);
    }

    let n = |x| Sym::N(x);
    sg.prod_named(file, "File", vec![n(stmts)]);
    sg.prod_named(stmts, "StmtsEmpty", vec![]);
    sg.prod_named(stmts, "StmtsMore", vec![n(stmts), n(stmt)]);
    sg.prod_named(
        stmt,
        "LetStmt",
        vec![t(ids.kw_id("let")), t(ids.ident), t(ids.eq), n(expr), t(ids.semi)],
    );
    sg.prod_named(stmt, "ExprStmt", vec![n(expr), t(ids.semi)]);
    sg.prod_named(stmt, "BlockStmt", vec![n(block)]);
    sg.prod_named(
        stmt,
        "IfStmt",
        vec![t(ids.kw_id("if")), t(ids.lparen), n(expr), t(ids.rparen), n(block)],
    );
    sg.prod_named(
        stmt,
        "IfElseStmt",
        vec![
            t(ids.kw_id("if")),
            t(ids.lparen),
            n(expr),
            t(ids.rparen),
            n(block),
            t(ids.kw_id("else")),
            n(block),
        ],
    );
    sg.prod_named(block, "Block", vec![t(ids.lbrace), n(stmts), t(ids.rbrace)]);

    sg.prod_named(expr, "AddExpr", vec![n(expr), t(ids.plus), n(expr)]);
    sg.prod_named(expr, "SubExpr", vec![n(expr), t(ids.minus), n(expr)]);
    sg.prod_named(expr, "MulExpr", vec![n(expr), t(ids.star), n(expr)]);
    sg.prod_named(expr, "DivExpr", vec![n(expr), t(ids.slash), n(expr)]);
    sg.prod_named(expr, "NumLit", vec![t(ids.number)]);
    sg.prod_named(expr, "StrLit", vec![t(ids.string)]);
    sg.prod_named(expr, "NameRef", vec![t(ids.ident)]);
    sg.prod_named(expr, "CallExpr", vec![t(ids.ident), t(ids.lparen), n(args), t(ids.rparen)]);
    sg.prod_named(expr, "ParenExpr", vec![t(ids.lparen), n(expr), t(ids.rparen)]);
    sg.prod_named(expr, "ListExpr", vec![t(ids.lbracket), n(args), t(ids.rbracket)]);

    // Named to match what the .qana surface's `expr* % ","` sugar
    // generates (the text≡code gate compares production names too).
    sg.prod_named(args, "ArgsNone", vec![]);
    sg.prod_named(args, "ArgsSome", vec![n(args_ne)]);
    sg.prod_named(args_ne, "ArgsNeFirst", vec![n(expr)]);
    sg.prod_named(args_ne, "ArgsNeMore", vec![n(args_ne), t(ids.comma), n(expr)]);

    sg
}
