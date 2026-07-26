//! The hot-reloadable language definition. Two sources, one certified
//! pipeline:
//!
//! * `chartlang.qana` (preferred): the FULL textual grammar surface —
//!   tokens, modes, keywords, precedence, productions, binding and
//!   style annotations — compiled by qana-lang and certified by the
//!   envelope. Refusals carry source spans.
//! * `chartlang.toml` (legacy fallback): keywords + operator precedence
//!   parameterizing the built-in demo grammar.
//!
//! Bad definitions are refused with the tool's own counterexamples; the
//! server surfaces them as diagnostics on the definition file. This is
//! the envelope's authoring loop, live.

use qana_grammar::demo::{demo_grammar_with_keywords, demo_syn_grammar_prec};
use qana_grammar::{build_lr, Assoc, CompiledLexer, LrTables, SynGrammar, TokenId};
use qana_lang::compile::{certify, QanaDiag};
use qana_lang::{compile_source, qana_binding_config, qana_outline_config, qana_styles, QanaToolchain};
use qana_sem::{demo_binding_config, BindingConfig};
use qana_services::demo_glue::{demo_outline_config, demo_styles};
use qana_services::{OutlineConfig, Styles};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LangConfig {
    pub keywords: Vec<String>,
    /// (operator char, level, assoc)
    pub prec: Vec<(char, u8, Assoc)>,
}

impl Default for LangConfig {
    fn default() -> Self {
        LangConfig {
            keywords: qana_grammar::demo::KEYWORDS.iter().map(|s| s.to_string()).collect(),
            prec: vec![
                ('+', 1, Assoc::Left),
                ('-', 1, Assoc::Left),
                ('*', 2, Assoc::Left),
                ('/', 2, Assoc::Left),
            ],
        }
    }
}

pub fn parse_config(text: &str) -> Result<LangConfig, String> {
    let mut keywords: Option<Vec<String>> = None;
    let mut prec: Vec<(char, u8, Assoc)> = Vec::new();
    for (ln, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap().trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!("line {}: expected `key = value`", ln + 1));
        };
        let (key, value) = (key.trim(), value.trim());
        if key == "keywords" {
            let words: Vec<String> = value.split_whitespace().map(|s| s.to_string()).collect();
            keywords = Some(words);
        } else if let Some(rest) = key.strip_prefix("prec.") {
            let (assoc, level) = match rest.split_once('.') {
                Some(("left", l)) => (Assoc::Left, l),
                Some(("right", l)) => (Assoc::Right, l),
                _ => return Err(format!("line {}: expected prec.left.N or prec.right.N", ln + 1)),
            };
            let level: u8 = level
                .parse()
                .map_err(|_| format!("line {}: precedence level must be 1..=9", ln + 1))?;
            if !(1..=9).contains(&level) {
                return Err(format!("line {}: precedence level must be 1..=9", ln + 1));
            }
            for op in value.split_whitespace() {
                let mut chars = op.chars();
                let (Some(c), None) = (chars.next(), chars.next()) else {
                    return Err(format!("line {}: operators are single chars", ln + 1));
                };
                if !"+-*/".contains(c) {
                    return Err(format!("line {}: unknown operator `{c}`", ln + 1));
                }
                if prec.iter().any(|&(pc, _, _)| pc == c) {
                    return Err(format!("line {}: operator `{c}` declared twice", ln + 1));
                }
                prec.push((c, level, assoc));
            }
        } else {
            return Err(format!("line {}: unknown key `{key}`", ln + 1));
        }
    }
    let keywords = keywords.unwrap_or_else(|| LangConfig::default().keywords);
    for required in ["let", "if", "else"] {
        if !keywords.iter().any(|k| k == required) {
            return Err(format!(
                "keyword `{required}` is required by the syntax grammar (statements use it)"
            ));
        }
    }
    for w in &keywords {
        if !w.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            || !w.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        {
            return Err(format!("keyword `{w}` must be identifier-shaped"));
        }
    }
    Ok(LangConfig { keywords, prec })
}

/// A fully built, certified language pipeline. Reloads leak the previous
/// pipeline (a few hundred KB per reload — bounded and deliberate; the
/// `Arc`-ification of session lifetimes is the documented refinement).
pub struct Pipeline {
    pub lexer: &'static CompiledLexer,
    pub sg: &'static SynGrammar,
    pub tables: &'static LrTables,
    pub styles: Styles,
    pub outline_cfg: OutlineConfig,
    pub binding: BindingConfig,
    /// The declared type tier (empty for languages that declare none —
    /// the legacy toml config and the `.qana` surface itself).
    pub types: qana_sem::TypeConfig,
}

pub fn build_pipeline(cfg: &LangConfig) -> Result<Pipeline, String> {
    let (g, ids) = demo_grammar_with_keywords(&cfg.keywords);
    let lexer = CompiledLexer::build(&g).map_err(|e| format!("envelope refused: {e}"))?;
    let prec: Vec<(TokenId, u8, Assoc)> = cfg
        .prec
        .iter()
        .map(|&(c, level, assoc)| {
            let id = match c {
                '+' => ids.plus,
                '-' => ids.minus,
                '*' => ids.star,
                _ => ids.slash,
            };
            (id, level, assoc)
        })
        .collect();
    let sg = demo_syn_grammar_prec(&ids, &lexer.vocab, &prec);
    let tables = build_lr(&sg);
    if let Some(c) = tables.conflicts.first() {
        return Err(format!(
            "grammar conflict refused ({} on {}) — example input: {}\n  {}",
            c.kind,
            sg.term_name(c.lookahead),
            c.example,
            c.items.join("\n  ")
        ));
    }
    let styles = demo_styles(&ids);
    let outline_cfg = demo_outline_config(&sg);
    let binding = demo_binding_config(&sg);
    Ok(Pipeline {
        lexer: Box::leak(Box::new(lexer)),
        sg: Box::leak(Box::new(sg)),
        tables: Box::leak(Box::new(tables)),
        styles,
        outline_cfg,
        binding,
        types: qana_sem::TypeConfig::default(),
    })
}

/// Build the target-language pipeline from a `.qana` grammar source. Parse
/// repairs, compile errors, and envelope refusals all come back as
/// span-carrying diagnostics for the grammar file.
pub fn build_pipeline_qana(tc: &QanaToolchain, text: &str) -> Result<Pipeline, Vec<QanaDiag>> {
    let out = compile_source(tc, text);
    let mut diags: Vec<QanaDiag> = qana_parse_diags(tc, text, &out.repairs);
    diags.extend(out.diags.iter().cloned());
    if !diags.is_empty() {
        return Err(diags);
    }
    let (lexer, tables) = certify(&out.def)?;
    Ok(Pipeline {
        lexer: Box::leak(Box::new(lexer)),
        sg: Box::leak(Box::new(out.def.sg)),
        tables: Box::leak(Box::new(tables)),
        styles: out.def.styles,
        outline_cfg: out.def.outline,
        binding: out.def.binding,
        types: out.def.types,
    })
}

/// Parse repairs of a `.qana` text as span diagnostics (helper shared by
/// the reload path and the live-editing path).
pub fn qana_parse_diags(
    tc: &QanaToolchain,
    text: &str,
    repairs: &[qana_grammar::Repair],
) -> Vec<QanaDiag> {
    use qana_engine::LexedBuffer;
    let buf = LexedBuffer::new(&tc.lexer, text);
    qana_services::diagnostics(&tc.lexer, &buf, &tc.sg, repairs)
        .into_iter()
        .map(|d| QanaDiag { span: d.span, msg: d.message, severity: 1 })
        .collect()
}

/// The static pipeline for serving `.qana` documents THEMSELVES — the
/// dogfood loop: grammar files get qana-powered highlighting, outline,
/// navigation, and live envelope diagnostics.
pub fn qana_service_pipeline(tc: &'static QanaToolchain) -> Pipeline {
    Pipeline {
        lexer: &tc.lexer,
        sg: &tc.sg,
        tables: &tc.tables,
        styles: qana_styles(&tc.ids),
        outline_cfg: qana_outline_config(&tc.sg),
        binding: qana_binding_config(&tc.sg),
        types: qana_sem::TypeConfig::default(),
    }
}
