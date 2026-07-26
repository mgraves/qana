//! The LSP server core: a pure `handle(message) -> outgoing messages`
//! state machine over the incremental sessions — fully testable without
//! stdio. Transport lives in main.rs.
//!
//! Two languages are served: the TARGET language (`.cl` documents,
//! pipeline hot-reloaded from `chartlang.qana` — or legacy
//! `chartlang.toml`), and the `.qana` grammar surface ITSELF (`.qana`
//! documents get qana-powered highlighting, outline, navigation, and
//! live envelope diagnostics — the dogfood loop closed).

use crate::config::{
    build_pipeline, build_pipeline_qana, parse_config, qana_service_pipeline, LangConfig, Pipeline,
};
use qana_engine::{split_lines, IncSession, Line, LineEdit};
use qana_grammar::green::ancestor_spans;
use qana_lang::compile::{certify, compile, QanaDiag};
use qana_lang::QanaToolchain;
use qana_sem::SemDb;
use qana_services::{
    completion_at, diagnostics, folding_ranges, outline, semantic_tokens_full, FoldKind,
    SemanticTokens,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::SystemTime;

#[derive(Clone, Copy, PartialEq, Eq)]
enum DocLang {
    /// The hot-reloadable target language.
    Target,
    /// A `.qana` grammar file.
    Qana,
}

struct Doc {
    lang: DocLang,
    session: IncSession<'static>,
    cache: SemanticTokens,
    /// Snapshot behind the last published resultId (delta anchor).
    published: Option<(String, Vec<u32>)>,
    result_counter: u64,
}

pub struct Server {
    pub pipeline: Pipeline,
    qana_pipe: Pipeline,
    tc: &'static QanaToolchain,
    docs: HashMap<String, Doc>,
    sem: SemDb,
    qana_sem: SemDb,
    pub root: Option<PathBuf>,
    config_mtime: Option<SystemTime>,
    next_server_req: i64,
}

impl Server {
    pub fn new() -> Self {
        let pipeline = build_pipeline(&LangConfig::default()).expect("default config builds");
        let tc: &'static QanaToolchain = Box::leak(Box::new(QanaToolchain::new()));
        let qana_pipe = qana_service_pipeline(tc);
        let mut sem = SemDb::new(pipeline.binding.clone());
        sem.set_types(pipeline.types.clone());
        let qana_sem = SemDb::new(qana_pipe.binding.clone());
        Server {
            pipeline,
            qana_pipe,
            tc,
            sem,
            qana_sem,
            docs: HashMap::new(),
            root: None,
            config_mtime: None,
            next_server_req: 1_000_000,
        }
    }

    /// The language-definition file. `chartlang.qana` wins if present (the
    /// demo and playground rely on it); otherwise ANY single `.qana` file
    /// in the workspace root is the language — so a folder holding your
    /// own `mylang.qana` is served without renaming anything. Several
    /// root `.qana` files are ambiguous, so the lowest name wins and the
    /// choice is deterministic rather than arbitrary.
    fn config_path(&self) -> Option<PathBuf> {
        let root = self.root.as_ref()?;
        let named = root.join("chartlang.qana");
        if named.exists() {
            return Some(named);
        }
        let mut found: Vec<PathBuf> = std::fs::read_dir(root)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "qana") && p.is_file())
            .collect();
        found.sort();
        found.into_iter().next().or_else(|| Some(root.join("chartlang.toml")))
    }

    fn pipe_of(&self, lang: DocLang) -> &Pipeline {
        match lang {
            DocLang::Target => &self.pipeline,
            DocLang::Qana => &self.qana_pipe,
        }
    }

    fn doc_lang(&self, uri: &str) -> DocLang {
        self.docs.get(uri).map(|d| d.lang).unwrap_or(DocLang::Target)
    }

    fn sem_of(&mut self, lang: DocLang) -> &mut SemDb {
        match lang {
            DocLang::Target => &mut self.sem,
            DocLang::Qana => &mut self.qana_sem,
        }
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
                    "selectionRangeProvider": true,
                    "definitionProvider": true,
                    "referencesProvider": true,
                    "renameProvider": true
                });
                if utf8_ok {
                    caps["positionEncoding"] = json!("utf-8");
                }
                vec![resp(id, json!({"capabilities": caps, "serverInfo": {"name": "qana-lsp"}}))]
            }
            "initialized" => self.check_reload(),
            "shutdown" => vec![resp(id, Value::Null)],
            "textDocument/didOpen" => {
                let uri = str_at(&params, "/textDocument/uri");
                let text = str_at(&params, "/textDocument/text");
                let lang_id = str_at(&params, "/textDocument/languageId");
                let lang = if lang_id == "qana-grammar" || uri.ends_with(".qana") {
                    DocLang::Qana
                } else {
                    DocLang::Target
                };
                let mut out = self.open_doc(&uri, &text, lang);
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
                            let lang = self.doc_lang(&uri);
                            out.extend(self.open_doc(&uri, &text, lang));
                            continue;
                        }
                        Some(range) => {
                            let lang = self.doc_lang(&uri);
                            let pipe = match lang {
                                DocLang::Target => &self.pipeline,
                                DocLang::Qana => &self.qana_pipe,
                            };
                            if let Some(doc) = self.docs.get_mut(&uri) {
                                let edit = range_to_line_edit(doc, range, &text);
                                if let Ok(outcome) =
                                    doc.session.edit(pipe.sg, pipe.tables, &[edit])
                                {
                                    doc.cache.update(
                                        pipe.lexer,
                                        &doc.session.buf,
                                        &pipe.styles,
                                        &outcome.damage,
                                    );
                                }
                                if let Some(tree) = doc.session.tree() {
                                    let tree = tree.clone();
                                    self.sem_of(lang).set_tree(&uri, tree);
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
                let pipe = self.pipe_of(doc.lang);
                let folds: Vec<Value> = folding_ranges(pipe.lexer, &doc.session.buf)
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
                let pipe = self.pipe_of(doc.lang);
                let syms: Vec<Value> = outline(tree, &pipe.outline_cfg)
                    .into_iter()
                    .map(|s| {
                        json!({
                            "name": s.name,
                            "kind": symbol_kind(s.kind),
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
                let lang = doc.lang;
                let off = pos_to_offset(doc, params.pointer("/position").unwrap_or(&Value::Null));
                // Binding-aware names first (innermost scopes, then other
                // files' exports), then the grammar's expected tokens.
                let scoped = self.sem_of(lang).names_in_scope(&uri, off);
                let mut seen: std::collections::HashSet<String> = Default::default();
                let mut items: Vec<Value> = Vec::new();
                for (i, name) in scoped.into_iter().enumerate() {
                    if seen.insert(name.clone()) {
                        items.push(json!({
                            "label": name,
                            "kind": 6, // Variable
                            "sortText": format!("0_{i:04}")
                        }));
                    }
                }
                let doc = self.docs.get(&uri).unwrap();
                let pipe = self.pipe_of(lang);
                for i in completion_at(pipe.lexer, &doc.session.buf, pipe.sg, pipe.tables, off) {
                    if seen.insert(i.label.clone()) {
                        items.push(json!({
                            "label": i.label,
                            "kind": if i.is_keyword { 14 } else { 1 },
                            "sortText": format!("1_{}", i.label)
                        }));
                    }
                }
                vec![resp(id, json!(items))]
            }
            "textDocument/definition" => {
                let uri = str_at(&params, "/textDocument/uri");
                let Some(doc) = self.docs.get(&uri) else { return vec![resp(id, Value::Null)] };
                let lang = doc.lang;
                let off = pos_to_offset(doc, params.pointer("/position").unwrap_or(&Value::Null));
                match self.sem_of(lang).definition(&uri, off) {
                    Some((target_uri, span)) => {
                        let range = self.range_in(&target_uri, span);
                        vec![resp(id, json!({"uri": target_uri, "range": range}))]
                    }
                    None => vec![resp(id, Value::Null)],
                }
            }
            "textDocument/references" => {
                let uri = str_at(&params, "/textDocument/uri");
                let Some(doc) = self.docs.get(&uri) else { return vec![resp(id, json!([]))] };
                let lang = doc.lang;
                let off = pos_to_offset(doc, params.pointer("/position").unwrap_or(&Value::Null));
                let include_decl = params
                    .pointer("/context/includeDeclaration")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                match self.sem_of(lang).references(&uri, off) {
                    Some((refs, decl)) => {
                        let mut locs: Vec<Value> = Vec::new();
                        if include_decl {
                            locs.push(json!({"uri": decl.0, "range": self.range_in(&decl.0, decl.1)}));
                        }
                        for (u, span) in refs {
                            locs.push(json!({"uri": u, "range": self.range_in(&u, span)}));
                        }
                        vec![resp(id, json!(locs))]
                    }
                    None => vec![resp(id, json!([]))],
                }
            }
            "textDocument/rename" => {
                let uri = str_at(&params, "/textDocument/uri");
                let new_name = str_at(&params, "/newName");
                let Some(doc) = self.docs.get(&uri) else { return vec![resp(id, Value::Null)] };
                let lang = doc.lang;
                let off = pos_to_offset(doc, params.pointer("/position").unwrap_or(&Value::Null));
                match self.sem_of(lang).rename_edits(&uri, off) {
                    Some(per_uri) => {
                        let mut changes = serde_json::Map::new();
                        for (u, spans) in per_uri {
                            let edits: Vec<Value> = spans
                                .into_iter()
                                .map(|span| {
                                    json!({"range": self.range_in(&u, span), "newText": new_name})
                                })
                                .collect();
                            changes.insert(u, json!(edits));
                        }
                        vec![resp(id, json!({"changes": changes}))]
                    }
                    None => vec![resp(id, Value::Null)],
                }
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

    fn open_doc(&mut self, uri: &str, text: &str, lang: DocLang) -> Vec<Value> {
        let pipe = self.pipe_of(lang);
        let session = IncSession::new(pipe.lexer, pipe.sg, pipe.tables, text)
            .expect("total parsing");
        let cache = semantic_tokens_full(pipe.lexer, &session.buf, &pipe.styles);
        let tree = session.tree().expect("total").clone();
        self.sem_of(lang).set_tree(uri, tree);
        self.docs.insert(
            uri.to_string(),
            Doc { lang, session, cache, published: None, result_counter: 0 },
        );
        self.publish_diagnostics(uri)
    }

    fn range_in(&self, uri: &str, span: (u32, u32)) -> Value {
        match self.docs.get(uri) {
            Some(doc) => range_json(doc, span),
            None => json!({"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}}),
        }
    }

    fn publish_diagnostics(&mut self, uri: &str) -> Vec<Value> {
        let Some(doc) = self.docs.get(uri) else { return Vec::new() };
        let lang = doc.lang;
        let pipe = self.pipe_of(lang);
        let mut diags: Vec<Value> = diagnostics(
            pipe.lexer,
            &doc.session.buf,
            pipe.sg,
            &doc.session.last_repairs,
        )
        .into_iter()
        .map(|d| {
            json!({"range": range_json(doc, d.span), "severity": 1, "message": d.message})
        })
        .collect();
        match lang {
            DocLang::Target => {
                // Semantic layer: unresolved variable reads (warnings)
                // and declared-type-tier mismatches (errors).
                let unresolved = self.sem.unresolved(uri);
                let not_exported = self.sem.not_exported(uri);
                let qualified = self.sem.qualified_errors(uri);
                let type_diags = self.sem.types(uri).diags;
                let doc = self.docs.get(uri).unwrap();
                for (name, span) in unresolved {
                    diags.push(json!({
                        "range": range_json(doc, span),
                        "severity": 2,
                        "message": format!("cannot find `{name}`")
                    }));
                }
                for (name, span) in not_exported {
                    diags.push(json!({
                        "range": range_json(doc, span),
                        "severity": 1,
                        "message": format!("`{name}` exists but is not exported by its file")
                    }));
                }
                for (msg, span) in qualified {
                    diags.push(json!({
                        "range": range_json(doc, span),
                        "severity": 1,
                        "message": msg
                    }));
                }
                for d in type_diags {
                    diags.push(json!({
                        "range": range_json(doc, d.span),
                        "severity": 1,
                        "message": d.msg
                    }));
                }
            }
            DocLang::Qana => {
                // Live grammar authoring: compile the CURRENT tree and,
                // when it compiles, run the envelope. Refusals point at
                // the offending construct while you type.
                if doc.session.last_repairs.is_empty() {
                    if let Some(tree) = doc.session.tree() {
                        let (def, cdiags) = compile(tree, &self.tc.prods);
                        let mut all = cdiags;
                        if all.is_empty() {
                            if let Err(e) = certify(&def) {
                                all = e;
                            }
                        }
                        for d in all {
                            diags.push(json!({
                                "range": range_json(doc, d.span),
                                "severity": d.severity,
                                "message": d.msg
                            }));
                        }
                    }
                }
            }
        }
        vec![notif(
            "textDocument/publishDiagnostics",
            json!({"uri": uri, "diagnostics": diags}),
        )]
    }

    /// The hot-reload heartbeat: if the language definition changed,
    /// rebuild the WHOLE pipeline. Bad definitions are refused with the
    /// tool's own counterexamples as diagnostics on the definition file
    /// (span-accurate for `.qana`); good ones rebuild every open target
    /// document and ask the client to re-request semantic tokens.
    pub fn check_reload(&mut self) -> Vec<Value> {
        let Some(path) = self.config_path() else { return Vec::new() };
        let Ok(meta) = std::fs::metadata(&path) else { return Vec::new() };
        let mtime = meta.modified().ok();
        if mtime == self.config_mtime {
            return Vec::new();
        }
        self.config_mtime = mtime;
        let config_uri = format!("file://{}", path.display());
        let config_open = self.docs.contains_key(&config_uri);
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let is_qana = path.extension().is_some_and(|e| e == "qana");
        let built: Result<Pipeline, Vec<QanaDiag>> = if is_qana {
            build_pipeline_qana(self.tc, &text)
        } else {
            parse_config(&text)
                .and_then(|cfg| build_pipeline(&cfg).map_err(|e| e))
                .map_err(|msg| vec![QanaDiag { span: (0, 1), msg, severity: 1 }])
        };
        match built {
            Err(diags) => {
                if config_open {
                    // The open-document path owns diagnostics for it.
                    return Vec::new();
                }
                let list: Vec<Value> = diags
                    .into_iter()
                    .map(|d| {
                        json!({
                            "range": range_in_text(&text, d.span),
                            "severity": d.severity,
                            "message": d.msg
                        })
                    })
                    .collect();
                vec![notif(
                    "textDocument/publishDiagnostics",
                    json!({"uri": config_uri, "diagnostics": list}),
                )]
            }
            Ok(pipeline) => {
                self.pipeline = pipeline;
                // New grammar ⇒ new binding + type configs; trees
                // repopulate below.
                self.sem = SemDb::new(self.pipeline.binding.clone());
                self.sem.set_types(self.pipeline.types.clone());
                let uris: Vec<(String, DocLang)> =
                    self.docs.iter().map(|(u, d)| (u.clone(), d.lang)).collect();
                let mut out = Vec::new();
                for (uri, lang) in uris {
                    if lang == DocLang::Target {
                        let text = self.docs[&uri].session.buf.reproduce();
                        out.extend(self.open_doc(&uri, &text, lang));
                    }
                }
                if !config_open {
                    out.push(notif(
                        "textDocument/publishDiagnostics",
                        json!({"uri": config_uri, "diagnostics": []}),
                    ));
                }
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

/// LSP SymbolKind numbers for the outline kinds grammars may declare.
fn symbol_kind(kind: &str) -> u32 {
    match kind {
        "function" => 12,
        "struct" => 23,
        "module" => 2,
        "constant" => 14,
        "class" => 5,
        _ => 13, // variable
    }
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

/// Byte span → LSP range within a plain text (for files not open as
/// documents, e.g. the config file during reload).
fn range_in_text(text: &str, span: (u32, u32)) -> Value {
    let pos = |off: u32| {
        let off = (off as usize).min(text.len());
        let mut line = 0usize;
        let mut col = 0usize;
        for (i, b) in text.bytes().enumerate() {
            if i == off {
                break;
            }
            if b == b'\n' {
                line += 1;
                col = 0;
            } else {
                col += 1;
            }
        }
        json!({"line": line, "character": col})
    };
    json!({"start": pos(span.0), "end": pos(span.1)})
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
