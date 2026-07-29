//! The `.qana` compiler: a green tree (from the bootstrap parser) in, the
//! toolchain's grammar VALUES out — `LexGrammar`, `SynGrammar`,
//! `BindingConfig`, `Styles`, `OutlineConfig` — plus span-carrying
//! diagnostics. Certification (`certify`) then runs the same envelope
//! gates every programmatic grammar passes, mapping lint witnesses and
//! LR conflict counterexamples back to `.qana` source spans: the refusal
//! UX points at the construct that caused it.

use crate::bootstrap::QanaProds;
use crate::pat_parse::parse_pattern;
use qana_grammar::green::ERROR_NT;
use qana_grammar::model::{BracketKind, LexGrammar, TokenDef, TokenId};
use qana_grammar::pat::Pat;
use qana_grammar::syn::{Assoc, Sym, SynGrammar};
use qana_grammar::{build_lr, CompiledLexer, GreenChild, GreenNode, GreenToken, LrTables, Vocab};
use qana_sem::{BindingConfig, RefKind, TyTerm, TypeConfig, TypeRule};
use qana_services::{OutlineConfig, OutlineEntry, Styles, LEGEND};
use std::collections::HashMap;

/// Outline kinds a grammar may declare (closed set, `&'static` for the
/// services layer). `field` covers struct/class members — outline
/// LEAVES, present in every structure view and breadcrumb chain (LSP's
/// SymbolKind has it for the same reason).
pub const OUTLINE_KINDS: &[&str] =
    &["variable", "constant", "function", "struct", "module", "class", "field"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QanaDiag {
    pub span: (u32, u32),
    pub msg: String,
    /// 1 = error, 2 = warning.
    pub severity: u8,
}

/// A compiled language definition: pure values + the source-span maps
/// that let certification report refusals against the `.qana` text.
pub struct LangDef {
    pub lex: LexGrammar,
    pub sg: SynGrammar,
    pub binding: BindingConfig,
    /// The declared type tier (`@type` annotations, as data). Empty
    /// when the grammar declares none — and then so is the tier.
    pub types: TypeConfig,
    /// The declared META tier (`@macro`/`@splice`, as data). Empty
    /// when the grammar declares none — and then so is the tier.
    pub macros: qana_sem::macros::MacroConfig,
    pub styles: Styles,
    pub outline: OutlineConfig,
    /// Token id → span of its declaring name (keywords: the item span).
    pub token_spans: Vec<(u32, u32)>,
    /// Production index → span of its alternative label.
    pub prod_spans: Vec<(u32, u32)>,
}

// ---------------------------------------------------------------------------
// Offset-carrying green-tree cursor
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Cur<'g> {
    n: &'g GreenNode,
    base: u32,
}

enum Child<'g> {
    Node(Cur<'g>),
    Tok(&'g GreenToken, (u32, u32)),
}

impl<'g> Cur<'g> {
    /// k-th grammar-symbol child with its byte span (trivia, missing
    /// repair tokens, and error nodes skipped — same positional contract
    /// as the typed accessors).
    fn sym(&self, k: usize) -> Option<Child<'g>> {
        let mut off = self.base;
        let mut idx = 0usize;
        for c in &self.n.children {
            let w = c.width();
            let is_symbol = match c {
                GreenChild::Token(t) => !t.trivia && !t.is_missing(),
                GreenChild::Node(m) => m.nt != ERROR_NT,
            };
            if is_symbol {
                if idx == k {
                    return Some(match c {
                        GreenChild::Token(t) => Child::Tok(t, (off, off + w)),
                        GreenChild::Node(m) => Child::Node(Cur { n: m, base: off }),
                    });
                }
                idx += 1;
            }
            off += w;
        }
        None
    }

    fn tok(&self, k: usize) -> Option<(&'g GreenToken, (u32, u32))> {
        match self.sym(k)? {
            Child::Tok(t, s) => Some((t, s)),
            Child::Node(_) => None,
        }
    }

    fn node(&self, k: usize) -> Option<Cur<'g>> {
        match self.sym(k)? {
            Child::Node(c) => Some(c),
            Child::Tok(..) => None,
        }
    }

    /// Flattened element nodes of a LIST subtree (RUN nodes expanded),
    /// each with its base offset.
    fn items(&self) -> Vec<Cur<'g>> {
        fn go<'g>(n: &'g GreenNode, base: u32, out: &mut Vec<Cur<'g>>) {
            let mut off = base;
            for c in &n.children {
                match c {
                    GreenChild::Node(m) if m.prod == qana_grammar::green::RUN_PROD => {
                        go(m, off, out)
                    }
                    GreenChild::Node(m) if m.nt != ERROR_NT => out.push(Cur { n: m, base: off }),
                    _ => {}
                }
                off += c.width();
            }
        }
        let mut out = Vec::new();
        go(self.n, self.base, &mut out);
        out
    }
}

// ---------------------------------------------------------------------------
// Source-shaped intermediate view (one walk, then passes over it)
// ---------------------------------------------------------------------------

struct AttrIr {
    name: String,
    name_span: (u32, u32),
    /// (text, span, is_string) per argument.
    args: Vec<(String, (u32, u32), bool)>,
}

struct TokenIr {
    name: String,
    name_span: (u32, u32),
    mode: u16,
    /// Regex interior or unescaped literal text.
    pat_src: String,
    pat_span: (u32, u32),
    pat_is_regex: bool,
    attrs: Vec<AttrIr>,
}

/// An EBNF sugar operator (postfix on symbols, or a rule-level form).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum SugarOp {
    Opt,
    Star,
    Plus,
}

impl SugarOp {
    fn suffix(self) -> &'static str {
        match self {
            SugarOp::Opt => "opt",
            SugarOp::Star => "star",
            SugarOp::Plus => "plus",
        }
    }
}

struct SymIr {
    label: Option<String>,
    /// NAME text or unescaped STRING content.
    text: String,
    span: (u32, u32),
    is_string: bool,
    postfix: Option<SugarOp>,
}

struct AltIr {
    label: String,
    label_span: (u32, u32),
    syms: Vec<SymIr>,
    attrs: Vec<AttrIr>,
}

enum RuleBody {
    Alts(Vec<AltIr>),
    /// `rule R = elem OP [% sep]` — desugared with deterministic names.
    Sugar { op: SugarOp, elem: TokRefIr, sep: Option<TokRefIr> },
}

struct RuleIr {
    name: String,
    name_span: (u32, u32),
    body: RuleBody,
}

#[derive(Clone)]
struct TokRefIr {
    text: String,
    span: (u32, u32),
    is_string: bool,
}

/// Token-id-assigning declarations, in FILE order (ids are positional:
/// the order tokens and keywords appear is the order they're numbered).
enum IdEvent {
    Def(TokenIr),
    Keywords { base: String, base_span: (u32, u32), items: Vec<TokRefIr> },
}

#[derive(Default)]
struct FileIr {
    language: Option<(String, (u32, u32))>,
    max_stack: Option<(u8, (u32, u32))>,
    events: Vec<IdEvent>,
    mode_names: Vec<(String, (u32, u32))>,
    brackets: Vec<(TokRefIr, TokRefIr)>,
    /// (assoc, ops) in declaration order — level = position + 1.
    precs: Vec<(Assoc, Vec<TokRefIr>)>,
    start: Option<(String, (u32, u32))>,
    rules: Vec<RuleIr>,
}

impl Default for IdEvent {
    fn default() -> Self {
        IdEvent::Keywords { base: String::new(), base_span: (0, 0), items: Vec::new() }
    }
}

/// Strip one level of `\c → c` escaping from a STRING token's content.
fn unquote(text: &str) -> String {
    let inner = text.strip_prefix('"').unwrap_or(text);
    let inner = inner.strip_suffix('"').unwrap_or(inner);
    let mut out = String::with_capacity(inner.len());
    let mut esc = false;
    for c in inner.chars() {
        if esc {
            out.push(c);
            esc = false;
        } else if c == '\\' {
            esc = true;
        } else {
            out.push(c);
        }
    }
    out
}

fn lower_attrs(cur: Cur<'_>, p: &QanaProds) -> Vec<AttrIr> {
    // cur is an `attrs` LIST node.
    cur.items()
        .iter()
        .filter_map(|a| {
            let (name_tok, name_span) = a.tok(1)?;
            let mut args = Vec::new();
            if a.n.prod == p.attr_args as u16 {
                if let Some(list) = a.node(3) {
                    // arg_list items are `arg` nodes (the comma is a
                    // terminal inside the cons production, so the
                    // flattened node view yields the args directly).
                    for arg in list.items() {
                        let Some((t, s)) = arg.tok(0) else { continue };
                        let is_string = arg.n.prod == p.arg_str as u16;
                        let text = if is_string { unquote(&t.text) } else { t.text.clone() };
                        args.push((text, s, is_string));
                    }
                }
            }
            Some(AttrIr { name: name_tok.text.clone(), name_span, args })
        })
        .collect()
}

fn lower_tok_ref(cur: Cur<'_>, p: &QanaProds) -> Option<TokRefIr> {
    let (t, span) = cur.tok(0)?;
    let is_string = cur.n.prod == p.tok_str as u16;
    let text = if is_string { unquote(&t.text) } else { t.text.clone() };
    Some(TokRefIr { text, span, is_string })
}

fn lower_elem(cur: Cur<'_>, p: &QanaProds) -> Option<TokRefIr> {
    let (t, span) = cur.tok(0)?;
    let is_string = cur.n.prod == p.elem_str as u16;
    let text = if is_string { unquote(&t.text) } else { t.text.clone() };
    Some(TokRefIr { text, span, is_string })
}

fn lower_token_def(cur: Cur<'_>, mode: u16, p: &QanaProds) -> Option<TokenIr> {
    let (name_tok, name_span) = cur.tok(1)?;
    let pat = cur.node(3)?;
    let (pat_tok, pat_span) = pat.tok(0)?;
    let pat_is_regex = pat.n.prod == p.pat_regex as u16;
    let pat_src = if pat_is_regex {
        let inner = pat_tok.text.strip_prefix('/').unwrap_or(&pat_tok.text);
        inner.strip_suffix('/').unwrap_or(inner).to_string()
    } else {
        unquote(&pat_tok.text)
    };
    let attrs = cur.node(4).map(|a| lower_attrs(a, p)).unwrap_or_default();
    Some(TokenIr {
        name: name_tok.text.clone(),
        name_span,
        mode,
        pat_src,
        pat_span,
        pat_is_regex,
        attrs,
    })
}

fn lower(tree: &GreenNode, p: &QanaProds, diags: &mut Vec<QanaDiag>) -> FileIr {
    let mut ir = FileIr::default();
    let root = Cur { n: tree, base: 0 };
    let Some(decls) = root.node(0) else { return ir };
    // First sweep: mode declarations (mode numbers are declaration
    // order; tokens can @push modes declared later in the file).
    for d in decls.items() {
        if d.n.prod == p.mode_decl as u16 {
            if let Some((t, s)) = d.tok(1) {
                if ir.mode_names.iter().any(|(n, _)| *n == t.text) {
                    diags.push(QanaDiag {
                        span: s,
                        msg: format!("mode `{}` declared twice", t.text),
                        severity: 1,
                    });
                } else {
                    ir.mode_names.push((t.text.clone(), s));
                }
            }
        }
    }
    for d in decls.items() {
        let prod = d.n.prod as usize;
        if prod == p.lang_decl {
            if let Some((t, s)) = d.tok(1) {
                if ir.language.is_some() {
                    diags.push(QanaDiag { span: s, msg: "duplicate `language`".into(), severity: 1 });
                } else {
                    ir.language = Some((t.text.clone(), s));
                }
            }
        } else if prod == p.max_stack_decl {
            if let Some((t, s)) = d.tok(1) {
                match t.text.parse::<u8>() {
                    Ok(n) => ir.max_stack = Some((n, s)),
                    Err(_) => diags.push(QanaDiag {
                        span: s,
                        msg: "max_stack must fit in a u8".into(),
                        severity: 1,
                    }),
                }
            }
        } else if prod == p.kw_decl {
            let Some((base, base_span)) = d.tok(1) else { continue };
            let mut items = Vec::new();
            if let Some(list) = d.node(3) {
                for item in list.items() {
                    let Some((t, s)) = item.tok(0) else { continue };
                    let is_string = item.n.prod == p.kw_str as u16;
                    let text = if is_string { unquote(&t.text) } else { t.text.clone() };
                    items.push(TokRefIr { text, span: s, is_string });
                }
            }
            ir.events.push(IdEvent::Keywords { base: base.text.clone(), base_span, items });
        } else if prod == p.token_decl {
            if let Some(td) = d.node(0).and_then(|c| lower_token_def(c, 0, p)) {
                ir.events.push(IdEvent::Def(td));
            }
        } else if prod == p.mode_decl {
            // Mode number = position in the (deduplicated) mode list.
            let mode = d
                .tok(1)
                .and_then(|(t, _)| ir.mode_names.iter().position(|(n, _)| *n == t.text))
                .map(|i| (i + 1) as u16)
                .unwrap_or(0);
            if let Some(defs) = d.node(3) {
                for td in defs.items() {
                    if let Some(t) = lower_token_def(td, mode, p) {
                        ir.events.push(IdEvent::Def(t));
                    }
                }
            }
        } else if prod == p.bracket_decl {
            let (Some(o), Some(c)) = (
                d.node(1).and_then(|c| lower_tok_ref(c, p)),
                d.node(2).and_then(|c| lower_tok_ref(c, p)),
            ) else {
                continue;
            };
            ir.brackets.push((o, c));
        } else if prod == p.prec_decl {
            let assoc = match d.node(1).map(|a| a.n.prod as usize) {
                Some(x) if x == p.assoc_right => Assoc::Right,
                _ => Assoc::Left,
            };
            let mut ops = Vec::new();
            if let Some(list) = d.node(2) {
                for r in list.items() {
                    if let Some(t) = lower_tok_ref(r, p) {
                        ops.push(t);
                    }
                }
            }
            ir.precs.push((assoc, ops));
        } else if prod == p.start_decl {
            if let Some((t, s)) = d.tok(1) {
                if ir.start.is_some() {
                    diags.push(QanaDiag { span: s, msg: "duplicate `start`".into(), severity: 1 });
                } else {
                    ir.start = Some((t.text.clone(), s));
                }
            }
        } else if prod == p.rule_decl || prod == p.rule_decl_bar {
            let Some((name, name_span)) = d.tok(1) else { continue };
            let alt_list_pos = if prod == p.rule_decl_bar { 4 } else { 3 };
            let mut alts = Vec::new();
            if let Some(list) = d.node(alt_list_pos) {
                for a in list.items() {
                    let Some((label, label_span)) = a.tok(0) else { continue };
                    let mut syms = Vec::new();
                    if let Some(sym_list) = a.node(2) {
                        for s in sym_list.items() {
                            let sp = s.n.prod as usize;
                            let (label_s, tok_at) = if sp == p.sym_labeled || sp == p.sym_labeled_str
                            {
                                let l = s.tok(0).map(|(t, _)| t.text.clone());
                                (l, 2)
                            } else {
                                (None, 0)
                            };
                            let Some((t, span)) = s.tok(tok_at) else { continue };
                            let is_string = sp == p.sym_str
                                || sp == p.sym_labeled_str
                                || sp == p.sym_str_opt
                                || sp == p.sym_str_star
                                || sp == p.sym_str_plus;
                            let postfix = if sp == p.sym_name_opt || sp == p.sym_str_opt {
                                Some(SugarOp::Opt)
                            } else if sp == p.sym_name_star || sp == p.sym_str_star {
                                Some(SugarOp::Star)
                            } else if sp == p.sym_name_plus || sp == p.sym_str_plus {
                                Some(SugarOp::Plus)
                            } else {
                                None
                            };
                            let text =
                                if is_string { unquote(&t.text) } else { t.text.clone() };
                            syms.push(SymIr { label: label_s, text, span, is_string, postfix });
                        }
                    }
                    let attrs = a.node(3).map(|x| lower_attrs(x, p)).unwrap_or_default();
                    alts.push(AltIr { label: label.text.clone(), label_span, syms, attrs });
                }
            }
            ir.rules.push(RuleIr {
                name: name.text.clone(),
                name_span,
                body: RuleBody::Alts(alts),
            });
        } else if prod == p.rule_star || prod == p.rule_plus || prod == p.rule_opt {
            let Some((name, name_span)) = d.tok(1) else { continue };
            let op = if prod == p.rule_star {
                SugarOp::Star
            } else if prod == p.rule_plus {
                SugarOp::Plus
            } else {
                SugarOp::Opt
            };
            let Some(elem) = d.node(3).and_then(|e| lower_elem(e, p)) else { continue };
            let sep = if prod == p.rule_opt {
                None
            } else {
                d.node(5).and_then(|rs| {
                    (rs.n.prod == p.rep_sep_some as u16)
                        .then(|| rs.node(1).and_then(|e| lower_elem(e, p)))
                        .flatten()
                })
            };
            ir.rules.push(RuleIr {
                name: name.text.clone(),
                name_span,
                body: RuleBody::Sugar { op, elem, sep },
            });
        }
    }
    ir
}

// ---------------------------------------------------------------------------
// Compilation passes
// ---------------------------------------------------------------------------

pub fn compile(tree: &GreenNode, p: &QanaProds) -> (LangDef, Vec<QanaDiag>) {
    let mut diags = Vec::new();
    let ir = lower(tree, p, &mut diags);
    let d = &mut diags;
    let error = |d: &mut Vec<QanaDiag>, span: (u32, u32), msg: String| {
        d.push(QanaDiag { span, msg, severity: 1 })
    };

    let lang_name = ir.language.as_ref().map(|(n, _)| n.clone()).unwrap_or_else(|| "Lang".into());
    let mode_names: Vec<String> = std::iter::once("DEFAULT".to_string())
        .chain(ir.mode_names.iter().map(|(n, _)| n.clone()))
        .collect();
    let mode_refs: Vec<&str> = mode_names.iter().map(|s| s.as_str()).collect();
    let mut lex = LexGrammar::new(&lang_name, &mode_refs);
    // Per-mode `eol` agreement across pushes (`@push(M, eol)`).
    let mut mode_eol: HashMap<u16, (bool, (u32, u32))> = HashMap::new();
    if let Some((n, _)) = ir.max_stack {
        lex.max_stack = Some(n);
    }

    // ---- tokens (ids in declaration order) ----
    let mut token_ids: HashMap<String, TokenId> = HashMap::new();
    let mut token_spans: Vec<(u32, u32)> = Vec::new();
    let mut lit_text: HashMap<String, TokenId> = HashMap::new();
    let mut styles = Styles::new(LEGEND.to_vec());
    let mut style_reqs: Vec<(TokenId, String, (u32, u32))> = Vec::new();

    let mut kw_text: HashMap<String, TokenId> = HashMap::new();
    // (base name, base span, indices into lex.keywords) — the OWNER id
    // is patched after the token pass (a keywords declaration may
    // precede its base token's declaration).
    let mut kw_groups: Vec<(String, (u32, u32), Vec<usize>)> = Vec::new();
    for ev in &ir.events {
        let t = match ev {
            IdEvent::Def(t) => t,
            IdEvent::Keywords { base, base_span, items } => {
                let mut indices = Vec::new();
                for item in items {
                    if kw_text.contains_key(&item.text) {
                        error(d, item.span, format!("keyword `{}` declared twice", item.text));
                        continue;
                    }
                    let id = lex.add(TokenDef::new(
                        &format!("KW_{}", item.text.to_uppercase()),
                        0,
                        Pat::Never,
                    ));
                    indices.push(lex.keywords.len());
                    lex.keywords.push((item.text.clone(), id, 0));
                    kw_text.insert(item.text.clone(), id);
                    token_spans.push(item.span);
                    styles.set(id, "keyword");
                }
                kw_groups.push((base.clone(), *base_span, indices));
                continue;
            }
        };
        if token_ids.contains_key(&t.name) {
            error(d, t.name_span, format!("token `{}` declared twice", t.name));
            continue;
        }
        let pat = if t.pat_is_regex {
            match parse_pattern(&t.pat_src) {
                Ok(p) => p,
                Err(e) => {
                    let at = t.pat_span.0 + 1 + e.pos as u32;
                    error(d, (at, at + 1), format!("pattern: {}", e.msg));
                    Pat::Never
                }
            }
        } else {
            if t.pat_src.is_empty() {
                error(d, t.pat_span, "literal token text cannot be empty".into());
            }
            Pat::Lit(t.pat_src.clone())
        };
        let mut def = TokenDef::new(&t.name, t.mode, pat.clone());
        let mut classes: Vec<(String, (u32, u32))> = Vec::new();
        for a in &t.attrs {
            let arity = |d: &mut Vec<QanaDiag>, want: usize| {
                if a.args.len() != want {
                    error(
                        d,
                        a.name_span,
                        format!("@{} takes {} argument(s), got {}", a.name, want, a.args.len()),
                    );
                    false
                } else {
                    true
                }
            };
            match a.name.as_str() {
                "trivia" => def.trivia = true,
                "error" => def.error = true,
                "specialize" => def.specialize = true,
                // Line splice (C's backslash-newline): as the line's
                // final token it keeps the eol-bounded mode alive into
                // the next line. The envelope check refuses it in
                // modes that aren't `@push(M, eol)`.
                "continues" => def.continues = true,
                "pop" => def.action = qana_grammar::model::Action::Pop,
                "push" => {
                    if a.args.is_empty() || a.args.len() > 2 {
                        error(d, a.name_span, "@push takes a mode and optionally `eol`".into());
                    } else {
                        let (m, ms, _) = &a.args[0];
                        let eol = match a.args.get(1) {
                            None => false,
                            Some((w, _, _)) if w == "eol" => true,
                            Some((w, ws, _)) => {
                                error(d, *ws, format!("unknown @push option `{w}` (only `eol`)"));
                                false
                            }
                        };
                        match mode_names.iter().position(|n| n == m) {
                            Some(i) => {
                                def.action = qana_grammar::model::Action::Push(i as u16);
                                // Line-boundedness is a property of the
                                // MODE: every push must agree.
                                match mode_eol.get(&(i as u16)) {
                                    Some(&(prev, _)) if prev != eol => error(
                                        d,
                                        *ms,
                                        format!("mode `{m}` is pushed both with and without `eol`"),
                                    ),
                                    _ => {
                                        mode_eol.insert(i as u16, (eol, *ms));
                                    }
                                }
                            }
                            None => error(d, *ms, format!("unknown mode `{m}`")),
                        }
                    }
                }
                "style" => {
                    if arity(d, 1) {
                        let (c, cs, _) = &a.args[0];
                        classes.push((c.clone(), *cs));
                    }
                }
                other => error(
                    d,
                    a.name_span,
                    format!(
                        "unknown token attribute `@{other}` (expected @trivia, @error, @specialize, @style, @push, @pop, @continues)"
                    ),
                ),
            }
        }
        let id = lex.add(def);
        for (class, span) in classes {
            style_reqs.push((id, class, span));
        }
        token_ids.insert(t.name.clone(), id);
        token_spans.push(t.name_span);
        // Only DEFAULT-mode, non-trivia tokens claim literal spellings
        // in rules. A mode-local literal (`"if"` in a preprocessor
        // mode, `/\*/` in a comment mode) silently shadowing the base
        // language's token of the same spelling compiled IfStmt against
        // a token the base mode can never produce — both cases found by
        // the C exerciser. Mode-local tokens are referenced BY NAME.
        let is_trivia = t.attrs.iter().any(|a| a.name == "trivia");
        if let (Pat::Lit(s), false, 0) = (&pat, is_trivia, t.mode) {
            lit_text.entry(s.clone()).or_insert(id);
        }
    }

    // Keyword bases validate against the COMPLETE token map, and each
    // group's entries get their OWNER patched in (specialization is
    // per-owner).
    for (base, base_span, indices) in &kw_groups {
        match token_ids.get(base) {
            None => error(d, *base_span, format!("unknown token `{base}`")),
            Some(&id) => {
                if !lex.tokens[id as usize].specialize {
                    error(
                        d,
                        *base_span,
                        format!("keyword base token `{base}` must be declared @specialize"),
                    );
                }
                for &i in indices {
                    lex.keywords[i].2 = id;
                }
            }
        }
    }

    // Apply @style requests (legend is a closed set).
    for (id, class, span) in &style_reqs {
        match LEGEND.iter().find(|e| *e == class) {
            Some(entry) => styles.set(*id, entry),
            None => error(
                d,
                *span,
                format!("unknown style class `{class}` (legend: {})", LEGEND.join(", ")),
            ),
        }
    }

    // ---- brackets ----
    let resolve_tok = |r: &TokRefIr,
                       token_ids: &HashMap<String, TokenId>,
                       d: &mut Vec<QanaDiag>|
     -> Option<TokenId> {
        if r.is_string {
            if let Some(&id) = lit_text.get(&r.text) {
                return Some(id);
            }
            if let Some(&id) = kw_text.get(&r.text) {
                return Some(id);
            }
            error(
                d,
                r.span,
                format!("`\"{}\"` is not a declared token text or keyword", r.text),
            );
            None
        } else {
            match token_ids.get(&r.text) {
                Some(&id) => Some(id),
                None => {
                    error(d, r.span, format!("unknown token `{}`", r.text));
                    None
                }
            }
        }
    };

    for (open, close) in &ir.brackets {
        let (Some(o), Some(c)) =
            (resolve_tok(open, &token_ids, d), resolve_tok(close, &token_ids, d))
        else {
            continue;
        };
        let kind = match lex.tokens[o as usize].pat {
            Pat::Lit(ref s) if s == "(" => Some((BracketKind::Paren, ")")),
            Pat::Lit(ref s) if s == "[" => Some((BracketKind::Bracket, "]")),
            Pat::Lit(ref s) if s == "{" => Some((BracketKind::Brace, "}")),
            _ => None,
        };
        let Some((kind, want_close)) = kind else {
            error(d, open.span, "bracket pairs must open with `(`, `[`, or `{`".into());
            continue;
        };
        match &lex.tokens[c as usize].pat {
            Pat::Lit(s) if s == want_close => {}
            _ => {
                error(d, close.span, format!("expected the matching `{want_close}` token"));
                continue;
            }
        }
        lex.tokens[o as usize].bracket = Some((kind, true));
        lex.tokens[c as usize].bracket = Some((kind, false));
    }

    // ---- precedence (level = declaration order, later binds tighter) ----
    let mut prec_levels: HashMap<TokenId, (u8, Assoc)> = HashMap::new();
    let mut prec_list: Vec<(TokenId, u8, Assoc)> = Vec::new();
    for (i, (assoc, ops)) in ir.precs.iter().enumerate() {
        let level = (i + 1) as u8;
        for r in ops {
            if let Some(id) = resolve_tok(r, &token_ids, d) {
                if prec_levels.insert(id, (level, *assoc)).is_some() {
                    error(d, r.span, format!("`{}` already has a precedence", r.text));
                }
                prec_list.push((id, level, *assoc));
            }
        }
    }

    // ---- rules ----
    for (&m, &(eol, _)) in &mode_eol {
        if eol {
            lex.eol_pop[m as usize] = true;
        }
    }
    let vocab = Vocab::of(&lex);
    let mut sg = SynGrammar::new(&format!("{lang_name}Syn"), vocab.names.clone());
    let mut nt_ids: HashMap<String, u16> = HashMap::new();
    for r in &ir.rules {
        if nt_ids.contains_key(&r.name) {
            error(d, r.name_span, format!("rule `{}` declared twice", r.name));
            continue;
        }
        if token_ids.contains_key(&r.name) {
            error(d, r.name_span, format!("`{}` is already a token name", r.name));
            continue;
        }
        nt_ids.insert(r.name.clone(), sg.nt(&r.name));
        // `X* % SEP` needs a nonempty inner list rule, declared right
        // after its wrapper (deterministic ids; matches the hand-written
        // `args`/`args_ne` convention).
        if let RuleBody::Sugar { op: SugarOp::Star, sep: Some(_), .. } = &r.body {
            let inner = format!("{}_ne", r.name);
            if nt_ids.contains_key(&inner) || token_ids.contains_key(&inner) {
                error(
                    d,
                    r.name_span,
                    format!("generated rule `{inner}` collides with an existing declaration"),
                );
            } else {
                nt_ids.insert(inner.clone(), sg.nt(&inner));
            }
        }
    }
    for (id, level, assoc) in &prec_list {
        sg.set_token_prec(*id, *level, *assoc);
    }

    // Symbol resolution, shared by explicit productions, sugar forms,
    // and helper collection (`diags: None` probes silently — errors are
    // reported once, at production emission).
    #[allow(clippy::too_many_arguments)]
    fn resolve_symbol(
        text: &str,
        is_string: bool,
        span: (u32, u32),
        token_ids: &HashMap<String, TokenId>,
        nt_ids: &HashMap<String, u16>,
        lit_text: &HashMap<String, TokenId>,
        kw_text: &HashMap<String, TokenId>,
        diags: Option<&mut Vec<QanaDiag>>,
    ) -> Option<Sym> {
        let err = |diags: Option<&mut Vec<QanaDiag>>, msg: String| {
            if let Some(dd) = diags {
                dd.push(QanaDiag { span, msg, severity: 1 });
            }
        };
        if is_string {
            match lit_text.get(text).or_else(|| kw_text.get(text)) {
                Some(&id) => Some(Sym::T(id)),
                None => {
                    err(
                        diags,
                        format!(
                            "`\"{text}\"` is not a declared token text or keyword — declare it (fixed token or `keywords` entry)"
                        ),
                    );
                    None
                }
            }
        } else if let Some(&id) = token_ids.get(text) {
            Some(Sym::T(id))
        } else if let Some(&nt2) = nt_ids.get(text) {
            Some(Sym::N(nt2))
        } else {
            err(diags, format!("cannot find token or rule `{text}`"));
            None
        }
    }

    // Inline-postfix helpers: ONE shared nonterminal per (element, op),
    // declared after every explicit rule, in first-use order — so
    // `expr?` anywhere in the grammar is the same `expr_opt` rule.
    struct Helper {
        nt: u16,
        sym: Sym,
        op: SugarOp,
        span: (u32, u32),
    }
    let mut helper_key: HashMap<(Sym, SugarOp), u16> = HashMap::new();
    let mut helper_list: Vec<Helper> = Vec::new();
    for r in &ir.rules {
        let RuleBody::Alts(alts) = &r.body else { continue };
        for alt in alts {
            for s in &alt.syms {
                let Some(op) = s.postfix else { continue };
                let Some(sym) =
                    resolve_symbol(&s.text, s.is_string, s.span, &token_ids, &nt_ids, &lit_text, &kw_text, None)
                else {
                    continue;
                };
                if op != SugarOp::Opt && matches!(sym, Sym::T(_)) {
                    continue; // refused (with a hint) at production emission
                }
                if helper_key.contains_key(&(sym, op)) {
                    continue;
                }
                let base = match sym {
                    Sym::T(t) => sg.term_name(t).to_lowercase(),
                    Sym::N(n2) => sg.nt_names[n2 as usize].clone(),
                };
                let hname = format!("{base}_{}", op.suffix());
                if nt_ids.contains_key(&hname) || token_ids.contains_key(&hname) {
                    error(
                        d,
                        s.span,
                        format!("generated rule `{hname}` collides with an existing declaration"),
                    );
                    continue;
                }
                let hnt = sg.nt(&hname);
                nt_ids.insert(hname, hnt);
                helper_key.insert((sym, op), hnt);
                helper_list.push(Helper { nt: hnt, sym, op, span: s.span });
            }
        }
    }

    let mut binding = BindingConfig::default();
    let mut macros = qana_sem::macros::MacroConfig::default();
    let mut type_cfg = TypeConfig::default();
    let mut outline = OutlineConfig::default();
    let mut prod_spans: Vec<(u32, u32)> = Vec::new();
    let mut labels_seen: HashMap<String, ()> = HashMap::new();

    fn gen_prod(
        sg: &mut SynGrammar,
        labels_seen: &mut HashMap<String, ()>,
        prod_spans: &mut Vec<(u32, u32)>,
        d: &mut Vec<QanaDiag>,
        nt: u16,
        name: String,
        rhs: Vec<Sym>,
        span: (u32, u32),
    ) {
        if labels_seen.insert(name.clone(), ()).is_some() {
            d.push(QanaDiag {
                span,
                msg: format!("generated production name `{name}` collides with an existing label"),
                severity: 1,
            });
        }
        sg.prod_named(nt, &name, rhs);
        prod_spans.push(span);
    }

    const WRAP_HINT: &str = "repetition elements must be rules — wrap the token in a single-alternative rule to get a balanced, typed list";

    for r in &ir.rules {
        let Some(&nt) = nt_ids.get(&r.name) else { continue };
        let alts = match &r.body {
            RuleBody::Sugar { op, elem, sep } => {
                let c = SynGrammar::camel_name(&r.name);
                let Some(esym) = resolve_symbol(
                    &elem.text, elem.is_string, elem.span, &token_ids, &nt_ids, &lit_text, &kw_text, Some(d),
                ) else {
                    continue;
                };
                if *op != SugarOp::Opt && matches!(esym, Sym::T(_)) {
                    error(d, elem.span, WRAP_HINT.into());
                    continue;
                }
                let sepsym = match sep {
                    None => None,
                    Some(sref) => match resolve_symbol(
                        &sref.text, sref.is_string, sref.span, &token_ids, &nt_ids, &lit_text, &kw_text, Some(d),
                    ) {
                        Some(Sym::T(t)) => Some(t),
                        Some(Sym::N(_)) => {
                            error(d, sref.span, "separators must be tokens".into());
                            continue;
                        }
                        None => continue,
                    },
                };
                let sp = r.name_span;
                let g = &mut sg;
                match (op, sepsym) {
                    (SugarOp::Opt, _) => {
                        gen_prod(g, &mut labels_seen, &mut prod_spans, d, nt, format!("{c}None"), vec![], sp);
                        gen_prod(g, &mut labels_seen, &mut prod_spans, d, nt, format!("{c}Some"), vec![esym], sp);
                    }
                    (SugarOp::Star, None) => {
                        gen_prod(g, &mut labels_seen, &mut prod_spans, d, nt, format!("{c}Empty"), vec![], sp);
                        gen_prod(g, &mut labels_seen, &mut prod_spans, d, nt, format!("{c}More"), vec![Sym::N(nt), esym], sp);
                    }
                    (SugarOp::Plus, None) => {
                        gen_prod(g, &mut labels_seen, &mut prod_spans, d, nt, format!("{c}First"), vec![esym], sp);
                        gen_prod(g, &mut labels_seen, &mut prod_spans, d, nt, format!("{c}More"), vec![Sym::N(nt), esym], sp);
                    }
                    (SugarOp::Plus, Some(s)) => {
                        gen_prod(g, &mut labels_seen, &mut prod_spans, d, nt, format!("{c}First"), vec![esym], sp);
                        gen_prod(g, &mut labels_seen, &mut prod_spans, d, nt, format!("{c}More"), vec![Sym::N(nt), Sym::T(s), esym], sp);
                    }
                    (SugarOp::Star, Some(s)) => {
                        let inner_name = format!("{}_ne", r.name);
                        let Some(&inner) = nt_ids.get(&inner_name) else { continue };
                        let ci = SynGrammar::camel_name(&inner_name);
                        gen_prod(g, &mut labels_seen, &mut prod_spans, d, nt, format!("{c}None"), vec![], sp);
                        gen_prod(g, &mut labels_seen, &mut prod_spans, d, nt, format!("{c}Some"), vec![Sym::N(inner)], sp);
                        gen_prod(g, &mut labels_seen, &mut prod_spans, d, inner, format!("{ci}First"), vec![esym], sp);
                        gen_prod(g, &mut labels_seen, &mut prod_spans, d, inner, format!("{ci}More"), vec![Sym::N(inner), Sym::T(s), esym], sp);
                    }
                }
                continue;
            }
            RuleBody::Alts(alts) => alts,
        };
        for alt in alts {
            if labels_seen.insert(alt.label.clone(), ()).is_some() {
                error(
                    d,
                    alt.label_span,
                    format!("alternative label `{}` used twice (labels name typed-AST types)", alt.label),
                );
            }
            let mut rhs: Vec<Sym> = Vec::new();
            let mut positions: HashMap<&str, usize> = HashMap::new();
            for s in alt.syms.iter() {
                let Some(mut sym) = resolve_symbol(
                    &s.text, s.is_string, s.span, &token_ids, &nt_ids, &lit_text, &kw_text, Some(d),
                ) else {
                    continue;
                };
                if let Some(op) = s.postfix {
                    if op != SugarOp::Opt && matches!(sym, Sym::T(_)) {
                        error(d, s.span, WRAP_HINT.into());
                        continue;
                    }
                    match helper_key.get(&(sym, op)) {
                        Some(&h) => sym = Sym::N(h),
                        None => continue, // collision reported during collection
                    }
                }
                // Position = index in the FINAL rhs (robust when an
                // earlier sym failed to resolve under errors).
                if let Some(l) = &s.label {
                    positions.insert(l.as_str(), rhs.len());
                }
                rhs.push(sym);
            }
            let prod = sg.prod_named(nt, &alt.label, rhs) as u16;
            prod_spans.push(alt.label_span);

            // `@type`, `@export`, and `@import` are resolved AFTER the
            // other attributes: they read the binding entries `@def`/
            // `@ref` create, and attribute order must not matter.
            // (`@import` runs before `@type` so `@type(ref)` can flow a
            // foreign type through an import.)
            let mut pending_type_ir: Vec<usize> = Vec::new();
            let mut pending_export: Option<(u32, u32)> = None;
            let mut pending_import: Option<usize> = None;
            let mut pending_ns: Option<usize> = None;
            let mut pending_macro: Option<usize> = None;
            let mut pending_splice: Option<usize> = None;
            let mut pending_reflect: Option<usize> = None;
            let mut pending_module: Option<usize> = None;
            let mut pending_qualify: Option<usize> = None;

            for (ai, a) in alt.attrs.iter().enumerate() {
                let pos_of = |d: &mut Vec<QanaDiag>, arg: &(String, (u32, u32), bool)| -> Option<usize> {
                    match positions.get(arg.0.as_str()) {
                        Some(&k) => Some(k),
                        None => {
                            error(
                                d,
                                arg.1,
                                format!("no symbol labeled `{}` in this alternative", arg.0),
                            );
                            None
                        }
                    }
                };
                match a.name.as_str() {
                    "def" => {
                        if a.args.len() == 1 {
                            if let Some(k) = pos_of(d, &a.args[0]) {
                                binding.defs.push((nt, prod, k));
                            }
                        } else {
                            error(d, a.name_span, "@def takes one label argument".into());
                        }
                    }
                    "ref" => {
                        if a.args.is_empty() || a.args.len() > 2 {
                            error(d, a.name_span, "@ref takes a label and an optional kind".into());
                        } else if let Some(k) = pos_of(d, &a.args[0]) {
                            let kind = match a.args.get(1).map(|x| x.0.as_str()) {
                                None | Some("var") => RefKind::Var,
                                Some("call") => RefKind::Call,
                                Some(other) => {
                                    error(
                                        d,
                                        a.args[1].1,
                                        format!("unknown ref kind `{other}` (var, call)"),
                                    );
                                    RefKind::Var
                                }
                            };
                            binding.refs.push((nt, prod, k, kind));
                        }
                    }
                    "scope" => {
                        // `@scope` = ordered lexical scope; `@scope(unordered)`
                        // = declaration-language scope (forward refs legal).
                        let unordered = match a.args.first() {
                            None => false,
                            Some((arg, _, _)) if arg == "unordered" => true,
                            Some((arg, span, _)) => {
                                error(
                                    d,
                                    *span,
                                    format!("unknown scope kind `{arg}` (only `unordered`)"),
                                );
                                false
                            }
                        };
                        binding.scopes.push((nt, prod, unordered, false));
                    }
                    "outline" => {
                        if a.args.is_empty() || a.args.len() > 2 {
                            error(d, a.name_span, "@outline takes a label and an optional kind".into());
                        } else if let Some(k) = pos_of(d, &a.args[0]) {
                            let kind = match a.args.get(1) {
                                None => "variable",
                                Some(arg) => match OUTLINE_KINDS.iter().find(|e| **e == arg.0) {
                                    Some(e) => e,
                                    None => {
                                        error(
                                            d,
                                            arg.1,
                                            format!(
                                                "unknown outline kind `{}` ({})",
                                                arg.0,
                                                OUTLINE_KINDS.join(", ")
                                            ),
                                        );
                                        "variable"
                                    }
                                },
                            };
                            outline.entries.push(OutlineEntry { nt, prod, name_child: k, kind });
                        }
                    }
                    // Spelled in full: `prec` is a reserved word of the
                    // surface, so `@prec` lexes as a keyword and can
                    // never reach an attribute name.
                    "type" => pending_type_ir.push(ai),
                    "export" => {
                        if !a.args.is_empty() {
                            error(d, a.name_span, "@export takes no arguments".into());
                        } else if pending_export.replace(a.name_span).is_some() {
                            error(d, a.name_span, "duplicate @export".into());
                        }
                    }
                    "import" => {
                        if a.args.len() != 1 {
                            error(d, a.name_span, "@import takes one label argument".into());
                        } else if pending_import.replace(ai).is_some() {
                            error(d, a.name_span, "duplicate @import".into());
                        }
                    }
                    "module" => {
                        if a.args.len() != 1 {
                            error(d, a.name_span, "@module takes one body label argument".into());
                        } else if pending_module.replace(ai).is_some() {
                            error(d, a.name_span, "duplicate @module".into());
                        }
                    }
                    "qualify" => {
                        if a.args.len() != 2 {
                            error(d, a.name_span, "@qualify takes base and name labels".into());
                        } else if pending_qualify.replace(ai).is_some() {
                            error(d, a.name_span, "duplicate @qualify".into());
                        }
                    }
                    "ns" => {
                        // `@ns(tag)` — this alternative's def and refs
                        // live in a NAMED namespace. Named namespaces
                        // resolve hoisted (order-free) in every scope:
                        // per-namespace ordering as declared data.
                        if a.args.len() != 1 || a.args[0].2 {
                            error(d, a.name_span, "@ns takes one bare namespace name".into());
                        } else if pending_ns.replace(ai).is_some() {
                            error(d, a.name_span, "duplicate @ns".into());
                        }
                    }
                    "macro" => {
                        // `@macro([params,] body)` — the meta tier: this
                        // alternative's @def introduces a MACRO whose
                        // parameters are the defs inside `params` and
                        // whose template is the `body` child.
                        if a.args.is_empty() || a.args.len() > 2 {
                            error(d, a.name_span, "@macro takes ([params,] body) labels".into());
                        } else if pending_macro.replace(ai).is_some() {
                            error(d, a.name_span, "duplicate @macro".into());
                        }
                    }
                    "splice" => {
                        // `@splice(name[, args])` — expansion may happen
                        // HERE: when `name`'s @ref resolves to a macro,
                        // this node is replaced by the substituted body.
                        if a.args.is_empty() || a.args.len() > 2 {
                            error(d, a.name_span, "@splice takes (name[, args]) labels".into());
                        } else if pending_splice.replace(ai).is_some() {
                            error(d, a.name_span, "duplicate @splice".into());
                        }
                    }
                    "reflect" => {
                        // `@reflect(ty[, "sep"[, facet…]])` — this
                        // @splice's macro iterates the resolved type's
                        // declared members, its parameters binding to
                        // the named FACETS in order (default: the
                        // member's name, then its declared type).
                        if a.args.is_empty() {
                            error(d, a.name_span, "@reflect takes (ty[, \"sep\"[, facet…]])".into());
                        } else if a.args.len() >= 2 && !a.args[1].2 {
                            error(d, a.args[1].1, "@reflect's separator must be a string".into());
                        } else if pending_reflect.replace(ai).is_some() {
                            error(d, a.name_span, "duplicate @reflect".into());
                        }
                    }
                    "precedence" => {
                        if a.args.len() != 1 {
                            error(d, a.name_span, "@precedence takes one token argument".into());
                        } else {
                            let arg = &a.args[0];
                            let r = TokRefIr { text: arg.0.clone(), span: arg.1, is_string: arg.2 };
                            if let Some(id) = resolve_tok(&r, &token_ids, d) {
                                match prec_levels.get(&id) {
                                    Some(&(level, _)) => {
                                        let idx = sg.prods.len() - 1;
                                        sg.prods[idx].prec = Some(level);
                                    }
                                    None => error(
                                        d,
                                        arg.1,
                                        format!("`{}` has no declared precedence", arg.0),
                                    ),
                                }
                            }
                        }
                    }
                    other => error(
                        d,
                        a.name_span,
                        format!(
                            "unknown alternative attribute `@{other}` (expected @def, @ref, @scope, @outline, @precedence, @type, @export, @import, @module, @qualify, @ns, @macro, @splice)"
                        ),
                    ),
                }
            }

            // ---- the module tier: `@export` / `@import` ----
            if let Some(span) = pending_export {
                if binding.defs.iter().any(|e| e.0 == nt && e.1 == prod) {
                    binding.exports.push((nt, prod));
                } else {
                    error(d, span, "@export requires @def on the same alternative".into());
                }
            }
            // ---- per-namespace ordering: `@ns` ----
            if let Some(ai) = pending_ns {
                let a = &alt.attrs[ai];
                let has_site = binding.defs.iter().any(|e| e.0 == nt && e.1 == prod)
                    || binding.refs.iter().any(|r| r.0 == nt && r.1 == prod);
                if has_site {
                    binding.namespaces.push((nt, prod, a.args[0].0.clone()));
                } else {
                    error(d, a.name_span, "@ns requires @def or @ref on the same alternative".into());
                }
            }
            if let Some(ai) = pending_import {
                let a = &alt.attrs[ai];
                let arg = &a.args[0];
                match positions.get(arg.0.as_str()) {
                    None => error(
                        d,
                        arg.1,
                        format!("no symbol labeled `{}` in this alternative", arg.0),
                    ),
                    Some(&k) => match sg.prods[prod as usize].rhs.get(k) {
                        Some(Sym::T(_)) => {
                            if binding.refs.iter().any(|r| r.0 == nt && r.1 == prod) {
                                error(
                                    d,
                                    a.name_span,
                                    "at most one reference per alternative (@ref or @import)".into(),
                                );
                            } else {
                                binding.refs.push((nt, prod, k, RefKind::Import));
                            }
                        }
                        _ => error(
                            d,
                            arg.1,
                            format!("`{}` labels a rule — the imported name must be a token", arg.0),
                        ),
                    },
                }
            }

            if let Some(ai) = pending_module {
                let a = &alt.attrs[ai];
                match (rule_pos_of(d, &positions, &sg, prod, &a.args[0]), binding.defs.iter().any(|e| e.0 == nt && e.1 == prod)) {
                    (Some(body), true) => binding.modules.push((nt, prod, body)),
                    (_, false) => {
                        error(d, a.name_span, "@module requires @def on the same alternative".into())
                    }
                    _ => {}
                }
            }
            if let Some(ai) = pending_qualify {
                let a = &alt.attrs[ai];
                let base = match positions.get(a.args[0].0.as_str()) {
                    Some(&k) => Some(k),
                    None => {
                        error(
                            d,
                            a.args[0].1,
                            format!("no symbol labeled `{}` in this alternative", a.args[0].0),
                        );
                        None
                    }
                };
                let name = match positions.get(a.args[1].0.as_str()) {
                    None => {
                        error(
                            d,
                            a.args[1].1,
                            format!("no symbol labeled `{}` in this alternative", a.args[1].0),
                        );
                        None
                    }
                    Some(&k) => match sg.prods[prod as usize].rhs.get(k) {
                        Some(Sym::T(_)) => Some(k),
                        _ => {
                            error(
                                d,
                                a.args[1].1,
                                format!("`{}` labels a rule — the qualified name must be a token", a.args[1].0),
                            );
                            None
                        }
                    },
                };
                if let (Some(base_child), Some(name_child)) = (base, name) {
                    if binding.refs.iter().any(|r| r.0 == nt && r.1 == prod && r.3 == RefKind::Import) {
                        error(d, a.name_span, "@qualify cannot combine with @import".into());
                    } else {
                        binding.refs.push((nt, prod, name_child, RefKind::Qualified));
                        binding.quals.push((nt, prod, base_child, name_child));
                    }
                }
            }

            // ---- the meta tier: `@macro` / `@splice` ----
            if let Some(ai) = pending_macro {
                let a = &alt.attrs[ai];
                if !binding.defs.iter().any(|e| e.0 == nt && e.1 == prod) {
                    error(d, a.name_span, "@macro requires @def on the same alternative".into());
                } else {
                    let (pargs, barg) = match a.args.len() {
                        1 => (None, &a.args[0]),
                        _ => (Some(&a.args[0]), &a.args[1]),
                    };
                    let params = match pargs {
                        None => Some(None),
                        Some(p) => rule_pos_of(d, &positions, &sg, prod, p).map(Some),
                    };
                    let body = rule_pos_of(d, &positions, &sg, prod, barg);
                    if let (Some(params), Some(body)) = (params, body) {
                        macros.defs.push((nt, prod, params, body));
                    }
                }
            }
            if let Some(ai) = pending_splice {
                let a = &alt.attrs[ai];
                let name = match positions.get(a.args[0].0.as_str()) {
                    Some(&k) if binding.refs.iter().any(|r| r.0 == nt && r.1 == prod && r.2 == k) => {
                        Some(k)
                    }
                    Some(_) => {
                        error(
                            d,
                            a.args[0].1,
                            "@splice's name must carry @ref on the same alternative".into(),
                        );
                        None
                    }
                    None => {
                        error(
                            d,
                            a.args[0].1,
                            format!("no symbol labeled `{}` in this alternative", a.args[0].0),
                        );
                        None
                    }
                };
                let args = match a.args.get(1) {
                    None => Some(None),
                    Some(arg) => rule_pos_of(d, &positions, &sg, prod, arg).map(Some),
                };
                if let (Some(name), Some(args)) = (name, args) {
                    macros.uses.push((nt, prod, name, args));
                }
            }
            if let Some(ai) = pending_reflect {
                let a = &alt.attrs[ai];
                if !macros.uses.iter().any(|u| u.0 == nt && u.1 == prod) {
                    error(d, a.name_span, "@reflect requires @splice on the same alternative".into());
                } else {
                    let ty = match positions.get(a.args[0].0.as_str()) {
                        Some(&k)
                            if binding.refs.iter().any(|r| r.0 == nt && r.1 == prod && r.2 == k) =>
                        {
                            Some(k)
                        }
                        Some(_) => {
                            error(
                                d,
                                a.args[0].1,
                                "@reflect's type must carry @ref on the same alternative".into(),
                            );
                            None
                        }
                        None => {
                            error(
                                d,
                                a.args[0].1,
                                format!("no symbol labeled `{}` in this alternative", a.args[0].0),
                            );
                            None
                        }
                    };
                    // Facets, in the macro's parameter order.
                    let mut facets = Vec::new();
                    for arg in a.args.iter().skip(2) {
                        if arg.2 {
                            error(d, arg.1, "a @reflect facet is a bare name".into());
                            continue;
                        }
                        match qana_sem::macros::Facet::parse(&arg.0) {
                            Some(f) => facets.push(f),
                            None => error(
                                d,
                                arg.1,
                                format!(
                                    "unknown reflection facet `{}` ({})",
                                    arg.0,
                                    qana_sem::macros::Facet::NAMES.join(", ")
                                ),
                            ),
                        }
                    }
                    if facets.is_empty() {
                        facets = vec![
                            qana_sem::macros::Facet::Name,
                            qana_sem::macros::Facet::Type,
                        ];
                    }
                    if let Some(ty) = ty {
                        let sep = a.args.get(1).map(|s| s.0.clone()).unwrap_or_else(|| " ".into());
                        macros.reflects.push((nt, prod, ty, sep, facets));
                    }
                }
            }

            // ---- the declared type tier: `@type(…)` forms ----
            let mut type_declared = false;
            for ai in pending_type_ir {
                let a = &alt.attrs[ai];
                if std::mem::replace(&mut type_declared, true) {
                    error(d, a.name_span, "at most one @type per alternative".into());
                    continue;
                }
                // A term of the vocabulary: Capitalized = atom (interned
                // into the grammar's own vocabulary), lowercase = local
                // type variable (sig only). Strings/numbers are neither.
                let term = |cfg: &mut TypeConfig,
                            d: &mut Vec<QanaDiag>,
                            vars: &mut Vec<String>,
                            arg: &(String, (u32, u32), bool)|
                 -> Option<TyTerm> {
                    let (text, span, is_string) = arg;
                    let first = text.chars().next().unwrap_or('0');
                    if *is_string || !first.is_ascii_alphabetic() {
                        error(d, *span, "type terms are names: `Atom` or a lowercase variable".into());
                        return None;
                    }
                    if first.is_ascii_uppercase() {
                        return Some(TyTerm::Atom(cfg.intern(text)));
                    }
                    let v = match vars.iter().position(|v| v == text) {
                        Some(i) => i,
                        None => {
                            vars.push(text.clone());
                            vars.len() - 1
                        }
                    };
                    if v >= 26 {
                        error(d, *span, "too many type variables in one signature".into());
                        return None;
                    }
                    Some(TyTerm::Var(v as u8))
                };
                // Label → symbol position, and the position must hold a
                // RULE (types attach to nodes; tokens are untyped).
                let rule_pos = |d: &mut Vec<QanaDiag>, arg: &(String, (u32, u32), bool)| -> Option<usize> {
                    let k = match positions.get(arg.0.as_str()) {
                        Some(&k) => k,
                        None => {
                            error(d, arg.1, format!("no symbol labeled `{}` in this alternative", arg.0));
                            return None;
                        }
                    };
                    match sg.prods[prod as usize].rhs.get(k) {
                        Some(Sym::N(_)) => Some(k),
                        _ => {
                            error(d, arg.1, format!("`{}` labels a token — @type sources must be rule symbols", arg.0));
                            None
                        }
                    }
                };
                let head = a.args.first().map(|x| x.0.as_str()).unwrap_or("");
                let rule: Option<TypeRule> = match (head, a.args.len()) {
                    (_, 0) => {
                        error(d, a.name_span, "@type needs arguments: an Atom, `ref`, `of, label`, `def, label`, or `sig, …`".into());
                        None
                    }
                    ("ref", 1) => match binding.refs.iter().find(|r| r.0 == nt && r.1 == prod) {
                        Some(&(_, _, k, _)) => Some(TypeRule::FromRef { ref_child: k }),
                        None => {
                            error(d, a.name_span, "@type(ref) requires @ref on the same alternative".into());
                            None
                        }
                    },
                    // Multi-reference productions (a path's base @ref +
                    // @qualify name) select WHICH reference by label.
                    ("ref", 2) => match positions.get(a.args[1].0.as_str()) {
                        Some(&k) if binding.refs.iter().any(|r| r.0 == nt && r.1 == prod && r.2 == k) => {
                            Some(TypeRule::FromRef { ref_child: k })
                        }
                        Some(_) => {
                            error(d, a.args[1].1, "that label is not a reference on this alternative".into());
                            None
                        }
                        None => {
                            error(d, a.args[1].1, format!("no symbol labeled `{}` in this alternative", a.args[1].0));
                            None
                        }
                    },
                    ("deftype", n @ (1 | 2)) => {
                        let body = if n == 2 { rule_pos(d, &a.args[1]).map(Some) } else { Some(None) };
                        match (binding.defs.iter().find(|e| e.0 == nt && e.1 == prod), body) {
                            (Some(&(_, _, k)), Some(body_child)) => {
                                Some(TypeRule::DefType { def_child: k, body_child })
                            }
                            (None, _) => {
                                error(d, a.name_span, "@type(deftype) requires @def on the same alternative".into());
                                None
                            }
                            _ => None,
                        }
                    }
                    ("member", 3) => {
                        let base = rule_pos(d, &a.args[1]);
                        // The member NAME must label a TOKEN (the field
                        // name), unlike every other form's rule sources.
                        let name = match positions.get(a.args[2].0.as_str()) {
                            None => {
                                error(
                                    d,
                                    a.args[2].1,
                                    format!("no symbol labeled `{}` in this alternative", a.args[2].0),
                                );
                                None
                            }
                            Some(&k) => match sg.prods[prod as usize].rhs.get(k) {
                                Some(Sym::T(_)) => Some(k),
                                _ => {
                                    error(
                                        d,
                                        a.args[2].1,
                                        format!("`{}` labels a rule — the member name must be a token", a.args[2].0),
                                    );
                                    None
                                }
                            },
                        };
                        match (base, name) {
                            (Some(base_child), Some(name_child)) => {
                                Some(TypeRule::Member { base_child, name_child })
                            }
                            _ => None,
                        }
                    }
                    ("named", 1) => match binding.refs.iter().find(|r| r.0 == nt && r.1 == prod) {
                        Some(&(_, _, k, _)) => Some(TypeRule::Named { ref_child: k }),
                        None => {
                            error(d, a.name_span, "@type(named) requires @ref on the same alternative".into());
                            None
                        }
                    },
                    ("fn", 3) => {
                        let params = rule_pos(d, &a.args[1]);
                        let rt = rule_pos(d, &a.args[2]);
                        match (params, rt) {
                            (Some(params_child), Some(rt_child)) => {
                                Some(TypeRule::FnArrow { params_child, rt_child })
                            }
                            _ => None,
                        }
                    }
                    ("apply", 2) => {
                        let args = rule_pos(d, &a.args[1]);
                        let callee = binding.refs.iter().find(|r| r.0 == nt && r.1 == prod);
                        match (callee, args) {
                            (Some(&(_, _, k, _)), Some(args_child)) => {
                                Some(TypeRule::Apply { ref_child: k, args_child })
                            }
                            (None, _) => {
                                error(d, a.name_span, "@type(apply, …) requires @ref on the same alternative".into());
                                None
                            }
                            _ => None,
                        }
                    }
                    ("returns", 2) => rule_pos(d, &a.args[1]).map(|expr_child| TypeRule::Returns { expr_child }),
                    ("of", 2) => rule_pos(d, &a.args[1]).map(TypeRule::OfChild),
                    ("def", 2) => {
                        let src = rule_pos(d, &a.args[1]);
                        let def = binding.defs.iter().find(|e| e.0 == nt && e.1 == prod);
                        match (src, def) {
                            (Some(src), Some(&(_, _, def_child))) => {
                                Some(TypeRule::DefFrom { src, def_child })
                            }
                            (Some(_), None) => {
                                error(d, a.name_span, "@type(def, …) requires @def on the same alternative".into());
                                None
                            }
                            _ => None,
                        }
                    }
                    ("sig", n) if n >= 2 => {
                        let mut vars: Vec<String> = Vec::new();
                        let params: Vec<TyTerm> = a.args[1..n - 1]
                            .iter()
                            .filter_map(|arg| term(&mut type_cfg, d, &mut vars, arg))
                            .collect();
                        let result = term(&mut type_cfg, d, &mut vars, &a.args[n - 1]);
                        let n_rules = sg.prods[prod as usize]
                            .rhs
                            .iter()
                            .filter(|s| matches!(s, Sym::N(_)))
                            .count();
                        if params.len() != n - 2 || result.is_none() {
                            None // term() already reported
                        } else if params.len() != n_rules {
                            error(
                                d,
                                a.name_span,
                                format!(
                                    "@type(sig, …) has {} parameter(s) but the alternative has {} rule symbol(s)",
                                    params.len(),
                                    n_rules
                                ),
                            );
                            None
                        } else {
                            Some(TypeRule::Sig { params, result: result.unwrap() })
                        }
                    }
                    (_, 1) => {
                        let arg = &a.args[0];
                        let first = arg.0.chars().next().unwrap_or('0');
                        if arg.2 || !first.is_ascii_uppercase() {
                            error(
                                d,
                                arg.1,
                                format!(
                                    "`{}` is not a type atom — atoms are Capitalized names (lowercase names are sig variables)",
                                    arg.0
                                ),
                            );
                            None
                        } else {
                            Some(TypeRule::Const(type_cfg.intern(&arg.0)))
                        }
                    }
                    (other, _) => {
                        error(
                            d,
                            a.name_span,
                            format!(
                                "unknown @type form `{other}` (expected an Atom, `ref`, `named`, `deftype`, `member`, `of`, `def`, `sig`, `fn`, `apply`, or `returns`)"
                            ),
                        );
                        None
                    }
                };
                if let Some(r) = rule {
                    type_cfg.rules.push((nt, prod, r));
                }
            }
        }
    }

    // Inline-helper productions, after every explicit production.
    for h in &helper_list {
        let hname = sg.nt_names[h.nt as usize].clone();
        let c = SynGrammar::camel_name(&hname);
        let g = &mut sg;
        match h.op {
            SugarOp::Opt => {
                gen_prod(g, &mut labels_seen, &mut prod_spans, d, h.nt, format!("{c}None"), vec![], h.span);
                gen_prod(g, &mut labels_seen, &mut prod_spans, d, h.nt, format!("{c}Some"), vec![h.sym], h.span);
            }
            SugarOp::Star => {
                gen_prod(g, &mut labels_seen, &mut prod_spans, d, h.nt, format!("{c}Empty"), vec![], h.span);
                gen_prod(g, &mut labels_seen, &mut prod_spans, d, h.nt, format!("{c}More"), vec![Sym::N(h.nt), h.sym], h.span);
            }
            SugarOp::Plus => {
                gen_prod(g, &mut labels_seen, &mut prod_spans, d, h.nt, format!("{c}First"), vec![h.sym], h.span);
                gen_prod(g, &mut labels_seen, &mut prod_spans, d, h.nt, format!("{c}More"), vec![Sym::N(h.nt), h.sym], h.span);
            }
        }
    }

    // ---- start symbol ----
    match &ir.start {
        Some((name, span)) => match nt_ids.get(name) {
            Some(&nt) => sg.start = nt,
            None => error(d, *span, format!("cannot find rule `{name}`")),
        },
        None => sg.start = 0,
    }
    if ir.rules.is_empty() {
        error(d, (0, 0), "a grammar needs at least one rule".into());
    }

    // `@scope(unordered)` on the START rule means the FILE's own scope is
    // unordered — a declaration language, where top-level names see each
    // other regardless of order. Per-scope entries only govern ordering
    // within one item, so the whole-file case needs the global flag too;
    // without this the annotation on the root would silently do nothing.
    if binding.scopes.iter().any(|&(nt, _, unordered, _)| unordered && nt == sg.start) {
        binding.unordered = true;
    }

    (
        {
            // The outline's delegation walker borrows the binding
            // tier's def sites: an @outline whose label names a NODE
            // child means "the name is the first def inside it".
            let outline = OutlineConfig { defs: binding.defs.clone(), ..outline };
            LangDef {
                lex,
                sg,
                binding,
                types: type_cfg,
                macros,
                styles,
                outline,
                token_spans,
                prod_spans,
            }
        },
        diags,
    )
}


/// Label → symbol position that must hold a RULE (shared by the module
/// and type attribute arms).
fn rule_pos_of(
    d: &mut Vec<QanaDiag>,
    positions: &HashMap<&str, usize>,
    sg: &SynGrammar,
    prod: u16,
    arg: &(String, (u32, u32), bool),
) -> Option<usize> {
    let push = |d: &mut Vec<QanaDiag>, span, msg: String| {
        d.push(QanaDiag { span, msg, severity: 1 })
    };
    let k = match positions.get(arg.0.as_str()) {
        Some(&k) => k,
        None => {
            push(d, arg.1, format!("no symbol labeled `{}` in this alternative", arg.0));
            return None;
        }
    };
    match sg.prods[prod as usize].rhs.get(k) {
        Some(Sym::N(_)) => Some(k),
        _ => {
            push(d, arg.1, format!("`{}` labels a token — a body must be a rule symbol", arg.0));
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Certification: the envelope gates, with spans
// ---------------------------------------------------------------------------

/// Run the envelope over a compiled definition. Refusals come back as
/// span-carrying diagnostics: L1/L2 lint witnesses point at the token
/// declaration, LR conflicts point at an involved production and carry
/// the counterexample input.
pub fn certify(def: &LangDef) -> Result<(CompiledLexer, LrTables), Vec<QanaDiag>> {
    let span_of_token = |name: &str| -> (u32, u32) {
        def.lex
            .tokens
            .iter()
            .position(|t| t.name == name)
            .and_then(|i| def.token_spans.get(i).copied())
            .unwrap_or((0, 0))
    };
    let lexer = match CompiledLexer::build(&def.lex) {
        Ok(l) => l,
        Err(e) => {
            use qana_grammar::dfa::CompileError;
            use qana_grammar::lexer::BuildError;
            use qana_grammar::lints::LintError;
            let (span, msg) = match &e {
                BuildError::Compile(CompileError::NonAsciiLiteral { token, .. })
                | BuildError::Compile(CompileError::EmptyMatch { token }) => {
                    (span_of_token(token), format!("{e}"))
                }
                BuildError::Lint(LintError::TokenSpansLines { token, .. }) => {
                    (span_of_token(token), format!("{e}"))
                }
                BuildError::Lint(_) => ((0, 0), format!("{e}")),
            };
            return Err(vec![QanaDiag { span, msg, severity: 1 }]);
        }
    };
    let tables = build_lr(&def.sg);
    if !tables.conflicts.is_empty() {
        let diags = tables
            .conflicts
            .iter()
            .map(|c| {
                let span = c
                    .prods
                    .first()
                    .and_then(|&p| def.prod_spans.get(p as usize).copied())
                    .unwrap_or((0, 0));
                QanaDiag {
                    span,
                    msg: format!(
                        "grammar conflict ({} on {}) — example input: {}\n  {}",
                        c.kind,
                        def.sg.term_name(c.lookahead),
                        c.example,
                        c.items.join("\n  ")
                    ),
                    severity: 1,
                }
            })
            .collect();
        return Err(diags);
    }
    Ok((lexer, tables))
}

// ---------------------------------------------------------------------------
// Canonical dumps (the equality gates' comparison form)
// ---------------------------------------------------------------------------

pub fn dump_lex(g: &LexGrammar) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    writeln!(out, "language {} modes {:?} max_stack {:?} eol {:?}", g.name, g.mode_names, g.max_stack, g.eol_pop)
        .unwrap();
    for (i, t) in g.tokens.iter().enumerate() {
        writeln!(
            out,
            "{i}: {} mode={} trivia={} error={} spec={} bracket={:?} action={:?} pat={:?}",
            t.name, t.mode, t.trivia, t.error, t.specialize, t.bracket, t.action, t.pat
        )
        .unwrap();
    }
    writeln!(out, "keywords {:?}", g.keywords).unwrap();
    out
}

pub fn dump_syn(sg: &SynGrammar) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    writeln!(out, "syn {} start {}", sg.name, sg.nt_names[sg.start as usize]).unwrap();
    for (i, _) in sg.prods.iter().enumerate() {
        writeln!(
            out,
            "{i}: [{}] {} prec={:?}",
            sg.prod_name(i),
            sg.prod_display(i),
            sg.prods[i].prec
        )
        .unwrap();
    }
    let mut precs: Vec<String> = sg
        .token_prec
        .iter()
        .enumerate()
        .filter_map(|(t, p)| p.map(|(l, a)| format!("{} {l} {a:?}", sg.term_name(t as TokenId))))
        .collect();
    precs.sort();
    writeln!(out, "prec {precs:?}").unwrap();
    out
}

pub fn dump_tables(t: &LrTables) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    writeln!(out, "states {} conflicts {}", t.n_states, t.conflicts.len()).unwrap();
    for (si, row) in t.action.iter().enumerate() {
        let mut acts: Vec<String> = row.iter().map(|(k, v)| format!("{k}:{v:?}")).collect();
        acts.sort();
        let mut gotos: Vec<String> =
            t.goto_[si].iter().map(|(k, v)| format!("{k}:{v}")).collect();
        gotos.sort();
        writeln!(out, "{si}: {acts:?} {gotos:?}").unwrap();
    }
    writeln!(out, "fragile {:?}", t.fragile).unwrap();
    let mut lists: Vec<String> =
        t.lists.iter().map(|(nt, s)| format!("{nt}:{}/{}", s.cons, s.seed)).collect();
    lists.sort();
    writeln!(out, "lists {lists:?}").unwrap();
    out
}

pub fn dump_styles(s: &Styles, n_tokens: usize) -> String {
    let mut pairs: Vec<String> = (0..n_tokens as TokenId)
        .filter_map(|id| s.class_of(id).map(|c| format!("{id}:{}", s.legend[c as usize])))
        .collect();
    pairs.sort();
    format!("{pairs:?}")
}

pub fn dump_binding(b: &BindingConfig) -> String {
    format!("defs {:?} refs {:?} scopes {:?}", b.defs, b.refs, b.scopes)
}

pub fn dump_outline(o: &OutlineConfig) -> String {
    let entries: Vec<String> = o
        .entries
        .iter()
        .map(|e| format!("{}/{} child {} kind {}", e.nt, e.prod, e.name_child, e.kind))
        .collect();
    format!("{entries:?}")
}
