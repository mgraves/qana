//! Protocol-level gates for the LSP server core: the semantic-token
//! delta contract at the wire level (client applying edits must arrive
//! at the fresh full data), diagnostics publication, and the hot-reload
//! loop — including the envelope refusing a bad grammar config with a
//! conflict counterexample as a config-file diagnostic.

use rantlr_lsp::server::Server;
use serde_json::{json, Value};

const URI: &str = "file:///demo.cl";

fn init(server: &mut Server) {
    server.handle(&json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"capabilities": {"general": {"positionEncodings": ["utf-8", "utf-16"]}}}
    }));
}

fn open(server: &mut Server, text: &str) -> Vec<Value> {
    server.handle(&json!({
        "jsonrpc": "2.0", "method": "textDocument/didOpen",
        "params": {"textDocument": {"uri": URI, "text": text, "version": 1}}
    }))
}

fn change(server: &mut Server, range: Value, text: &str) -> Vec<Value> {
    server.handle(&json!({
        "jsonrpc": "2.0", "method": "textDocument/didChange",
        "params": {
            "textDocument": {"uri": URI, "version": 2},
            "contentChanges": [{"range": range, "text": text}]
        }
    }))
}

fn tokens_full(server: &mut Server) -> (String, Vec<u32>) {
    let out = server.handle(&json!({
        "jsonrpc": "2.0", "id": 10, "method": "textDocument/semanticTokens/full",
        "params": {"textDocument": {"uri": URI}}
    }));
    let r = out[0].pointer("/result").unwrap();
    (
        r["resultId"].as_str().unwrap().to_string(),
        r["data"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap() as u32).collect(),
    )
}

#[test]
fn initialize_negotiates_utf8_and_capabilities() {
    let mut s = Server::new();
    let out = s.handle(&json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"capabilities": {"general": {"positionEncodings": ["utf-8"]}}}
    }));
    let caps = out[0].pointer("/result/capabilities").unwrap();
    assert_eq!(caps["positionEncoding"], "utf-8");
    assert_eq!(caps["semanticTokensProvider"]["full"]["delta"], true);
    assert!(caps["semanticTokensProvider"]["legend"]["tokenTypes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t == "keyword"));
}

#[test]
fn didchange_delta_applied_client_side_equals_full() {
    let mut s = Server::new();
    init(&mut s);
    open(&mut s, "let a = 1;\nlet b = 2;\nlet c = 3;\n");
    let (rid, old_data) = tokens_full(&mut s);

    // Edit line 1: replace `2` with `29 + x` (range covers the literal).
    change(
        &mut s,
        json!({"start": {"line": 1, "character": 8}, "end": {"line": 1, "character": 9}}),
        "29 + x",
    );

    // Delta request against the previous resultId.
    let out = s.handle(&json!({
        "jsonrpc": "2.0", "id": 11, "method": "textDocument/semanticTokens/full/delta",
        "params": {"textDocument": {"uri": URI}, "previousResultId": rid}
    }));
    let result = out[0].pointer("/result").unwrap();
    let edits = result["edits"].as_array().expect("delta edits, not full");

    // Client-side application.
    let mut applied = old_data.clone();
    for e in edits {
        let start = e["start"].as_u64().unwrap() as usize;
        let del = e["deleteCount"].as_u64().unwrap() as usize;
        let data: Vec<u32> =
            e["data"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap() as u32).collect();
        applied.splice(start..start + del, data);
    }
    let (_, fresh) = tokens_full(&mut s);
    assert_eq!(applied, fresh, "client-applied delta must equal fresh full");
    // And the delta is small: the edit touched one line.
    let total: usize = edits
        .iter()
        .map(|e| e["data"].as_array().unwrap().len() + 2)
        .sum();
    assert!(total < fresh.len() / 2, "delta ({total}) should be much smaller than full ({})", fresh.len());
}

#[test]
fn diagnostics_published_for_broken_docs_and_heal() {
    let mut s = Server::new();
    init(&mut s);
    let out = open(&mut s, "let a = ;\n");
    let diags = out
        .iter()
        .find(|m| m["method"] == "textDocument/publishDiagnostics")
        .and_then(|m| m.pointer("/params/diagnostics"))
        .and_then(|d| d.as_array())
        .expect("diagnostics published on open");
    assert!(!diags.is_empty());
    assert!(diags[0]["message"].as_str().unwrap().starts_with("missing"));

    // Heal it; diagnostics must clear.
    let out = change(
        &mut s,
        json!({"start": {"line": 0, "character": 8}, "end": {"line": 0, "character": 8}}),
        "42",
    );
    let diags = out
        .iter()
        .find(|m| m["method"] == "textDocument/publishDiagnostics")
        .and_then(|m| m.pointer("/params/diagnostics"))
        .and_then(|d| d.as_array())
        .expect("diagnostics republished");
    assert!(diags.is_empty(), "healed doc must clear diagnostics: {diags:?}");
}

#[test]
fn folding_symbols_completion_selection_respond() {
    let mut s = Server::new();
    init(&mut s);
    open(&mut s, "if (x) {\n  let deep = 1;\n}\nlet top = 2;\n");

    let folds = s.handle(&json!({
        "jsonrpc": "2.0", "id": 20, "method": "textDocument/foldingRange",
        "params": {"textDocument": {"uri": URI}}
    }));
    assert!(!folds[0]["result"].as_array().unwrap().is_empty());

    let syms = s.handle(&json!({
        "jsonrpc": "2.0", "id": 21, "method": "textDocument/documentSymbol",
        "params": {"textDocument": {"uri": URI}}
    }));
    let names: Vec<&str> = syms[0]["result"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["deep", "top"]);

    let comp = s.handle(&json!({
        "jsonrpc": "2.0", "id": 22, "method": "textDocument/completion",
        "params": {"textDocument": {"uri": URI}, "position": {"line": 3, "character": 0}}
    }));
    let labels: Vec<&str> = comp[0]["result"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["label"].as_str().unwrap())
        .collect();
    assert!(labels.contains(&"let") && labels.contains(&"if"));

    let sel = s.handle(&json!({
        "jsonrpc": "2.0", "id": 23, "method": "textDocument/selectionRange",
        "params": {"textDocument": {"uri": URI}, "positions": [{"line": 1, "character": 6}]}
    }));
    // Innermost range with a chain of parents.
    let first = &sel[0]["result"][0];
    assert!(first["range"].is_object());
    assert!(first["parent"]["range"].is_object());
}

#[test]
fn hot_reload_accepts_good_configs_and_refuses_bad_ones_with_counterexamples() {
    let dir = std::env::temp_dir().join(format!("rantlr-lsp-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = dir.join("chartlang.toml");

    let mut s = Server::new();
    s.root = Some(dir.clone());
    init(&mut s);
    open(&mut s, "let a = 1 + 2 * 3;\nasync work();\n");

    // Baseline: `async` is just an identifier.
    let (_, before) = tokens_full(&mut s);

    // GOOD reload: add `async` as a keyword.
    std::fs::write(
        &cfg,
        "keywords = fn let if else while for return struct enum impl match true false mut pub use async\n\
         prec.left.1 = + -\nprec.left.2 = * /\n",
    )
    .unwrap();
    let out = s.check_reload();
    assert!(
        out.iter().any(|m| m["method"] == "workspace/semanticTokens/refresh"),
        "good reload must request token refresh: {out:?}"
    );
    let (_, after) = tokens_full(&mut s);
    assert_ne!(before, after, "adding a keyword must re-colorize `async`");

    // BAD reload: leave `*` and `/` without precedence — the grammar now
    // has unresolved shift/reduce conflicts. The envelope refuses, with
    // the conflict trace as a diagnostic on the CONFIG file.
    std::fs::write(&cfg, "keywords = let if else\nprec.left.1 = + -\n").unwrap();
    // mtime granularity: force re-check by clearing the stamp via a fresh
    // write far enough apart is flaky — the server rechecks on content
    // hash of mtime; emulate by touching with a distinct time.
    filetime_touch(&cfg);
    let out = s.check_reload();
    let diag = out
        .iter()
        .find(|m| {
            m["method"] == "textDocument/publishDiagnostics"
                && m.pointer("/params/uri").and_then(|u| u.as_str()).is_some_and(|u| u.contains("chartlang.toml"))
                && !m.pointer("/params/diagnostics").unwrap().as_array().unwrap().is_empty()
        })
        .expect("config diagnostic on refusal");
    let msg = diag.pointer("/params/diagnostics/0/message").unwrap().as_str().unwrap();
    assert!(msg.contains("conflict"), "refusal must explain: {msg}");
    assert!(msg.contains("example input"), "refusal carries the counterexample: {msg}");

    // The old (good) pipeline stays live: tokens still served.
    let (_, still) = tokens_full(&mut s);
    assert_eq!(still, after, "refused reload must not change the live pipeline");

    std::fs::remove_dir_all(&dir).ok();
}

/// Bump a file's mtime deterministically (test helper).
fn filetime_touch(path: &std::path::Path) {
    let content = std::fs::read(path).unwrap();
    std::fs::write(path, &content).unwrap();
    // Ensure the mtime differs even on coarse filesystems.
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(path, &content).unwrap();
}
