// C89-flavored subset — the envelope exerciser. The goal is not full C
// (yet); it is to find exactly where C fights the envelope and record
// each wall precisely. Deliberate v0 semantics:
//
//  * Preprocessor directives are SYNTAX, not expansion: a `#...` line
//    parses as one PpDirective item — lossless, greppable, styled.
//    Expansion is the meta tier's job (materialized, with provenance),
//    never the parser's (L5).
//  * Typedef-name heads work WITHOUT lexer feedback, split by context:
//    file scope / params / fields take full typedef'd declarators, and
//    block level rides the EXPRESSION-TIER SPLIT: statement
//    expressions (`s_expr`, below) surrender exactly one derivation —
//    a bare identifier as the left operand of `*` — so a statement
//    beginning `IDENT *` is a DECLARATION, deterministically. That is
//    C's own reading whenever the head is a typedef; the flip side is
//    pinned: `x * y + z;` (a multiplication STATEMENT led by a bare
//    name) is refused, because it now reads as a broken declaration.
//  * Casts and sizeof(type) take KEYWORD-led type names in full
//    (`(unsigned long *)p`, `sizeof(struct point *)`). Bare typedef
//    names converge instead of conflicting: `sizeof(word_t)` parses
//    as sizeof-of-a-VALUE (the ref still resolves), and a bare-name
//    cast is written call-shaped, `(word_t)(x)` — the classic C
//    unification. `(word_t) x` (juxtaposition) is refused. The R/R
//    core these convergences dodge is machine-provable: ask for
//    `sizeof "(" IDENT ptr+ ")"` and the certifier refuses with a
//    shift/reduce counterexample on `*` (see the exerciser gate).
//  * Labels/goto: the statement tier keeps `:` out of the bare name's
//    lookahead, so `name: stmt` shifts uncontested, and the hoisted
//    label namespace makes forward gotos resolve. Labels are
//    block-scoped (not C's function-scoped): goto INTO a nested
//    block is the pinned residue. Function-like macro adjacency is
//    space-sensitive and invisible over trivia (meta-tier leaning).
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
  // Typedef-name heads, the COVERING way — no lexer feedback, ever.
  // File scope has no expression statements, so an IDENT-led item is
  // unambiguously a typedef'd declaration: FULL declarators, pointers
  // included. The head is a REFERENCE — uses of `word_t` navigate to
  // its typedef, and an unknown type is a "cannot find" diagnostic.
  | TdFunctionDef: head:IDENT declarator compound_stmt @ref(head)
  | TdDeclaration: head:IDENT init_declarators ";" @ref(head)
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

// Tags live in C's TAG namespace — declared with @ns(tag), which also
// declares its ORDERING: named namespaces resolve hoisted (order-free)
// in every scope, so `typedef struct point point_t;` may precede the
// struct definition — per-NAMESPACE ordering, while values stay
// define-before-use.
rule struct_spec =
  | StructDef:  su tag:IDENT su_body @def(tag) @ns(tag) @outline(tag, struct)
  | StructAnon: su su_body
  | StructRef:  su tag:IDENT @ref(tag) @ns(tag)

rule su =
  | SuStruct: "struct"
  | SuUnion:  "union"

rule su_body = SuBody: "{" field_decls "}" @scope

rule field_decls = field_decl*

rule field_decl =
  | FieldDecl: decl_specs init_declarators ";"
  | BitField:  decl_specs declarator ":" expr ";"
  // Struct fields have no expression competitors either: full
  // typedef'd fields, pointers included.
  | TdField:   head:IDENT init_declarators ";" @ref(head)

rule enum_spec =
  | EnumDef:  "enum" tag:IDENT enum_body @def(tag) @ns(tag) @outline(tag, constant)
  | EnumAnon: "enum" enum_body
  | EnumRef:  "enum" tag:IDENT @ref(tag) @ns(tag)

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
  | ParamsList: param_list

rule param_list = param+ % ","

// Parameters have no expression competitors, so typedef'd params get
// the FULL treatment — named, pointered, and abstract. The abstract
// forms share the `ptrs` prefix with the named ones: after the
// pointers, an IDENT or `(` continues a declarator, while `)` or `,`
// reduces the abstract form — disjoint lookaheads, LR(1)-clean.
rule param =
  | Param:       decl_specs declarator
  | ParamAbs:    decl_specs ptrs
  | TdParam:     head:IDENT declarator @ref(head)
  | TdParamAbs:  head:IDENT ptrs @ref(head)
  | VarArgs:     ELLIPSIS

// Block-level typedef'd declarators: pointered (the expression-tier
// split freed the `IDENT *` prefix), but still no leading `(` — a
// parenthesized declarator head collides with call statements, so
// block-local function POINTERS remain file-scope-only.
rule n_init_declarators = n_init_declarator+ % ","

rule n_init_declarator =
  | NInitDecl:     n_declarator
  | NInitDeclInit: n_declarator "=" initializer

rule n_declarator = NDeclarator: ptrs n_direct

rule n_direct =
  | NDName:  name:IDENT @def(name)
  | NDArray: n_direct "[" opt_expr "]"
  | NDFunc:  n_direct "(" params ")"

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
  // Block level DOES have expression statements — but those use the
  // RESTRICTED tier (s_cexpr), which cannot begin `IDENT *`. So a
  // typedef'd local takes pointered declarators after all: `T *p;`
  // shifts into `ptrs` with no competitor, because the one expression
  // that wanted those two tokens (bare-name multiplication) is the
  // derivation the statement tier gave up.
  | TdLocal: head:IDENT n_init_declarators ";" @ref(head)
  | ItemStmt: stmt

rule stmt =
  | ExprStmt:    s_cexpr ";"
  | EmptyStmt:   ";"
  | CompoundS:   compound_stmt
  // Labels ride BOTH campaigns at once: the statement tier keeps `:`
  // out of the bare name's lookahead (a statement-leading IDENT
  // followed by `:` shifts here, uncontested), and the label
  // namespace is hoisted (@ns) so a forward `goto done;` resolves.
  // Residue: labels are BLOCK-scoped here, not function-scoped — a
  // goto cannot jump INTO a nested block (see the exerciser gate).
  | LabelStmt:   name:IDENT ":" stmt @def(name) @ns(label)
  | GotoStmt:    "goto" name:IDENT ";" @ref(name) @ns(label)
  | IfStmt:      "if" "(" cexpr ")" stmt @precedence(",")
  | IfElseStmt:  "if" "(" cexpr ")" stmt "else" stmt
  | WhileStmt:   "while" "(" cexpr ")" stmt
  | DoStmt:      "do" stmt "while" "(" cexpr ")" ";"
  | ForStmt:     "for" "(" opt_cexpr ";" opt_cexpr ";" opt_cexpr ")" stmt
  | ReturnStmt:  "return" opt_cexpr ";"
  | BreakStmt:   "break" ";"
  | ContinueStmt: "continue" ";"
  | SwitchStmt:  "switch" "(" cexpr ")" stmt
  | CaseStmt:    "case" expr ":" stmt
  | DefaultStmt: "default" ":" stmt
  | PpStmt:      pp_directive

rule opt_expr = expr?

// ---- expressions ----
//
// THE EXPRESSION-TIER SPLIT. Three tiers, one derivation surrendered:
//
//   cexpr    the comma operator — above assignment, and only where C
//            allows it (parens, conditions, for clauses, return,
//            statements). Call arguments and initializer lists keep
//            their SEPARATOR commas at the `expr` tier, which is why
//            `f(a, b)` stays two arguments.
//   expr     assignment tier and below — unchanged, used everywhere
//            an operand is wanted.
//   s_expr   the STATEMENT expression. Identical to expr except for
//            one missing production: a bare identifier may not be the
//            left operand of `*`. Every other left spine is present
//            (`f() * y;`, `(x) * y;`, `5 * y;` all parse). The freed
//            two-token prefix `IDENT *` is what lets typedef'd
//            pointer locals be declarations, deterministically.
//
// The statement tier is a left-spine mirror: each left-recursive
// production takes `s_expr` on the left and plain `expr` on the
// right, because the restriction is only about how a STATEMENT may
// BEGIN. Prefix forms take full `expr` operands for the same reason.

rule s_cexpr =
  | ScFirst: s_expr
  | ScComma: s_cexpr "," expr

rule s_expr =
  | SeName: name:IDENT @ref(name)
  | SeWide: s_wide

rule s_wide =
  | SAssign:    s_expr "=" expr
  | SAddAssign: s_expr "+=" expr
  | SSubAssign: s_expr "-=" expr
  | SMulAssign: s_expr "*=" expr
  | SDivAssign: s_expr "/=" expr
  | SModAssign: s_expr "%=" expr
  | SAndAssign: s_expr "&=" expr
  | SOrAssign:  s_expr "|=" expr
  | SXorAssign: s_expr "^=" expr
  | SShlAssign: s_expr "<<=" expr
  | SShrAssign: s_expr ">>=" expr
  | SCond:      s_expr "?" expr ":" expr @precedence("?")
  | SLogOr:     s_expr "||" expr
  | SLogAnd:    s_expr "&&" expr
  | SBitOr:     s_expr "|" expr
  | SBitXor:    s_expr "^" expr
  | SBitAnd:    s_expr "&" expr
  | SEqExpr:    s_expr "==" expr
  | SNeExpr:    s_expr "!=" expr
  | SLtExpr:    s_expr "<" expr
  | SGtExpr:    s_expr ">" expr
  | SLeExpr:    s_expr "<=" expr
  | SGeExpr:    s_expr ">=" expr
  | SShlExpr:   s_expr "<<" expr
  | SShrExpr:   s_expr ">>" expr
  | SAddExpr:   s_expr "+" expr
  | SSubExpr:   s_expr "-" expr
  // The surrendered derivation: `s_wide * expr`, never `s_expr * expr`.
  // A bare name followed by `*` at statement start is a declaration.
  | SMulExpr:   s_wide "*" expr
  | SDivExpr:   s_expr "/" expr
  | SModExpr:   s_expr "%" expr
  | SNeg:       "-" expr @precedence("!")
  | SPos:       "+" expr @precedence("!")
  | SNot:       "!" expr
  | SBitNot:    "~" expr
  | SDeref:     "*" expr @precedence("!")
  | SAddrOf:    "&" expr @precedence("!")
  | SPreInc:    "++" expr
  | SPreDec:    "--" expr
  | SSizeofE:   "sizeof" expr
  | SSizeofT:   "sizeof" "(" type_name ")" @precedence("sizeof")
  | SCast:      "(" type_name ")" expr @precedence("!")
  | SCall:      s_expr "(" args ")" @precedence(".")
  | SIndex:     s_expr "[" expr "]" @precedence(".")
  | SMember:    s_expr "." IDENT
  | SArrow:     s_expr "->" IDENT
  | SPostInc:   s_expr "++" @precedence(".")
  | SPostDec:   s_expr "--" @precedence(".")
  | SNumLit:    NUMBER
  | SCharLit:   CHAR
  | SStrLit:    STRING
  | SParen:     "(" cexpr ")"

rule cexpr =
  | CFirst: expr
  | CComma: cexpr "," expr

rule opt_cexpr = cexpr?

// A type name for casts and sizeof: KEYWORD-led specifiers plus
// pointers. Keywords never begin an expression, so `(` peels cast
// from parenthesized expression on the very next token — no cover
// grammar, no feedback. Bare typedef names take the convergent
// spellings instead (see the header).
rule type_name = TypeName: decl_specs ptrs

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
  | SizeofT:   "sizeof" "(" type_name ")" @precedence("sizeof")
  | Cast:      "(" type_name ")" expr @precedence("!")
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
  | ParenExpr: "(" cexpr ")"

rule args = expr* % ","
