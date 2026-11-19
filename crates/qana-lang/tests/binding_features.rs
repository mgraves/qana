//! Binding for a language whose names may come from where the file cannot
//! see, whose patterns bind after their statement, whose items are
//! visible before their declaration, whose names reach their tokens
//! through shared rules, and whose types have members declared elsewhere:
//! escaping references, `@defer`, hoisted definitions, binding through
//! rule labels, namespaces given by where a subtree stands, and open
//! namespaces. Rust needs them all.

use qana_engine::IncSession;
use qana_lang::compile::certify;
use qana_lang::{compile_source, QanaToolchain};
use qana_sem::{RefPaint, SemDb, Target};
use qana_services::paint::facts_at;

/// A small language using every feature. Its file is an ORDERED scope, so
/// hoisting shows at the top level as well as in blocks.
const LANG: &str = r#"
language Bind

token WS     = /\s+/ @trivia
token IDENT  = /[\a_][\w_]*/ @specialize @style(variable)
token SEMI   = ";"
token EQ     = "="
token BANG   = "!"
token SEP    = "::"
token LBRACE = "{"
token RBRACE = "}"
token LT     = "<"
token GT     = ">"

keywords IDENT = "let" "fn" "mac" "for" "in" "mod" "struct" "enum" "self" "seal"

pair LBRACE RBRACE

start file

rule file = File: stmts

rule stmts = stmt*

rule block = Block: "{" stmts "}" @scope

// What is defined directly in a body is a member: reached by a
// qualified name only.
rule body = Body: "{" stmts "}" @scope(members)

// Names outside a sealed block are not seen inside it.
rule sealed = Sealed: "{" stmts "}" @scope(barrier)

rule name = Name: t:IDENT @style(inherit, t)

// A list's shape: its alternatives carry attributes, so it keeps their
// nodes rather than being stored as a balanced list.
rule path =
  | One:  n:name @ref(n, escaping) @style(inherit, n)
  | More: b:path "::" n:name @qualify(b, n) @style(inherit, n)

// A value: a path; `self::x`, whose base holds no reference; and `w<t>::x`,
// whose base is the name `w<t>` passes on, not the last name inside it.
rule value =
  | Path:  p:path @style(inherit, p)
  | Kw:    k:kw "::" n:name @qualify(k, n) @style(inherit, n)
  | After: b:targs "::" n:name @qualify(b, n) @style(inherit, n)

rule kw = SelfKw: "self"

rule targs = TArgs: n:name "<" a:path ">" @ref(n, escaping) @style(inherit, n)

rule pat = Bind: n:name @def(n)

rule stmt =
  | Let:    "let" p:pat "=" e:value ";" @defer(p)
  | Fn:     "fn" n:name b:block @def(n, hoisted)
  | Mod:    "mod" n:name b:block @def(n, hoisted) @module(b)
  | Use:    p:path ";"
  | MacDef: "mac" n:name ";" @def(n) @ns(macro)
  | MacUse: p:path "!" ";" @ns(macro, p)
  | For:    "for" p:pat "in" e:path b:block @defer(p, e) @scope
  | Struct: "struct" n:name ";" @def(n, hoisted, open)
  | Enum:   "enum" n:name b:body @def(n, hoisted, open) @module(b)
  | Seal:   "seal" s:sealed
  | Blk:    block
"#;

struct Doc {
    db: SemDb,
    text: String,
}

impl Doc {
    fn new(text: &str) -> Doc {
        let tc = QanaToolchain::new();
        let out = compile_source(&tc, LANG);
        assert!(out.diags.is_empty(), "the language compiles: {:?}", out.diags);
        let (lexer, tables) = certify(&out.def).expect("in envelope");
        // The tables and lexer outlive the session: leak them, as the
        // embedded languages do.
        let lexer: &'static _ = Box::leak(Box::new(lexer));
        let tables: &'static _ = Box::leak(Box::new(tables));
        let sg: &'static _ = Box::leak(Box::new(out.def.sg.clone()));
        let session = IncSession::new(lexer, sg, tables, text).unwrap();
        let mut db = SemDb::new(out.def.binding.clone());
        db.set_tree("d", session.tree().unwrap().clone());
        Doc { db, text: text.to_string() }
    }

    /// The offset of the `nth` (from 0) occurrence of `word` as a whole
    /// word.
    fn at(&self, word: &str, nth: usize) -> u32 {
        let mut found = 0;
        let bytes = self.text.as_bytes();
        let ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
        for (i, _) in self.text.match_indices(word) {
            let before = i == 0 || !ident(bytes[i - 1]);
            let after = i + word.len() == bytes.len() || !ident(bytes[i + word.len()]);
            if before && after {
                if found == nth {
                    return i as u32;
                }
                found += 1;
            }
        }
        panic!("no occurrence {nth} of `{word}` in {:?}", self.text)
    }

    /// Where the occurrence `nth` of `word` is defined, as the occurrence
    /// number of the same word at the definition (or `None`).
    fn goes_to(&mut self, word: &str, nth: usize) -> Option<usize> {
        let at = self.at(word, nth);
        let (_, span) = self.db.definition("d", at)?;
        (0..).find(|&k| self.at(word, k) == span.0)
    }

    /// How the occurrence `nth` of `word` is painted, as a reference.
    fn paint_of(&mut self, word: &str, nth: usize) -> Option<RefPaint> {
        let at = self.at(word, nth);
        // Items are the file's statements: find the one holding `at`.
        let n_items = self.items();
        for item in 0..n_items {
            let p = self.db.item_paint("d", item);
            let base = self.item_base(item);
            if let Some(&(_, rp)) = p.refs.iter().find(|((s, _), _)| base + s == at) {
                return Some(rp);
            }
        }
        None
    }

    fn items(&mut self) -> u32 {
        self.db.paint_sync("d").len() as u32
    }

    fn item_base(&mut self, item: u32) -> u32 {
        self.db.paint_sync("d")[item as usize].start
    }
}

#[test]
fn a_name_that_escapes_is_not_a_problem_and_one_the_file_defines_binds() {
    let mut doc = Doc::new("let a = nowhere;\nlet b = a;\n");
    assert!(doc.db.unresolved("d").is_empty(), "an escaping name is not diagnosed");
    assert_eq!(doc.paint_of("nowhere", 0), Some(RefPaint::Quiet), "nor painted unresolved");
    let (text, at) = (doc.text.clone(), doc.at("nowhere", 0));
    let card = facts_at(&mut doc.db, "d", &text, at).expect("a card");
    assert_eq!(card.problem, None, "nor is its fact card a problem");
    assert!(card.def_site.is_none());
    assert!(card.escapes, "the card says it escapes");
    assert_eq!(doc.goes_to("a", 1), Some(0), "a name the file defines binds");
    assert_eq!(doc.paint_of("a", 1), Some(RefPaint::Ref));
}

#[test]
fn a_qualified_name_escapes_with_its_base_and_resolves_among_a_modules_members() {
    let mut doc = Doc::new(
        "let a = far::away::deep;\nmod m { mod n { fn x { } } fn y { } }\nlet b = m::n::x;\nlet c = m::y;\n",
    );
    assert!(
        doc.db.qualified_errors("d").is_empty(),
        "`away`, and `deep` after it, escape with `far`"
    );
    for word in ["away", "deep"] {
        assert_eq!(doc.paint_of(word, 0), Some(RefPaint::Quiet), "{word}");
        let (text, at) = (doc.text.clone(), doc.at(word, 0));
        let card = facts_at(&mut doc.db, "d", &text, at).expect("a card");
        assert_eq!(card.problem, None, "{word}");
    }
    assert_eq!(doc.goes_to("x", 1), Some(0), "`m::n::x` is the inner module's `x`");
    assert_eq!(doc.goes_to("y", 1), Some(0), "`m::y` is the module's `y`");
}

#[test]
fn a_list_shaped_rule_whose_alternatives_carry_attributes_keeps_their_nodes() {
    let out = compile_source(&QanaToolchain::new(), LANG);
    let (_, tables) = certify(&out.def).expect("in envelope");
    let nt = |name: &str| out.def.sg.nt_names.iter().position(|n| n == name).unwrap() as u16;
    assert!(!tables.lists.contains_key(&nt("path")), "`path` keeps its nodes");
    assert!(
        tables.lists.contains_key(&nt("stmts")),
        "a list whose alternatives say nothing is balanced"
    );
}

#[test]
fn a_patterns_names_take_effect_after_its_statement() {
    let mut doc = Doc::new("let x = y;\nlet x = x;\nx;\n");
    // `let x = x;`: the right-hand `x` is the first line's.
    assert_eq!(doc.goes_to("x", 2), Some(0));
    // After it, `x` is the second line's.
    assert_eq!(doc.goes_to("x", 3), Some(1));
}

#[test]
fn a_loops_names_take_effect_after_what_it_iterates() {
    let mut doc = Doc::new("let i = z;\nfor i in i { i; }\ni;\n");
    assert_eq!(doc.goes_to("i", 2), Some(0), "`in i` is the earlier `i`");
    assert_eq!(doc.goes_to("i", 3), Some(1), "the body sees the loop's `i`");
    assert_eq!(doc.goes_to("i", 4), Some(0), "after the loop its `i` is gone");
}

#[test]
fn a_hoisted_definition_is_visible_before_it_and_a_plain_one_is_not() {
    let mut doc = Doc::new("{ g; fn g { } v; let v = q; v; }\n");
    assert_eq!(doc.goes_to("g", 0), Some(1), "an item is visible before it");
    assert_eq!(doc.goes_to("v", 0), None, "a `let` is not");
    assert_eq!(doc.goes_to("v", 2), Some(1), "after it, it is");
}

#[test]
fn a_hoisted_definition_is_visible_to_earlier_items_of_an_ordered_file() {
    let mut doc = Doc::new("h;\nlate;\nfn h { }\nlet late = q;\n");
    assert_eq!(doc.goes_to("h", 0), Some(1), "a hoisted top-level item");
    assert_eq!(doc.goes_to("late", 0), None, "a plain one keeps the file's order");
    assert_eq!(doc.paint_of("h", 0), Some(RefPaint::Ref));
}

#[test]
fn a_subtree_stands_in_the_namespace_its_place_gives_it() {
    let mut doc = Doc::new("mac m;\nlet m = q;\nm!;\nm;\n");
    assert_eq!(doc.goes_to("m", 2), Some(0), "`m!` is the macro");
    assert_eq!(doc.goes_to("m", 3), Some(1), "`m` is the value");
}

#[test]
fn binding_through_a_rule_that_passes_its_name_to_several_symbols_is_refused() {
    let lang = "language Bad\n\
        token WS = /\\s+/ @trivia\n\
        token IDENT = /[a-z]+/\n\
        start file\n\
        rule file = File: d:both @def(d)\n\
        rule both = Both: a:IDENT b:IDENT @style(inherit, a, b)\n";
    let out = compile_source(&QanaToolchain::new(), lang);
    assert!(out.diags.iter().any(|d| d.msg.contains("no one token to bind")), "{:?}", out.diags);
    let lang = "language Bad\n\
        token WS = /\\s+/ @trivia\n\
        token IDENT = /[a-z]+/\n\
        start file\n\
        rule file = File: d:quiet @def(d)\n\
        rule quiet = Quiet: IDENT\n";
    let out = compile_source(&QanaToolchain::new(), lang);
    assert!(
        out.diags.iter().any(|d| d.msg.contains("passes no name on to a token")),
        "{:?}",
        out.diags
    );
}

#[test]
fn a_name_an_open_namespace_does_not_show_escapes_and_one_a_closed_module_lacks_does_not() {
    let mut doc = Doc::new(
        "struct S;\nenum E { fn V { } }\nmod m { fn x { } }\n\
         let a = S::new;\nlet b = E::V;\nlet c = E::other;\nlet d = m::gone;\n",
    );
    let errors = doc.db.qualified_errors("d");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].0.contains("no member `gone`"), "{errors:?}");
    let syms = doc.db.symbols("d");
    let res = doc.db.resolve("d");
    for word in ["new", "other"] {
        let at = doc.at(word, 0);
        assert!(doc.db.escapes("d", at), "{word}");
        assert_eq!(doc.paint_of(word, 0), Some(RefPaint::Quiet), "{word}");
        let text = doc.text.clone();
        assert_eq!(facts_at(&mut doc.db, "d", &text, at).expect("a card").problem, None, "{word}");
        let i = syms.refs.iter().position(|r| r.span.0 == at).unwrap();
        assert_eq!(res[i], Target::Escapes, "{word}");
    }
    assert_eq!(doc.goes_to("S", 1), Some(0), "the base binds");
    assert_eq!(doc.goes_to("V", 1), Some(0), "an open namespace's own members bind");
    let gone = doc.at("gone", 0);
    assert!(!doc.db.escapes("d", gone), "a closed module's missing member does not escape");
    assert_eq!(doc.paint_of("gone", 0), Some(RefPaint::Unresolved));
}

#[test]
fn a_qualified_name_whose_base_holds_no_reference_escapes() {
    let mut doc = Doc::new("let a = self::x;\n");
    assert!(doc.db.qualified_errors("d").is_empty(), "{:?}", doc.db.qualified_errors("d"));
    assert!(doc.db.escapes("d", doc.at("x", 0)));
    assert_eq!(doc.paint_of("x", 0), Some(RefPaint::Quiet));
}

#[test]
fn a_qualified_names_base_is_the_name_its_base_passes_on() {
    // The base of `w<t>::new` is `w`, which `w<t>` passes on: `t`, the
    // last name inside it, is a `let`, not a module.
    let mut doc = Doc::new("mod w { fn new { } }\nlet t = q;\nlet a = w<t>::new;\n");
    assert!(doc.db.qualified_errors("d").is_empty(), "{:?}", doc.db.qualified_errors("d"));
    assert_eq!(doc.goes_to("new", 1), Some(0));
}

#[test]
fn a_definition_takes_each_flag_once() {
    let lang = "language Bad\n\
        token WS = /\\s+/ @trivia\n\
        token IDENT = /[a-z]+/\n\
        start file\n\
        rule file = File: d:IDENT @def(d, open, open)\n";
    let out = compile_source(&QanaToolchain::new(), lang);
    assert!(out.diags.iter().any(|d| d.msg.contains("`open` is given twice")), "{:?}", out.diags);
}

#[test]
fn a_member_is_reached_by_a_qualified_name_and_never_by_a_plain_one() {
    let mut doc = Doc::new("enum E { fn V { } fn W { V; } }\nlet a = E::V;\nV;\n");
    assert_eq!(doc.goes_to("V", 2), Some(0), "`E::V` is the member");
    assert_eq!(doc.goes_to("V", 1), None, "not a plain name inside its own body");
    assert_eq!(doc.goes_to("V", 3), None, "nor outside it");
}

#[test]
fn a_modules_members_are_its_own_definitions_not_those_inside_them() {
    let mut doc = Doc::new("mod m { fn f { let x = q; } }\nlet a = m::f;\nlet b = m::x;\n");
    assert_eq!(doc.goes_to("f", 1), Some(0));
    let errors = doc.db.qualified_errors("d");
    assert!(errors.iter().any(|(m, _)| m.contains("no member `x`")), "{errors:?}");
}

#[test]
fn names_outside_a_barrier_are_not_seen_inside_it() {
    let mut doc = Doc::new("fn outer { }\nseal { outer; fn inner { } inner; }\nouter;\n");
    assert_eq!(doc.goes_to("outer", 1), None, "the barrier stops it");
    assert_eq!(doc.goes_to("inner", 1), Some(0), "what is inside binds");
    assert_eq!(doc.goes_to("outer", 2), Some(0));
}

#[test]
fn a_namespace_for_the_alternative_and_one_for_its_children_are_separate() {
    let bad = |rule: &str| {
        let lang = format!(
            "language Bad\n\
             token WS = /\\s+/ @trivia\n\
             token IDENT = /[a-z]+/\n\
             start file\n\
             rule file = File: x:item y:item\n\
             rule item = Item: t:IDENT @ref(t)\n\
             {rule}\n"
        );
        compile_source(&QanaToolchain::new(), &lang)
            .diags
            .into_iter()
            .map(|d| d.msg)
            .collect::<Vec<_>>()
    };
    let msgs = bad("rule other = Other: x:item @ns(tag)");
    assert!(msgs.iter().any(|m| m.contains("@ns(name) requires @def or @ref")), "{msgs:?}");
    let msgs = bad("rule other = Other: x:item y:item @ns(a, x) @ns(b, x, y)");
    assert!(msgs.iter().any(|m| m.contains("`x` already stands in a namespace")), "{msgs:?}");
    let msgs = bad("rule other = Other: x:item @scope(members, sealed, members)");
    assert!(msgs.iter().any(|m| m.contains("unknown scope kind `sealed`")), "{msgs:?}");
    assert!(msgs.iter().any(|m| m.contains("`members` is given twice")), "{msgs:?}");
}
