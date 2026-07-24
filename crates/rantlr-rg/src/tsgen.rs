//! Tree-sitter grammar emission: the "derived artifact" of the
//! feasibility report's Part I. Any envelope-certified grammar emits a
//! `grammar.js` (+ `queries/highlights.scm`) so languages built with
//! rantlr get native-editor reach — Neovim, Helix, Zed, GitHub — for
//! free, from the same single definition.
//!
//! The envelope makes the mapping principled instead of heroic:
//! * trivia-by-declaration → tree-sitter `extras`
//! * keyword specialization → `word:` + inline keyword strings (its
//!   keyword-extraction pass mirrors our specialization semantics)
//! * declarative precedence (L3) → `prec.left/right(level, …)`
//! * L4 lists → idiomatic `repeat1` / separated-`seq` forms
//! * binding name children (L8) → `field("name", …)`
//! * bounded self-push trivia modes (L2) → RECURSIVE extra rules —
//!   nested comments without an external scanner (tree-sitter's
//!   recursion is unbounded where ours is depth-capped: divergence is
//!   in the permissive direction and noted in the emitted header)
//!
//! Deliberate scope: @error tokens are skipped (tree-sitter has its own
//! ERROR recovery); non-trivia modes (string interpolation etc.) are
//! refused — that's external-scanner territory, documented as such.

use rantlr_grammar::model::{Action, LexGrammar, TokenId};
use rantlr_grammar::pat::{ClassSet, Pat};
use rantlr_grammar::syn::{Assoc, Sym, SynGrammar};
use rantlr_grammar::LrTables;
use rantlr_sem::BindingConfig;
use rantlr_services::Styles;
use std::collections::{HashMap, HashSet};
use std::fmt::Write;

#[derive(Debug)]
pub struct TsOutput {
    pub grammar_js: String,
    pub highlights_scm: String,
    pub warnings: Vec<String>,
}

/// How each terminal renders inside productions and queries.
enum TermForm {
    /// Inline string literal (fixed-text tokens and keywords).
    Str(String),
    /// Named token rule (pattern tokens).
    Rule(String),
    /// Not representable in productions (trivia/error/never) — an
    /// internal error if referenced.
    Absent,
}

pub fn emit_tree_sitter(
    lex: &LexGrammar,
    sg: &SynGrammar,
    tables: &LrTables,
    styles: &Styles,
    binding: &BindingConfig,
) -> Result<TsOutput, Vec<String>> {
    let mut errors: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let nt_names: HashSet<&str> = sg.nt_names.iter().map(|s| s.as_str()).collect();
    let fresh_name = |base: String, taken: &HashSet<String>| -> String {
        if !nt_names.contains(base.as_str()) && !taken.contains(&base) {
            base
        } else {
            format!("{base}_token")
        }
    };

    // ---- classify tokens ----
    let kw_of: HashMap<TokenId, &str> =
        lex.keywords.iter().map(|(w, id, _)| (*id, w.as_str())).collect();
    let mut term_form: Vec<TermForm> = Vec::new();
    let mut token_rules: Vec<(String, String)> = Vec::new(); // (name, regex)
    let mut extras: Vec<String> = Vec::new();
    let mut comment_rules: Vec<(String, String)> = Vec::new(); // (name, body)
    let mut taken_names: HashSet<String> = HashSet::new();
    let mut has_ws_extra = false;
    let mut word_rule: Option<String> = None;

    // Non-default modes: only fully-trivia "comment islands" are
    // representable (entered by a trivia Lit push from mode 0, left by
    // a Lit pop). Anything else needs an external scanner.
    let mut mode_rule: HashMap<u16, String> = HashMap::new();
    for m in 1..lex.mode_names.len() as u16 {
        let mode_name = &lex.mode_names[m as usize];
        let toks: Vec<usize> = (0..lex.tokens.len()).filter(|&i| lex.tokens[i].mode == m).collect();
        if toks.iter().any(|&i| !lex.tokens[i].trivia) {
            errors.push(format!(
                "mode `{mode_name}` has non-trivia tokens — tree-sitter emission supports \
                 trivia-only modes (comment islands); anything richer is external-scanner \
                 territory (roadmap)"
            ));
            continue;
        }
        let entries: Vec<usize> = (0..lex.tokens.len())
            .filter(|&i| {
                lex.tokens[i].mode == 0
                    && lex.tokens[i].trivia
                    && lex.tokens[i].action == Action::Push(m)
            })
            .collect();
        let [entry] = entries.as_slice() else {
            errors.push(format!(
                "mode `{mode_name}` needs exactly one trivia entry token pushing it from the \
                 base mode (found {})",
                entries.len()
            ));
            continue;
        };
        let Pat::Lit(open) = &lex.tokens[*entry].pat else {
            errors.push(format!("mode `{mode_name}`'s entry token must be a fixed literal"));
            continue;
        };
        let close = toks.iter().find_map(|&i| {
            (lex.tokens[i].action == Action::Pop).then(|| match &lex.tokens[i].pat {
                Pat::Lit(s) => Some(s.clone()),
                _ => None,
            })
        });
        let Some(Some(close)) = close else {
            errors.push(format!("mode `{mode_name}` needs a fixed-literal pop token"));
            continue;
        };
        let rule = fresh_name(format!("{}_comment", mode_name.to_lowercase()), &taken_names);
        taken_names.insert(rule.clone());
        let mut branches: Vec<String> = Vec::new();
        for &i in &toks {
            let t = &lex.tokens[i];
            if t.action == Action::Pop {
                continue;
            }
            if t.action == Action::Push(m) {
                branches.insert(0, format!("$.{rule}")); // recursion first
                continue;
            }
            match &t.pat {
                Pat::Lit(s) => branches.push(js_str(s)),
                p => branches.push(format!("/{}/", pat_regex(p))),
            }
        }
        let body = format!(
            "seq({}, repeat(choice({})), {})",
            js_str(open),
            branches.join(", "),
            js_str(&close)
        );
        comment_rules.push((rule.clone(), body));
        warnings.push(format!(
            "mode `{mode_name}` emitted as recursive extra `{rule}`: tree-sitter nests it \
             unboundedly where the envelope caps depth at {} — permissive-direction divergence",
            lex.max_stack.unwrap_or(8)
        ));
        mode_rule.insert(m, rule);
    }

    for (i, t) in lex.tokens.iter().enumerate() {
        let id = i as TokenId;
        if t.mode != 0 {
            term_form.push(TermForm::Absent);
            continue;
        }
        if let Some(w) = kw_of.get(&id) {
            term_form.push(TermForm::Str((*w).to_string()));
            continue;
        }
        if t.error {
            warnings.push(format!(
                "error-flavored token `{}` skipped (tree-sitter's own ERROR recovery covers it)",
                t.name
            ));
            term_form.push(TermForm::Absent);
            continue;
        }
        if t.trivia {
            match t.action {
                Action::Push(_) => {} // handled by the mode pass
                _ => {
                    if is_lws_pattern(&t.pat) {
                        has_ws_extra = true;
                    } else {
                        let rule =
                            fresh_name(t.name.to_lowercase(), &taken_names);
                        taken_names.insert(rule.clone());
                        comment_rules.push((rule, format!("/{}/", pat_regex(&t.pat))));
                    }
                }
            }
            term_form.push(TermForm::Absent);
            continue;
        }
        match &t.pat {
            Pat::Never => term_form.push(TermForm::Absent),
            Pat::Lit(s) => term_form.push(TermForm::Str(s.clone())),
            p => {
                let rule = fresh_name(t.name.to_lowercase(), &taken_names);
                taken_names.insert(rule.clone());
                token_rules.push((rule.clone(), pat_regex(p)));
                if t.specialize {
                    word_rule = Some(rule.clone());
                }
                term_form.push(TermForm::Rule(rule));
            }
        }
    }
    if has_ws_extra {
        extras.push("/\\s/".to_string());
    }
    for (name, _) in &comment_rules {
        extras.push(format!("$.{name}"));
    }

    if !errors.is_empty() {
        return Err(errors);
    }

    // ---- nullability (ε productions become optional(...) at refs) ----
    let mut nullable = vec![false; sg.nt_names.len()];
    loop {
        let mut changed = false;
        for p in &sg.prods {
            if !nullable[p.lhs as usize]
                && p.rhs.iter().all(|s| matches!(s, Sym::N(n) if nullable[*n as usize]))
            {
                nullable[p.lhs as usize] = true;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    // Binding name-children → tree-sitter fields.
    let field_at: HashSet<(u16, u16, usize)> =
        binding.defs.iter().map(|&(nt, p, k)| (nt, p, k)).collect();

    let render_sym = |s: &Sym| -> String {
        match s {
            Sym::T(t) => match &term_form[*t as usize] {
                TermForm::Str(text) => js_str(text),
                TermForm::Rule(name) => format!("$.{name}"),
                TermForm::Absent => "/*unrepresentable*/".to_string(),
            },
            Sym::N(n) => {
                if nullable[*n as usize] {
                    format!("optional($.{})", sg.nt_names[*n as usize])
                } else {
                    format!("$.{}", sg.nt_names[*n as usize])
                }
            }
        }
    };
    // Inside list bodies the element repeats — never optional-wrapped.
    let render_plain = |s: &Sym| -> String {
        match s {
            Sym::T(_) => render_sym(s),
            Sym::N(n) => format!("$.{}", sg.nt_names[*n as usize]),
        }
    };

    let mut rules: Vec<(String, String)> = Vec::new();
    // Root first: tree-sitter's entry rule is the first in the map.
    let mut order: Vec<u16> = (0..sg.nt_names.len() as u16).collect();
    order.sort_by_key(|&nt| (nt != sg.start, nt));

    for nt in order {
        let name = &sg.nt_names[nt as usize];
        let prods: Vec<u16> =
            (0..sg.prods.len() as u16).filter(|&i| sg.prods[i as usize].lhs == nt).collect();

        // L4 lists → idiomatic repetition (the common shapes).
        if let Some(shape) = tables.lists.get(&nt) {
            let cons = &sg.prods[shape.cons as usize].rhs[1..];
            let seed = &sg.prods[shape.seed as usize].rhs;
            let body = match (seed.len(), cons.len()) {
                (0, 1) => Some(format!("repeat1({})", render_plain(&cons[0]))),
                (1, 1) if seed[0] == cons[0] => {
                    Some(format!("repeat1({})", render_plain(&cons[0])))
                }
                (1, 2) if seed[0] == cons[1] => Some(format!(
                    "seq({}, repeat(seq({}, {})))",
                    render_plain(&seed[0]),
                    render_plain(&cons[0]),
                    render_plain(&cons[1])
                )),
                _ => None, // irregular list: literal BNF below
            };
            if let Some(body) = body {
                rules.push((name.clone(), body));
                continue;
            }
        }

        let mut alts: Vec<String> = Vec::new();
        for p in prods {
            let prod = &sg.prods[p as usize];
            if prod.rhs.is_empty() {
                continue; // ε handled via optional(...) at references
            }
            let parts: Vec<String> = prod
                .rhs
                .iter()
                .enumerate()
                .map(|(k, s)| {
                    let r = render_sym(s);
                    if field_at.contains(&(nt, p, k)) {
                        format!("field(\"name\", {r})")
                    } else {
                        r
                    }
                })
                .collect();
            let seq = if parts.len() == 1 {
                parts.into_iter().next().unwrap()
            } else {
                format!("seq({})", parts.join(", "))
            };
            let wrapped = match sg.prod_precedence(prod) {
                Some((level, Assoc::Left)) => format!("prec.left({level}, {seq})"),
                Some((level, Assoc::Right)) => format!("prec.right({level}, {seq})"),
                Some((level, Assoc::NonAssoc)) => format!("prec({level}, {seq})"),
                None => seq,
            };
            alts.push(wrapped);
        }
        let body = match alts.len() {
            0 => {
                warnings.push(format!("rule `{name}` matches only ε — emitted as blank()"));
                "blank()".to_string()
            }
            1 => alts.into_iter().next().unwrap(),
            _ => format!("choice({})", alts.join(", ")),
        };
        rules.push((name.clone(), body));
    }

    for (name, re) in &token_rules {
        rules.push((name.clone(), format!("/{re}/")));
    }
    for (name, body) in &comment_rules {
        rules.push((name.clone(), body.clone()));
    }

    // ---- grammar.js ----
    let lang = lex.name.to_lowercase();
    let mut js = String::new();
    writeln!(js, "// @generated by rantlr (rg2ts). DO NOT EDIT.").unwrap();
    writeln!(js, "// Emitted from the envelope-certified `{}` grammar: deterministic", lex.name).unwrap();
    writeln!(js, "// LR(1), so tree-sitter's GLR power is never needed — precedence").unwrap();
    writeln!(js, "// annotations below carry the same declarative disambiguation.").unwrap();
    for w in &warnings {
        writeln!(js, "// NOTE: {w}").unwrap();
    }
    writeln!(js, "module.exports = grammar({{").unwrap();
    writeln!(js, "  name: '{lang}',").unwrap();
    if let Some(w) = &word_rule {
        writeln!(js, "  word: $ => $.{w},").unwrap();
    }
    if !extras.is_empty() {
        writeln!(js, "  extras: $ => [{}],", extras.join(", ")).unwrap();
    }
    writeln!(js, "  rules: {{").unwrap();
    for (name, body) in &rules {
        writeln!(js, "    {name}: $ => {body},").unwrap();
    }
    writeln!(js, "  }}").unwrap();
    writeln!(js, "}});").unwrap();

    // ---- queries/highlights.scm ----
    let capture_of = |class: &str| -> &'static str {
        match class {
            "keyword" => "@keyword",
            "variable" => "@variable",
            "number" => "@number",
            "string" => "@string",
            "comment" => "@comment",
            "operator" => "@operator",
            "punctuation" => "@punctuation.delimiter",
            "bracket" => "@punctuation.bracket",
            "regexp" => "@string.regexp",
            _ => "@none",
        }
    };
    // capture → (strings, node names), in legend order then token order.
    let mut groups: Vec<(&'static str, Vec<String>, Vec<String>)> = Vec::new();
    let mut group_idx: HashMap<&'static str, usize> = HashMap::new();
    let add = |cap: &'static str, item: String, is_node: bool,
                   groups: &mut Vec<(&'static str, Vec<String>, Vec<String>)>,
                   group_idx: &mut HashMap<&'static str, usize>| {
        let gi = *group_idx.entry(cap).or_insert_with(|| {
            groups.push((cap, Vec::new(), Vec::new()));
            groups.len() - 1
        });
        if is_node {
            groups[gi].2.push(item);
        } else {
            groups[gi].1.push(item);
        }
    };
    for (i, t) in lex.tokens.iter().enumerate() {
        let id = i as TokenId;
        let Some(class_idx) = styles.class_of(id) else { continue };
        let cap = capture_of(styles.legend[class_idx as usize]);
        match &term_form[i] {
            TermForm::Str(s) => add(cap, js_str(s), false, &mut groups, &mut group_idx),
            TermForm::Rule(name) => {
                add(cap, format!("({name})"), true, &mut groups, &mut group_idx)
            }
            TermForm::Absent => {
                // Trivia comment rules keep their capture; mode tokens are
                // covered by the island rule below.
                if t.trivia && t.mode == 0 && t.action == Action::None && !is_lws_pattern(&t.pat)
                {
                    let rule = t.name.to_lowercase();
                    add(cap, format!("({rule})"), true, &mut groups, &mut group_idx);
                }
            }
        }
    }
    for m in 1..lex.mode_names.len() as u16 {
        if let Some(rule) = mode_rule.get(&m) {
            add("@comment", format!("({rule})"), true, &mut groups, &mut group_idx);
        }
    }

    let mut scm = String::new();
    writeln!(scm, "; @generated by rantlr (rg2ts). DO NOT EDIT.").unwrap();
    writeln!(scm, "; Derived from the grammar's @style annotations.").unwrap();
    for (cap, strs, nodes) in &groups {
        if !strs.is_empty() {
            if strs.len() == 1 {
                writeln!(scm, "{} {}", strs[0], cap).unwrap();
            } else {
                writeln!(scm, "[{}] {}", strs.join(" "), cap).unwrap();
            }
        }
        for n in nodes {
            writeln!(scm, "{n} {cap}").unwrap();
        }
    }

    Ok(TsOutput { grammar_js: js, highlights_scm: scm, warnings })
}

/// Is this the whitespace-trivia pattern (folds into the `/\s/` extra)?
fn is_lws_pattern(p: &Pat) -> bool {
    matches!(p, Pat::Plus(inner) | Pat::Star(inner)
        if matches!(&**inner, Pat::Class(c) if c.lws && !c.negated && c.chars.is_empty()
            && c.ranges.is_empty() && !c.alpha && !c.alnum && !c.digit))
}

// ---------------------------------------------------------------------------
// Pat → JS regex source
// ---------------------------------------------------------------------------

fn js_str(s: &str) -> String {
    let mut out = String::from("'");
    for c in s.chars() {
        match c {
            '\'' => out.push_str("\\'"),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

fn regex_escape_char(c: char, out: &mut String) {
    if "\\^$.|?*+()[]{}/".contains(c) {
        out.push('\\');
    }
    out.push(c);
}

fn class_escape_char(c: char, out: &mut String) {
    match c {
        ']' | '\\' | '^' | '-' | '/' => {
            out.push('\\');
            out.push(c);
        }
        '\r' => out.push_str("\\r"),
        '\n' => out.push_str("\\n"),
        '\t' => out.push_str("\\t"),
        c => out.push(c),
    }
}

fn class_regex(cs: &ClassSet) -> String {
    // Standalone linear-whitespace class gets the tidy JS idiom.
    if cs.lws && !cs.negated && cs.chars.is_empty() && cs.ranges.is_empty() && !cs.alpha
        && !cs.alnum && !cs.digit
    {
        return "[^\\S\\r\\n]".to_string();
    }
    let mut body = String::new();
    for &c in &cs.chars {
        class_escape_char(c, &mut body);
    }
    for &(a, b) in &cs.ranges {
        class_escape_char(a, &mut body);
        body.push('-');
        class_escape_char(b, &mut body);
    }
    if cs.alpha {
        body.push_str("\\p{L}");
    }
    if cs.alnum {
        body.push_str("\\p{L}\\p{N}");
    }
    if cs.digit {
        body.push_str("0-9");
    }
    if cs.lws {
        body.push_str(" \\t\\f\\v\\p{Zs}");
    }
    format!("[{}{body}]", if cs.negated { "^" } else { "" })
}

fn needs_group(p: &Pat) -> bool {
    match p {
        Pat::Lit(s) => s.chars().count() > 1,
        Pat::Class(_) => false,
        Pat::Seq(_) | Pat::Alt(_) => true,
        Pat::Star(_) | Pat::Plus(_) | Pat::Opt(_) => false,
        Pat::Never => false,
    }
}

pub fn pat_regex(p: &Pat) -> String {
    match p {
        Pat::Lit(s) => {
            let mut out = String::new();
            for c in s.chars() {
                regex_escape_char(c, &mut out);
            }
            out
        }
        Pat::Class(cs) => class_regex(cs),
        Pat::Seq(ps) => ps
            .iter()
            .map(|q| {
                if matches!(q, Pat::Alt(_)) {
                    format!("(?:{})", pat_regex(q))
                } else {
                    pat_regex(q)
                }
            })
            .collect(),
        Pat::Alt(ps) => ps.iter().map(pat_regex).collect::<Vec<_>>().join("|"),
        Pat::Star(q) => rep(q, '*'),
        Pat::Plus(q) => rep(q, '+'),
        Pat::Opt(q) => rep(q, '?'),
        Pat::Never => "[^\\s\\S]".to_string(), // matches nothing
    }
}

fn rep(q: &Pat, op: char) -> String {
    if needs_group(q) {
        format!("(?:{}){op}", pat_regex(q))
    } else {
        format!("{}{op}", pat_regex(q))
    }
}
