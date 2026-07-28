//! The identity-cached overlay against its oracle: after EVERY edit in
//! a hostile script, the incremental painter's frame must equal a
//! from-scratch painter's frame on the same document — run for run,
//! bit for bit. The C grammar is typeless, so `LiveDoc` takes the
//! identity-cached path; the oracle is a fresh open of the post-edit
//! text, whose overlay comes from the composed-view path.
//!
//! The script covers what the identity cache must survive: body edits
//! (one item's marks change), edits that ADD and REMOVE top-level
//! definitions (resolution cascades to other items), item insertion
//! and deletion (positional shifts), an edit inside a block comment
//! (mode state), a preprocessor line, and EOF edits.

use linework::{LineEdit, Limner, PaintDelta, Run};
use qana_lang::live::LiveDoc;
use qana_lang::EmbeddedLang;
use std::sync::Arc;

/// Consume a delta exactly the way an editor does: splice the damaged
/// window, then apply repaints at their (post-splice) line indices.
fn apply_delta(frame: &mut Vec<Vec<Run>>, delta: &PaintDelta) {
    if let Some(((lo, hi), repl)) = &delta.splice {
        frame.splice(*lo as usize..*hi as usize, repl.iter().cloned());
    }
    for (li, runs) in &delta.repaints {
        frame[*li as usize] = runs.clone();
    }
}

const C_QANA: &str = include_str!("../../../examples/c/c.qana");
const C_DEMO: &str = include_str!("../../../examples/c/demo.c");

fn apply(text: &str, edit: &LineEdit) -> String {
    let mut lines: Vec<&str> = text.lines().collect();
    let repl: Vec<&str> = edit.lines.iter().map(|s| s.as_str()).collect();
    let end = (edit.end as usize).min(lines.len());
    lines.splice(edit.start as usize..end, repl);
    let mut out = lines.join("\n");
    if text.ends_with('\n') {
        out.push('\n');
    }
    out
}

#[test]
fn incremental_overlay_equals_fresh_open_after_every_edit() {
    let lang = Arc::new(EmbeddedLang::from_qana_source(C_QANA).expect("c.qana certifies"));

    // Two copies of the demo so cross-item resolution has real targets.
    let mut text = format!("{C_DEMO}\n{C_DEMO}");
    let mut live: Box<dyn Limner> = Box::new(LiveDoc::open(lang.clone(), "diff.c", &text));
    let mut frame = live.open(&text).lines;

    let n = text.lines().count() as u32;
    let script: Vec<LineEdit> = vec![
        // Body edit inside a function.
        LineEdit { start: n / 3, end: n / 3 + 1, lines: vec!["  int zz = 1 + 2;".into()] },
        // Add a top-level definition (cascades env fingerprints).
        LineEdit { start: 0, end: 0, lines: vec!["int added_global;".into()] },
        // Reference it far away (marks in a different item).
        LineEdit {
            start: (n * 2) / 3,
            end: (n * 2) / 3 + 1,
            lines: vec!["  added_global = 7;".into()],
        },
        // Remove the definition again (unresolves the reference).
        LineEdit { start: 0, end: 1, lines: vec![] },
        // Insert a whole new item (positional shift for the suffix).
        LineEdit {
            start: 2,
            end: 2,
            lines: vec!["static int inserted(void) { return 3; }".into()],
        },
        // Preprocessor line (mode PP).
        LineEdit { start: 1, end: 1, lines: vec!["#define DIFF_GATE 1".into()] },
        // Open a block comment mid-file, then close it (mode BLK).
        LineEdit { start: 8, end: 8, lines: vec!["/* opened".into()] },
        LineEdit { start: 9, end: 9, lines: vec!["closed */".into()] },
        // Edit the final line.
        LineEdit {
            start: n.saturating_sub(1) + 4,
            end: n.saturating_sub(1) + 5,
            lines: vec!["/* eof tweak */".into()],
        },
        // Delete the inserted item again.
        LineEdit { start: 2, end: 3, lines: vec![] },
    ];

    for (step, edit) in script.iter().enumerate() {
        let delta = live.edit(edit);
        apply_delta(&mut frame, &delta);
        text = apply(&text, edit);
        assert_eq!(live.text(), text, "step {step}: mirror lost the document");

        let mut oracle: Box<dyn Limner> =
            Box::new(LiveDoc::open(lang.clone(), "oracle.c", &text));
        let fresh = oracle.open(&text);
        assert_eq!(frame.len(), fresh.lines.len(), "step {step}: line counts diverge");
        for (li, (a, b)) in frame.iter().zip(fresh.lines.iter()).enumerate() {
            assert_eq!(a, b, "step {step}: line {li} runs diverge");
        }
    }
}
