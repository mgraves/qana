// Structlang — structs, functions, and DOCUMENT-LEVEL types.
//
// `@type(deftype)` on StructDecl means each `struct` declaration in a
// DOCUMENT introduces a new type, named by its name and identified by
// its declaration site (two `struct T`s in different scopes are
// different types — scoping and shadowing come from the binding tier).
// `@type(named)` on TyName/NewExpr makes annotations and `new T`
// denote those types. The toolchain still ships zero types.

language Structlang

token WS           = /\s+/ @trivia
token LINE_COMMENT = /#.*/ @trivia @style(comment)
token NUMBER       = /\d+(\.\d+)?/ @style(number)
token STRING       = /"(\\.|[^"\\])*"/ @style(string)
token IDENT        = /[\a_][\w_]*/ @specialize @style(variable)
token LBRACE = "{" @style(bracket)
token RBRACE = "}" @style(bracket)
token LPAREN = "(" @style(bracket)
token RPAREN = ")" @style(bracket)
token COLON  = ":" @style(punctuation)
token COMMA  = "," @style(punctuation)
token SEMI   = ";" @style(punctuation)
token DOT    = "." @style(punctuation)
token ARROW  = "->" @style(operator)
token EQ     = "=" @style(operator)
token PLUS   = "+" @style(operator)

// `Num` and `Str` are KEYWORDS of Structlang: the closed part of its
// type vocabulary maps 1:1 onto grammar atoms. Struct names are the
// OPEN part — and that is where v0 will go silent.
keywords IDENT = struct fn let return new Num Str

pair LPAREN RPAREN
pair LBRACE RBRACE

prec left "+"
prec left "."

start file

// Declaration language: top-level names see each other in any order.
rule file = File: decls @scope(unordered)

rule decls = decl*

rule decl =
  | StructDecl: "struct" name:IDENT struct_body @def(name) @outline(name, struct) @type(deftype)
  | FnDecl:     "fn" name:IDENT t:fn_tail @def(name) @outline(name, function) @type(def, t)
  | LetDecl:    "let" name:IDENT ti:typed_init ";" @def(name) @outline(name) @type(def, ti)

rule struct_body = StructBody: "{" fields "}" @scope

rule fields = field* % ","

// Fields are typed definitions in the struct's own scope.
rule field = Field: name:IDENT ":" t:ty @def(name) @type(def, t)

// The fn's params + return + body share one scope: params are visible
// in the block and sealed from the top level. The tail's type is the
// declared RETURN type — so the fn NAME carries it (via @type(def, t)
// above), and every call site gets it back through @type(ref).
rule fn_tail = FnTail: "(" params ")" "->" rt:ty block @scope @type(of, rt)

rule params = param* % ","

rule param = Param: name:IDENT ":" t:ty @def(name) @type(def, t)

// A type EXPRESSION. The closed vocabulary types itself; a struct name
// resolves (navigation works!) but its DEF carries no type in v0.
rule ty =
  | TyNum:  "Num" @type(Num)
  | TyStr:  "Str" @type(Str)
  | TyName: name:IDENT @ref(name) @type(named)

// The annotation-agreement trick: `: T = e` is one node whose sig
// unifies the annotation with the initializer — `let x: Num = "s"`
// is reported on the exact initializer.
rule typed_init = TypedInit: ":" t:ty "=" e:expr @type(sig, t, t, t)

rule block = Block: "{" stmts "}" @scope

rule stmts = stmt*

rule stmt =
  | LetStmt:  "let" name:IDENT ti:typed_init ";" @def(name) @type(def, ti)
  | RetStmt:  "return" expr ";"
  | ExprStmt: expr ";"

rule expr =
  | AddExpr:   expr "+" expr @type(sig, Num, Num, Num)
  | FieldExpr: expr "." IDENT
  | CallExpr:  callee:IDENT "(" args ")" @ref(callee, call) @type(ref)
  | NewExpr:   "new" name:IDENT @ref(name) @type(named)
  | NumLit:    NUMBER @type(Num)
  | StrLit:    STRING @type(Str)
  | NameRef:   name:IDENT @ref(name) @type(ref)
  | ParenExpr: "(" e:expr ")" @type(of, e)

rule args = expr* % ","
