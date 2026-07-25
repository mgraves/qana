//! The expansion FIXPOINT: parse, bind, substitute — repeat on the
//! output until nothing expands. This is the meta tier's whole trick:
//! there is no separate macro interpreter. Each pass's output is an
//! ordinary document run through the ordinary engine (same lexer,
//! same LR tables, same binding walk), which is why expansion output
//! gets full editor intelligence for free and why a broken expansion
//! surfaces as ordinary diagnostics in the materialized text.
//!
//! L5 stands: the PARSER never expands. Expansion is a downstream
//! derivation that consumes parses — and always terminates, because
//! the pass count is capped and the cap is a diagnostic, not a hang.

use crate::compile::LangDef;
use rantlr_engine::IncSession;
use rantlr_grammar::{CompiledLexer, LrTables};
use rantlr_sem::macros::{compose, expand_pass, MacroDiag, Seg, SegKind, SyntaxInfo};
use rantlr_sem::{SemDb, Target};

pub struct ExpandOutcome {
    pub text: String,
    /// Maps every output byte to the ORIGINAL document (verbatim /
    /// body / arg), tiling the output exactly.
    pub segs: Vec<Seg>,
    pub passes: u32,
    pub substitutions: u32,
    pub diags: Vec<MacroDiag>,
    /// Repairs seen while parsing intermediate text (0 for healthy
    /// macros — nonzero means an expansion produced broken syntax,
    /// which the materialized file will show as ordinary errors).
    pub repairs: usize,
}

/// Expand `text` to fixpoint (bounded by `max_passes`). `siblings`
/// are (uri, text) pairs for the document's neighbors: macros DEFINED
/// there expand here (their bodies splice from the sibling's text,
/// with provenance naming the file), and the spliced output re-binds
/// in THIS document's context. Sibling texts are fixed — only the
/// primary document iterates.
pub fn expand_document(
    lexer: &CompiledLexer,
    def: &LangDef,
    tables: &LrTables,
    text: &str,
    siblings: &[(String, String)],
    max_passes: u32,
) -> Result<ExpandOutcome, String> {
    // Parse each sibling ONCE — their trees are pass-invariant.
    // What the grammar declares about shape — precedence and its own
    // grouping productions. This is what makes substitution
    // syntax-aware instead of textual.
    let syn = SyntaxInfo::derive(&def.lex, &def.sg);
    let mut sib_trees = Vec::new();
    for (uri, stext) in siblings {
        let s = IncSession::new(lexer, &def.sg, tables, stext)
            .map_err(|e| format!("sibling {uri} parse failed: {e:?}"))?;
        let t = s.tree().ok_or_else(|| format!("sibling {uri} produced no tree"))?.clone();
        sib_trees.push((uri.clone(), t));
    }
    let mut current = text.to_string();
    // Identity provenance to start: everything verbatim.
    let mut segs = vec![Seg {
        out: (0, current.len() as u32),
        src: (0, current.len() as u32),
        kind: SegKind::Verbatim,
        src_uri: None,
    }];
    let mut diags = Vec::new();
    let mut passes = 0u32;
    let mut substitutions = 0u32;
    let mut repairs = 0usize;
    loop {
        let session = IncSession::new(lexer, &def.sg, tables, &current)
            .map_err(|e| format!("expansion parse failed: {e:?}"))?;
        repairs += session.last_repairs.len();
        let tree = session.tree().ok_or("expansion parse produced no tree")?;
        let mut db = SemDb::new(def.binding.clone());
        for (uri, t) in &sib_trees {
            db.set_tree(uri, t.clone());
        }
        db.set_tree("expand", tree.clone());
        let pass =
            expand_pass(&mut db, "expand", &tree, &current, &def.macros, Some(&def.types), &syn);
        diags.extend(pass.diags.iter().cloned());
        if pass.substitutions == 0 {
            break;
        }
        substitutions += pass.substitutions;
        passes += 1;
        segs = compose(&segs, &pass.segs);
        current = pass.text;
        if passes >= max_passes {
            diags.push(MacroDiag {
                span: (0, 0),
                msg: format!(
                    "expansion did not converge in {max_passes} passes — recursive macro? (capped)"
                ),
            });
            break;
        }
    }
    // Hygiene is checked on the RESULT, once: every reference that
    // survived must still mean what it meant where it was written.
    if substitutions > 0 {
        diags.extend(check_hygiene(lexer, def, tables, text, siblings, &current, &segs));
    }
    Ok(ExpandOutcome { text: current, segs, passes, substitutions, diags, repairs })
}

// ---------------------------------------------------------------------------
// Hygiene: expansion must not change what a name MEANS
// ---------------------------------------------------------------------------

/// The whole hygiene property, in one sentence the binding tier can
/// check: EVERY REFERENCE THAT SURVIVES EXPANSION MUST RESOLVE TO THE
/// SAME DEFINITION AFTERWARDS AS IT DID BEFORE. A macro body's free
/// name that binds to a local at the use site (cpp's classic capture),
/// an argument whose name is captured by a binding inside the body,
/// a reference that loses its binding entirely — all three are the
/// same violation of the same rule, so one check finds them all.
///
/// Provenance is what makes it decidable: every output byte knows the
/// file and span it came from, so a reference in the output can be
/// looked up in the world it was WRITTEN in and compared with the
/// world it LANDED in. v1 REPORTS captures (renaming to avoid them is
/// the named successor — it needs the same information, plus a way to
/// mint names the grammar admits).
fn check_hygiene(
    lexer: &CompiledLexer,
    def: &LangDef,
    tables: &LrTables,
    src_text: &str,
    siblings: &[(String, String)],
    out_text: &str,
    segs: &[Seg],
) -> Vec<MacroDiag> {
    let mut diags = Vec::new();
    let world = |uri: &str, text: &str| -> Option<SemDb> {
        let mut db = SemDb::new(def.binding.clone());
        for (su, st) in siblings {
            let s = IncSession::new(lexer, &def.sg, tables, st).ok()?;
            db.set_tree(su, s.tree()?.clone());
        }
        let s = IncSession::new(lexer, &def.sg, tables, text).ok()?;
        db.set_tree(uri, s.tree()?.clone());
        Some(db)
    };
    let (Some(mut before), Some(mut after)) = (world("src", src_text), world("out", out_text))
    else {
        return diags;
    };

    // Where an output span came from: (file, span in that file).
    let origin = |span: (u32, u32)| -> Option<(String, (u32, u32))> {
        let i = segs.partition_point(|s| s.out.1 <= span.0);
        let s = segs.get(i)?;
        if span.1 > s.out.1 || s.kind.synthesized() {
            return None;
        }
        let d = span.0 - s.out.0;
        Some((
            s.src_uri.clone().unwrap_or_else(|| "src".into()),
            (s.src.0 + d, s.src.0 + d + (span.1 - span.0)),
        ))
    };

    let out_syms = after.symbols("out");
    let out_res = after.resolve("out");
    for (ri, r) in out_syms.refs.iter().enumerate() {
        let Some((home, home_span)) = origin(r.span) else { continue };
        // What this reference meant where it was WRITTEN.
        let (bsyms, bres) = (before.symbols(&home), before.resolve(&home));
        let Some(bi) = bsyms.refs.iter().position(|x| x.span == home_span) else { continue };
        let was = match bres.get(bi) {
            Some(&Target::Local { def }) => {
                bsyms.defs.get(def).map(|d| (home.clone(), d.span))
            }
            Some(Target::Foreign { uri, def }) => {
                before.symbols(uri).defs.get(*def).map(|d| (uri.clone(), d.span))
            }
            _ => None,
        };
        // What it means where it LANDED, expressed in the same terms.
        let now = match out_res.get(ri) {
            Some(&Target::Local { def }) => {
                out_syms.defs.get(def).and_then(|d| origin(d.span))
            }
            Some(Target::Foreign { uri, def }) => {
                after.symbols(uri).defs.get(*def).map(|d| (uri.clone(), d.span))
            }
            _ => None,
        };
        if was == now {
            continue;
        }
        let where_ = |t: &Option<(String, (u32, u32))>| match t {
            Some((u, s)) if u == "src" => format!("the definition at byte {}", s.0),
            Some((u, s)) => format!("the definition at byte {} of {u}", s.0),
            None => "nothing".to_string(),
        };
        diags.push(MacroDiag {
            span: home_span,
            msg: format!(
                "hygiene: `{}` changes meaning when expanded — where it is written it names {}, but after expansion it binds {}",
                r.name,
                where_(&was),
                where_(&now)
            ),
        });
    }
    diags
}

/// The provenance sidecar, serialized without a JSON dependency: an
/// object with the source hash and the segment list. Deterministic —
/// byte-identical for identical inputs (the write-if-changed key).
pub fn provenance_json(source_uri: &str, source: &str, out: &ExpandOutcome) -> String {
    let mut s = String::new();
    s.push_str("{\n");
    s.push_str(&format!("  \"generated_from\": {:?},\n", source_uri));
    s.push_str(&format!("  \"source_fnv\": \"{:016x}\",\n", fnv64(source.as_bytes())));
    s.push_str(&format!("  \"passes\": {},\n", out.passes));
    s.push_str(&format!("  \"substitutions\": {},\n", out.substitutions));
    s.push_str("  \"segments\": [\n");
    for (i, seg) in out.segs.iter().enumerate() {
        let kind = match seg.kind {
            SegKind::Verbatim => "verbatim",
            SegKind::Body => "body",
            SegKind::Arg => "arg",
            SegKind::Sep => "sep",
            SegKind::Paren => "paren",
            SegKind::Meta => "meta",
        };
        let file = match &seg.src_uri {
            Some(u) => format!(", \"file\": {u:?}"),
            None => String::new(),
        };
        s.push_str(&format!(
            "    {{\"out\": [{}, {}], \"src\": [{}, {}], \"kind\": \"{}\"{}}}{}\n",
            seg.out.0,
            seg.out.1,
            seg.src.0,
            seg.src.1,
            kind,
            file,
            if i + 1 == out.segs.len() { "" } else { "," }
        ));
    }
    s.push_str("  ]\n}\n");
    s
}

fn fnv64(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}
