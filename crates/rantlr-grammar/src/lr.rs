//! Canonical LR(1) table construction with declarative conflict
//! resolution and CONFLICT TRACES: every surviving conflict is reported
//! with the conflicting items, a shortest path to the conflict state, and
//! an example input prefix (shortest terminal yields) exhibiting it —
//! the envelope's "refuse with counterexamples" promise at the syntax
//! tier.
//!
//! Scope notes (deliberate): canonical LR(1) — correct and simple; state
//! merging (Pager/IELR) is a deferred optimization. Traces are
//! shortest-path examples, not full unifying derivations (Isradisaikul &
//! Myers 2015) — an upgrade path, not a rewrite.

use crate::model::TokenId;
use crate::syn::{Assoc, Sym, SynGrammar, EOF};
use std::collections::{BTreeSet, HashMap, VecDeque};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LrAct {
    Shift(u16),
    Reduce(u16),
    Accept,
    /// Nonassoc-declared combination: explicit syntax error.
    Error,
}

#[derive(Clone, Debug)]
pub struct Conflict {
    pub state: u16,
    pub lookahead: TokenId,
    pub kind: &'static str, // "shift/reduce" | "reduce/reduce"
    /// Human-readable conflicting items (with dots).
    pub items: Vec<String>,
    /// Example input: terminal names reaching the conflict state, then
    /// `·`, then the lookahead.
    pub example: String,
    /// User-grammar production indices involved in the conflict (for
    /// mapping the refusal back to grammar-source spans).
    pub prods: Vec<u16>,
}

#[derive(Debug)]
pub struct LrTables {
    /// action[state][terminal] — terminals include EOF.
    pub action: Vec<HashMap<TokenId, LrAct>>,
    pub goto_: Vec<HashMap<u16, u16>>,
    pub n_states: usize,
    pub conflicts: Vec<Conflict>,
    /// Shift/reduce conflicts silently settled by precedence/assoc.
    pub resolved_by_prec: usize,
    /// Productions involved in precedence-resolved conflicts (Wagner §6
    /// "fragile" set): reused subtrees rooted at these productions must
    /// be broken down during incremental parsing, because their shape
    /// depends on disambiguation context the LR automaton alone doesn't
    /// re-check on a nonterminal shift.
    pub fragile: Vec<bool>,
    /// List-shaped nonterminals (envelope L4): exactly one cons prod
    /// `L → L α` (nt not recurring in α, non-fragile) and one seed prod.
    /// Their trees use balanced LIST/RUN nodes instead of cons spines.
    pub lists: std::collections::HashMap<u16, ListShape>,
}

/// One symbol of a list's repetition unit, for splice-alignment checks:
/// either the element nonterminal or a specific separator terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitSym {
    Elem,
    Tok(TokenId),
}

#[derive(Clone, Copy, Debug)]
pub struct ListShape {
    pub cons: u16,
    pub seed: u16,
    /// The element nonterminal (the one NT in the cons repetition).
    pub elem: u16,
    /// First symbol of the cons repetition α (in `L → L α`) — what a
    /// CONTINUATION of the list must begin with.
    pub alpha_first: UnitSym,
    /// Last symbol of α — what any whole number of repetitions ends
    /// with. A reused run may only splice associatively if it ends here
    /// (Wagner §7: the LR state after the splice assumes whole units).
    pub alpha_last: UnitSym,
    /// First symbol of the seed production (None for an ε seed) — what
    /// the FIRST piece of a list must begin with.
    pub seed_first: Option<UnitSym>,
}

fn detect_lists(g: &SynGrammar, fragile: &[bool]) -> std::collections::HashMap<u16, ListShape> {
    let mut out = std::collections::HashMap::new();
    for nt in 0..g.nt_names.len() as u16 {
        let prods: Vec<u16> =
            (0..g.prods.len() as u16).filter(|&i| g.prods[i as usize].lhs == nt).collect();
        if prods.len() != 2 {
            continue;
        }
        let is_cons = |p: u16| {
            let rhs = &g.prods[p as usize].rhs;
            rhs.len() >= 2
                && rhs[0] == Sym::N(nt)
                && !rhs[1..].contains(&Sym::N(nt))
                // Exactly one element nonterminal per repetition, so the
                // typed items() accessor has a single well-defined type.
                && rhs[1..].iter().filter(|s| matches!(s, Sym::N(_))).count() == 1
        };
        let refs_self = |p: u16| g.prods[p as usize].rhs.contains(&Sym::N(nt));
        let (cons, seed) = match (is_cons(prods[0]), is_cons(prods[1])) {
            (true, false) if !refs_self(prods[1]) => (prods[0], prods[1]),
            (false, true) if !refs_self(prods[0]) => (prods[1], prods[0]),
            _ => continue,
        };
        if fragile[cons as usize] || fragile[seed as usize] {
            continue;
        }
        let alpha = &g.prods[cons as usize].rhs[1..];
        let unit = |s: &Sym| match s {
            Sym::N(_) => UnitSym::Elem,
            Sym::T(t) => UnitSym::Tok(*t),
        };
        let elem = alpha
            .iter()
            .find_map(|s| match s {
                Sym::N(n) => Some(*n),
                Sym::T(_) => None,
            })
            .expect("cons has exactly one element NT");
        out.insert(nt, ListShape {
            cons,
            seed,
            elem,
            alpha_first: unit(&alpha[0]),
            alpha_last: unit(alpha.last().unwrap()),
            seed_first: g.prods[seed as usize].rhs.first().map(|s| unit(s)),
        });
    }
    out
}

impl LrTables {
    /// The completion primitive: legal terminals in a state, by name.
    pub fn expected_tokens(&self, state: u16, g: &SynGrammar) -> Vec<String> {
        let mut v: Vec<String> = self.action[state as usize]
            .keys()
            .map(|&t| g.term_name(t).to_string())
            .collect();
        v.sort();
        v
    }
}

/// LR(1) item: (production, dot position, lookahead terminal).
type Item = (u16, u16, TokenId);

fn item_display(g: &SynGrammar, aug: &[(u16, Vec<Sym>)], item: Item) -> String {
    let (p, dot, la) = item;
    let (lhs_disp, rhs) = &aug[p as usize];
    let mut parts: Vec<String> = Vec::new();
    for (i, s) in rhs.iter().enumerate() {
        if i == dot as usize {
            parts.push("·".to_string());
        }
        parts.push(match s {
            Sym::T(t) => g.term_name(*t).to_string(),
            Sym::N(n) => g.nt_names[*n as usize].clone(),
        });
    }
    if dot as usize == rhs.len() {
        parts.push("·".to_string());
    }
    let lhs_name = if *lhs_disp == u16::MAX {
        "<start>"
    } else {
        &g.nt_names[*lhs_disp as usize]
    };
    format!("[{} → {} , {}]", lhs_name, parts.join(" "), g.term_name(la))
}

pub fn build_lr(g: &SynGrammar) -> LrTables {
    // Augmented grammar: production 0 is <start> → start_nt, EOF handled
    // via the accept action. `aug` holds (lhs, rhs) with lhs u16::MAX for
    // the synthetic start.
    let mut aug: Vec<(u16, Vec<Sym>)> = Vec::with_capacity(g.prods.len() + 1);
    aug.push((u16::MAX, vec![Sym::N(g.start)]));
    for p in &g.prods {
        aug.push((p.lhs, p.rhs.clone()));
    }
    let n_nts = g.nt_names.len();

    // prods_of[nt] = augmented production indices with that lhs
    let mut prods_of: Vec<Vec<u16>> = vec![Vec::new(); n_nts];
    for (i, (lhs, _)) in aug.iter().enumerate().skip(1) {
        prods_of[*lhs as usize].push(i as u16);
    }

    // ---- nullable + FIRST over nonterminals ----
    let mut nullable = vec![false; n_nts];
    let mut first: Vec<BTreeSet<TokenId>> = vec![BTreeSet::new(); n_nts];
    loop {
        let mut changed = false;
        for (lhs, rhs) in aug.iter().skip(1) {
            let l = *lhs as usize;
            let mut all_nullable = true;
            for s in rhs {
                match s {
                    Sym::T(t) => {
                        if first[l].insert(*t) {
                            changed = true;
                        }
                        all_nullable = false;
                        break;
                    }
                    Sym::N(n) => {
                        let adds: Vec<TokenId> = first[*n as usize].iter().copied().collect();
                        for t in adds {
                            if first[l].insert(t) {
                                changed = true;
                            }
                        }
                        if !nullable[*n as usize] {
                            all_nullable = false;
                            break;
                        }
                    }
                }
            }
            if all_nullable && !nullable[l] {
                nullable[l] = true;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    // FIRST of a symbol string followed by a lookahead.
    let first_of = |syms: &[Sym], la: TokenId| -> BTreeSet<TokenId> {
        let mut out = BTreeSet::new();
        for s in syms {
            match s {
                Sym::T(t) => {
                    out.insert(*t);
                    return out;
                }
                Sym::N(n) => {
                    out.extend(first[*n as usize].iter().copied());
                    if !nullable[*n as usize] {
                        return out;
                    }
                }
            }
        }
        out.insert(la);
        out
    };

    // ---- closure over LR(1) item sets ----
    let closure = |kernel: &BTreeSet<Item>| -> BTreeSet<Item> {
        let mut set = kernel.clone();
        let mut queue: VecDeque<Item> = kernel.iter().copied().collect();
        while let Some((p, dot, la)) = queue.pop_front() {
            let rhs = &aug[p as usize].1;
            if (dot as usize) >= rhs.len() {
                continue;
            }
            if let Sym::N(n) = rhs[dot as usize] {
                let rest = &rhs[dot as usize + 1..];
                for t in first_of(rest, la) {
                    for &q in &prods_of[n as usize] {
                        let item = (q, 0u16, t);
                        if set.insert(item) {
                            queue.push_back(item);
                        }
                    }
                }
            }
        }
        set
    };

    // ---- canonical collection ----
    let start_kernel: BTreeSet<Item> = [(0u16, 0u16, EOF)].into_iter().collect();
    let mut states: Vec<BTreeSet<Item>> = vec![closure(&start_kernel)];
    let mut index: HashMap<BTreeSet<Item>, u16> = HashMap::new();
    index.insert(states[0].clone(), 0);
    // transitions[state] : Sym -> state
    let mut transitions: Vec<HashMap<Sym, u16>> = vec![HashMap::new()];

    let mut wi = 0usize;
    while wi < states.len() {
        let cur = states[wi].clone();
        // Group by next symbol.
        let mut by_sym: HashMap<Sym, BTreeSet<Item>> = HashMap::new();
        for &(p, dot, la) in &cur {
            let rhs = &aug[p as usize].1;
            if (dot as usize) < rhs.len() {
                by_sym.entry(rhs[dot as usize]).or_default().insert((p, dot + 1, la));
            }
        }
        let mut syms: Vec<Sym> = by_sym.keys().copied().collect();
        syms.sort();
        for sym in syms {
            let kernel = &by_sym[&sym];
            let full = closure(kernel);
            let id = match index.get(&full) {
                Some(&id) => id,
                None => {
                    let id = states.len() as u16;
                    index.insert(full.clone(), id);
                    states.push(full);
                    transitions.push(HashMap::new());
                    id
                }
            };
            transitions[wi].insert(sym, id);
        }
        wi += 1;
    }

    // ---- ACTION / GOTO with precedence resolution ----
    let mut action: Vec<HashMap<TokenId, LrAct>> = vec![HashMap::new(); states.len()];
    let mut goto_: Vec<HashMap<u16, u16>> = vec![HashMap::new(); states.len()];
    let mut conflicts: Vec<Conflict> = Vec::new();
    let mut resolved = 0usize;
    let mut fragile = vec![false; g.prods.len()];

    // Shortest path (in symbols) from state 0 to each state — for traces.
    let paths: Vec<Vec<Sym>> = {
        let mut paths: Vec<Option<Vec<Sym>>> = vec![None; states.len()];
        paths[0] = Some(vec![]);
        let mut q = VecDeque::from([0u16]);
        while let Some(s) = q.pop_front() {
            let base = paths[s as usize].clone().unwrap();
            let mut ts: Vec<(Sym, u16)> = transitions[s as usize].iter().map(|(k, v)| (*k, *v)).collect();
            ts.sort();
            for (sym, t) in ts {
                if paths[t as usize].is_none() {
                    let mut p = base.clone();
                    p.push(sym);
                    paths[t as usize] = Some(p);
                    q.push_back(t);
                }
            }
        }
        paths.into_iter().map(|p| p.unwrap_or_default()).collect()
    };

    // Shortest terminal yield per NT — to render example inputs.
    let yields: Vec<Option<Vec<TokenId>>> = {
        let mut y: Vec<Option<Vec<TokenId>>> = vec![None; n_nts];
        loop {
            let mut changed = false;
            for (lhs, rhs) in aug.iter().skip(1) {
                let mut acc: Vec<TokenId> = Vec::new();
                let mut ok = true;
                for s in rhs {
                    match s {
                        Sym::T(t) => acc.push(*t),
                        Sym::N(n) => match &y[*n as usize] {
                            Some(v) => acc.extend(v.iter().copied()),
                            None => {
                                ok = false;
                                break;
                            }
                        },
                    }
                }
                if ok {
                    let l = *lhs as usize;
                    if y[l].as_ref().map_or(true, |old| acc.len() < old.len()) {
                        y[l] = Some(acc);
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }
        y
    };

    let example_for = |state: u16, la: TokenId| -> String {
        let mut terms: Vec<String> = Vec::new();
        for sym in &paths[state as usize] {
            match sym {
                Sym::T(t) => terms.push(g.term_name(*t).to_string()),
                Sym::N(n) => {
                    if let Some(y) = &yields[*n as usize] {
                        terms.extend(y.iter().map(|t| g.term_name(*t).to_string()));
                    } else {
                        terms.push(format!("<{}>", g.nt_names[*n as usize]));
                    }
                }
            }
        }
        format!("{} · {}", terms.join(" "), g.term_name(la))
    };

    for (si, set) in states.iter().enumerate() {
        // GOTO + shifts.
        for (sym, &t) in &transitions[si] {
            match sym {
                Sym::N(n) => {
                    goto_[si].insert(*n, t);
                }
                Sym::T(tok) => {
                    action[si].insert(*tok, LrAct::Shift(t));
                }
            }
        }
        // Reductions / accept.
        for &(p, dot, la) in set {
            let rhs = &aug[p as usize].1;
            if (dot as usize) != rhs.len() {
                continue;
            }
            if p == 0 {
                action[si].insert(EOF, LrAct::Accept);
                continue;
            }
            let red = LrAct::Reduce(p - 1); // user production index
            match action[si].get(&la).copied() {
                None => {
                    action[si].insert(la, red);
                }
                Some(LrAct::Shift(_)) => {
                    // Shift/reduce: try precedence.
                    let prod = &g.prods[(p - 1) as usize];
                    let pp = g.prod_precedence(prod);
                    let tp = if la == EOF { None } else { g.token_prec[la as usize] };
                    match (pp, tp) {
                        (Some((pl, _)), Some((tl, _))) if pl > tl => {
                            action[si].insert(la, red);
                            resolved += 1;
                            fragile[(p - 1) as usize] = true;
                        }
                        (Some((pl, _)), Some((tl, _))) if pl < tl => {
                            resolved += 1; // keep shift
                            fragile[(p - 1) as usize] = true;
                        }
                        (Some((pl, pa)), Some((tl, _))) => {
                            debug_assert_eq!(pl, tl);
                            match pa {
                                Assoc::Left => {
                                    action[si].insert(la, red);
                                }
                                Assoc::Right => {} // keep shift
                                Assoc::NonAssoc => {
                                    action[si].insert(la, LrAct::Error);
                                }
                            }
                            resolved += 1;
                            fragile[(p - 1) as usize] = true;
                        }
                        _ => {
                            let mut item_set: BTreeSet<String> = BTreeSet::new();
                            let mut prod_set: BTreeSet<u16> = BTreeSet::new();
                            for &it in set {
                                let (q, d, l) = it;
                                let r = &aug[q as usize].1;
                                let reduce_side = (d as usize) == r.len() && q == p && l == la;
                                let shift_side =
                                    (d as usize) < r.len() && r[d as usize] == Sym::T(la);
                                if reduce_side || shift_side {
                                    item_set.insert(item_display(g, &aug, it));
                                    if q > 0 {
                                        prod_set.insert(q - 1);
                                    }
                                }
                            }
                            let items: Vec<String> = item_set.into_iter().collect();
                            conflicts.push(Conflict {
                                state: si as u16,
                                lookahead: la,
                                kind: "shift/reduce",
                                items,
                                example: example_for(si as u16, la),
                                prods: prod_set.into_iter().collect(),
                            });
                        }
                    }
                }
                Some(LrAct::Reduce(other)) if other != p - 1 => {
                    let items: Vec<String> = set
                        .iter()
                        .filter(|&&(q, d, l)| {
                            l == la && (d as usize) == aug[q as usize].1.len() && (q == p || q == other + 1)
                        })
                        .map(|&it| item_display(g, &aug, it))
                        .collect();
                    let mut prods: Vec<u16> = vec![p - 1, other];
                    prods.sort_unstable();
                    prods.dedup();
                    conflicts.push(Conflict {
                        state: si as u16,
                        lookahead: la,
                        kind: "reduce/reduce",
                        items,
                        example: example_for(si as u16, la),
                        prods,
                    });
                }
                Some(_) => {}
            }
        }
    }

    let lists = detect_lists(g, &fragile);
    LrTables {
        action,
        goto_,
        n_states: states.len(),
        conflicts,
        resolved_by_prec: resolved,
        fragile,
        lists,
    }
}
