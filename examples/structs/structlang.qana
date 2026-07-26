// Structlang — structs, functions, and DOCUMENT-LEVEL types.
//
// `@type(deftype, b)` on StructDecl means each `struct` declaration
// in a DOCUMENT introduces a new type (identity = declaration site;
// scoping and shadowing come from the binding tier), and the typed
// defs inside its body are the type's MEMBERS — fields are ordinary
// definitions. `@type(named)` makes annotations and `new T` denote the
// type; `@type(member, b, m)` makes `p.x` look `x` up in `p`'s type.
// The toolchain still ships zero types.

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
token BANG   = "!" @style(operator)
token FATARROW = "=>" @style(operator)

// `Num` and `Str` are KEYWORDS of Structlang: the closed part of its
// type vocabulary maps 1:1 onto grammar atoms. Struct names are the
// OPEN part — and that is where v0 will go silent.
keywords IDENT = struct fn let return new macro Num Str

pair LPAREN RPAREN
pair LBRACE RBRACE

prec left "+"
prec left "."

start file

// Declaration language: top-level names see each other in any order.
rule file = File: decls @scope(unordered)

rule decls = decl*

rule decl =
  | StructDecl: "struct" name:IDENT b:struct_body @def(name) @outline(name, struct) @type(deftype, b)
  | FnDecl:     "fn" name:IDENT t:fn_tail @def(name) @outline(name, function) @type(def, t)
  | LetDecl:    "let" name:IDENT ti:typed_init ";" @def(name) @outline(name) @type(def, ti)
  // The META tier meets the TYPE tier. A macro's body is a real expr;
  // its two parameters bind per-MEMBER when spliced through @reflect.
  | MacroDecl:  "macro" name:IDENT "(" ps:mparams ")" "=>" "{" body:expr "}" @def(name) @macro(ps, body) @scope @outline(name, function)

rule mparams = mparam* % ","

rule mparam = MParam: name:IDENT @def(name)

rule struct_body = StructBody: "{" fields "}" @scope

rule fields = field* % ","

// Fields are typed definitions in the struct's own scope.
rule field = Field: name:IDENT ":" t:ty @def(name) @type(def, t)

// The fn's params + return + body share one scope: params are visible
// in the block and sealed from the top level. `@type(fn, p, rt)` gives
// the tail an ARROW type assembled from the param defs and the return
// annotation; `@type(def, t)` above hands it to the fn NAME, so calls
// check arity and arguments (`@type(apply, a)`) and `return` statements
// check against the declaration (`@type(returns, e)`).
rule fn_tail = FnTail: "(" p:params ")" "->" rt:ty block @scope @type(fn, p, rt)

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
  | RetStmt:  "return" e:expr ";" @type(returns, e)
  | ExprStmt: expr ";"

rule expr =
  | AddExpr:   expr "+" expr @type(sig, Num, Num, Num)
  | FieldExpr: b:expr "." m:IDENT @type(member, b, m)
  | CallExpr:  callee:IDENT "(" a:args ")" @ref(callee, call) @type(apply, a)
  | NewExpr:   "new" name:IDENT @ref(name) @type(named)
  // REFLECTION: `sum!{Point}` substitutes the macro's body once per
  // declared member of Point — parameter 1 is the member's NAME
  // (matched at member positions, `origin.f`), parameter 2 its TYPE
  // (an ordinary binding ref, `new t`) — joined by the declared "+".
  // The member map IS the type tier's own declarations; the engine
  // brings no schema of its own.
  // An ordinary macro call, `m!(args)`. Substituting at a member
  // position needs a NAME, so `pick!(x)` works and `pick!(1 + 2)` is
  // refused rather than emitted as `origin.(1 + 2)`.
  | Splice:    name:IDENT "!" "(" a:args ")" @ref(name) @splice(name, a)
  | Reflect:   name:IDENT "!" "{" ty:IDENT "}" @ref(name) @ref(ty) @splice(name) @reflect(ty, " + ")
  // The same reflection with a richer FACET list: this macro's three
  // parameters bind to each member's name, its declared type, and its
  // index. `owner` and `count` are available too — every facet is
  // read from the type tier's declarations or computed from them.
  | ReflectAt:  name:IDENT "!" "!" "{" ty:IDENT "}" @ref(name) @ref(ty) @splice(name) @reflect(ty, " + ", name, type, index)
  | NumLit:    NUMBER @type(Num)
  | StrLit:    STRING @type(Str)
  | NameRef:   name:IDENT @ref(name) @type(ref)
  | ParenExpr: "(" e:expr ")" @type(of, e)

rule args = expr* % ","
