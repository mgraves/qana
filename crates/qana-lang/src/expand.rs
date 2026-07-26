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
use qana_engine::IncSession;
use qana_grammar::{CompiledLexer, LrTables};
use qana_sem::macros::{compose, expand_pass, MacroDiag, Seg, SegKind, SyntaxInfo};
use qana_sem::{SemDb, Target};

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
            diags.push(MacroDiag::error(
                (0, 0),
                format!(
                    "expansion did not converge in {max_passes} passes — recursive macro? (capped)"
                ),
            ));
            break;
        }
    }
    // Hygiene, on the RESULT: every reference that survived must still
    // mean what it meant where it was written — and where it does not,
    // alpha-convert the binder that swallowed it and check again. A
    // round that fails to reduce the captures (or produces text that
    // no longer parses) is abandoned, and the captures are reported
    // instead: repair is attempted, never assumed.
    if substitutions > 0 {
        let mut caps = captures(lexer, def, tables, text, siblings, &current, &segs);
        for _ in 0..4 {
            if caps.is_empty() {
                break;
            }
            let Some((t2, s2, notes)) =
                rename_capturers(lexer, def, tables, &current, &segs, &caps)
            else {
                break;
            };
            let clean = IncSession::new(lexer, &def.sg, tables, &t2)
                .map(|s| s.last_repairs.is_empty())
                .unwrap_or(false);
            let caps2 = captures(lexer, def, tables, text, siblings, &t2, &s2);
            if !clean || caps2.len() >= caps.len() {
                break;
            }
            current = t2;
            segs = s2;
            diags.extend(notes);
            caps = caps2;
        }
        for c in caps {
            diags.push(MacroDiag::error(
                c.home_span,
                format!(
                    "hygiene: `{}` changes meaning when expanded — where it is written it names {}, but after expansion it binds {}",
                    c.name, c.was, c.now
                ),
            ));
        }
    }
    Ok(ExpandOutcome { text: current, segs, passes, substitutions, diags, repairs })
}

// ---------------------------------------------------------------------------
// Hygiene: expansion must not change what a name MEANS
// ---------------------------------------------------------------------------

/// One reference whose meaning expansion changed.
struct Capture {
    /// Index into the OUTPUT's ref table.
    ref_idx: usize,
    name: String,
    /// Where the reference was written (span in its own file).
    home_span: (u32, u32),
    /// The output definition it now (wrongly) binds — the CAPTURING
    /// binder, and the thing renaming has to get out of the way.
    now_def: Option<usize>,
    was: String,
    now: String,
}

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
/// world it LANDED in.
fn captures(
    lexer: &CompiledLexer,
    def: &LangDef,
    tables: &LrTables,
    src_text: &str,
    siblings: &[(String, String)],
    out_text: &str,
    segs: &[Seg],
) -> Vec<Capture> {
    let mut out = Vec::new();
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
        return out;
    };

    // Where an output span came from: (file, span in that file).
    let origin = |span: (u32, u32)| -> Option<(String, (u32, u32))> {
        let i = segs.partition_point(|s| s.out.1 <= span.0);
        let s = segs.get(i)?;
        if span.1 > s.out.1 || s.kind.synthesized() {
            return None;
        }
        let d = span.0 - s.out.0;
        // A renamed name is not a copy: it stands for its whole
        // source token, whatever the lengths.
        if s.kind == SegKind::Rename {
            return Some((s.src_uri.clone().unwrap_or_else(|| "src".into()), s.src));
        }
        Some((
            s.src_uri.clone().unwrap_or_else(|| "src".into()),
            (s.src.0 + d, s.src.0 + d + (span.1 - span.0)),
        ))
    };
    let describe = |t: &Option<(String, (u32, u32))>| match t {
        Some((u, s)) if u == "src" => format!("the definition at byte {}", s.0),
        Some((u, s)) => format!("the definition at byte {} of {u}", s.0),
        None => "nothing".to_string(),
    };

    let out_syms = after.symbols("out");
    let out_res = after.resolve("out");
    for (ri, r) in out_syms.refs.iter().enumerate() {
        let Some((home, home_span)) = origin(r.span) else { continue };
        // What this reference meant where it was WRITTEN.
        let (bsyms, bres) = (before.symbols(&home), before.resolve(&home));
        let Some(bi) = bsyms.refs.iter().position(|x| x.span == home_span) else { continue };
        let was = match bres.get(bi) {
            Some(&Target::Local { def }) => bsyms.defs.get(def).map(|d| (home.clone(), d.span)),
            Some(Target::Foreign { uri, def }) => {
                before.symbols(uri).defs.get(*def).map(|d| (uri.clone(), d.span))
            }
            _ => None,
        };
        // What it means where it LANDED, expressed in the same terms.
        let now = match out_res.get(ri) {
            Some(&Target::Local { def }) => out_syms.defs.get(def).and_then(|d| origin(d.span)),
            Some(Target::Foreign { uri, def }) => {
                after.symbols(uri).defs.get(*def).map(|d| (uri.clone(), d.span))
            }
            _ => None,
        };
        if was == now {
            continue;
        }
        out.push(Capture {
            ref_idx: ri,
            name: r.name.clone(),
            home_span,
            now_def: match out_res.get(ri) {
                Some(&Target::Local { def }) => Some(def),
                _ => None,
            },
            was: describe(&was),
            now: describe(&now),
        });
    }
    out
}

/// REPAIR the captures by alpha-converting the capturing binder.
///
/// Whichever side introduced the shadow — a local at the use site that
/// swallowed a body's free name, or a binding inside the body that
/// swallowed an argument — renaming THAT binder (and every reference
/// that legitimately resolves to it) restores every captured
/// reference to the definition it named, and changes nothing else: a
/// consistent rename of a binding and its references is invisible
/// semantics. The captured references keep their spelling, which is
/// the whole point — they go back to meaning what they say.
///
/// Returns the rewritten text, its provenance, and one note per
/// rename. `None` when no capture is repairable this way.
fn rename_capturers(
    lexer: &CompiledLexer,
    def: &LangDef,
    tables: &LrTables,
    out_text: &str,
    segs: &[Seg],
    caps: &[Capture],
) -> Option<(String, Vec<Seg>, Vec<MacroDiag>)> {
    let session = IncSession::new(lexer, &def.sg, tables, out_text).ok()?;
    let mut db = SemDb::new(def.binding.clone());
    db.set_tree("out", session.tree()?.clone());
    let syms = db.symbols("out");
    let res = db.resolve("out");

    // Group the captures by the binder that swallowed them.
    let mut by_def: Vec<(usize, Vec<usize>)> = Vec::new();
    for c in caps {
        let Some(d) = c.now_def else { continue };
        match by_def.iter_mut().find(|(dd, _)| *dd == d) {
            Some((_, v)) => v.push(c.ref_idx),
            None => by_def.push((d, vec![c.ref_idx])),
        }
    }
    if by_def.is_empty() {
        return None;
    }

    let mut edits: Vec<((u32, u32), String)> = Vec::new();
    let mut notes = Vec::new();
    for (d, captured) in &by_def {
        let dname = &syms.defs[*d].name;
        // A name that appears NOWHERE in the text cannot collide with
        // anything in it. (`_h` and digits are assumed to be
        // identifier characters — a language where they are not gets
        // detection without repair, and says so.)
        let fresh = (1..64)
            .map(|n| format!("{dname}_h{n}"))
            .find(|cand| !out_text.contains(cand.as_str()))?;
        // The binder itself, plus every reference that SHOULD keep
        // naming it — never the captured ones, which must go back to
        // meaning what they say.
        edits.push((syms.defs[*d].span, fresh.clone()));
        for (ri, _) in syms.refs.iter().enumerate() {
            if captured.contains(&ri) {
                continue;
            }
            if matches!(res.get(ri), Some(&Target::Local { def }) if def == *d) {
                edits.push((syms.refs[ri].span, fresh.clone()));
            }
        }
        notes.push(MacroDiag::note(
            syms.defs[*d].span,
            format!(
                "hygiene: renamed `{dname}` to `{fresh}` so `{}` keeps its meaning after expansion",
                caps.iter()
                    .find(|c| c.now_def == Some(*d))
                    .map(|c| c.name.as_str())
                    .unwrap_or(dname)
            ),
        ));
    }
    edits.sort_by_key(|(s, _)| s.0);

    // Rewrite text and provenance together. A renamed name is a
    // segment of its own, still pointing at the name as written.
    let mut text = String::with_capacity(out_text.len());
    let mut nsegs: Vec<Seg> = Vec::new();
    let copy = |text: &mut String, nsegs: &mut Vec<Seg>, s: &Seg, a: u32, b: u32| {
        if b <= a {
            return;
        }
        let start = text.len() as u32;
        text.push_str(&out_text[a as usize..b as usize]);
        let src = if s.kind.copies() {
            (s.src.0 + (a - s.out.0), s.src.0 + (b - s.out.0))
        } else {
            s.src
        };
        nsegs.push(Seg { out: (start, text.len() as u32), src, kind: s.kind, src_uri: s.src_uri.clone() });
    };
    let mut ei = 0usize;
    for s in segs {
        let mut cur = s.out.0;
        while ei < edits.len() && edits[ei].0 .0 < s.out.1 {
            let (espan, fresh) = &edits[ei];
            if espan.0 < cur {
                ei += 1;
                continue;
            }
            copy(&mut text, &mut nsegs, s, cur, espan.0);
            let start = text.len() as u32;
            text.push_str(fresh);
            nsegs.push(Seg {
                out: (start, text.len() as u32),
                src: if s.kind.copies() {
                    (s.src.0 + (espan.0 - s.out.0), s.src.0 + (espan.1 - s.out.0))
                } else {
                    s.src
                },
                kind: SegKind::Rename,
                src_uri: s.src_uri.clone(),
            });
            cur = espan.1;
            ei += 1;
        }
        copy(&mut text, &mut nsegs, s, cur, s.out.1);
    }
    Some((text, nsegs, notes))
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
            SegKind::Rename => "rename",
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
