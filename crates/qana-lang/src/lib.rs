//! P5: the `.qana` textual grammar surface, self-hosted on the qana
//! engine. A `.qana` file is parsed by the same certified incremental
//! toolchain every target language uses (the bootstrap grammar in
//! [`bootstrap`]), compiled by [`compile`] into the toolchain's value
//! types, and refused — with source-span diagnostics carrying the
//! envelope's own witnesses and counterexamples — when it steps outside
//! the envelope.

pub mod bootstrap;
pub mod compile;
pub mod expand;
pub mod live;
pub mod pat_parse;
pub mod qana_ast;
pub mod tsgen;

use bootstrap::{qana_lex_grammar, qana_syn_grammar, QanaIds, QanaProds};
use compile::{compile, QanaDiag};
use qana_engine::IncSession;
use qana_grammar::incremental::Repair;
use qana_grammar::{build_lr, CompiledLexer, GreenNode, LrTables, SynGrammar};
use std::sync::Arc;

/// The bootstrap toolchain for `.qana` files themselves: certified lexer,
/// LR tables, and production indices — everything needed to parse and
/// compile grammar files with the same engine target languages use.
pub struct QanaToolchain {
    pub lexer: CompiledLexer,
    pub sg: SynGrammar,
    pub tables: LrTables,
    pub ids: QanaIds,
    pub prods: QanaProds,
}

impl QanaToolchain {
    pub fn new() -> Self {
        let (g, ids) = qana_lex_grammar();
        let lexer = CompiledLexer::build(&g).expect("bootstrap grammar is in envelope");
        let (sg, prods) = qana_syn_grammar(&ids, &lexer.vocab);
        let tables = build_lr(&sg);
        assert!(tables.conflicts.is_empty(), "bootstrap grammar is conflict-free");
        QanaToolchain { lexer, sg, tables, ids, prods }
    }
}

impl Default for QanaToolchain {
    fn default() -> Self {
        Self::new()
    }
}

/// Everything one `.qana` source produces: the (total, lossless) parse and
/// the compiled language definition with its diagnostics.
pub struct QanaOutcome {
    pub tree: Arc<GreenNode>,
    pub repairs: Vec<Repair>,
    pub def: compile::LangDef,
    pub diags: Vec<QanaDiag>,
}

/// An embeddable language pipeline for GUI/editor hosts: everything a
/// text widget needs to serve a `.qana`-defined language IN PROCESS — no
/// LSP hop. Owns its certified pipeline with `'static` lifetimes (one
/// bounded leak per grammar load, the same deliberate pattern the LSP
/// server uses; grammar loads are rare and small).
#[derive(Clone)]
pub struct EmbeddedLang {
    pub lexer: &'static CompiledLexer,
    pub sg: &'static SynGrammar,
    pub tables: &'static LrTables,
    pub styles: qana_services::Styles,
    pub outline: qana_services::OutlineConfig,
    pub binding: qana_sem::BindingConfig,
    /// The declared type tier (empty when the grammar declares none).
    pub types: qana_sem::TypeConfig,
    /// The declared meta tier (macro bodies are type-exempt; live
    /// documents need to know where they are).
    pub macros: qana_sem::macros::MacroConfig,
}

impl EmbeddedLang {
    /// Compile + certify a `.qana` grammar source. Refusals come back as
    /// span-carrying diagnostics against the grammar text.
    pub fn from_rg_source(src: &str) -> Result<Self, Vec<QanaDiag>> {
        let tc = QanaToolchain::new();
        let out = compile_source(&tc, src);
        let mut diags = out.diags.clone();
        if !out.repairs.is_empty() {
            diags.push(QanaDiag {
                span: (0, 1),
                msg: format!("{} parse repair(s) in the grammar source", out.repairs.len()),
                severity: 1,
            });
        }
        if !diags.is_empty() {
            return Err(diags);
        }
        let (lexer, tables) = compile::certify(&out.def)?;
        Ok(EmbeddedLang {
            lexer: Box::leak(Box::new(lexer)),
            sg: Box::leak(Box::new(out.def.sg)),
            tables: Box::leak(Box::new(tables)),
            styles: out.def.styles,
            outline: out.def.outline,
            binding: out.def.binding,
            types: out.def.types,
            macros: out.def.macros,
        })
    }

    /// Open an incremental session over a document (total under errors).
    pub fn session(&self, text: &str) -> IncSession<'static> {
        IncSession::new(self.lexer, self.sg, self.tables, text).expect("total parsing")
    }
}

/// Parse + compile a `.qana` source. Total: broken sources still yield a
/// tree (with repairs) and a best-effort definition (with diagnostics).
pub fn compile_source(tc: &QanaToolchain, src: &str) -> QanaOutcome {
    let session = IncSession::new(&tc.lexer, &tc.sg, &tc.tables, src)
        .expect("total parsing under recovery");
    let tree = session.tree().expect("total").clone();
    let repairs = session.last_repairs.clone();
    let (def, diags) = compile(&tree, &tc.prods);
    QanaOutcome { tree, repairs, def, diags }
}

// ---------------------------------------------------------------------------
// Composition (Part III tier 1): chartlang hosting `.qana` islands —
// code files carrying grammar definitions in fenced blocks. Built with
// the GENERIC compose() operator over the two envelope grammars this
// workspace already certifies, then re-certified as one product.
// ---------------------------------------------------------------------------

/// The composed chartlang⊃qana pipeline: `` ```qana `` … `` ``` `` fenced
/// islands are statements; island interiors are full `.qana` — lexing,
/// parsing, balancing, recovery, highlighting all from the ONE product
/// grammar.
pub struct ComposedToolchain {
    pub lexer: CompiledLexer,
    pub sg: SynGrammar,
    pub tables: qana_grammar::LrTables,
    pub styles: qana_services::Styles,
    pub binding: qana_sem::BindingConfig,
    pub map: qana_grammar::ComposeMap,
}

pub fn chartlang_with_rg_islands() -> ComposedToolchain {
    use qana_grammar::demo::{demo_grammar, demo_syn_grammar};
    let (host_lex, host_ids) = demo_grammar();
    let host_lexer = CompiledLexer::build(&host_lex).expect("host in envelope");
    let host_sg = demo_syn_grammar(&host_ids, &host_lexer.vocab);
    let (guest_lex, guest_ids) = qana_lex_grammar();
    let guest_lexer = CompiledLexer::build(&guest_lex).expect("guest in envelope");
    let (guest_sg, _) = qana_syn_grammar(&guest_ids, &guest_lexer.vocab);

    let (lex, sg, map) = qana_grammar::compose(
        &host_lex,
        &host_sg,
        &guest_lex,
        &guest_sg,
        &[qana_grammar::IslandSpec {
            open_text: "```qana".into(),
            close_text: "```".into(),
            host_mode: 0,
            attach_to: "stmt".into(),
            name: "qana".into(),
        }],
    )
    .expect("compositional checks pass");

    // The composition theorem, checked: the PRODUCT goes through the
    // unchanged certification pipeline.
    let lexer = CompiledLexer::build(&lex).expect("product in envelope (L1/L2 on the product)");
    let tables = build_lr(&sg);
    assert!(tables.conflicts.is_empty(), "product is LR(1)-deterministic");

    // Product styles: host classes at unchanged ids, guest classes at
    // the offset, fences as punctuation.
    let mut styles = qana_services::Styles::new(qana_services::LEGEND.to_vec());
    let host_styles = qana_services::demo_glue::demo_styles(&host_ids);
    for id in 0..host_lex.tokens.len() as qana_grammar::TokenId {
        if let Some(c) = host_styles.class_of(id) {
            styles.set(id, host_styles.legend[c as usize]);
        }
    }
    let guest_styles = qana_styles(&guest_ids);
    for id in 0..guest_lex.tokens.len() as qana_grammar::TokenId {
        if let Some(c) = guest_styles.class_of(id) {
            styles.set(id + map.guest_token_offset, guest_styles.legend[c as usize]);
        }
    }
    for &(open, close, _) in &map.islands {
        styles.set(open, "punctuation");
        styles.set(close, "punctuation");
    }

    // COMPOSED binding: host entries unchanged (host ids preserved),
    // guest entries offset, and every island a BARRIER scope carrying
    // the guest's ordering — the guest namespace is island-local,
    // forward-resolving, and sealed in both directions.
    let binding = qana_sem::compose_binding(
        &qana_sem::demo_binding_config(&sg),
        &qana_binding_config(&guest_sg),
        &sg,
        &map,
    );

    ComposedToolchain { lexer, sg, tables, styles, binding, map }
}

// ---------------------------------------------------------------------------
// Editor-service glue for `.qana` files themselves (the dogfood loop:
// grammar files get qana-powered highlighting, outline, navigation).
// These mirror what compiling `qana.qana` produces — gated in qana_e2e.
// ---------------------------------------------------------------------------

pub fn qana_styles(ids: &QanaIds) -> qana_services::Styles {
    let mut s = qana_services::Styles::new(qana_services::LEGEND.to_vec());
    for &kw in &ids.kw {
        s.set(kw, "keyword");
    }
    s.set(ids.line_comment, "comment");
    s.set(ids.name, "variable");
    s.set(ids.num, "number");
    s.set(ids.string, "string");
    s.set(ids.string_unterm, "string");
    s.set(ids.pattern, "regexp");
    s.set(ids.pattern_unterm, "regexp");
    s.set(ids.eq, "operator");
    s.set(ids.pipe, "operator");
    s.set(ids.star, "operator");
    s.set(ids.plus, "operator");
    s.set(ids.qmark, "operator");
    s.set(ids.percent, "operator");
    s.set(ids.colon, "punctuation");
    s.set(ids.at, "punctuation");
    s.set(ids.comma, "punctuation");
    for &b in &[ids.lparen, ids.rparen, ids.lbrace, ids.rbrace] {
        s.set(b, "bracket");
    }
    s
}

pub fn qana_outline_config(sg: &SynGrammar) -> qana_services::OutlineConfig {
    let mut cfg = qana_services::OutlineConfig::default();
    for i in 0..sg.prods.len() {
        let (nt, prod) = (sg.prods[i].lhs, i as u16);
        let entry = |name_child, kind| qana_services::OutlineEntry { nt, prod, name_child, kind };
        match sg.prod_name(i).as_str() {
            "ModeDecl" => cfg.entries.push(entry(1, "module")),
            "RuleDecl" | "RuleDeclBar" | "RuleStar" | "RulePlus" | "RuleOpt" => {
                cfg.entries.push(entry(1, "struct"))
            }
            "TokenDef" => cfg.entries.push(entry(1, "constant")),
            _ => {}
        }
    }
    cfg
}

/// Binding for grammar files: token/rule/mode declarations define names,
/// RHS symbol references read them — one flat, UNORDERED namespace
/// (forward references are the normal case in grammars).
pub fn qana_binding_config(sg: &SynGrammar) -> qana_sem::BindingConfig {
    let mut cfg = qana_sem::BindingConfig { unordered: true, ..Default::default() };
    for i in 0..sg.prods.len() {
        let (nt, prod) = (sg.prods[i].lhs, i as u16);
        match sg.prod_name(i).as_str() {
            "RuleDecl" | "RuleDeclBar" | "RuleStar" | "RulePlus" | "RuleOpt" | "TokenDef" => {
                cfg.defs.push((nt, prod, 1))
            }
            "KwDecl" | "StartDecl" => cfg.refs.push((nt, prod, 1, qana_sem::RefKind::Var)),
            "TokName" | "SymName" | "SymNameOpt" | "SymNameStar" | "SymNamePlus" | "ElemName" => {
                cfg.refs.push((nt, prod, 0, qana_sem::RefKind::Var))
            }
            "SymLabeled" => cfg.refs.push((nt, prod, 2, qana_sem::RefKind::Var)),
            _ => {}
        }
    }
    cfg
}

#[cfg(test)]
mod bootstrap_tests {
    use crate::bootstrap::{qana_lex_grammar, qana_syn_grammar};
    use qana_grammar::{build_lr, CompiledLexer};

    #[test]
    fn bootstrap_is_in_envelope_and_conflict_free() {
        let (g, ids) = qana_lex_grammar();
        let lexer = CompiledLexer::build(&g).expect("qana lexical grammar passes its own lints");
        let (sg, _) = qana_syn_grammar(&ids, &lexer.vocab);
        let tables = build_lr(&sg);
        assert!(
            tables.conflicts.is_empty(),
            "qana syntax grammar must be conflict-free:\n{}",
            tables
                .conflicts
                .iter()
                .map(|c| format!("{} on {}: {}", c.kind, sg.term_name(c.lookahead), c.example))
                .collect::<Vec<_>>()
                .join("\n")
        );
        assert!(!tables.lists.is_empty(), "qana grammar's lists are L4-detected");
    }
}
