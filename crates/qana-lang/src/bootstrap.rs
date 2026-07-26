//! The bootstrap grammar for `.qana` — the textual grammar surface —
//! expressed as grammar VALUES on the same certified toolchain every
//! target language uses. This is self-hosting stage 0: these values
//! parse `qana.qana` (the `.qana` grammar written in `.qana`), and the fixed-
//! point gate proves that compiling `qana.qana` reproduces exactly what is
//! written here.
//!
//! Declaration order here is the single source of truth for token and
//! nonterminal ids; `qana.qana` mirrors it line for line.

use qana_grammar::model::{BracketKind, LexGrammar, TokenDef, TokenId};
use qana_grammar::pat::{ClassSet, Pat};
use qana_grammar::syn::{Sym, SynGrammar};
use qana_grammar::Vocab;

/// The `.qana` language's reserved words, in declaration order. (`pair`
/// declares bracket pairs — NOT `bracket`, which would collide with the
/// style class of the same name in attribute arguments.)
pub const RG_KEYWORDS: &[&str] = &[
    "language", "max_stack", "keywords", "token", "mode", "pair", "prec", "left", "right",
    "start", "rule",
];

pub struct QanaIds {
    pub ws: TokenId,
    pub line_comment: TokenId,
    pub name: TokenId,
    pub num: TokenId,
    pub string: TokenId,
    pub string_unterm: TokenId,
    pub pattern: TokenId,
    pub pattern_unterm: TokenId,
    pub eq: TokenId,
    pub pipe: TokenId,
    pub colon: TokenId,
    pub at: TokenId,
    pub comma: TokenId,
    pub lparen: TokenId,
    pub rparen: TokenId,
    pub lbrace: TokenId,
    pub rbrace: TokenId,
    pub star: TokenId,
    pub plus: TokenId,
    pub qmark: TokenId,
    pub percent: TokenId,
    /// Keyword ids, aligned with [`RG_KEYWORDS`].
    pub kw: Vec<TokenId>,
}

impl QanaIds {
    pub fn kw_id(&self, word: &str) -> TokenId {
        let idx = RG_KEYWORDS.iter().position(|k| *k == word).expect("known qana keyword");
        self.kw[idx]
    }
}

/// Escaped-any unit: `\` followed by any non-terminator character.
fn esc() -> Pat {
    Pat::seq([Pat::lit("\\"), Pat::Class(ClassSet::any())])
}

pub fn qana_lex_grammar() -> (LexGrammar, QanaIds) {
    let mut g = LexGrammar::new("Qana", &["DEFAULT"]);
    const DEFAULT: u16 = 0;

    let ws = g.add(TokenDef::new("WS", DEFAULT, Pat::plus(Pat::Class(ClassSet::line_ws()))).trivia());
    let line_comment = g.add(
        TokenDef::new(
            "LINE_COMMENT",
            DEFAULT,
            Pat::seq([Pat::lit("//"), Pat::star(Pat::Class(ClassSet::any()))]),
        )
        .trivia(),
    );
    let name = g.add(
        TokenDef::new(
            "NAME",
            DEFAULT,
            Pat::seq([
                Pat::Class(ClassSet::ident_start()),
                Pat::star(Pat::Class(ClassSet::ident_cont())),
            ]),
        )
        .specialize(),
    );
    let num = g.add(TokenDef::new("NUM", DEFAULT, Pat::plus(Pat::Class(ClassSet::digit()))));

    let str_safe = Pat::Class(ClassSet::not_chars(&['"', '\\', '\r', '\n']));
    let str_body = Pat::star(Pat::alt([esc(), str_safe]));
    let string = g.add(TokenDef::new(
        "STRING",
        DEFAULT,
        Pat::seq([Pat::lit("\""), str_body.clone(), Pat::lit("\"")]),
    ));
    let string_unterm = g.add(
        TokenDef::new(
            "STRING_UNTERM",
            DEFAULT,
            Pat::seq([Pat::lit("\""), str_body, Pat::opt(Pat::lit("\\"))]),
        )
        .error(),
    );

    // A pattern literal `/.../` must contain at least one unit, and its
    // first unit cannot be a bare `/` — which is exactly what makes `//`
    // unambiguously a comment under maximal munch.
    let pat_safe = Pat::Class(ClassSet::not_chars(&['/', '\\', '\r', '\n']));
    let pat_unit = Pat::alt([esc(), pat_safe]);
    let pattern = g.add(TokenDef::new(
        "PATTERN",
        DEFAULT,
        Pat::seq([
            Pat::lit("/"),
            pat_unit.clone(),
            Pat::star(pat_unit.clone()),
            Pat::lit("/"),
        ]),
    ));
    let pattern_unterm = g.add(
        TokenDef::new(
            "PATTERN_UNTERM",
            DEFAULT,
            Pat::seq([
                Pat::lit("/"),
                pat_unit.clone(),
                Pat::star(pat_unit),
                Pat::opt(Pat::lit("\\")),
            ]),
        )
        .error(),
    );

    let eq = g.add(TokenDef::new("EQ", DEFAULT, Pat::lit("=")));
    let pipe = g.add(TokenDef::new("PIPE", DEFAULT, Pat::lit("|")));
    let colon = g.add(TokenDef::new("COLON", DEFAULT, Pat::lit(":")));
    let at = g.add(TokenDef::new("AT", DEFAULT, Pat::lit("@")));
    let comma = g.add(TokenDef::new("COMMA", DEFAULT, Pat::lit(",")));
    let lparen =
        g.add(TokenDef::new("LPAREN", DEFAULT, Pat::lit("(")).bracket(BracketKind::Paren, true));
    let rparen =
        g.add(TokenDef::new("RPAREN", DEFAULT, Pat::lit(")")).bracket(BracketKind::Paren, false));
    let lbrace =
        g.add(TokenDef::new("LBRACE", DEFAULT, Pat::lit("{")).bracket(BracketKind::Brace, true));
    let rbrace =
        g.add(TokenDef::new("RBRACE", DEFAULT, Pat::lit("}")).bracket(BracketKind::Brace, false));
    // EBNF sugar operators.
    let star = g.add(TokenDef::new("STAR", DEFAULT, Pat::lit("*")));
    let plus = g.add(TokenDef::new("PLUS", DEFAULT, Pat::lit("+")));
    let qmark = g.add(TokenDef::new("QMARK", DEFAULT, Pat::lit("?")));
    let percent = g.add(TokenDef::new("PERCENT", DEFAULT, Pat::lit("%")));

    let kw: Vec<TokenId> = RG_KEYWORDS
        .iter()
        .map(|w| g.add(TokenDef::new(&format!("KW_{}", w.to_uppercase()), DEFAULT, Pat::Never)))
        .collect();
    g.keywords = RG_KEYWORDS.iter().zip(&kw).map(|(w, id)| (w.to_string(), *id, name)).collect();

    (
        g,
        QanaIds {
            ws,
            line_comment,
            name,
            num,
            string,
            string_unterm,
            pattern,
            pattern_unterm,
            eq,
            pipe,
            colon,
            at,
            comma,
            lparen,
            rparen,
            lbrace,
            rbrace,
            star,
            plus,
            qmark,
            percent,
            kw,
        },
    )
}

/// Production indices of the `.qana` syntax grammar — the compiler's match
/// targets when walking `.qana` trees.
pub struct QanaProds {
    pub file: usize,
    pub decls_empty: usize,
    pub decls_more: usize,
    pub lang_decl: usize,
    pub max_stack_decl: usize,
    pub kw_decl: usize,
    pub token_decl: usize,
    pub mode_decl: usize,
    pub bracket_decl: usize,
    pub prec_decl: usize,
    pub start_decl: usize,
    pub rule_decl: usize,
    pub rule_decl_bar: usize,
    pub token_def: usize,
    pub kw_name: usize,
    pub kw_str: usize,
    pub pat_regex: usize,
    pub pat_lit: usize,
    pub attr_plain: usize,
    pub attr_args: usize,
    pub arg_name: usize,
    pub arg_num: usize,
    pub arg_str: usize,
    pub tok_name: usize,
    pub tok_str: usize,
    pub assoc_left: usize,
    pub assoc_right: usize,
    pub alt: usize,
    pub sym_name: usize,
    pub sym_str: usize,
    pub sym_labeled: usize,
    pub sym_labeled_str: usize,
    // EBNF sugar (P5 increment 2).
    pub rule_star: usize,
    pub rule_plus: usize,
    pub rule_opt: usize,
    pub sym_name_opt: usize,
    pub sym_name_star: usize,
    pub sym_name_plus: usize,
    pub sym_str_opt: usize,
    pub sym_str_star: usize,
    pub sym_str_plus: usize,
    pub elem_name: usize,
    pub elem_str: usize,
    pub rep_sep_none: usize,
    pub rep_sep_some: usize,
}

pub fn qana_syn_grammar(ids: &QanaIds, vocab: &Vocab) -> (SynGrammar, QanaProds) {
    let mut sg = SynGrammar::new("QanaSyn", vocab.names.clone());
    let t = |id: TokenId| Sym::T(id);
    let k = |w: &str| Sym::T(ids.kw_id(w));

    let file = sg.nt("file");
    let decls = sg.nt("decls");
    let decl = sg.nt("decl");
    let token_def = sg.nt("token_def");
    let token_defs = sg.nt("token_defs");
    let kw_list = sg.nt("kw_list");
    let kw_item = sg.nt("kw_item");
    let tok_pat = sg.nt("tok_pat");
    let attrs = sg.nt("attrs");
    let attr = sg.nt("attr");
    let arg_list = sg.nt("arg_list");
    let arg = sg.nt("arg");
    let tok_ref = sg.nt("tok_ref");
    let assoc = sg.nt("assoc");
    let prec_ops = sg.nt("prec_ops");
    let alt_list = sg.nt("alt_list");
    let alt = sg.nt("alt");
    let syms = sg.nt("syms");
    let sym = sg.nt("sym");
    let elem = sg.nt("elem");
    let rep_sep = sg.nt("rep_sep");
    sg.start = file;
    let n = |x| Sym::N(x);

    let p_file = sg.prod_named(file, "File", vec![n(decls)]);
    let p_decls_empty = sg.prod_named(decls, "DeclsEmpty", vec![]);
    let p_decls_more = sg.prod_named(decls, "DeclsMore", vec![n(decls), n(decl)]);

    let p_lang = sg.prod_named(decl, "LangDecl", vec![k("language"), t(ids.name)]);
    let p_max_stack = sg.prod_named(decl, "MaxStackDecl", vec![k("max_stack"), t(ids.num)]);
    let p_kw_decl =
        sg.prod_named(decl, "KwDecl", vec![k("keywords"), t(ids.name), t(ids.eq), n(kw_list)]);
    let p_token_decl = sg.prod_named(decl, "TokenDecl", vec![n(token_def)]);
    let p_mode_decl = sg.prod_named(
        decl,
        "ModeDecl",
        vec![k("mode"), t(ids.name), t(ids.lbrace), n(token_defs), t(ids.rbrace)],
    );
    let p_bracket_decl =
        sg.prod_named(decl, "PairDecl", vec![k("pair"), n(tok_ref), n(tok_ref)]);
    let p_prec_decl = sg.prod_named(decl, "PrecDecl", vec![k("prec"), n(assoc), n(prec_ops)]);
    let p_start_decl = sg.prod_named(decl, "StartDecl", vec![k("start"), t(ids.name)]);
    let p_rule_decl =
        sg.prod_named(decl, "RuleDecl", vec![k("rule"), t(ids.name), t(ids.eq), n(alt_list)]);
    let p_rule_decl_bar = sg.prod_named(
        decl,
        "RuleDeclBar",
        vec![k("rule"), t(ids.name), t(ids.eq), t(ids.pipe), n(alt_list)],
    );
    // EBNF sugar rule forms: `rule R = elem* [% sep]` etc.
    let p_rule_star = sg.prod_named(
        decl,
        "RuleStar",
        vec![k("rule"), t(ids.name), t(ids.eq), n(elem), t(ids.star), n(rep_sep)],
    );
    let p_rule_plus = sg.prod_named(
        decl,
        "RulePlus",
        vec![k("rule"), t(ids.name), t(ids.eq), n(elem), t(ids.plus), n(rep_sep)],
    );
    let p_rule_opt = sg.prod_named(
        decl,
        "RuleOpt",
        vec![k("rule"), t(ids.name), t(ids.eq), n(elem), t(ids.qmark)],
    );

    let p_token_def = sg.prod_named(
        token_def,
        "TokenDef",
        vec![k("token"), t(ids.name), t(ids.eq), n(tok_pat), n(attrs)],
    );
    sg.prod_named(token_defs, "TokenDefsEmpty", vec![]);
    sg.prod_named(token_defs, "TokenDefsMore", vec![n(token_defs), n(token_def)]);

    sg.prod_named(kw_list, "KwListFirst", vec![n(kw_item)]);
    sg.prod_named(kw_list, "KwListMore", vec![n(kw_list), n(kw_item)]);
    let p_kw_name = sg.prod_named(kw_item, "KwName", vec![t(ids.name)]);
    let p_kw_str = sg.prod_named(kw_item, "KwStr", vec![t(ids.string)]);

    let p_pat_regex = sg.prod_named(tok_pat, "PatRegex", vec![t(ids.pattern)]);
    let p_pat_lit = sg.prod_named(tok_pat, "PatLit", vec![t(ids.string)]);

    sg.prod_named(attrs, "AttrsEmpty", vec![]);
    sg.prod_named(attrs, "AttrsMore", vec![n(attrs), n(attr)]);
    let p_attr_plain = sg.prod_named(attr, "AttrPlain", vec![t(ids.at), t(ids.name)]);
    let p_attr_args = sg.prod_named(
        attr,
        "AttrArgs",
        vec![t(ids.at), t(ids.name), t(ids.lparen), n(arg_list), t(ids.rparen)],
    );

    sg.prod_named(arg_list, "ArgListFirst", vec![n(arg)]);
    sg.prod_named(arg_list, "ArgListMore", vec![n(arg_list), t(ids.comma), n(arg)]);
    let p_arg_name = sg.prod_named(arg, "ArgName", vec![t(ids.name)]);
    let p_arg_num = sg.prod_named(arg, "ArgNum", vec![t(ids.num)]);
    let p_arg_str = sg.prod_named(arg, "ArgStr", vec![t(ids.string)]);

    let p_tok_name = sg.prod_named(tok_ref, "TokName", vec![t(ids.name)]);
    let p_tok_str = sg.prod_named(tok_ref, "TokStr", vec![t(ids.string)]);

    let p_assoc_left = sg.prod_named(assoc, "AssocLeft", vec![k("left")]);
    let p_assoc_right = sg.prod_named(assoc, "AssocRight", vec![k("right")]);

    sg.prod_named(prec_ops, "PrecOpsFirst", vec![n(tok_ref)]);
    sg.prod_named(prec_ops, "PrecOpsMore", vec![n(prec_ops), n(tok_ref)]);

    sg.prod_named(alt_list, "AltListFirst", vec![n(alt)]);
    sg.prod_named(alt_list, "AltListMore", vec![n(alt_list), t(ids.pipe), n(alt)]);
    let p_alt = sg.prod_named(alt, "Alt", vec![t(ids.name), t(ids.colon), n(syms), n(attrs)]);

    sg.prod_named(syms, "SymsEmpty", vec![]);
    sg.prod_named(syms, "SymsMore", vec![n(syms), n(sym)]);
    let p_sym_name = sg.prod_named(sym, "SymName", vec![t(ids.name)]);
    let p_sym_str = sg.prod_named(sym, "SymStr", vec![t(ids.string)]);
    let p_sym_labeled =
        sg.prod_named(sym, "SymLabeled", vec![t(ids.name), t(ids.colon), t(ids.name)]);
    let p_sym_labeled_str =
        sg.prod_named(sym, "SymLabeledStr", vec![t(ids.name), t(ids.colon), t(ids.string)]);
    // Inline postfix sugar on RHS symbols.
    let p_sym_name_opt = sg.prod_named(sym, "SymNameOpt", vec![t(ids.name), t(ids.qmark)]);
    let p_sym_name_star = sg.prod_named(sym, "SymNameStar", vec![t(ids.name), t(ids.star)]);
    let p_sym_name_plus = sg.prod_named(sym, "SymNamePlus", vec![t(ids.name), t(ids.plus)]);
    let p_sym_str_opt = sg.prod_named(sym, "SymStrOpt", vec![t(ids.string), t(ids.qmark)]);
    let p_sym_str_star = sg.prod_named(sym, "SymStrStar", vec![t(ids.string), t(ids.star)]);
    let p_sym_str_plus = sg.prod_named(sym, "SymStrPlus", vec![t(ids.string), t(ids.plus)]);

    let p_elem_name = sg.prod_named(elem, "ElemName", vec![t(ids.name)]);
    let p_elem_str = sg.prod_named(elem, "ElemStr", vec![t(ids.string)]);
    let p_rep_sep_none = sg.prod_named(rep_sep, "RepSepNone", vec![]);
    let p_rep_sep_some =
        sg.prod_named(rep_sep, "RepSepSome", vec![t(ids.percent), n(elem)]);

    let prods = QanaProds {
        file: p_file,
        decls_empty: p_decls_empty,
        decls_more: p_decls_more,
        lang_decl: p_lang,
        max_stack_decl: p_max_stack,
        kw_decl: p_kw_decl,
        token_decl: p_token_decl,
        mode_decl: p_mode_decl,
        bracket_decl: p_bracket_decl,
        prec_decl: p_prec_decl,
        start_decl: p_start_decl,
        rule_decl: p_rule_decl,
        rule_decl_bar: p_rule_decl_bar,
        token_def: p_token_def,
        kw_name: p_kw_name,
        kw_str: p_kw_str,
        pat_regex: p_pat_regex,
        pat_lit: p_pat_lit,
        attr_plain: p_attr_plain,
        attr_args: p_attr_args,
        arg_name: p_arg_name,
        arg_num: p_arg_num,
        arg_str: p_arg_str,
        tok_name: p_tok_name,
        tok_str: p_tok_str,
        assoc_left: p_assoc_left,
        assoc_right: p_assoc_right,
        alt: p_alt,
        sym_name: p_sym_name,
        sym_str: p_sym_str,
        sym_labeled: p_sym_labeled,
        sym_labeled_str: p_sym_labeled_str,
        rule_star: p_rule_star,
        rule_plus: p_rule_plus,
        rule_opt: p_rule_opt,
        sym_name_opt: p_sym_name_opt,
        sym_name_star: p_sym_name_star,
        sym_name_plus: p_sym_name_plus,
        sym_str_opt: p_sym_str_opt,
        sym_str_star: p_sym_str_star,
        sym_str_plus: p_sym_str_plus,
        elem_name: p_elem_name,
        elem_str: p_elem_str,
        rep_sep_none: p_rep_sep_none,
        rep_sep_some: p_rep_sep_some,
    };
    (sg, prods)
}
