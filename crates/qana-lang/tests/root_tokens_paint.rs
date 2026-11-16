//! A grammar may put tokens of its root production ahead of its list of
//! items: `module main;` and then the functions. Those tokens are not
//! items. Edit them, and every item moves without any item changing.
//!
//! The painter keeps each item's marks across edits, with where the item
//! starts. It refreshed the start of the items AFTER the ones an edit
//! rebuilt, and took the ones before for granted. Here no item is
//! rebuilt at all, so all of them were taken for granted, and their
//! semantic marks were painted where the items used to be.
//!
//! The oracle is the usual one: after every edit the incremental frame
//! equals a fresh open of the same text.

use linework::{LineEdit, Limner, PaintDelta, Run};
use qana_lang::live::LiveDoc;
use qana_lang::EmbeddedLang;
use std::sync::Arc;

const MODULE_QANA: &str = r#"
language Mod

token WS = /\s+/ @trivia
token IDENT = /[\a_][\w_]*/ @specialize @style(variable)
token LBRACE = "{" @style(bracket)
token RBRACE = "}" @style(bracket)
token SEMI = ";" @style(punctuation)

keywords IDENT = module fn

pair LBRACE RBRACE

start file
rule file = File: "module" name:IDENT ";" items @outline(name, module)
rule items = item*
rule item = Fn: "fn" name:IDENT "{" calls "}" @def(name) @outline(name, function)
rule calls = call*
rule call = Call: name:IDENT ";" @ref(name)
"#;

fn apply_delta(frame: &mut Vec<Vec<Run>>, delta: &PaintDelta) {
    if let Some(((lo, hi), repl)) = &delta.splice {
        frame.splice(*lo as usize..*hi as usize, repl.iter().cloned());
    }
    for (li, runs) in &delta.repaints {
        frame[*li as usize] = runs.clone();
    }
}

fn apply(text: &str, edit: &LineEdit) -> String {
    let mut lines: Vec<&str> = text.lines().collect();
    let repl: Vec<&str> = edit.lines.iter().map(|s| s.as_str()).collect();
    let end = (edit.end as usize).min(lines.len());
    lines.splice(edit.start as usize..end, repl);
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

#[test]
fn items_moved_by_an_edit_ahead_of_them_are_painted_where_they_are() {
    let lang = Arc::new(EmbeddedLang::from_qana_source(MODULE_QANA).expect("the grammar certifies"));

    // `beta` calls `alpha` (defined above it: resolves) and `nowhere`
    // (unresolved), so its line carries semantic marks whose columns
    // are the thing that goes stale.
    let mut text = String::from(
        "module main;\nfn alpha { }\nfn beta { alpha; nowhere; }\nfn gamma { beta; alpha; } fn delta { gamma; missing; }\n",
    );
    let mut live: Box<dyn Limner> = Box::new(LiveDoc::open(lang.clone(), "root.mod", &text));
    let mut frame = live.open(&text).lines;

    let script = [
        // A blank line above everything: every item starts one byte
        // later, and no item is a new subtree. (The blank is trivia of
        // the `module` token, which belongs to the root, not to an item.)
        LineEdit { start: 0, end: 0, lines: vec!["".into()] },
        // Now type in `delta`, which shares its line with `gamma`. The
        // line is repainted from both items' marks, and `gamma` is one
        // of the items that moved: painted from where it used to start,
        // its marks land a column early.
        LineEdit {
            start: 4,
            end: 5,
            lines: vec!["fn gamma { beta; alpha; } fn delta { gamma; missing; alpha; }".into()],
        },
        // Two more blank lines, then one taken away again.
        LineEdit { start: 0, end: 0, lines: vec!["".into(), "".into()] },
        LineEdit { start: 1, end: 2, lines: vec![] },
        // A longer module name.
        LineEdit { start: 2, end: 3, lines: vec!["module renamed;".into()] },
        // Back to the top of the file for the rest of the script.
        LineEdit { start: 0, end: 2, lines: vec![] },
        // A shorter one.
        LineEdit { start: 0, end: 1, lines: vec!["module m;".into()] },
        // The header and the first function on one line: the items move
        // within their lines.
        LineEdit { start: 0, end: 2, lines: vec!["module m; fn alpha { }".into()] },
        // Then an ordinary body edit, with the starts settled.
        LineEdit { start: 1, end: 2, lines: vec!["fn beta { alpha; nowhere; alpha; }".into()] },
        // And the header moved again, after an item was rebuilt.
        LineEdit { start: 0, end: 1, lines: vec!["module again; fn alpha { }".into()] },
        // The shared line once more, with everything ahead of it moved.
        LineEdit {
            start: 2,
            end: 3,
            lines: vec!["fn gamma { beta; alpha; } fn delta { gamma; }".into()],
        },
    ];
    for (step, edit) in script.iter().enumerate() {
        let delta = live.edit(edit);
        apply_delta(&mut frame, &delta);
        text = apply(&text, edit);
        assert_eq!(live.text(), text, "step {step}: mirror lost the document");

        let mut oracle: Box<dyn Limner> = Box::new(LiveDoc::open(lang.clone(), "oracle.mod", &text));
        let fresh = oracle.open(&text);
        assert_eq!(frame.len(), fresh.lines.len(), "step {step}: line counts diverge");
        for (li, (a, b)) in frame.iter().zip(fresh.lines.iter()).enumerate() {
            assert_eq!(
                a,
                b,
                "step {step} ({edit:?}): line {li} `{}` runs diverge",
                text.lines().nth(li).unwrap_or("")
            );
        }
    }
}
