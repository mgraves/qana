//! @generated typed AST for the `RgSyn` grammar. DO NOT EDIT.
//! Regenerate with the grammar's astgen binary (drift-gated).
#![allow(dead_code)]

use rantlr_grammar::typed::{AstNode, NodeRef, TokenRef};

/// `file → decls`
#[derive(Clone, Copy, Debug)]
pub struct File<'g>(pub NodeRef<'g>);

impl<'g> AstNode<'g> for File<'g> {
    fn cast(node: NodeRef<'g>) -> Option<Self> {
        (node.nt() == 0 && node.prod() == 0).then(|| File(node))
    }
    fn node(&self) -> NodeRef<'g> {
        self.0
    }
}

impl<'g> File<'g> {
    pub fn decls(&self) -> Option<Decls<'g>> {
        Decls::cast(self.0.child_node(0)?)
    }
}

/// `decls` — balanced list of `Decl` (envelope L4).
#[derive(Clone, Copy, Debug)]
pub struct Decls<'g>(pub NodeRef<'g>);

impl<'g> AstNode<'g> for Decls<'g> {
    fn cast(node: NodeRef<'g>) -> Option<Self> {
        (node.nt() == 1 && node.prod() == rantlr_grammar::green::LIST_PROD).then(|| Decls(node))
    }
    fn node(&self) -> NodeRef<'g> {
        self.0
    }
}

impl<'g> Decls<'g> {
    pub fn items(&self) -> Vec<Decl<'g>> {
        self.0
            .flat_symbol_children()
            .into_iter()
            .filter_map(|c| match c {
                rantlr_grammar::typed::SymbolChild::Node(n) => Decl::cast(n),
                _ => None,
            })
            .collect()
    }
}

/// `decl` — 10 productions.
#[derive(Clone, Copy, Debug)]
pub enum Decl<'g> {
    LangDecl(LangDecl<'g>),
    MaxStackDecl(MaxStackDecl<'g>),
    KwDecl(KwDecl<'g>),
    TokenDecl(TokenDecl<'g>),
    ModeDecl(ModeDecl<'g>),
    PairDecl(PairDecl<'g>),
    PrecDecl(PrecDecl<'g>),
    StartDecl(StartDecl<'g>),
    RuleDecl(RuleDecl<'g>),
    RuleDeclBar(RuleDeclBar<'g>),
}

impl<'g> AstNode<'g> for Decl<'g> {
    fn cast(node: NodeRef<'g>) -> Option<Self> {
        if node.nt() != 2 {
            return None;
        }
        match node.prod() {
            3 => Some(Decl::LangDecl(LangDecl(node))),
            4 => Some(Decl::MaxStackDecl(MaxStackDecl(node))),
            5 => Some(Decl::KwDecl(KwDecl(node))),
            6 => Some(Decl::TokenDecl(TokenDecl(node))),
            7 => Some(Decl::ModeDecl(ModeDecl(node))),
            8 => Some(Decl::PairDecl(PairDecl(node))),
            9 => Some(Decl::PrecDecl(PrecDecl(node))),
            10 => Some(Decl::StartDecl(StartDecl(node))),
            11 => Some(Decl::RuleDecl(RuleDecl(node))),
            12 => Some(Decl::RuleDeclBar(RuleDeclBar(node))),
            _ => None,
        }
    }
    fn node(&self) -> NodeRef<'g> {
        match self {
            Decl::LangDecl(x) => x.0,
            Decl::MaxStackDecl(x) => x.0,
            Decl::KwDecl(x) => x.0,
            Decl::TokenDecl(x) => x.0,
            Decl::ModeDecl(x) => x.0,
            Decl::PairDecl(x) => x.0,
            Decl::PrecDecl(x) => x.0,
            Decl::StartDecl(x) => x.0,
            Decl::RuleDecl(x) => x.0,
            Decl::RuleDeclBar(x) => x.0,
        }
    }
}

/// `decl → KW_LANGUAGE NAME`
#[derive(Clone, Copy, Debug)]
pub struct LangDecl<'g>(pub NodeRef<'g>);

impl<'g> AstNode<'g> for LangDecl<'g> {
    fn cast(node: NodeRef<'g>) -> Option<Self> {
        (node.nt() == 2 && node.prod() == 3).then(|| LangDecl(node))
    }
    fn node(&self) -> NodeRef<'g> {
        self.0
    }
}

impl<'g> LangDecl<'g> {
    pub fn kw_language_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(0, 17) // KW_LANGUAGE
    }
    pub fn name_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(1, 2) // NAME
    }
}

/// `decl → KW_MAX_STACK NUM`
#[derive(Clone, Copy, Debug)]
pub struct MaxStackDecl<'g>(pub NodeRef<'g>);

impl<'g> AstNode<'g> for MaxStackDecl<'g> {
    fn cast(node: NodeRef<'g>) -> Option<Self> {
        (node.nt() == 2 && node.prod() == 4).then(|| MaxStackDecl(node))
    }
    fn node(&self) -> NodeRef<'g> {
        self.0
    }
}

impl<'g> MaxStackDecl<'g> {
    pub fn kw_max_stack_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(0, 18) // KW_MAX_STACK
    }
    pub fn num_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(1, 3) // NUM
    }
}

/// `decl → KW_KEYWORDS NAME EQ kw_list`
#[derive(Clone, Copy, Debug)]
pub struct KwDecl<'g>(pub NodeRef<'g>);

impl<'g> AstNode<'g> for KwDecl<'g> {
    fn cast(node: NodeRef<'g>) -> Option<Self> {
        (node.nt() == 2 && node.prod() == 5).then(|| KwDecl(node))
    }
    fn node(&self) -> NodeRef<'g> {
        self.0
    }
}

impl<'g> KwDecl<'g> {
    pub fn kw_keywords_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(0, 19) // KW_KEYWORDS
    }
    pub fn name_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(1, 2) // NAME
    }
    pub fn eq_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(2, 8) // EQ
    }
    pub fn kw_list(&self) -> Option<KwList<'g>> {
        KwList::cast(self.0.child_node(3)?)
    }
}

/// `decl → token_def`
#[derive(Clone, Copy, Debug)]
pub struct TokenDecl<'g>(pub NodeRef<'g>);

impl<'g> AstNode<'g> for TokenDecl<'g> {
    fn cast(node: NodeRef<'g>) -> Option<Self> {
        (node.nt() == 2 && node.prod() == 6).then(|| TokenDecl(node))
    }
    fn node(&self) -> NodeRef<'g> {
        self.0
    }
}

impl<'g> TokenDecl<'g> {
    pub fn token_def(&self) -> Option<TokenDef<'g>> {
        TokenDef::cast(self.0.child_node(0)?)
    }
}

/// `decl → KW_MODE NAME LBRACE token_defs RBRACE`
#[derive(Clone, Copy, Debug)]
pub struct ModeDecl<'g>(pub NodeRef<'g>);

impl<'g> AstNode<'g> for ModeDecl<'g> {
    fn cast(node: NodeRef<'g>) -> Option<Self> {
        (node.nt() == 2 && node.prod() == 7).then(|| ModeDecl(node))
    }
    fn node(&self) -> NodeRef<'g> {
        self.0
    }
}

impl<'g> ModeDecl<'g> {
    pub fn kw_mode_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(0, 21) // KW_MODE
    }
    pub fn name_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(1, 2) // NAME
    }
    pub fn lbrace_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(2, 15) // LBRACE
    }
    pub fn token_defs(&self) -> Option<TokenDefs<'g>> {
        TokenDefs::cast(self.0.child_node(3)?)
    }
    pub fn rbrace_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(4, 16) // RBRACE
    }
}

/// `decl → KW_PAIR tok_ref tok_ref`
#[derive(Clone, Copy, Debug)]
pub struct PairDecl<'g>(pub NodeRef<'g>);

impl<'g> AstNode<'g> for PairDecl<'g> {
    fn cast(node: NodeRef<'g>) -> Option<Self> {
        (node.nt() == 2 && node.prod() == 8).then(|| PairDecl(node))
    }
    fn node(&self) -> NodeRef<'g> {
        self.0
    }
}

impl<'g> PairDecl<'g> {
    pub fn kw_pair_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(0, 22) // KW_PAIR
    }
    pub fn tok_ref(&self) -> Option<TokRef<'g>> {
        TokRef::cast(self.0.child_node(1)?)
    }
    pub fn tok_ref_2(&self) -> Option<TokRef<'g>> {
        TokRef::cast(self.0.child_node(2)?)
    }
}

/// `decl → KW_PREC assoc prec_ops`
#[derive(Clone, Copy, Debug)]
pub struct PrecDecl<'g>(pub NodeRef<'g>);

impl<'g> AstNode<'g> for PrecDecl<'g> {
    fn cast(node: NodeRef<'g>) -> Option<Self> {
        (node.nt() == 2 && node.prod() == 9).then(|| PrecDecl(node))
    }
    fn node(&self) -> NodeRef<'g> {
        self.0
    }
}

impl<'g> PrecDecl<'g> {
    pub fn kw_prec_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(0, 23) // KW_PREC
    }
    pub fn assoc(&self) -> Option<Assoc<'g>> {
        Assoc::cast(self.0.child_node(1)?)
    }
    pub fn prec_ops(&self) -> Option<PrecOps<'g>> {
        PrecOps::cast(self.0.child_node(2)?)
    }
}

/// `decl → KW_START NAME`
#[derive(Clone, Copy, Debug)]
pub struct StartDecl<'g>(pub NodeRef<'g>);

impl<'g> AstNode<'g> for StartDecl<'g> {
    fn cast(node: NodeRef<'g>) -> Option<Self> {
        (node.nt() == 2 && node.prod() == 10).then(|| StartDecl(node))
    }
    fn node(&self) -> NodeRef<'g> {
        self.0
    }
}

impl<'g> StartDecl<'g> {
    pub fn kw_start_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(0, 26) // KW_START
    }
    pub fn name_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(1, 2) // NAME
    }
}

/// `decl → KW_RULE NAME EQ alt_list`
#[derive(Clone, Copy, Debug)]
pub struct RuleDecl<'g>(pub NodeRef<'g>);

impl<'g> AstNode<'g> for RuleDecl<'g> {
    fn cast(node: NodeRef<'g>) -> Option<Self> {
        (node.nt() == 2 && node.prod() == 11).then(|| RuleDecl(node))
    }
    fn node(&self) -> NodeRef<'g> {
        self.0
    }
}

impl<'g> RuleDecl<'g> {
    pub fn kw_rule_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(0, 27) // KW_RULE
    }
    pub fn name_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(1, 2) // NAME
    }
    pub fn eq_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(2, 8) // EQ
    }
    pub fn alt_list(&self) -> Option<AltList<'g>> {
        AltList::cast(self.0.child_node(3)?)
    }
}

/// `decl → KW_RULE NAME EQ PIPE alt_list`
#[derive(Clone, Copy, Debug)]
pub struct RuleDeclBar<'g>(pub NodeRef<'g>);

impl<'g> AstNode<'g> for RuleDeclBar<'g> {
    fn cast(node: NodeRef<'g>) -> Option<Self> {
        (node.nt() == 2 && node.prod() == 12).then(|| RuleDeclBar(node))
    }
    fn node(&self) -> NodeRef<'g> {
        self.0
    }
}

impl<'g> RuleDeclBar<'g> {
    pub fn kw_rule_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(0, 27) // KW_RULE
    }
    pub fn name_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(1, 2) // NAME
    }
    pub fn eq_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(2, 8) // EQ
    }
    pub fn pipe_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(3, 9) // PIPE
    }
    pub fn alt_list(&self) -> Option<AltList<'g>> {
        AltList::cast(self.0.child_node(4)?)
    }
}

/// `token_def → KW_TOKEN NAME EQ tok_pat attrs`
#[derive(Clone, Copy, Debug)]
pub struct TokenDef<'g>(pub NodeRef<'g>);

impl<'g> AstNode<'g> for TokenDef<'g> {
    fn cast(node: NodeRef<'g>) -> Option<Self> {
        (node.nt() == 3 && node.prod() == 13).then(|| TokenDef(node))
    }
    fn node(&self) -> NodeRef<'g> {
        self.0
    }
}

impl<'g> TokenDef<'g> {
    pub fn kw_token_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(0, 20) // KW_TOKEN
    }
    pub fn name_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(1, 2) // NAME
    }
    pub fn eq_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(2, 8) // EQ
    }
    pub fn tok_pat(&self) -> Option<TokPat<'g>> {
        TokPat::cast(self.0.child_node(3)?)
    }
    pub fn attrs(&self) -> Option<Attrs<'g>> {
        Attrs::cast(self.0.child_node(4)?)
    }
}

/// `token_defs` — balanced list of `TokenDef` (envelope L4).
#[derive(Clone, Copy, Debug)]
pub struct TokenDefs<'g>(pub NodeRef<'g>);

impl<'g> AstNode<'g> for TokenDefs<'g> {
    fn cast(node: NodeRef<'g>) -> Option<Self> {
        (node.nt() == 4 && node.prod() == rantlr_grammar::green::LIST_PROD).then(|| TokenDefs(node))
    }
    fn node(&self) -> NodeRef<'g> {
        self.0
    }
}

impl<'g> TokenDefs<'g> {
    pub fn items(&self) -> Vec<TokenDef<'g>> {
        self.0
            .flat_symbol_children()
            .into_iter()
            .filter_map(|c| match c {
                rantlr_grammar::typed::SymbolChild::Node(n) => TokenDef::cast(n),
                _ => None,
            })
            .collect()
    }
}

/// `kw_list` — balanced list of `KwItem` (envelope L4).
#[derive(Clone, Copy, Debug)]
pub struct KwList<'g>(pub NodeRef<'g>);

impl<'g> AstNode<'g> for KwList<'g> {
    fn cast(node: NodeRef<'g>) -> Option<Self> {
        (node.nt() == 5 && node.prod() == rantlr_grammar::green::LIST_PROD).then(|| KwList(node))
    }
    fn node(&self) -> NodeRef<'g> {
        self.0
    }
}

impl<'g> KwList<'g> {
    pub fn items(&self) -> Vec<KwItem<'g>> {
        self.0
            .flat_symbol_children()
            .into_iter()
            .filter_map(|c| match c {
                rantlr_grammar::typed::SymbolChild::Node(n) => KwItem::cast(n),
                _ => None,
            })
            .collect()
    }
}

/// `kw_item` — 2 productions.
#[derive(Clone, Copy, Debug)]
pub enum KwItem<'g> {
    KwName(KwName<'g>),
    KwStr(KwStr<'g>),
}

impl<'g> AstNode<'g> for KwItem<'g> {
    fn cast(node: NodeRef<'g>) -> Option<Self> {
        if node.nt() != 6 {
            return None;
        }
        match node.prod() {
            18 => Some(KwItem::KwName(KwName(node))),
            19 => Some(KwItem::KwStr(KwStr(node))),
            _ => None,
        }
    }
    fn node(&self) -> NodeRef<'g> {
        match self {
            KwItem::KwName(x) => x.0,
            KwItem::KwStr(x) => x.0,
        }
    }
}

/// `kw_item → NAME`
#[derive(Clone, Copy, Debug)]
pub struct KwName<'g>(pub NodeRef<'g>);

impl<'g> AstNode<'g> for KwName<'g> {
    fn cast(node: NodeRef<'g>) -> Option<Self> {
        (node.nt() == 6 && node.prod() == 18).then(|| KwName(node))
    }
    fn node(&self) -> NodeRef<'g> {
        self.0
    }
}

impl<'g> KwName<'g> {
    pub fn name_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(0, 2) // NAME
    }
}

/// `kw_item → STRING`
#[derive(Clone, Copy, Debug)]
pub struct KwStr<'g>(pub NodeRef<'g>);

impl<'g> AstNode<'g> for KwStr<'g> {
    fn cast(node: NodeRef<'g>) -> Option<Self> {
        (node.nt() == 6 && node.prod() == 19).then(|| KwStr(node))
    }
    fn node(&self) -> NodeRef<'g> {
        self.0
    }
}

impl<'g> KwStr<'g> {
    pub fn string_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(0, 4) // STRING
    }
}

/// `tok_pat` — 2 productions.
#[derive(Clone, Copy, Debug)]
pub enum TokPat<'g> {
    PatRegex(PatRegex<'g>),
    PatLit(PatLit<'g>),
}

impl<'g> AstNode<'g> for TokPat<'g> {
    fn cast(node: NodeRef<'g>) -> Option<Self> {
        if node.nt() != 7 {
            return None;
        }
        match node.prod() {
            20 => Some(TokPat::PatRegex(PatRegex(node))),
            21 => Some(TokPat::PatLit(PatLit(node))),
            _ => None,
        }
    }
    fn node(&self) -> NodeRef<'g> {
        match self {
            TokPat::PatRegex(x) => x.0,
            TokPat::PatLit(x) => x.0,
        }
    }
}

/// `tok_pat → PATTERN`
#[derive(Clone, Copy, Debug)]
pub struct PatRegex<'g>(pub NodeRef<'g>);

impl<'g> AstNode<'g> for PatRegex<'g> {
    fn cast(node: NodeRef<'g>) -> Option<Self> {
        (node.nt() == 7 && node.prod() == 20).then(|| PatRegex(node))
    }
    fn node(&self) -> NodeRef<'g> {
        self.0
    }
}

impl<'g> PatRegex<'g> {
    pub fn pattern_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(0, 6) // PATTERN
    }
}

/// `tok_pat → STRING`
#[derive(Clone, Copy, Debug)]
pub struct PatLit<'g>(pub NodeRef<'g>);

impl<'g> AstNode<'g> for PatLit<'g> {
    fn cast(node: NodeRef<'g>) -> Option<Self> {
        (node.nt() == 7 && node.prod() == 21).then(|| PatLit(node))
    }
    fn node(&self) -> NodeRef<'g> {
        self.0
    }
}

impl<'g> PatLit<'g> {
    pub fn string_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(0, 4) // STRING
    }
}

/// `attrs` — balanced list of `Attr` (envelope L4).
#[derive(Clone, Copy, Debug)]
pub struct Attrs<'g>(pub NodeRef<'g>);

impl<'g> AstNode<'g> for Attrs<'g> {
    fn cast(node: NodeRef<'g>) -> Option<Self> {
        (node.nt() == 8 && node.prod() == rantlr_grammar::green::LIST_PROD).then(|| Attrs(node))
    }
    fn node(&self) -> NodeRef<'g> {
        self.0
    }
}

impl<'g> Attrs<'g> {
    pub fn items(&self) -> Vec<Attr<'g>> {
        self.0
            .flat_symbol_children()
            .into_iter()
            .filter_map(|c| match c {
                rantlr_grammar::typed::SymbolChild::Node(n) => Attr::cast(n),
                _ => None,
            })
            .collect()
    }
}

/// `attr` — 2 productions.
#[derive(Clone, Copy, Debug)]
pub enum Attr<'g> {
    AttrPlain(AttrPlain<'g>),
    AttrArgs(AttrArgs<'g>),
}

impl<'g> AstNode<'g> for Attr<'g> {
    fn cast(node: NodeRef<'g>) -> Option<Self> {
        if node.nt() != 9 {
            return None;
        }
        match node.prod() {
            24 => Some(Attr::AttrPlain(AttrPlain(node))),
            25 => Some(Attr::AttrArgs(AttrArgs(node))),
            _ => None,
        }
    }
    fn node(&self) -> NodeRef<'g> {
        match self {
            Attr::AttrPlain(x) => x.0,
            Attr::AttrArgs(x) => x.0,
        }
    }
}

/// `attr → AT NAME`
#[derive(Clone, Copy, Debug)]
pub struct AttrPlain<'g>(pub NodeRef<'g>);

impl<'g> AstNode<'g> for AttrPlain<'g> {
    fn cast(node: NodeRef<'g>) -> Option<Self> {
        (node.nt() == 9 && node.prod() == 24).then(|| AttrPlain(node))
    }
    fn node(&self) -> NodeRef<'g> {
        self.0
    }
}

impl<'g> AttrPlain<'g> {
    pub fn at_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(0, 11) // AT
    }
    pub fn name_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(1, 2) // NAME
    }
}

/// `attr → AT NAME LPAREN arg_list RPAREN`
#[derive(Clone, Copy, Debug)]
pub struct AttrArgs<'g>(pub NodeRef<'g>);

impl<'g> AstNode<'g> for AttrArgs<'g> {
    fn cast(node: NodeRef<'g>) -> Option<Self> {
        (node.nt() == 9 && node.prod() == 25).then(|| AttrArgs(node))
    }
    fn node(&self) -> NodeRef<'g> {
        self.0
    }
}

impl<'g> AttrArgs<'g> {
    pub fn at_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(0, 11) // AT
    }
    pub fn name_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(1, 2) // NAME
    }
    pub fn lparen_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(2, 13) // LPAREN
    }
    pub fn arg_list(&self) -> Option<ArgList<'g>> {
        ArgList::cast(self.0.child_node(3)?)
    }
    pub fn rparen_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(4, 14) // RPAREN
    }
}

/// `arg_list` — balanced list of `Arg` (envelope L4).
#[derive(Clone, Copy, Debug)]
pub struct ArgList<'g>(pub NodeRef<'g>);

impl<'g> AstNode<'g> for ArgList<'g> {
    fn cast(node: NodeRef<'g>) -> Option<Self> {
        (node.nt() == 10 && node.prod() == rantlr_grammar::green::LIST_PROD).then(|| ArgList(node))
    }
    fn node(&self) -> NodeRef<'g> {
        self.0
    }
}

impl<'g> ArgList<'g> {
    pub fn items(&self) -> Vec<Arg<'g>> {
        self.0
            .flat_symbol_children()
            .into_iter()
            .filter_map(|c| match c {
                rantlr_grammar::typed::SymbolChild::Node(n) => Arg::cast(n),
                _ => None,
            })
            .collect()
    }
}

/// `arg` — 3 productions.
#[derive(Clone, Copy, Debug)]
pub enum Arg<'g> {
    ArgName(ArgName<'g>),
    ArgNum(ArgNum<'g>),
    ArgStr(ArgStr<'g>),
}

impl<'g> AstNode<'g> for Arg<'g> {
    fn cast(node: NodeRef<'g>) -> Option<Self> {
        if node.nt() != 11 {
            return None;
        }
        match node.prod() {
            28 => Some(Arg::ArgName(ArgName(node))),
            29 => Some(Arg::ArgNum(ArgNum(node))),
            30 => Some(Arg::ArgStr(ArgStr(node))),
            _ => None,
        }
    }
    fn node(&self) -> NodeRef<'g> {
        match self {
            Arg::ArgName(x) => x.0,
            Arg::ArgNum(x) => x.0,
            Arg::ArgStr(x) => x.0,
        }
    }
}

/// `arg → NAME`
#[derive(Clone, Copy, Debug)]
pub struct ArgName<'g>(pub NodeRef<'g>);

impl<'g> AstNode<'g> for ArgName<'g> {
    fn cast(node: NodeRef<'g>) -> Option<Self> {
        (node.nt() == 11 && node.prod() == 28).then(|| ArgName(node))
    }
    fn node(&self) -> NodeRef<'g> {
        self.0
    }
}

impl<'g> ArgName<'g> {
    pub fn name_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(0, 2) // NAME
    }
}

/// `arg → NUM`
#[derive(Clone, Copy, Debug)]
pub struct ArgNum<'g>(pub NodeRef<'g>);

impl<'g> AstNode<'g> for ArgNum<'g> {
    fn cast(node: NodeRef<'g>) -> Option<Self> {
        (node.nt() == 11 && node.prod() == 29).then(|| ArgNum(node))
    }
    fn node(&self) -> NodeRef<'g> {
        self.0
    }
}

impl<'g> ArgNum<'g> {
    pub fn num_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(0, 3) // NUM
    }
}

/// `arg → STRING`
#[derive(Clone, Copy, Debug)]
pub struct ArgStr<'g>(pub NodeRef<'g>);

impl<'g> AstNode<'g> for ArgStr<'g> {
    fn cast(node: NodeRef<'g>) -> Option<Self> {
        (node.nt() == 11 && node.prod() == 30).then(|| ArgStr(node))
    }
    fn node(&self) -> NodeRef<'g> {
        self.0
    }
}

impl<'g> ArgStr<'g> {
    pub fn string_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(0, 4) // STRING
    }
}

/// `tok_ref` — 2 productions.
#[derive(Clone, Copy, Debug)]
pub enum TokRef<'g> {
    TokName(TokName<'g>),
    TokStr(TokStr<'g>),
}

impl<'g> AstNode<'g> for TokRef<'g> {
    fn cast(node: NodeRef<'g>) -> Option<Self> {
        if node.nt() != 12 {
            return None;
        }
        match node.prod() {
            31 => Some(TokRef::TokName(TokName(node))),
            32 => Some(TokRef::TokStr(TokStr(node))),
            _ => None,
        }
    }
    fn node(&self) -> NodeRef<'g> {
        match self {
            TokRef::TokName(x) => x.0,
            TokRef::TokStr(x) => x.0,
        }
    }
}

/// `tok_ref → NAME`
#[derive(Clone, Copy, Debug)]
pub struct TokName<'g>(pub NodeRef<'g>);

impl<'g> AstNode<'g> for TokName<'g> {
    fn cast(node: NodeRef<'g>) -> Option<Self> {
        (node.nt() == 12 && node.prod() == 31).then(|| TokName(node))
    }
    fn node(&self) -> NodeRef<'g> {
        self.0
    }
}

impl<'g> TokName<'g> {
    pub fn name_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(0, 2) // NAME
    }
}

/// `tok_ref → STRING`
#[derive(Clone, Copy, Debug)]
pub struct TokStr<'g>(pub NodeRef<'g>);

impl<'g> AstNode<'g> for TokStr<'g> {
    fn cast(node: NodeRef<'g>) -> Option<Self> {
        (node.nt() == 12 && node.prod() == 32).then(|| TokStr(node))
    }
    fn node(&self) -> NodeRef<'g> {
        self.0
    }
}

impl<'g> TokStr<'g> {
    pub fn string_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(0, 4) // STRING
    }
}

/// `assoc` — 2 productions.
#[derive(Clone, Copy, Debug)]
pub enum Assoc<'g> {
    AssocLeft(AssocLeft<'g>),
    AssocRight(AssocRight<'g>),
}

impl<'g> AstNode<'g> for Assoc<'g> {
    fn cast(node: NodeRef<'g>) -> Option<Self> {
        if node.nt() != 13 {
            return None;
        }
        match node.prod() {
            33 => Some(Assoc::AssocLeft(AssocLeft(node))),
            34 => Some(Assoc::AssocRight(AssocRight(node))),
            _ => None,
        }
    }
    fn node(&self) -> NodeRef<'g> {
        match self {
            Assoc::AssocLeft(x) => x.0,
            Assoc::AssocRight(x) => x.0,
        }
    }
}

/// `assoc → KW_LEFT`
#[derive(Clone, Copy, Debug)]
pub struct AssocLeft<'g>(pub NodeRef<'g>);

impl<'g> AstNode<'g> for AssocLeft<'g> {
    fn cast(node: NodeRef<'g>) -> Option<Self> {
        (node.nt() == 13 && node.prod() == 33).then(|| AssocLeft(node))
    }
    fn node(&self) -> NodeRef<'g> {
        self.0
    }
}

impl<'g> AssocLeft<'g> {
    pub fn kw_left_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(0, 24) // KW_LEFT
    }
}

/// `assoc → KW_RIGHT`
#[derive(Clone, Copy, Debug)]
pub struct AssocRight<'g>(pub NodeRef<'g>);

impl<'g> AstNode<'g> for AssocRight<'g> {
    fn cast(node: NodeRef<'g>) -> Option<Self> {
        (node.nt() == 13 && node.prod() == 34).then(|| AssocRight(node))
    }
    fn node(&self) -> NodeRef<'g> {
        self.0
    }
}

impl<'g> AssocRight<'g> {
    pub fn kw_right_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(0, 25) // KW_RIGHT
    }
}

/// `prec_ops` — balanced list of `TokRef` (envelope L4).
#[derive(Clone, Copy, Debug)]
pub struct PrecOps<'g>(pub NodeRef<'g>);

impl<'g> AstNode<'g> for PrecOps<'g> {
    fn cast(node: NodeRef<'g>) -> Option<Self> {
        (node.nt() == 14 && node.prod() == rantlr_grammar::green::LIST_PROD).then(|| PrecOps(node))
    }
    fn node(&self) -> NodeRef<'g> {
        self.0
    }
}

impl<'g> PrecOps<'g> {
    pub fn items(&self) -> Vec<TokRef<'g>> {
        self.0
            .flat_symbol_children()
            .into_iter()
            .filter_map(|c| match c {
                rantlr_grammar::typed::SymbolChild::Node(n) => TokRef::cast(n),
                _ => None,
            })
            .collect()
    }
}

/// `alt_list` — balanced list of `Alt` (envelope L4).
#[derive(Clone, Copy, Debug)]
pub struct AltList<'g>(pub NodeRef<'g>);

impl<'g> AstNode<'g> for AltList<'g> {
    fn cast(node: NodeRef<'g>) -> Option<Self> {
        (node.nt() == 15 && node.prod() == rantlr_grammar::green::LIST_PROD).then(|| AltList(node))
    }
    fn node(&self) -> NodeRef<'g> {
        self.0
    }
}

impl<'g> AltList<'g> {
    pub fn items(&self) -> Vec<Alt<'g>> {
        self.0
            .flat_symbol_children()
            .into_iter()
            .filter_map(|c| match c {
                rantlr_grammar::typed::SymbolChild::Node(n) => Alt::cast(n),
                _ => None,
            })
            .collect()
    }
}

/// `alt → NAME COLON syms attrs`
#[derive(Clone, Copy, Debug)]
pub struct Alt<'g>(pub NodeRef<'g>);

impl<'g> AstNode<'g> for Alt<'g> {
    fn cast(node: NodeRef<'g>) -> Option<Self> {
        (node.nt() == 16 && node.prod() == 39).then(|| Alt(node))
    }
    fn node(&self) -> NodeRef<'g> {
        self.0
    }
}

impl<'g> Alt<'g> {
    pub fn name_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(0, 2) // NAME
    }
    pub fn colon_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(1, 10) // COLON
    }
    pub fn syms(&self) -> Option<Syms<'g>> {
        Syms::cast(self.0.child_node(2)?)
    }
    pub fn attrs(&self) -> Option<Attrs<'g>> {
        Attrs::cast(self.0.child_node(3)?)
    }
}

/// `syms` — balanced list of `Sym` (envelope L4).
#[derive(Clone, Copy, Debug)]
pub struct Syms<'g>(pub NodeRef<'g>);

impl<'g> AstNode<'g> for Syms<'g> {
    fn cast(node: NodeRef<'g>) -> Option<Self> {
        (node.nt() == 17 && node.prod() == rantlr_grammar::green::LIST_PROD).then(|| Syms(node))
    }
    fn node(&self) -> NodeRef<'g> {
        self.0
    }
}

impl<'g> Syms<'g> {
    pub fn items(&self) -> Vec<Sym<'g>> {
        self.0
            .flat_symbol_children()
            .into_iter()
            .filter_map(|c| match c {
                rantlr_grammar::typed::SymbolChild::Node(n) => Sym::cast(n),
                _ => None,
            })
            .collect()
    }
}

/// `sym` — 4 productions.
#[derive(Clone, Copy, Debug)]
pub enum Sym<'g> {
    SymName(SymName<'g>),
    SymStr(SymStr<'g>),
    SymLabeled(SymLabeled<'g>),
    SymLabeledStr(SymLabeledStr<'g>),
}

impl<'g> AstNode<'g> for Sym<'g> {
    fn cast(node: NodeRef<'g>) -> Option<Self> {
        if node.nt() != 18 {
            return None;
        }
        match node.prod() {
            42 => Some(Sym::SymName(SymName(node))),
            43 => Some(Sym::SymStr(SymStr(node))),
            44 => Some(Sym::SymLabeled(SymLabeled(node))),
            45 => Some(Sym::SymLabeledStr(SymLabeledStr(node))),
            _ => None,
        }
    }
    fn node(&self) -> NodeRef<'g> {
        match self {
            Sym::SymName(x) => x.0,
            Sym::SymStr(x) => x.0,
            Sym::SymLabeled(x) => x.0,
            Sym::SymLabeledStr(x) => x.0,
        }
    }
}

/// `sym → NAME`
#[derive(Clone, Copy, Debug)]
pub struct SymName<'g>(pub NodeRef<'g>);

impl<'g> AstNode<'g> for SymName<'g> {
    fn cast(node: NodeRef<'g>) -> Option<Self> {
        (node.nt() == 18 && node.prod() == 42).then(|| SymName(node))
    }
    fn node(&self) -> NodeRef<'g> {
        self.0
    }
}

impl<'g> SymName<'g> {
    pub fn name_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(0, 2) // NAME
    }
}

/// `sym → STRING`
#[derive(Clone, Copy, Debug)]
pub struct SymStr<'g>(pub NodeRef<'g>);

impl<'g> AstNode<'g> for SymStr<'g> {
    fn cast(node: NodeRef<'g>) -> Option<Self> {
        (node.nt() == 18 && node.prod() == 43).then(|| SymStr(node))
    }
    fn node(&self) -> NodeRef<'g> {
        self.0
    }
}

impl<'g> SymStr<'g> {
    pub fn string_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(0, 4) // STRING
    }
}

/// `sym → NAME COLON NAME`
#[derive(Clone, Copy, Debug)]
pub struct SymLabeled<'g>(pub NodeRef<'g>);

impl<'g> AstNode<'g> for SymLabeled<'g> {
    fn cast(node: NodeRef<'g>) -> Option<Self> {
        (node.nt() == 18 && node.prod() == 44).then(|| SymLabeled(node))
    }
    fn node(&self) -> NodeRef<'g> {
        self.0
    }
}

impl<'g> SymLabeled<'g> {
    pub fn name_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(0, 2) // NAME
    }
    pub fn colon_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(1, 10) // COLON
    }
    pub fn name_token_2(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(2, 2) // NAME
    }
}

/// `sym → NAME COLON STRING`
#[derive(Clone, Copy, Debug)]
pub struct SymLabeledStr<'g>(pub NodeRef<'g>);

impl<'g> AstNode<'g> for SymLabeledStr<'g> {
    fn cast(node: NodeRef<'g>) -> Option<Self> {
        (node.nt() == 18 && node.prod() == 45).then(|| SymLabeledStr(node))
    }
    fn node(&self) -> NodeRef<'g> {
        self.0
    }
}

impl<'g> SymLabeledStr<'g> {
    pub fn name_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(0, 2) // NAME
    }
    pub fn colon_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(1, 10) // COLON
    }
    pub fn string_token(&self) -> Option<TokenRef<'g>> {
        self.0.child_token(2, 4) // STRING
    }
}
