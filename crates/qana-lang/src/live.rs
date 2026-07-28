//! A LIVE DOCUMENT: the whole certified pipeline behind one
//! [`linework::Limner`]. This is what an editor widget holds — through
//! the trait, never through this crate — to get keystroke-synchronous
//! paint, semantic modifier bits, hover facts, and inline hints from
//! a single `.qana` grammar.
//!
//! Layering (the point of the exercise): the editor depends on `linework`
//! alone; this crate depends on `linework` and implements it. The two
//! meet only at the trait.

use crate::EmbeddedLang;
use linework::{FactCard, Hint, LineEdit, Limner, Mark, Paint, PaintDelta};
use qana_engine::{IncSession, Line, LineTerm};
use qana_sem::SemDb;
use qana_services::paint::{facts_at, type_hints, Painter};
use std::sync::Arc;

/// One open document over one language. Owns the incremental session,
/// the semantic database, and the painter; answers the [`Limner`]
/// contract from them.
pub struct LiveDoc {
    lang: Arc<EmbeddedLang>,
    uri: String,
    session: IncSession<'static>,
    db: SemDb,
    painter: Painter,
}

impl LiveDoc {
    /// Open `text` under `lang`. The `uri` names the document in fact
    /// cards (and, later, in multi-document worlds).
    pub fn open(lang: Arc<EmbeddedLang>, uri: impl Into<String>, text: &str) -> LiveDoc {
        let uri = uri.into();
        let session = lang.session(text);
        let mut db = SemDb::new(lang.binding.clone());
        db.set_types(lang.types.clone());
        db.set_macro_bodies(&lang.macros);
        db.set_tree(&uri, session.tree().expect("total").clone());
        let (painter, _) = Painter::new(&session, &lang.styles, Some((&mut db, &uri)));
        LiveDoc { lang, uri, session, db, painter }
    }
}

impl Limner for LiveDoc {
    fn open(&mut self, text: &str) -> Paint {
        self.session = self.lang.session(text);
        self.db = SemDb::new(self.lang.binding.clone());
        self.db.set_types(self.lang.types.clone());
        self.db.set_macro_bodies(&self.lang.macros);
        self.db.set_tree(&self.uri, self.session.tree().expect("total").clone());
        let (painter, paint) =
            Painter::new(&self.session, &self.lang.styles, Some((&mut self.db, &self.uri)));
        self.painter = painter;
        paint
    }

    fn edit(&mut self, edit: &LineEdit) -> PaintDelta {
        // Terminators: interior replacement lines keep the replaced
        // lines' own, positionally (CRLF fidelity in mixed files);
        // extras get the document's prevailing flavor (the protocol
        // stays out of newline politics). THE CANONICAL-FORM CLAUSE:
        // when the edit reaches the document's end, the replacement's
        // last line becomes the new final line and must carry `None` —
        // and a positionally-reused `None` must never land anywhere
        // else. (Blind positional reuse turned a Return that split the
        // final line into two invariant violations at once: the old
        // final line's `None` landed on the now-interior left half,
        // and the new final line was padded with `Lf`.)
        let old_len = self.session.buf.lines.len();
        let prevailing = self
            .session
            .buf
            .lines
            .first()
            .map(|l| l.term)
            .filter(|t| !matches!(t, LineTerm::None))
            .unwrap_or(LineTerm::Lf);
        let (mut start, end) = (edit.start as usize, edit.end as usize);
        let reaches_end = end >= old_len;
        let n = edit.lines.len();
        let mut replacement: Vec<Line> = edit
            .lines
            .iter()
            .enumerate()
            .map(|(i, text)| {
                let term = if reaches_end && i + 1 == n {
                    LineTerm::None
                } else {
                    self.session
                        .buf
                        .lines
                        .get(start + i)
                        .filter(|_| start + i < end)
                        .map(|l| l.term)
                        .filter(|t| !matches!(t, LineTerm::None))
                        .unwrap_or(prevailing)
                };
                Line::new(text.clone(), term)
            })
            .collect();
        // A pure tail deletion would leave an old, terminated line as
        // the new final line; widen the batch by one line so the same
        // edit re-terminates it. (An emptied document keeps canonical
        // form as a single term-less empty line.)
        if reaches_end && replacement.is_empty() {
            if start > 0 {
                start -= 1;
                let kept = self.session.buf.lines[start].text.clone();
                replacement.push(Line::new(kept, LineTerm::None));
            } else {
                replacement.push(Line::new(String::new(), LineTerm::None));
            }
        }
        // `QANA_TRACE_EDIT=1` prints per-stage timings — the scale
        // harness's scalpel for attributing keystroke cost.
        let trace = std::env::var_os("QANA_TRACE_EDIT").is_some();
        let t0 = std::time::Instant::now();
        let outcome = self
            .session
            .edit(self.lang.sg, self.lang.tables, &[qana_engine::LineEdit {
                start,
                end,
                replacement,
            }])
            .expect("total under errors");
        let t_parse = t0.elapsed();
        let t1 = std::time::Instant::now();
        self.db.set_tree(&self.uri, self.session.tree().expect("total").clone());
        let t_sem = t1.elapsed();
        let t2 = std::time::Instant::now();
        let delta = self.painter.update(
            &self.session,
            &outcome.damage,
            &self.lang.styles,
            Some((&mut self.db, &self.uri)),
        );
        if trace {
            eprintln!(
                "[edit-trace] parse {t_parse:?} | sem set_tree {t_sem:?} | paint+resolve {:?}",
                t2.elapsed()
            );
        }
        delta
    }

    fn facts(&mut self, offset: u32) -> Option<FactCard> {
        let text = self.session.buf.reproduce();
        facts_at(&mut self.db, &self.uri, &text, offset)
    }

    fn hints(&mut self) -> Vec<Hint> {
        let text = self.session.buf.reproduce();
        type_hints(&mut self.db, &self.uri, &text)
    }

    fn legend(&self) -> Vec<String> {
        self.lang.styles.legend.iter().map(|s| s.to_string()).collect()
    }

    fn text(&mut self) -> String {
        self.session.buf.reproduce()
    }

    fn marks(&mut self) -> Vec<Mark> {
        let kinds = mark_kinds(&self.lang.outline);
        let tree = self.session.tree().expect("total");
        qana_services::outline(tree, &self.lang.outline)
            .into_iter()
            .map(|s| Mark {
                category: kinds.iter().position(|k| *k == s.kind).unwrap_or(0) as u8,
                start: s.span.0,
                end: s.span.1,
                name_start: s.selection.0,
                name_end: s.selection.1,
                name: s.name,
            })
            .collect()
    }

    fn enclosing(&mut self, offset: u32) -> Vec<Mark> {
        // Containment filter over the outline walk. Document order is
        // pre-order, so the surviving chain is outermost-first for
        // free. (A targeted tree descent is the documented fast path if
        // a giant document ever makes this cursor-move query warm.)
        self.marks()
            .into_iter()
            .filter(|m| m.start <= offset && offset < m.end)
            .collect()
    }

    fn mark_legend(&self) -> Vec<String> {
        mark_kinds(&self.lang.outline).iter().map(|k| k.to_string()).collect()
    }
}

/// Mark categories = the outline config's kinds, deduped in first-use
/// order. `marks` and `mark_legend` both derive from this so category
/// indices always agree.
fn mark_kinds(cfg: &qana_services::OutlineConfig) -> Vec<&'static str> {
    let mut kinds: Vec<&'static str> = Vec::new();
    for e in &cfg.entries {
        if !kinds.contains(&e.kind) {
            kinds.push(e.kind);
        }
    }
    kinds
}
