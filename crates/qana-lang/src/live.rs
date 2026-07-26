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
use linework::{FactCard, Hint, LineEdit, Limner, Paint, PaintDelta};
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
        // Terminators: keep the replaced lines' own, positionally;
        // extras get the document's prevailing flavor (the protocol
        // stays out of newline politics).
        let prevailing = self
            .session
            .buf
            .lines
            .first()
            .map(|l| l.term)
            .filter(|t| !matches!(t, LineTerm::None))
            .unwrap_or(LineTerm::Lf);
        let (start, end) = (edit.start as usize, edit.end as usize);
        let replacement: Vec<Line> = edit
            .lines
            .iter()
            .enumerate()
            .map(|(i, text)| {
                let term = self
                    .session
                    .buf
                    .lines
                    .get(start + i)
                    .filter(|_| start + i < end)
                    .map(|l| l.term)
                    .unwrap_or(prevailing);
                Line::new(text.clone(), term)
            })
            .collect();
        let outcome = self
            .session
            .edit(self.lang.sg, self.lang.tables, &[qana_engine::LineEdit {
                start,
                end,
                replacement,
            }])
            .expect("total under errors");
        self.db.set_tree(&self.uri, self.session.tree().expect("total").clone());
        self.painter.update(
            &self.session,
            &outcome.damage,
            &self.lang.styles,
            Some((&mut self.db, &self.uri)),
        )
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
}
