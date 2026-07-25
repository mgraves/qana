// C89-flavored subset — the envelope exerciser. The goal is not full C
// (yet); it is to find exactly where C fights the envelope and record
// each wall precisely. Deliberate v0 semantics:
//
//  * Preprocessor directives are SYNTAX, not expansion: a `#...` line
//    parses as one PpDirective item — lossless, greppable, styled.
//    Expansion is the meta tier's job (materialized, with provenance),
//    never the parser's (L5).
//  * The typedef-name ambiguity (`T * x;`) is dodged in v0: declaration
//    specifiers are KEYWORDS (int, struct Foo, ...). The plan of record
//    for typedef names is a covering grammar + semantic classification,
//    never lexer feedback.
//  * Remaining cuts, each recorded in the wall list: the typedef-name
//    campaign (typedef heads, casts, sizeof(type), abstract
//    declarators — one covering-grammar effort), labels/goto (the
//    IDENT `:` shift needs default-shift semantics precedence cannot
//    express safely), the comma operator (needs the expression-tier
//    split real C grammars use), and function-like macro adjacency
//    (`F(x)` vs `F (x)` is space-sensitive — invisible to a parser
//    over trivia).
//
// Dangling else is resolved the envelope way: EXPLICITLY. `else` gets a
// precedence, and the else-less `if` is marked lower — the classic yacc
// resolution, but declared instead of defaulted.

language C

token WS           = /\s+/ @trivia
token LINE_COMMENT = /\/\/.*/ @trivia @style(comment)
token BLOCK_OPEN   = "/*" @trivia @push(BLK) @style(comment)
token HASH         = "#" @push(PP, eol) @style(regexp)
token STRING       = /"(\\.|[^"\\])*"/ @style(string)
token CHAR         = /'(\\.|[^'\\])+'/ @style(string)
token NUMBER       = /((0[xX][0-9a-fA-F]+)|(\d+(\.\d*)?([eE]([+-])?\d+)?))[uUlLfF]*/ @style(number)
token IDENT        = /[\a_][\w_]*/ @specialize @style(variable)

mode BLK {
  token B_CLOSE   = "*/" @trivia @pop @style(comment)
  token B_CONTENT = /[^*]+/ @trivia @style(comment)
  token B_STAR    = /\*/ @trivia @style(comment)
}

// The preprocessor as a LINE-BOUNDED mode: `#` enters, end-of-line
// leaves (the mode never reaches another line's entry state, so an
// edit inside a directive can never damage its neighbors). Directives
// are STRUCTURED syntax — `#define NAME body` really defines NAME, and
// uses of it in code resolve — while expansion remains the meta tier's
// job, never the parser's.
mode PP {
  token PP_WS      = /[ \t]+/ @trivia
  token PP_INCLUDE = "include" @style(keyword)
  token PP_DEFINE  = "define" @style(keyword)
  token PP_UNDEF   = "undef" @style(keyword)
  token PP_IFDEF   = "ifdef" @style(keyword)
  token PP_IFNDEF  = "ifndef" @style(keyword)
  token PP_IF      = "if" @style(keyword)
  token PP_ELIF    = "elif" @style(keyword)
  token PP_ELSE    = "else" @style(keyword)
  token PP_ENDIF   = "endif" @style(keyword)
  token PP_HEADER  = /<[^>]+>/ @style(string)
  token PP_STRING  = /"(\\.|[^"\\])*"/ @style(string)
  token PP_NAME    = /[\a_][\w_]*/ @style(variable)
  token PP_NUM     = /\d[\w.]*/ @style(number)
  token PP_ANY     = /./ @style(regexp)
}

token ELLIPSIS = "..." @style(punctuation)
token ARROW  = "->" @style(operator)
token INC    = "++" @style(operator)
token DEC    = "--" @style(operator)
token SHL    = "<<" @style(operator)
token SHR    = ">>" @style(operator)
token LE     = "<=" @style(operator)
token GE     = ">=" @style(operator)
token EQEQ   = "==" @style(operator)
token NEQ    = "!=" @style(operator)
token ANDAND = "&&" @style(operator)
token OROR   = "||" @style(operator)
token ADDEQ  = "+=" @style(operator)
token SUBEQ  = "-=" @style(operator)
token MULEQ  = "*=" @style(operator)
token DIVEQ  = "/=" @style(operator)
token MODEQ  = "%=" @style(operator)
token ANDEQ  = "&=" @style(operator)
token OREQ   = "|=" @style(operator)
token XOREQ  = "^=" @style(operator)
token SHLEQ  = "<<=" @style(operator)
token SHREQ  = ">>=" @style(operator)
token LPAREN = "(" @style(bracket)
token RPAREN = ")" @style(bracket)
token LBRACK = "[" @style(bracket)
token RBRACK = "]" @style(bracket)
token LBRACE = "{" @style(bracket)
token RBRACE = "}" @style(bracket)
token SEMI   = ";" @style(punctuation)
token COMMA  = "," @style(punctuation)
token COLON  = ":" @style(punctuation)
token QMARK  = "?" @style(operator)
token DOT    = "." @style(punctuation)
token EQ     = "=" @style(operator)
token PLUS   = "+" @style(operator)
token MINUS  = "-" @style(operator)
token STAR   = "*" @style(operator)
token SLASH  = "/" @style(operator)
token PERCENT = "%" @style(operator)
token AMP    = "&" @style(operator)
token PIPE   = "|" @style(operator)
token CARET  = "^" @style(operator)
token TILDE  = "~" @style(operator)
token BANG   = "!" @style(operator)
token LT     = "<" @style(operator)
token GT     = ">" @style(operator)

keywords IDENT = auto break case char const continue default do double else enum extern float for goto if int long register return short signed sizeof static struct switch typedef union unsigned void volatile while

pair LPAREN RPAREN
pair LBRACK RBRACK
pair LBRACE RBRACE

// Precedence, lowest binds loosest. Level 1 (`,`) exists as the
// dangling-else anchor; the comma OPERATOR is a v0 cut.
prec left ","
prec right "else"
prec right "=" "+=" "-=" "*=" "/=" "%=" "&=" "|=" "^=" "<<=" ">>="
prec right "?"
prec left "||"
prec left "&&"
prec left "|"
prec left "^"
prec left "&"
prec left "==" "!="
prec left "<" ">" "<=" ">="
prec left "<<" ">>"
prec left "+" "-"
prec left "*" "/" "%"
prec right "!" "~" "++" "--" "sizeof"
prec left "." "->" "(" "["

start translation_unit

rule translation_unit = TranslationUnit: external_decls

rule external_decls = external_decl*

rule external_decl =
  | FunctionDef: decl_specs declarator compound_stmt
  | Declaration: decl_specs init_declarators ";"
  | BareDecl:    decl_specs ";"
  | PpItem:      pp_directive

// ---- declaration specifiers (keywords only in v0) ----

rule pp_directive =
  | PpInclude:    "#" PP_INCLUDE PP_HEADER
  | PpIncludeStr: "#" PP_INCLUDE PP_STRING
  | PpDefine:     "#" PP_DEFINE name:PP_NAME pp_tokens @def(name) @outline(name, constant)
  | PpUndef:      "#" PP_UNDEF name:PP_NAME @ref(name)
  | PpIfdef:      "#" PP_IFDEF name:PP_NAME @ref(name)
  | PpIfndef:     "#" PP_IFNDEF name:PP_NAME @ref(name)
  | PpIf:         "#" PP_IF pp_tokens
  | PpElif:       "#" PP_ELIF pp_tokens
  | PpElse:       "#" PP_ELSE
  | PpEndif:      "#" PP_ENDIF

rule pp_tokens = pp_tok*

rule pp_tok =
  | PpTokName: PP_NAME
  | PpTokNum:  PP_NUM
  | PpTokStr:  PP_STRING
  | PpTokHdr:  PP_HEADER
  | PpTokAny:  PP_ANY

rule decl_specs = decl_spec+

rule decl_spec =
  | SpecTypedef:  "typedef"
  | SpecExtern:   "extern"
  | SpecStatic:   "static"
  | SpecAuto:     "auto"
  | SpecRegister: "register"
  | SpecConst:    "const"
  | SpecVolatile: "volatile"
  | SpecVoid:     "void"
  | SpecChar:     "char"
  | SpecShort:    "short"
  | SpecInt:      "int"
  | SpecLong:     "long"
  | SpecFloat:    "float"
  | SpecDouble:   "double"
  | SpecSigned:   "signed"
  | SpecUnsigned: "unsigned"
  | SpecStruct:   struct_spec
  | SpecEnum:     enum_spec

rule struct_spec =
  | StructDef:  su tag:IDENT su_body @def(tag) @outline(tag, struct)
  | StructAnon: su su_body
  | StructRef:  su tag:IDENT @ref(tag)

rule su =
  | SuStruct: "struct"
  | SuUnion:  "union"

rule su_body = SuBody: "{" field_decls "}" @scope

rule field_decls = field_decl*

rule field_decl =
  | FieldDecl: decl_specs init_declarators ";"
  | BitField:  decl_specs declarator ":" expr ";"

rule enum_spec =
  | EnumDef:  "enum" tag:IDENT enum_body @def(tag) @outline(tag, constant)
  | EnumAnon: "enum" enum_body
  | EnumRef:  "enum" tag:IDENT @ref(tag)

rule enum_body = EnumBody: "{" enumerators "}"

rule enumerators = enumerator+ % ","

rule enumerator =
  | Enumerator:     name:IDENT @def(name)
  | EnumeratorInit: name:IDENT "=" expr @def(name)

// ---- declarators (the famous nesting: int (*f[3])(void) parses) ----

rule init_declarators = init_declarator+ % ","

rule init_declarator =
  | InitDecl:     declarator
  | InitDeclInit: declarator "=" initializer

rule declarator = Declarator: ptrs direct_declarator

rule ptrs = ptr*

rule ptr = Ptr: "*" type_quals

rule type_quals = type_qual*

rule type_qual =
  | QualConst:    "const"
  | QualVolatile: "volatile"

rule direct_declarator =
  | DName:  name:IDENT @def(name)
  | DParen: "(" declarator ")"
  | DArray: direct_declarator "[" opt_expr "]"
  | DFunc:  direct_declarator "(" params ")"

rule params =
  | ParamsNone:
  | ParamsVoid: "void"
  | ParamsList: param_list

rule param_list = param+ % ","

rule param =
  | Param:    decl_specs declarator
  | VarArgs:  ELLIPSIS

rule initializer =
  | InitExpr:   expr
  | InitList:   "{" init_items "}"
  | Designated: "." IDENT "=" initializer

rule init_items = initializer+ % ","

// ---- statements ----

rule compound_stmt = Compound: "{" block_items "}" @scope

rule block_items = block_item*

rule block_item =
  | ItemDecl: decl_specs init_declarators ";"
  | ItemStmt: stmt

rule stmt =
  | ExprStmt:    expr ";"
  | EmptyStmt:   ";"
  | CompoundS:   compound_stmt
  | IfStmt:      "if" "(" expr ")" stmt @precedence(",")
  | IfElseStmt:  "if" "(" expr ")" stmt "else" stmt
  | WhileStmt:   "while" "(" expr ")" stmt
  | DoStmt:      "do" stmt "while" "(" expr ")" ";"
  | ForStmt:     "for" "(" opt_expr ";" opt_expr ";" opt_expr ")" stmt
  | ReturnStmt:  "return" opt_expr ";"
  | BreakStmt:   "break" ";"
  | ContinueStmt: "continue" ";"
  | SwitchStmt:  "switch" "(" expr ")" stmt
  | CaseStmt:    "case" expr ":" stmt
  | DefaultStmt: "default" ":" stmt
  | PpStmt:      pp_directive

rule opt_expr = expr?

// ---- expressions ----

rule expr =
  | Assign:    expr "=" expr
  | AddAssign: expr "+=" expr
  | SubAssign: expr "-=" expr
  | MulAssign: expr "*=" expr
  | DivAssign: expr "/=" expr
  | ModAssign: expr "%=" expr
  | AndAssign: expr "&=" expr
  | OrAssign:  expr "|=" expr
  | XorAssign: expr "^=" expr
  | ShlAssign: expr "<<=" expr
  | ShrAssign: expr ">>=" expr
  | Cond:      expr "?" expr ":" expr @precedence("?")
  | LogOr:     expr "||" expr
  | LogAnd:    expr "&&" expr
  | BitOr:     expr "|" expr
  | BitXor:    expr "^" expr
  | BitAnd:    expr "&" expr
  | EqExpr:    expr "==" expr
  | NeExpr:    expr "!=" expr
  | LtExpr:    expr "<" expr
  | GtExpr:    expr ">" expr
  | LeExpr:    expr "<=" expr
  | GeExpr:    expr ">=" expr
  | ShlExpr:   expr "<<" expr
  | ShrExpr:   expr ">>" expr
  | AddExpr:   expr "+" expr
  | SubExpr:   expr "-" expr
  | MulExpr:   expr "*" expr
  | DivExpr:   expr "/" expr
  | ModExpr:   expr "%" expr
  | Neg:       "-" expr @precedence("!")
  | Pos:       "+" expr @precedence("!")
  | Not:       "!" expr
  | BitNot:    "~" expr
  | Deref:     "*" expr @precedence("!")
  | AddrOf:    "&" expr @precedence("!")
  | PreInc:    "++" expr
  | PreDec:    "--" expr
  | SizeofE:   "sizeof" expr
  | Call:      expr "(" args ")" @precedence(".")
  | Index:     expr "[" expr "]" @precedence(".")
  | Member:    expr "." IDENT
  | Arrow:     expr "->" IDENT
  | PostInc:   expr "++" @precedence(".")
  | PostDec:   expr "--" @precedence(".")
  | NameRef:   name:IDENT @ref(name)
  | NumLit:    NUMBER
  | CharLit:   CHAR
  | StrLit:    STRING
  | ParenExpr: "(" expr ")"

rule args = expr* % ","
