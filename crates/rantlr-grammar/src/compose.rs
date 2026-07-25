//! The composition engine (Part III, tier 1): language nesting as a
//! CONSTRUCTION over grammar values, not new machinery.
//!
//! `compose` builds the PRODUCT grammar of a host and a guest: guest
//! modes, tokens, and nonterminals are offset into one shared space;
//! each island declaration adds an OPEN token in a host mode that
//! pushes the guest's base mode, a CLOSE token in the guest's base mode
//! that pops, and a host production `Island → OPEN guest_start CLOSE`.
//!
//! The composition theorem, made checkable: the bounded mode stack
//! already carries finite line-start state, and pushing a guest base
//! mode is just another push — finite host state × finite guest state
//! stays finite (L2's bound IS the declared nesting depth). The product
//! is an ordinary `LexGrammar`/`SynGrammar`, so the WHOLE certified
//! pipeline re-runs per composition — L1/L2 lints with witnesses, LR
//! determinism with conflict counterexamples — and every engine
//! guarantee (losslessness, Wagner splicing, recovery, L4 balancing,
//! per-item memoization, derived services) applies to island content
//! verbatim, because the engine never learns there were two languages.
//!
//! Scope notes (deliberate): island EXTENT is lexical (mode-driven), so
//! an unterminated guest mode extends the island exactly like an
//! unclosed block comment — damage-bounded, lossless, recovered; full
//! skeleton-enforced error containment at the boundary is the named
//! refinement. Keyword texts must be disjoint between the languages
//! (per-mode keyword maps are the refinement). Guest binding configs
//! are not composed yet (per-entry ordering is the missing piece);
//! host binding works on the product unchanged because host ids are
//! preserved.

use crate::model::{LexGrammar, TokenDef, TokenId};
use crate::pat::Pat;
use crate::syn::{Prod, Sym, SynGrammar};

/// One island declaration: where the guest may be embedded in the host.
pub struct IslandSpec {
    /// Fixed text opening the island (a host token; wins ties against
    /// host tokens by maximal munch, then by being declared after them
    /// only when longer — make it distinctive).
    pub open_text: String,
    /// Fixed text closing the island (a token of the guest's BASE mode;
    /// declared before the guest's own tokens, so it wins priority ties).
    pub close_text: String,
    /// Host mode the open token is active in (0 = base).
    pub host_mode: u16,
    /// Host nonterminal (by name) that gains the island production.
    pub attach_to: String,
    /// Prefix for everything guest-side: token names get
    /// `{PREFIX}_{name}`, nonterminals `{prefix}_{name}`, production
    /// labels `{Prefix}{label}` — collision-free, astgen-clean.
    pub name: String,
}

/// Offsets and ids the instance layer needs to compose styles, outline
/// configs, and tooling over the product.
#[derive(Debug)]
pub struct ComposeMap {
    pub guest_token_offset: TokenId,
    pub guest_nt_offset: u16,
    pub guest_prod_offset: u16,
    /// (open token, close token, island production) per island spec.
    pub islands: Vec<(TokenId, TokenId, u16)>,
}

/// Build the product grammar. Errors are compositional (name/keyword
/// collisions, unknown attach points); ENVELOPE errors surface later,
/// from the ordinary certification of the returned values.
pub fn compose(
    host_lex: &LexGrammar,
    host_syn: &SynGrammar,
    guest_lex: &LexGrammar,
    guest_syn: &SynGrammar,
    islands: &[IslandSpec],
) -> Result<(LexGrammar, SynGrammar, ComposeMap), String> {
    if islands.is_empty() {
        return Err("compose needs at least one island declaration".into());
    }
    let prefix = &islands[0].name;
    let pfx_up = prefix.to_uppercase();
    let pfx_camel = crate::syn::camel(prefix);

    // ---- lexical product ----
    let host_modes = host_lex.mode_names.len() as u16;
    let guest_base = host_modes; // guest mode m → host_modes + m
    let mode_names: Vec<String> = host_lex
        .mode_names
        .iter()
        .cloned()
        .chain(guest_lex.mode_names.iter().map(|m| format!("{pfx_up}_{m}")))
        .collect();
    let mode_refs: Vec<&str> = mode_names.iter().map(|s| s.as_str()).collect();
    let mut lex = LexGrammar::new(&format!("{}+{}", host_lex.name, guest_lex.name), &mode_refs);
    lex.eol_pop = host_lex.eol_pop.iter().chain(guest_lex.eol_pop.iter()).copied().collect();
    // Product bound: the island entry adds one level on top of whatever
    // both languages need; L2 re-verifies against the engine cap.
    lex.max_stack = Some(
        host_lex
            .max_stack
            .unwrap_or(1)
            .max(guest_lex.max_stack.unwrap_or(1))
            .saturating_add(1)
            .min(crate::lexer::MAX_STACK as u8),
    );

    // Host tokens keep their ids.
    for t in &host_lex.tokens {
        lex.add(t.clone());
    }
    // Island delimiters: OPEN per island (host mode, pushes guest base);
    // ONE shared CLOSE (guest base mode, pops) — declared BEFORE guest
    // tokens so it wins priority ties in the guest's base mode.
    let mut open_ids = Vec::new();
    for (i, isl) in islands.iter().enumerate() {
        if isl.host_mode >= host_modes {
            return Err(format!("island {} references undeclared host mode", i));
        }
        let open = lex.add(
            TokenDef::new(
                &format!("{pfx_up}_OPEN{}", if islands.len() > 1 { i.to_string() } else { String::new() }),
                isl.host_mode,
                Pat::lit(&isl.open_text),
            )
            .push(guest_base),
        );
        open_ids.push(open);
    }
    let close_text = &islands[0].close_text;
    if islands.iter().any(|i| &i.close_text != close_text) {
        return Err("all islands of one guest share a close delimiter (v1)".into());
    }
    let close = lex.add(TokenDef::new(&format!("{pfx_up}_CLOSE"), guest_base, Pat::lit(close_text)).pop());

    // Guest tokens: renamed, modes and push targets offset.
    let guest_token_offset = lex.tokens.len() as TokenId;
    for t in &guest_lex.tokens {
        let mut def = t.clone();
        def.name = format!("{pfx_up}_{}", t.name);
        def.mode += guest_base;
        if let crate::model::Action::Push(m) = def.action {
            def.action = crate::model::Action::Push(m + guest_base);
        }
        lex.add(def);
    }
    // Keywords: merged with offset ids AND offset owners. Specialization
    // is per-owner, so the languages' keyword spaces stay separate — a
    // host keyword is an ordinary identifier inside a guest island.
    lex.keywords = host_lex.keywords.clone();
    for (w, id, owner) in &guest_lex.keywords {
        lex.keywords.push((
            w.clone(),
            id + guest_token_offset,
            owner + guest_token_offset,
        ));
    }

    // ---- syntactic product ----
    let vocab = crate::model::Vocab::of(&lex);
    let mut syn = SynGrammar::new(
        &format!("{}+{}", host_syn.name, guest_syn.name),
        vocab.names.clone(),
    );
    // Host precedence at unchanged ids; guest precedence at offset ids.
    for (t, p) in host_syn.token_prec.iter().enumerate() {
        if let Some((level, assoc)) = p {
            syn.set_token_prec(t as TokenId, *level, *assoc);
        }
    }
    for (t, p) in guest_syn.token_prec.iter().enumerate() {
        if let Some((level, assoc)) = p {
            syn.set_token_prec(t as TokenId + guest_token_offset, *level, *assoc);
        }
    }
    // Host nts keep ids; guest nts appended, prefixed.
    for n in &host_syn.nt_names {
        syn.nt(n);
    }
    let guest_nt_offset = syn.nt_names.len() as u16;
    for n in &guest_syn.nt_names {
        syn.nt(&format!("{prefix}_{n}"));
    }
    syn.start = host_syn.start;

    // Host productions keep their INDICES (host binding/outline configs
    // work on the product unchanged).
    for p in &host_syn.prods {
        syn.prods.push(p.clone());
    }
    // Island productions.
    let mut island_ids = Vec::new();
    for (i, isl) in islands.iter().enumerate() {
        let Some(attach) = host_syn.nt_names.iter().position(|n| n == &isl.attach_to) else {
            return Err(format!("island {} attaches to unknown host rule `{}`", i, isl.attach_to));
        };
        let prod = syn.prods.len() as u16;
        syn.prods.push(Prod {
            lhs: attach as u16,
            rhs: vec![
                Sym::T(open_ids[i]),
                Sym::N(guest_nt_offset + guest_syn.start),
                Sym::T(close),
            ],
            prec: None,
            name: Some(format!(
                "{pfx_camel}Island{}",
                if islands.len() > 1 { i.to_string() } else { String::new() }
            )),
        });
        island_ids.push((open_ids[i], close, prod));
    }
    // Guest productions: symbols remapped, labels prefixed.
    let guest_prod_offset = syn.prods.len() as u16;
    for (i, p) in guest_syn.prods.iter().enumerate() {
        let rhs = p
            .rhs
            .iter()
            .map(|s| match s {
                Sym::T(t) => Sym::T(t + guest_token_offset),
                Sym::N(n) => Sym::N(n + guest_nt_offset),
            })
            .collect();
        syn.prods.push(Prod {
            lhs: p.lhs + guest_nt_offset,
            rhs,
            prec: p.prec,
            name: Some(format!("{pfx_camel}{}", guest_syn.prod_name(i))),
        });
    }

    Ok((
        lex,
        syn,
        ComposeMap { guest_token_offset, guest_nt_offset, guest_prod_offset, islands: island_ids },
    ))
}
