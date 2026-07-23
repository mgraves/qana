// rg.rg — the .rg grammar surface, described in itself.
//
// This file is the self-hosting fixed point: parsed by the bootstrap
// grammar (crates/rantlr-rg/src/bootstrap.rs) and compiled, it must
// reproduce that bootstrap EXACTLY — token for token, production for
// production, table for table (gated in tests/rg_e2e.rs). Declaration
// order here is the source of truth the bootstrap mirrors.
//
// Since P5 increment 2 the surface uses its OWN EBNF sugar: the list
// rules below are one-liners that desugar to exactly the productions
// the bootstrap declares by hand (naming convention: Empty/More for
// `x*`, First/More for `x+`, None/Some for `x?`).

language Rg

token WS = /\s+/ @trivia
token LINE_COMMENT = /\/\/.*/ @trivia @style(comment)
token NAME = /[\a_][\w_]*/ @specialize @style(variable)
token NUM = /\d+/ @style(number)
token STRING = /"(\\.|[^"\\])*"/ @style(string)
token STRING_UNTERM = /"(\\.|[^"\\])*\\?/ @error @style(string)
token PATTERN = /\/(\\.|[^\/\\])(\\.|[^\/\\])*\// @style(regexp)
token PATTERN_UNTERM = /\/(\\.|[^\/\\])(\\.|[^\/\\])*\\?/ @error @style(regexp)
token EQ = "=" @style(operator)
token PIPE = "|" @style(operator)
token COLON = ":" @style(punctuation)
token AT = "@" @style(punctuation)
token COMMA = "," @style(punctuation)
token LPAREN = "(" @style(bracket)
token RPAREN = ")" @style(bracket)
token LBRACE = "{" @style(bracket)
token RBRACE = "}" @style(bracket)
token STAR = "*" @style(operator)
token PLUS = "+" @style(operator)
token QMARK = "?" @style(operator)
token PERCENT = "%" @style(operator)

// The surface's own reserved words (quoted: they ARE .rg keywords).
keywords NAME = "language" "max_stack" "keywords" "token" "mode" "pair" "prec" "left" "right" "start" "rule"

pair LPAREN RPAREN
pair LBRACE RBRACE

start file

rule file = File: decls

rule decls = decl*

rule decl =
  | LangDecl: "language" NAME
  | MaxStackDecl: "max_stack" NUM
  | KwDecl: "keywords" base:NAME "=" kw_list @ref(base)
  | TokenDecl: token_def
  | ModeDecl: "mode" name:NAME "{" token_defs "}" @outline(name, module)
  | PairDecl: "pair" tok_ref tok_ref
  | PrecDecl: "prec" assoc prec_ops
  | StartDecl: "start" name:NAME @ref(name)
  | RuleDecl: "rule" name:NAME "=" alt_list @def(name) @outline(name, struct)
  | RuleDeclBar: "rule" name:NAME "=" "|" alt_list @def(name) @outline(name, struct)
  | RuleStar: "rule" name:NAME "=" elem "*" rep_sep @def(name) @outline(name, struct)
  | RulePlus: "rule" name:NAME "=" elem "+" rep_sep @def(name) @outline(name, struct)
  | RuleOpt: "rule" name:NAME "=" elem "?" @def(name) @outline(name, struct)

rule token_def = TokenDef: "token" name:NAME "=" tok_pat attrs @def(name) @outline(name, constant)

rule token_defs = token_def*

rule kw_list = kw_item+

rule kw_item =
  | KwName: NAME
  | KwStr: STRING

rule tok_pat =
  | PatRegex: PATTERN
  | PatLit: STRING

rule attrs = attr*

rule attr =
  | AttrPlain: "@" NAME
  | AttrArgs: "@" NAME "(" arg_list ")"

rule arg_list = arg+ % ","

rule arg =
  | ArgName: NAME
  | ArgNum: NUM
  | ArgStr: STRING

rule tok_ref =
  | TokName: name:NAME @ref(name)
  | TokStr: STRING

rule assoc =
  | AssocLeft: "left"
  | AssocRight: "right"

rule prec_ops = tok_ref+

rule alt_list = alt+ % "|"

rule alt = Alt: label:NAME ":" syms attrs

rule syms = sym*

rule sym =
  | SymName: name:NAME @ref(name)
  | SymStr: STRING
  | SymLabeled: label:NAME ":" name:NAME @ref(name)
  | SymLabeledStr: label:NAME ":" STRING
  | SymNameOpt: name:NAME "?" @ref(name)
  | SymNameStar: name:NAME "*" @ref(name)
  | SymNamePlus: name:NAME "+" @ref(name)
  | SymStrOpt: STRING "?"
  | SymStrStar: STRING "*"
  | SymStrPlus: STRING "+"

rule elem =
  | ElemName: name:NAME @ref(name)
  | ElemStr: STRING

rule rep_sep =
  | RepSepNone:
  | RepSepSome: "%" elem
