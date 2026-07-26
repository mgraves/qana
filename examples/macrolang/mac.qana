// MacLang — the meta-tier exerciser. The point of this little
// language: a macro DEFINITION is ordinary syntax (`body` is a real
// `expr`, parsed and bound at the definition site — a typo there is a
// parse error before any use exists), and expansion is BINDING-GUIDED:
// a parameter occurrence in the body is an ordinary @ref resolving to
// the parameter's ordinary @def, so substitution means "replace each
// reference that resolves to parameter i with argument i's text".
// The grammar declares the whole macro system in two annotations.
//
// v0 substitution is cpp-grade TEXTUAL splicing: write bodies with
// the cpp discipline (parenthesize parameter uses) or inherit cpp's
// precedence traps. Syntax-aware substitution — auto-parenthesizing
// by comparing binding strengths, which the trees make possible — is
// the named v1 refinement.

language MacLang

token WS      = /\s+/ @trivia
token COMMENT = /\/\/.*/ @trivia @style(comment)
token NUMBER  = /\d+/ @style(number)
token IDENT   = /[\a_][\w_]*/ @specialize @style(variable)
token BANG    = "!" @style(operator)
token FATARROW = "=>" @style(operator)
token LP      = "(" @style(bracket)
token RP      = ")" @style(bracket)
token LB      = "{" @style(bracket)
token RB      = "}" @style(bracket)
token COMMA   = "," @style(punctuation)
token SEMI    = ";" @style(punctuation)
token EQ      = "=" @style(operator)
token PLUS    = "+" @style(operator)
token STAR    = "*" @style(operator)

keywords IDENT = let macro

pair LP RP
pair LB RB

prec left "+"
prec left "*"

start file

rule file = File: items

rule items = item*

rule item =
  // The meta tier, declared: this @def introduces a MACRO whose
  // parameters are the defs inside `ps` and whose template is the
  // `body` child. @scope keeps the parameters out of the file's
  // namespace — and makes body-refs-to-params resolve locally, which
  // is exactly what the expansion engine substitutes on.
  | MacroDef: "macro" name:IDENT "(" ps:mparams ")" "=>" "{" body:expr "}" @def(name) @macro(ps, body) @scope @outline(name, function)
  | LetDef:   "let" name:IDENT "=" expr ";" @def(name) @outline(name, variable)

rule mparams = mparam* % ","

rule mparam = MParam: name:IDENT @def(name)

// A block with a local binding — the thing a macro body can be
// captured BY. `{ let unit = 99; e }` scopes `unit` to `e`. (The
// binding lives on its own production so it lands INSIDE the scope:
// a production carrying both @def and @scope defines in the
// ENCLOSING scope, which is what `macro m(x) => …` wants.)
rule local_bind = LocalBind: "let" name:IDENT "=" expr ";" @def(name)

rule expr =
  | Scoped:  "{" local_bind expr "}" @scope
  | Add:     expr "+" expr
  | Mul:     expr "*" expr
  // Expansion may happen HERE and only here: `name!(args)`.
  | Splice:  name:IDENT "!" "(" args:sargs ")" @ref(name) @splice(name, args)
  | NameRef: name:IDENT @ref(name)
  | Num:     NUMBER
  | Paren:   "(" expr ")"

rule sargs = expr* % ","
