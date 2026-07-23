//! The LSP server core: a pure `handle(message) -> outgoing messages`
//! state machine over the incremental sessions — fully testable without
//! stdio. Transport lives in main.rs.

use crate::config::{build_pipeline, parse_config, LangConfig, Pipeline};
use rantlr_engine::{split_lines, IncSession, Line, LineEdit};
use rantlr_grammar::green::ancestor_spans;
use rantlr_services::{
    completion_at, diagnostics, folding_ranges, outline, semantic_tokens_full, FoldKind,
    SemanticTokens,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::SystemTime;

struct Doc {
    session: IncSession<'static>,
    cache: SemanticTokens,
    /// Snapshot behind the last published resultId (delta anchor).
    published: Option<(String, Vec<u32>)>,
    result_counter: u64,
}

pub struct Server {
    pub pipeline: Pipeline,
    docs: HashMap<String, Doc>,
    pub root: Option<PathBuf>,
    config_mtime: Option<SystemTime>,
    next_server_req: i64,
}

impl Server {
    pub fn new() -> Self {
        Server {
            pipeline: build_pipeline(&LangConfig::default()).expect("default config builds"),
            docs: HashMap::new(),
            root: None,
            config_mtime: None,
            next_server_req: 1_000_000,
        }
    }

    fn config_path(&self) -> Option<PathBuf> {
        self.root.as_ref().map(|r| r.join("chartlang.toml"))
    }

    /// Handle one incoming JSON-RPC message; returns outgoing messages
    /// (responses, notifications, and server→client requests).
    pub fn handle(&mut self, msg: &Value) -> Vec<Value> {
        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let id = msg.get("id").cloned();
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        match method {
            "initialize" => {
                if let Some(uri) = params
                    .pointer("/rootUri")
                    .and_then(|u| u.as_str())
                    .and_then(|u| u.strip_prefix("file://"))
                {
                    self.root = Some(PathBuf::from(uri));
                }
                let utf8_ok = params
                    .pointer("/capabilities/general/positionEncodings")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().any(|e| e.as_str() == Some("utf-8")))
                    .unwrap_or(false);
                let mut caps = json!({
                    "textDocumentSync": {"openClose": true, "change": 2},
                    "semanticTokensProvider": {
                        "legend": {"tokenTypes": self.pipeline.styles.legend, "tokenModifiers": []},
                        "full": {"delta": true}
                    },
                    "foldingRangeProvider": true,
                    "documentSymbolProvider": true,
                    "completionProvider": {},
                    "selectionRangeProvider": true
                });
                if utf8_ok {
                    caps["positionEncoding"] = json!("utf-8");
                }
                vec![resp(id, json!({"capabilities": caps, "serverInfo": {"name": "rantlr-lsp"}}))]
            }
            "initialized" => self.check_reload(),
            "shutdown" => vec![resp(id, Value::Null)],
            "textDocument/didOpen" => {
                let uri = str_at(&params, "/textDocument/uri");
                let text = str_at(&params, "/textDocument/text");
                let mut out = self.open_doc(&uri, &text);
                out.extend(self.check_reload());
                out
            }
            "textDocument/didChange" => {
                let uri = str_at(&params, "/textDocument/uri");
                let changes = params
                    .pointer("/contentChanges")
                    .and_then(|c| c.as_array())
                    .cloned()
                    .unwrap_or_default();
                let mut out = Vec::new();
                for ch in &changes {
                    let text = str_at(ch, "/text");
                    match ch.get("range") {
                        None => {
                            // Full-content sync fallback.
                            out.extend(self.open_doc(&uri, &text));
                            continue;
                        }
                        Some(range) => {
                            if let Some(doc) = self.docs.get_mut(&uri) {
                                let edit = range_to_line_edit(doc, range, &text);
                                if let Ok(outcome) = doc.session.edit(
                                    self.pipeline.sg,
                                    self.pipeline.tables,
                                    &[edit],
                                ) {
                                    doc.cache.update(
                                        self.pipeline.lexer,
                                        &doc.session.buf,
                                        &self.pipeline.styles,
                                        &outcome.damage,
                                    );
                                }
                            }
                        }
                    }
                }
                out.extend(self.publish_diagnostics(&uri));
                out.extend(self.check_reload());
                out
            }
            "textDocument/semanticTokens/full" => {
                let uri = str_at(&params, "/textDocument/uri");
                let Some(doc) = self.docs.get_mut(&uri) else { return vec![resp(id, Value::Null)] };
                doc.result_counter += 1;
                let rid = doc.result_counter.to_string();
                let data = doc.cache.data.clone();
                doc.published = Some((rid.clone(), data.clone()));
                vec![resp(id, json!({"resultId": rid, "data": data}))]
            }
            "textDocument/semanticTokens/full/delta" => {
                let uri = str_at(&params, "/textDocument/uri");
                let prev_id = str_at(&params, "/previousResultId");
                let Some(doc) = self.docs.get_mut(&uri) else { return vec![resp(id, Value::Null)] };
                doc.result_counter += 1;
                let rid = doc.result_counter.to_string();
                let data = doc.cache.data.clone();
                let result = match &doc.published {
                    Some((stored, old)) if *stored == prev_id => {
                        let (start, del, insert) = splice_diff(old, &data);
                        json!({"resultId": rid, "edits": [
                            {"start": start, "deleteCount": del, "data": insert}
                        ]})
                    }
                    _ => json!({"resultId": rid, "data": data}),
                };
                doc.published = Some((rid, data));
                vec![resp(id, result)]
            }
            "textDocument/foldingRange" => {
                let uri = str_at(&params, "/textDocument/uri");
                let Some(doc) = self.docs.get(&uri) else { return vec![resp(id, json!([]))] };
                let folds: Vec<Value> = folding_ranges(self.pipeline.lexer, &doc.session.buf)
                    .into_iter()
                    .map(|f| {
                        json!({
                            "startLine": f.start_line,
                            "endLine": f.end_line,
                            "kind": match f.kind { FoldKind::Block => "region", FoldKind::Comment => "comment" }
                        })
                    })
                    .collect();
                vec![resp(id, json!(folds))]
            }
            "textDocument/documentSymbol" => {
                let uri = str_at(&params, "/textDocument/uri");
                let Some(doc) = self.docs.get(&uri) else { return vec![resp(id, json!([]))] };
                let Some(tree) = doc.session.tree() else { return vec![resp(id, json!([]))] };
                let syms: Vec<Value> = outline(tree, &self.pipeline.outline_cfg)
                    .into_iter()
                    .map(|s| {
                        json!({
                            "name": s.name,
                            "kind": 13, // Variable
                            "range": range_json(doc, s.span),
                            "selectionRange": range_json(doc, s.selection)
                        })
                    })
                    .collect();
                vec![resp(id, json!(syms))]
            }
            "textDocument/completion" => {
                let uri = str_at(&params, "/textDocument/uri");
                let Some(doc) = self.docs.get(&uri) else { return vec![resp(id, json!([]))] };
                let off = pos_to_offset(doc, params.pointer("/position").unwrap_or(&Value::Null));
                let items: Vec<Value> = completion_at(
                    self.pipeline.lexer,
                    &doc.session.buf,
                    self.pipeline.sg,
                    self.pipeline.tables,
                    off,
                )
                .into_iter()
                .map(|i| {
                    json!({
                        "label": i.label,
                        "kind": if i.is_keyword { 14 } else { 1 }
                    })
                })
                .collect();
                vec![resp(id, json!(items))]
            }
            "textDocument/selectionRange" => {
                let uri = str_at(&params, "/textDocument/uri");
                let Some(doc) = self.docs.get(&uri) else { return vec![resp(id, json!([]))] };
                let positions = params
                    .pointer("/positions")
                    .and_then(|p| p.as_array())
                    .cloned()
                    .unwrap_or_default();
                let result: Vec<Value> = positions
                    .iter()
                    .map(|p| {
                        let off = pos_to_offset(doc, p);
                        let spans = doc
                            .session
                            .tree()
                            .map(|t| ancestor_spans(t, off))
                            .unwrap_or_default();
                        // Innermost first, each wrapping via "parent".
                        let mut node = Value::Null;
                        for span in spans {
                            let mut v = json!({"range": range_json(doc, span)});
                            if !node.is_null() {
                                // previous (wider) becomes parent
                            }
                            v["parent"] = node;
                            node = v;
                        }
                        node
                    })
                    .collect();
                vec![resp(id, json!(result))]
            }
            _ if id.is_some() => vec![resp(id, Value::Null)],
            _ => Vec::new(),
        }
    }

    fn open_doc(&mut self, uri: &str, text: &str) -> Vec<Value> {
        let session =
            IncSession::new(self.pipeline.lexer, self.pipeline.sg, self.pipeline.tables, text)
                .expect("total parsing");
        let cache = semantic_tokens_full(self.pipeline.lexer, &session.buf, &self.pipeline.styles);
        self.docs.insert(
            uri.to_string(),
            Doc { session, cache, published: None, result_counter: 0 },
        );
        self.publish_diagnostics(uri)
    }

    fn publish_diagnostics(&self, uri: &str) -> Vec<Value> {
        let Some(doc) = self.docs.get(uri) else { return Vec::new() };
        let diags: Vec<Value> = diagnostics(
            self.pipeline.lexer,
            &doc.session.buf,
            self.pipeline.sg,
            &doc.session.last_repairs,
        )
        .into_iter()
        .map(|d| {
            json!({"range": range_json(doc, d.span), "severity": 1, "message": d.message})
        })
        .collect();
        vec![notif(
            "textDocument/publishDiagnostics",
            json!({"uri": uri, "diagnostics": diags}),
        )]
    }

    /// The hot-reload heartbeat: if chartlang.toml changed, rebuild the
    /// WHOLE pipeline. Bad configs are refused with the tool's own
    /// counterexamples as diagnostics on the config file; good ones
    /// rebuild every open document and ask the client to re-request
    /// semantic tokens.
    pub fn check_reload(&mut self) -> Vec<Value> {
        let Some(path) = self.config_path() else { return Vec::new() };
        let Ok(meta) = std::fs::metadata(&path) else { return Vec::new() };
        let mtime = meta.modified().ok();
        if mtime == self.config_mtime {
            return Vec::new();
        }
        self.config_mtime = mtime;
        let config_uri = format!("file://{}", path.display());
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        match parse_config(&text).and_then(|cfg| build_pipeline(&cfg)) {
            Err(msg) => {
                vec![notif(
                    "textDocument/publishDiagnostics",
                    json!({"uri": config_uri, "diagnostics": [{
                        "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}},
                        "severity": 1,
                        "message": msg
                    }]}),
                )]
            }
            Ok(pipeline) => {
                self.pipeline = pipeline;
                let uris: Vec<String> = self.docs.keys().cloned().collect();
                let mut out = Vec::new();
                for uri in uris {
                    let text = self.docs[&uri].session.buf.reproduce();
                    out.extend(self.open_doc(&uri, &text));
                }
                out.push(notif(
                    "textDocument/publishDiagnostics",
                    json!({"uri": config_uri, "diagnostics": []}),
                ));
                self.next_server_req += 1;
                out.push(json!({
                    "jsonrpc": "2.0",
                    "id": self.next_server_req,
                    "method": "workspace/semanticTokens/refresh",
                    "params": null
                }));
                out
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn resp(id: Option<Value>, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id.unwrap_or(Value::Null), "result": result})
}

fn notif(method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "method": method, "params": params})
}

fn str_at(v: &Value, ptr: &str) -> String {
    v.pointer(ptr).and_then(|s| s.as_str()).unwrap_or("").to_string()
}

fn line_byte_start(doc: &Doc, line: usize) -> u32 {
    let mut pos = 0u32;
    for l in doc.session.buf.lines.iter().take(line) {
        pos += l.text.len() as u32 + l.term.as_str().len() as u32;
    }
    pos
}

fn pos_to_offset(doc: &Doc, pos: &Value) -> u32 {
    let line = pos.pointer("/line").and_then(|l| l.as_u64()).unwrap_or(0) as usize;
    let ch = pos.pointer("/character").and_then(|c| c.as_u64()).unwrap_or(0) as u32;
    let line = line.min(doc.session.buf.lines.len().saturating_sub(1));
    let max = doc.session.buf.lines[line].text.len() as u32;
    line_byte_start(doc, line) + ch.min(max)
}

fn offset_to_pos(doc: &Doc, offset: u32) -> Value {
    let mut pos = 0u32;
    for (li, l) in doc.session.buf.lines.iter().enumerate() {
        let end = pos + l.text.len() as u32 + l.term.as_str().len() as u32;
        if offset < end || li + 1 == doc.session.buf.lines.len() {
            let ch = offset.saturating_sub(pos).min(l.text.len() as u32);
            return json!({"line": li, "character": ch});
        }
        pos = end;
    }
    json!({"line": 0, "character": 0})
}

fn range_json(doc: &Doc, span: (u32, u32)) -> Value {
    json!({"start": offset_to_pos(doc, span.0), "end": offset_to_pos(doc, span.1)})
}

/// Map an LSP range+text change to a whole-line edit: replace the touched
/// lines with `prefix + text + suffix`, preserving the final line's
/// original terminator.
fn range_to_line_edit(doc: &Doc, range: &Value, text: &str) -> LineEdit {
    let n = doc.session.buf.lines.len();
    let sl = (range.pointer("/start/line").and_then(|v| v.as_u64()).unwrap_or(0) as usize)
        .min(n - 1);
    let el =
        (range.pointer("/end/line").and_then(|v| v.as_u64()).unwrap_or(0) as usize).min(n - 1);
    let sc = range.pointer("/start/character").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    let ec = range.pointer("/end/character").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    let start_text = &doc.session.buf.lines[sl].text;
    let end_line = &doc.session.buf.lines[el];
    let sc = sc.min(start_text.len());
    let ec = ec.min(end_line.text.len());
    let fragment = format!("{}{}{}", &start_text[..sc], text, &end_line.text[ec..]);
    let mut replacement: Vec<Line> = split_lines(&fragment);
    if let Some(last) = replacement.last_mut() {
        last.term = end_line.term;
    }
    LineEdit { start: sl, end: el + 1, replacement }
}

/// Minimal single-splice diff between two flat token arrays, aligned to
/// quintuple boundaries.
fn splice_diff(old: &[u32], new: &[u32]) -> (usize, usize, Vec<u32>) {
    let mut p = 0usize;
    while p < old.len() && p < new.len() && old[p] == new[p] {
        p += 1;
    }
    p -= p % 5;
    let mut s = 0usize;
    while s < old.len() - p && s < new.len() - p && old[old.len() - 1 - s] == new[new.len() - 1 - s]
    {
        s += 1;
    }
    s -= s % 5;
    (p, old.len() - p - s, new[p..new.len() - s].to_vec())
}
