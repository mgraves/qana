// chartlang.rg — the ENTIRE language definition, live-editable.
//
// Open demo.cl next to this file and edit anything here: tokens,
// keywords, operators, precedence, whole productions. On save, the
// rantlr pipeline (envelope lints, LR tables, styles, binding) rebuilds
// in milliseconds and open documents re-colorize. This file is ALSO
// served by rantlr itself — highlighting, outline, go-to-definition on
// rule names, and live envelope diagnostics as you type.
//
// Things to try:
//   * Add a keyword (say `async`) to the `keywords` line.
//   * Add an operator: declare `token CARET = "^" @style(operator)`,
//     give it a precedence line, and add
//     `| PowExpr: expr "^" expr` to `rule expr`.
//   * Delete the `prec` lines — the envelope REFUSES the now-ambiguous
//     expression grammar, pointing at a production with a concrete
//     counterexample input. Undo, and the language comes back.

language Demo
max_stack 8

token WS = /\s+/ @trivia
token LINE_COMMENT = /\/\/.*/ @trivia @style(comment)
token BLOCK_OPEN = "/*" @trivia @push(BLOCK) @style(comment)
token STRING = /"(\\.|[^"\\])*"/ @style(string)
token STRING_UNTERM = /"(\\.|[^"\\])*\\?/ @error @style(string)
token NUMBER = /\d+(\.\d+)?/ @style(number)
token IDENT = /[\a_][\w_]*/ @specialize @style(variable)
token LPAREN = "(" @style(bracket)
token RPAREN = ")" @style(bracket)
token LBRACKET = "[" @style(bracket)
token RBRACKET = "]" @style(bracket)
token LBRACE = "{" @style(bracket)
token RBRACE = "}" @style(bracket)
token PLUS = "+" @style(operator)
token MINUS = "-" @style(operator)
token STAR = "*" @style(operator)
token SLASH = "/" @style(operator)
token SEMI = ";" @style(punctuation)
token COMMA = "," @style(punctuation)
token EQ = "=" @style(operator)
token PUNCT = /[!-\/:-@\[-`{-~]/ @style(punctuation)

keywords IDENT = fn let if else while for return struct enum impl match true false mut pub use

// Nested block comments: BLOCK pushes itself, bounded by max_stack (L2).
mode BLOCK {
  token B_OPEN = "/*" @trivia @push(BLOCK) @style(comment)
  token B_CLOSE = "*/" @trivia @pop @style(comment)
  token B_CONTENT = /[^*\/]+/ @trivia @style(comment)
  token B_MISC = /[*\/]/ @trivia @style(comment)
}

pair LPAREN RPAREN
pair LBRACKET RBRACKET
pair LBRACE RBRACE

prec left "+" "-"
prec left "*" "/"

start file

rule file = File: stmts

rule stmts =
  | StmtsEmpty:
  | StmtsMore: stmts stmt

rule stmt =
  | LetStmt: "let" name:IDENT "=" expr ";" @def(name) @outline(name)
  | ExprStmt: expr ";"
  | BlockStmt: block
  | IfStmt: "if" "(" expr ")" block
  | IfElseStmt: "if" "(" expr ")" block "else" block

rule block = Block: "{" stmts "}" @scope

rule expr =
  | AddExpr: expr "+" expr
  | SubExpr: expr "-" expr
  | MulExpr: expr "*" expr
  | DivExpr: expr "/" expr
  | NumLit: NUMBER
  | StrLit: STRING
  | NameRef: name:IDENT @ref(name)
  | CallExpr: callee:IDENT "(" args ")" @ref(callee, call)
  | ParenExpr: "(" expr ")"
  | ListExpr: "[" args "]"

rule args =
  | ArgsEmpty:
  | ArgsSome: args_ne

rule args_ne =
  | ArgFirst: expr
  | ArgMore: args_ne "," expr
