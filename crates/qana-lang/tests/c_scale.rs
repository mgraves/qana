//! The C grammar at SCALE — the numbers behind "huge C files, live."
//!
//! Amplifies the C demo file to editor-hostile sizes and measures the
//! whole Limner surface an editor exercises: cold open+paint, mid-file
//! and end-of-file keystrokes, and the marks channel. Prints the
//! numbers (run with `--nocapture`; use `--release` for honest ones)
//! and asserts only generous sanity ceilings so the suite stays green
//! on slow machines and debug builds.
//!
//! The amplified document is plain concatenation: duplicate top-level
//! names produce semantic diagnostics, which is realistic noise — the
//! engine's job is to stay total and incremental regardless.

use linework::{LineEdit, Limner};
use qana_lang::live::LiveDoc;
use qana_lang::EmbeddedLang;
use std::sync::Arc;
use std::time::{Duration, Instant};

const C_QANA: &str = include_str!("../../../examples/c/c.qana");
const C_DEMO: &str = include_str!("../../../examples/c/demo.c");

fn amplified(target_lines: usize) -> String {
    let base_lines = C_DEMO.lines().count().max(1);
    let copies = target_lines.div_ceil(base_lines);
    let mut out = String::with_capacity((C_DEMO.len() + 1) * copies);
    for _ in 0..copies {
        out.push_str(C_DEMO);
        if !C_DEMO.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

struct Sample {
    lines: usize,
    bytes: usize,
    open: Duration,
    mid_edit: Duration,
    mid_splice_lines: usize,
    eof_edit: Duration,
    marks: Duration,
    mark_count: usize,
    enclosing: Duration,
}

fn measure(lang: &Arc<EmbeddedLang>, target_lines: usize) -> Sample {
    let text = amplified(target_lines);
    let lines = text.lines().count();
    let bytes = text.len();

    let t = Instant::now();
    let mut l: Box<dyn Limner> = Box::new(LiveDoc::open(lang.clone(), "scale.c", &text));
    let paint = l.open(&text);
    let open = t.elapsed();
    assert_eq!(
        paint.lines.len(),
        lines + usize::from(text.ends_with('\n')),
        "every line painted — totality at scale"
    );

    // A mid-file keystroke: retype one line with a tweak.
    let mid = (lines / 2) as u32;
    let mid_text: String = text.lines().nth(mid as usize).unwrap_or("").to_string();
    let t = Instant::now();
    let delta = l.edit(&LineEdit {
        start: mid,
        end: mid + 1,
        lines: vec![format!("{mid_text} ")],
    });
    let mid_edit = t.elapsed();
    let mid_splice_lines = delta
        .splice
        .as_ref()
        .map(|((lo, hi), _)| (*hi - *lo) as usize)
        .unwrap_or(0);

    // An end-of-file keystroke.
    let last = lines.saturating_sub(1) as u32;
    let last_text: String = text.lines().nth(last as usize).unwrap_or("").to_string();
    let t = Instant::now();
    let _ = l.edit(&LineEdit {
        start: last,
        end: last + 1,
        lines: vec![format!("{last_text} ")],
    });
    let eof_edit = t.elapsed();

    // The marks channel (structure view / folding provider feed).
    let t = Instant::now();
    let marks = l.marks();
    let marks_time = t.elapsed();
    let mark_count = marks.len();

    // The breadcrumbs query at a mid-file offset.
    let mid_offset = (bytes / 2) as u32;
    let t = Instant::now();
    let chain = l.enclosing(mid_offset);
    let enclosing = t.elapsed();
    let _ = chain;

    Sample {
        lines,
        bytes,
        open,
        mid_edit,
        mid_splice_lines,
        eof_edit,
        marks: marks_time,
        mark_count,
        enclosing,
    }
}

#[test]
fn huge_c_documents_stay_live() {
    let t = Instant::now();
    let lang = Arc::new(EmbeddedLang::from_qana_source(C_QANA).expect("c.qana certifies"));
    let certify = t.elapsed();
    println!("[c-scale] certify c.qana: {certify:?}");

    for target in [10_000usize, 50_000, 200_000] {
        let s = measure(&lang, target);
        println!(
            "[c-scale] {} lines ({} KB): open+paint {:?} | mid-edit {:?} (splice {} lines) | eof-edit {:?} | marks {:?} (n={}) | enclosing {:?}",
            s.lines,
            s.bytes / 1024,
            s.open,
            s.mid_edit,
            s.mid_splice_lines,
            s.eof_edit,
            s.marks,
            s.mark_count,
            s.enclosing,
        );

        // Sanity ceilings only — generous enough for debug builds on
        // slow machines; the printed numbers are the real product.
        assert!(s.open < Duration::from_secs(120), "cold open runaway");
        assert!(
            s.mid_edit < Duration::from_secs(5),
            "a keystroke must never cost seconds ({:?} at {} lines)",
            s.mid_edit,
            s.lines
        );
        assert!(s.eof_edit < Duration::from_secs(5), "EOF keystroke runaway");
        assert!(s.mark_count > 0, "the outline exists at scale");
    }
}
