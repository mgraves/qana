// Modlang — the module tier: exports, imports, visibility.
//
// The toolchain predefines no module system; this grammar DECLARES
// one, Rust-flavored: `pub` exports a definition (`@export`), `use`
// imports one from another file (`@import`), and everything else is
// file-private. Declaring either form activates strict semantics:
// cross-file resolution goes through imports only, and only exported
// names are importable — "not exported" is its own diagnostic,
// distinct from "unresolved". A grammar declaring neither keeps the
// open world.
//
// Open lib.ml and app.ml side by side; make `scale` private in lib.ml
// (delete `pub`) and watch app.ml's import turn into an access error.

language Modlang

token WS           = /\s+/ @trivia
token LINE_COMMENT = /#.*/ @trivia @style(comment)
token NUMBER       = /\d+/ @style(number)
token STRING       = /"(\\.|[^"\\])*"/ @style(string)
token IDENT        = /[\a_][\w_]*/ @specialize @style(variable)
token LPAREN = "(" @style(bracket)
token RPAREN = ")" @style(bracket)
token LBRACE = "{" @style(bracket)
token RBRACE = "}" @style(bracket)
token COLON  = ":" @style(punctuation)
token COMMA  = "," @style(punctuation)
token SEMI   = ";" @style(punctuation)
token ARROW  = "->" @style(operator)
token EQ     = "=" @style(operator)
token PLUS   = "+" @style(operator)

keywords IDENT = pub use as fn let return Num Str

pair LPAREN RPAREN
pair LBRACE RBRACE

prec left "+"

start file

rule file = File: decls @scope(unordered)

rule decls = decl*

rule decl =
  | UseDecl:    "use" name:IDENT ";" @def(name) @import(name) @type(ref) @outline(name)
  | UseAsDecl:  "use" target:IDENT "as" name:IDENT ";" @def(name) @import(target) @type(ref) @outline(name)
  | PubFnDecl:  "pub" "fn" name:IDENT t:fn_tail @def(name) @export @type(def, t) @outline(name, function)
  | FnDecl:     "fn" name:IDENT t:fn_tail @def(name) @type(def, t) @outline(name, function)
  | PubLetDecl: "pub" "let" name:IDENT "=" e:expr ";" @def(name) @export @type(def, e) @outline(name)
  | LetDecl:    "let" name:IDENT "=" e:expr ";" @def(name) @type(def, e) @outline(name)

rule fn_tail = FnTail: "(" p:params ")" "->" rt:ty block @scope @type(fn, p, rt)

rule params = param* % ","

rule param = Param: name:IDENT ":" t:ty @def(name) @type(def, t)

rule ty =
  | TyNum: "Num" @type(Num)
  | TyStr: "Str" @type(Str)

rule block = Block: "{" stmts "}" @scope

rule stmts = stmt*

rule stmt =
  | RetStmt:  "return" e:expr ";" @type(returns, e)
  | ExprStmt: expr ";"

rule expr =
  | AddExpr:  expr "+" expr @type(sig, Num, Num, Num)
  | CallExpr: callee:IDENT "(" a:args ")" @ref(callee, call) @type(apply, a)
  | NumLit:   NUMBER @type(Num)
  | StrLit:   STRING @type(Str)
  | NameRef:  name:IDENT @ref(name) @type(ref)

rule args = expr* % ","
