// {NAME} — a starter grammar. This one file IS the language: tokens,
// keywords, precedence, syntax, highlighting, outline, and navigation.
//
// Edit it and run `rantlr check` — or just save it with the editor
// extension running, and open documents re-derive in milliseconds.

language {NAME}

// ---------------------------------------------------------------------
// Lexical layer
//
// Every token is a line-local pattern (envelope rule L1): no token may
// span a newline, which is what lets the engine restart lexing at any
// line instead of re-scanning the file.
// ---------------------------------------------------------------------

token WS           = /\s+/                    @trivia
token LINE_COMMENT = /#.*/                    @trivia @style(comment)
token STRING       = /"(\\.|[^"\\])*"/        @style(string)
token NUMBER       = /\d+(\.\d+)?/            @style(number)
token IDENT        = /[\a_][\w_]*/ @specialize @style(variable)

token LPAREN = "(" @style(bracket)
token RPAREN = ")" @style(bracket)
token LBRACE = "{" @style(bracket)
token RBRACE = "}" @style(bracket)
token COMMA  = "," @style(punctuation)
token SEMI   = ";" @style(punctuation)
token EQ     = "=" @style(operator)
token PLUS   = "+" @style(operator)
token MINUS  = "-" @style(operator)
token STAR   = "*" @style(operator)
token SLASH  = "/" @style(operator)

// `@specialize` on IDENT means matched identifier text is looked up
// here. Each keyword gets its own terminal, so rules below can name
// them directly — and a keyword of one language stays an ordinary
// identifier inside another when languages are composed.
keywords IDENT = let print

// Bracket pairs drive folding and the incremental skeleton.
pair LPAREN RPAREN
pair LBRACE RBRACE

// Precedence settles what would otherwise be ambiguity. Delete these
// two lines and `rantlr check` refuses the grammar, printing a concrete
// input that parses two ways. That refusal is the whole point: the
// ambiguity is caught here, not at runtime on someone's document.
prec left "+" "-"
prec left "*" "/"

start program

// ---------------------------------------------------------------------
// Syntax layer
//
// `stmt*` is EBNF sugar for an auto-balanced list (envelope rule L4):
// the tree holds a shallow balanced run instead of a cons spine, so a
// 10,000-statement file still edits in constant time.
// ---------------------------------------------------------------------

rule program = Program: stmts

rule stmts = stmt*

rule stmt =
  | LetStmt:   "let" name:IDENT "=" e:expr ";" @def(name) @outline(name) @type(def, e)
  | PrintStmt: "print" args ";"
  | BlockStmt: block

// `expr+ % ","` is a comma-separated list of one or more.
rule args = expr+ % ","

// `@scope` makes a block its own namespace: names declared inside are
// invisible outside, and go-to-definition respects that.
rule block = Block: "{" stmts "}" @scope

// ---------------------------------------------------------------------
// The type tier
//
// The toolchain ships NO types. `Num` and `Str` below are THIS
// grammar's invented vocabulary; the rules are data; the checker is
// generic. `@type(sig, Num, Num, Num)` reads like a function signature
// over the alternative's rule symbols; `@type(def, e)` gives the
// defined name the type of its initializer; `@type(ref)` flows it back
// through every use, via the same resolution go-to-definition uses.
// Try `let x = 1 + "one";` in the sample — the mismatch is reported on
// the exact operand, and deleting these annotations deletes the tier.
// ---------------------------------------------------------------------

rule expr =
  | AddExpr:   expr "+" expr @type(sig, Num, Num, Num)
  | SubExpr:   expr "-" expr @type(sig, Num, Num, Num)
  | MulExpr:   expr "*" expr @type(sig, Num, Num, Num)
  | DivExpr:   expr "/" expr @type(sig, Num, Num, Num)
  | ParenExpr: "(" e:expr ")" @type(of, e)
  | NumLit:    NUMBER @type(Num)
  | StrLit:    STRING @type(Str)
  | NameRef:   name:IDENT @ref(name) @type(ref)
