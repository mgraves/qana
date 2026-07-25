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
use rantlr_sem::SemDb;

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
    Ok(ExpandOutcome { text: current, segs, passes, substitutions, diags, repairs })
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
