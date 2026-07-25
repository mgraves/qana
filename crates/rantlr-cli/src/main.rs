//! `rantlr` — the command-line front door to the toolchain.
//!
//! One `.rg` grammar file defines a language. Each subcommand shows one
//! layer of what the toolchain derives from it, in the order you'd meet
//! them building a language:
//!
//! * `new`     — scaffold a working grammar and a sample document
//! * `check`   — certify: the envelope report, or the refusal
//! * `tokens`  — the lex
//! * `parse`   — the lossless green tree (total, even when broken)
//! * `outline` — derived document symbols
//! * `defs`    — derived binding: definitions, references, unresolved
//! * `edit`    — incremental reparse: what got reused, and how fast
//! * `ts`      — export a tree-sitter grammar
//! * `ast`     — export a typed Rust AST
//!
//! Nothing here is a separate implementation: every command drives the
//! same certified pipeline the LSP server and the embedding API use.

mod render;

use render::{bold, cyan, dim, green, red, Src, TreeOpts};

use rantlr_engine::{IncSession, Line, LineEdit};
use rantlr_grammar::astgen::generate_with_paths;
use rantlr_grammar::{CompiledLexer, LrTables};
use rantlr_rg::compile::{certify, compile, LangDef, RgDiag};
use rantlr_rg::tsgen::emit_tree_sitter;
use rantlr_rg::RgToolchain;
use rantlr_sem::SemDb;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::time::Instant;

const USAGE: &str = "\
rantlr — envelope-certified incremental parsing

USAGE
    rantlr <command> [options]

COMMANDS
    new <dir> [--name Lang] [--ext .ext]
                            scaffold a grammar + sample document
    check <grammar.rg>      certify the grammar; print the envelope report
    tokens <grammar.rg> <file>
                            show the lex, token by token
    parse <grammar.rg> <file> [--trivia] [--depth N]
                            show the lossless parse tree and any repairs
    outline <grammar.rg> <file>
                            derived document symbols
    defs <grammar.rg> <file>
                            derived binding: defs, refs, unresolved
    types <grammar.rg> <file> [--all]
                            the declared type tier: typed defs, errors
    expand <grammar.rg> <file> [--check] [--print] [--depth N]
                            the declared META tier: materialize macro
                            expansion as <file-stem>.exp.<ext> plus a
                            provenance sidecar (write-if-changed);
                            --check verifies the materialized pair is
                            current (the read-only drift gate)
    edit <grammar.rg> <file> --line N --text \"...\"
                            reparse incrementally; report reuse and timing
    ts <grammar.rg> <outdir>
                            emit a tree-sitter grammar + highlight queries
    ast <grammar.rg>        emit a typed Rust AST to stdout

Every command reads the grammar through the same certification the
editor path uses: an out-of-envelope grammar produces a refusal with a
counterexample, not a broken parser.
";

fn main() {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let cmd = raw.first().map(|s| s.as_str()).unwrap_or("");
    let args = Args::parse(&raw[raw.len().min(1)..]);
    match cmd {
        "new" => cmd_new(&args),
        "check" => cmd_check(&args),
        "tokens" => cmd_tokens(&args),
        "parse" => cmd_parse(&args),
        "outline" => cmd_outline(&args),
        "defs" => cmd_defs(&args),
        "types" => cmd_types(&args),
        "expand" => cmd_expand(&args),
        "edit" => cmd_edit(&args),
        "ts" => cmd_ts(&args),
        "ast" => cmd_ast(&args),
        "help" | "--help" | "-h" | "" => print!("{USAGE}"),
        other => {
            eprintln!("rantlr: unknown command `{other}`\n");
            eprint!("{USAGE}");
            std::process::exit(2);
        }
    }
}

// ---------------------------------------------------------------------------
// Arguments
// ---------------------------------------------------------------------------

struct Args {
    pos: Vec<String>,
    vals: HashMap<String, String>,
    flags: HashSet<String>,
}

impl Args {
    fn parse(raw: &[String]) -> Args {
        let (mut pos, mut vals, mut flags) = (Vec::new(), HashMap::new(), HashSet::new());
        let mut i = 0;
        while i < raw.len() {
            match raw[i].strip_prefix("--") {
                Some(name) => match name.split_once('=') {
                    Some((k, v)) => {
                        vals.insert(k.to_string(), v.to_string());
                    }
                    None if i + 1 < raw.len() && !raw[i + 1].starts_with("--") => {
                        vals.insert(name.to_string(), raw[i + 1].clone());
                        i += 1;
                    }
                    None => {
                        flags.insert(name.to_string());
                    }
                },
                None => pos.push(raw[i].clone()),
            }
            i += 1;
        }
        Args { pos, vals, flags }
    }

    fn at(&self, i: usize, what: &str) -> &str {
        self.pos.get(i).map(|s| s.as_str()).unwrap_or_else(|| die(&format!("missing <{what}>")))
    }
    fn val(&self, k: &str) -> Option<&str> {
        self.vals.get(k).map(|s| s.as_str())
    }
    fn has(&self, k: &str) -> bool {
        self.flags.contains(k)
    }
}

fn die(msg: &str) -> ! {
    eprintln!("rantlr: {msg}\n");
    eprint!("{USAGE}");
    std::process::exit(2);
}

fn read_file(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("rantlr: cannot read {path}: {e}");
        std::process::exit(2);
    })
}

// ---------------------------------------------------------------------------
// Loading a grammar (the certification gate every command goes through)
// ---------------------------------------------------------------------------

/// A grammar that made it through the envelope: the compiled definition
/// plus the certified lexer and LR tables.
struct Lang {
    def: LangDef,
    lexer: CompiledLexer,
    tables: LrTables,
}

/// Compile + certify, or print the refusal and exit 1. This is the same
/// gate `rantlr check` reports on — the other commands just need it to
/// have passed before they have anything to show.
fn load(path: &str) -> Lang {
    let src = read_file(path);
    let s = Src::new(path, &src);
    let tc = RgToolchain::new();

    // Parse the grammar file itself (total: a broken grammar still
    // yields a tree, so we can report every error at once).
    let session = IncSession::new(&tc.lexer, &tc.sg, &tc.tables, &src).expect("parsing is total");
    let tree = session.tree().expect("total").clone();
    let syntax = rantlr_services::diagnostics(&tc.lexer, &session.buf, &tc.sg, &session.last_repairs);
    for d in &syntax {
        render::diagnostic(&s, 1, d.span, &d.message);
    }

    let (def, diags) = compile(&tree, &tc.prods);
    for d in &diags {
        render::diagnostic(&s, d.severity, d.span, &d.msg);
    }
    if !syntax.is_empty() || diags.iter().any(|d| d.severity == 1) {
        eprintln!("\n{}: {path} is not a valid grammar", red("refused"));
        std::process::exit(1);
    }

    match certify(&def) {
        Ok((lexer, tables)) => Lang { def, lexer, tables },
        Err(diags) => {
            for d in &diags {
                render::diagnostic(&s, d.severity, d.span, &d.msg);
            }
            eprintln!("\n{}: {path} is outside the envelope", red("refused"));
            explain(&diags);
            std::process::exit(1);
        }
    }
}

/// Point at the envelope rule behind a refusal. The diagnostics already
/// carry witnesses; this adds the "why does this rule exist" line that
/// turns a rejection into something actionable.
fn explain(diags: &[RgDiag]) {
    let mut seen = BTreeSet::new();
    for d in diags {
        let hint = if d.msg.contains("newline") || d.msg.contains("L1") {
            "L1 — tokens stay inside one line, so relexing can restart at any line."
        } else if d.msg.contains("stack") || d.msg.contains("max_stack") {
            "L2 — the mode stack is statically bounded, so a line's entry state is a small value."
        } else if d.msg.contains("conflict") || d.msg.contains("ambiguous") {
            "L3 — the grammar must be deterministic LR(1). Add a `prec` line, or refactor the rule."
        } else {
            continue;
        };
        seen.insert(hint);
    }
    for h in seen {
        eprintln!("  {} {h}", cyan("note:"));
    }
}

/// Open an incremental session over a target document.
fn session_for<'a>(lang: &'a Lang, text: &str) -> IncSession<'a> {
    IncSession::new(&lang.lexer, &lang.def.sg, &lang.tables, text).expect("parsing is total")
}


/// Load every sibling document with the same extension into the
/// semantic db — imports and cross-file types need the whole world.
fn load_siblings(lang: &Lang, db: &mut SemDb, doc_path: &str) {
    let path = std::path::Path::new(doc_path);
    let (Some(dir), Some(ext)) = (path.parent(), path.extension()) else { return };
    // A bare filename's parent is the EMPTY path; read_dir needs ".".
    let dir = if dir.as_os_str().is_empty() { std::path::Path::new(".") } else { dir };
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.extension() == Some(ext) && p != path && p.is_file() {
            if let Ok(text) = std::fs::read_to_string(&p) {
                let session = session_for(lang, &text);
                db.set_tree(p.to_str().unwrap_or_default(), session.tree().expect("total").clone());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// new — scaffold
// ---------------------------------------------------------------------------

const STARTER_RG: &str = include_str!("starter/starter.rg");
const STARTER_DOC: &str = include_str!("starter/starter.doc");

fn cmd_new(args: &Args) {
    let dir = args.at(0, "dir");
    let name = args.val("name").unwrap_or("Mylang").to_string();
    let ext = args.val("ext").unwrap_or(".my").trim_start_matches('.').to_string();
    let slug = name.to_lowercase();

    let root = std::path::Path::new(dir);
    if root.exists() && root.read_dir().map(|mut d| d.next().is_some()).unwrap_or(false) {
        eprintln!("rantlr: {dir} already exists and is not empty");
        std::process::exit(2);
    }
    std::fs::create_dir_all(root.join(".vscode")).unwrap_or_else(|e| {
        eprintln!("rantlr: cannot create {dir}: {e}");
        std::process::exit(2);
    });

    let grammar = STARTER_RG.replace("{NAME}", &name).replace("{EXT}", &ext);
    let doc = STARTER_DOC.replace("{NAME}", &name).replace("{EXT}", &ext);
    // The demo extension registers its target language under the id
    // `chartlang`; this association points YOUR extension at it.
    let settings = format!("{{\n  \"files.associations\": {{ \"*.{ext}\": \"chartlang\" }}\n}}\n");

    let g_path = root.join(format!("{slug}.rg"));
    let d_path = root.join(format!("example.{ext}"));
    write_file(&g_path, &grammar);
    write_file(&d_path, &doc);
    write_file(&root.join(".vscode/settings.json"), &settings);

    println!("{} {name} in {dir}/", green("scaffolded"));
    println!("  {}  the whole language definition", g_path.display());
    println!("  {}  a sample document", d_path.display());
    println!();
    println!("Next:");
    println!("  rantlr check {}", g_path.display());
    println!("  rantlr parse {} {}", g_path.display(), d_path.display());
    println!("  rantlr defs  {} {}", g_path.display(), d_path.display());
}

fn write_file(path: &std::path::Path, body: &str) {
    std::fs::write(path, body).unwrap_or_else(|e| {
        eprintln!("rantlr: cannot write {}: {e}", path.display());
        std::process::exit(2);
    });
}

// ---------------------------------------------------------------------------
// check — the envelope report
// ---------------------------------------------------------------------------

fn cmd_check(args: &Args) {
    let path = args.at(0, "grammar.rg");
    let lang = load(path);
    let (def, lexer, tables) = (&lang.def, &lang.lexer, &lang.tables);

    let classes: BTreeSet<&str> = (0..def.lex.tokens.len())
        .filter_map(|i| def.styles.class_of(i as u16))
        .map(|c| def.styles.legend[c as usize])
        .collect();
    let lists: Vec<&str> = {
        let mut v: Vec<&str> =
            tables.lists.keys().map(|&nt| def.sg.nt_names[nt as usize].as_str()).collect();
        v.sort_unstable();
        v
    };
    let fragile = tables.fragile.iter().filter(|&&f| f).count();

    println!("{} {} {}", green("✓"), bold(&def.lex.name), dim("— certified"));
    println!();
    println!("  {}", bold("lexical envelope"));
    row("modes", &format!("{} ({})", def.lex.mode_names.len(), def.lex.mode_names.join(", ")));
    row("tokens", &format!("{} + {} keywords", def.lex.tokens.len(), def.lex.keywords.len()));
    row("mode-stack bound", &format!("{}  {}", lexer.report.stack_bound, dim("(L2)")));
    row("line entry states", &format!("{}  {}", lexer.report.line_state_space, dim("(L2: bounded)")));
    row(
        "DFA states",
        &def.lex
            .mode_names
            .iter()
            .zip(&lexer.report.dfa_states)
            .map(|(m, n)| format!("{m} {n}"))
            .collect::<Vec<_>>()
            .join(", "),
    );
    println!();
    println!("  {}", bold("syntax envelope"));
    row("nonterminals", &def.sg.nt_names.len().to_string());
    row("productions", &def.sg.prods.len().to_string());
    row("LR(1) states", &tables.n_states.to_string());
    row("conflicts", &format!("{}  {}", tables.conflicts.len(), dim("(L3: deterministic)")));
    if tables.resolved_by_prec > 0 {
        row(
            "settled by prec",
            &format!("{}  {}", tables.resolved_by_prec, dim(&format!("({fragile} fragile prods)"))),
        );
    }
    row(
        "auto-balanced lists",
        &if lists.is_empty() {
            format!("0  {}", dim("(L4)"))
        } else {
            format!("{}  {} {}", lists.len(), dim("(L4):"), dim(&lists.join(", ")))
        },
    );
    println!();
    println!("  {}", bold("derived services"));
    row("highlight classes", &classes.iter().copied().collect::<Vec<_>>().join(", "));
    row("outline entries", &def.outline.entries.len().to_string());
    row(
        "binding sites",
        &format!(
            "{} defs, {} refs, {} scopes",
            def.binding.defs.len(),
            def.binding.refs.len(),
            def.binding.scopes.len()
        ),
    );
    row(
        "module tier",
        &{
            let imports =
                def.binding.refs.iter().filter(|r| r.3 == rantlr_sem::RefKind::Import).count();
            if def.binding.exports.is_empty() && imports == 0 {
                format!("none declared  {}", dim("(@export/@import — open world)"))
            } else {
                format!(
                    "{} exporting production(s), {} import form(s)  {}",
                    def.binding.exports.len(),
                    imports,
                    dim("(strict)")
                )
            }
        },
    );
    row(
        "type tier",
        &if def.types.rules.is_empty() {
            format!("none declared  {}", dim("(@type)"))
        } else {
            format!(
                "{} rules over {}",
                def.types.rules.len(),
                def.types.atoms.join(", ")
            )
        },
    );
}

fn row(label: &str, value: &str) {
    println!("    {:<22}{value}", label);
}

// ---------------------------------------------------------------------------
// tokens — the lex
// ---------------------------------------------------------------------------

fn cmd_tokens(args: &Args) {
    let lang = load(args.at(0, "grammar.rg"));
    let doc_path = args.at(1, "file");
    let text = read_file(doc_path);
    let session = session_for(&lang, &text);
    let buf = &session.buf;

    let mut shown = 0usize;
    for (li, lt) in buf.lexed.iter().enumerate() {
        let line = &buf.lines[li];
        let entry = buf.entry_state(li);
        println!(
            "{} {}",
            cyan(&format!("{:>4}", li + 1)),
            dim(&format!("entry state {entry:?}"))
        );
        let mut col = 0u32;
        for tok in &lt.tokens {
            let text = &line.text[col as usize..(col + tok.len) as usize];
            let name = lang.def.sg.term_name(tok.id);
            let class = lang
                .def
                .styles
                .class_of(tok.id)
                .map(|c| lang.def.styles.legend[c as usize])
                .unwrap_or("-");
            let kind = if lang.lexer.is_trivia(tok.id) { dim("trivia") } else { String::new() };
            println!(
                "       {:<4} {:<18} {:<12} {} {}",
                col,
                cyan(name),
                dim(class),
                green(&render::escape(text)),
                kind
            );
            col += tok.len;
            shown += 1;
        }
    }
    println!();
    println!("{} tokens over {} lines", shown, buf.lines.len());
    println!("{}", dim("Every token is line-local (L1) — that is what makes relexing restartable."));
}

// ---------------------------------------------------------------------------
// parse — the lossless tree
// ---------------------------------------------------------------------------

fn cmd_parse(args: &Args) {
    let lang = load(args.at(0, "grammar.rg"));
    let doc_path = args.at(1, "file");
    let text = read_file(doc_path);
    let session = session_for(&lang, &text);
    let tree = session.tree().expect("parsing is total");

    render::tree(
        tree,
        &lang.def.sg,
        &TreeOpts {
            trivia: args.has("trivia"),
            max_depth: args.val("depth").and_then(|d| d.parse().ok()).unwrap_or(0),
        },
    );

    println!();
    let lossless = tree.text() == text;
    println!(
        "{} {}",
        if lossless { green("✓") } else { red("✗") },
        if lossless {
            "lossless: the tree reproduces the source byte for byte"
        } else {
            "NOT lossless — this is a bug, please report it"
        }
    );

    let diags = rantlr_services::diagnostics(&lang.lexer, &session.buf, &lang.def.sg, &session.last_repairs);
    if diags.is_empty() {
        println!("{} no errors", green("✓"));
    } else {
        println!();
        let s = Src::new(doc_path, &text);
        for d in &diags {
            render::diagnostic(&s, 1, d.span, &d.message);
        }
        println!(
            "{}",
            dim("Parsing is total: the tree above is complete and usable despite these errors.")
        );
    }
    if !lossless {
        std::process::exit(1);
    }
}

// ---------------------------------------------------------------------------
// outline / defs — derived services
// ---------------------------------------------------------------------------

fn cmd_outline(args: &Args) {
    let lang = load(args.at(0, "grammar.rg"));
    let doc_path = args.at(1, "file");
    let text = read_file(doc_path);
    let session = session_for(&lang, &text);
    let s = Src::new(doc_path, &text);
    let syms = rantlr_services::outline(session.tree().expect("total"), &lang.def.outline);

    if syms.is_empty() {
        println!("{}", dim("no symbols — add @outline(name) to a rule alternative"));
        return;
    }
    for sym in &syms {
        let (line, col) = s.line_col(sym.selection.0);
        println!("{:<10} {:<24} {}", dim(sym.kind), bold(&sym.name), dim(&format!("{line}:{col}")));
    }
}

fn cmd_defs(args: &Args) {
    let lang = load(args.at(0, "grammar.rg"));
    let doc_path = args.at(1, "file");
    let text = read_file(doc_path);
    let session = session_for(&lang, &text);
    let s = Src::new(doc_path, &text);

    let mut db = SemDb::new(lang.def.binding.clone());
    load_siblings(&lang, &mut db, doc_path);
    db.set_tree(doc_path, session.tree().expect("total").clone());
    let syms = db.symbols(doc_path);
    let unresolved: HashSet<(String, (u32, u32))> = db.unresolved(doc_path).into_iter().collect();

    println!("{}", bold("definitions"));
    if syms.defs.is_empty() {
        println!("  {}", dim("none — add @def(name) to a rule alternative"));
    }
    let tier = lang.def.binding.module_tier();
    for d in &syms.defs {
        let (line, col) = s.line_col(d.span.0);
        let where_ = if d.top_level { "top level" } else { "nested scope" };
        let vis = if !tier || !d.top_level {
            String::new()
        } else if d.exported {
            green("pub")
        } else {
            dim("private")
        };
        println!(
            "  {:<24} {:<12} {:<10} {}",
            green(&d.name),
            dim(where_),
            vis,
            dim(&format!("{line}:{col}"))
        );
    }

    println!();
    println!("{}", bold("references"));
    if syms.refs.is_empty() {
        println!("  {}", dim("none — add @ref(name) to a rule alternative"));
    }
    for r in &syms.refs {
        let (line, col) = s.line_col(r.span.0);
        let target = db.definition(doc_path, r.span.0);
        let status = match &target {
            Some((duri, dspan)) if duri == doc_path => {
                let (dl, dc) = s.line_col(dspan.0);
                green(&format!("→ {dl}:{dc}"))
            }
            Some((duri, _)) => green(&format!("→ {duri}")),
            None if unresolved.contains(&(r.name.clone(), r.span)) => red("unresolved"),
            // The name exists somewhere in the file but is not in scope
            // at this point — declared later in an ordered language, or
            // sealed inside another scope.
            None => render::yellow("not visible here"),
        };
        println!("  {:<24} {:<12} {}", cyan(&r.name), dim(&format!("{line}:{col}")), status);
    }

    let hidden = db.not_exported(doc_path);
    if !hidden.is_empty() {
        println!();
        for (name, span) in &hidden {
            render::diagnostic(
                &s,
                1,
                *span,
                &format!("`{name}` exists but is not exported by its file"),
            );
        }
    }
    let qual_errs = db.qualified_errors(doc_path);
    if !qual_errs.is_empty() {
        println!();
        for (msg, span) in &qual_errs {
            render::diagnostic(&s, 1, *span, msg);
        }
    }

    let n_unres = unresolved.len();
    println!();
    match (n_unres, hidden.len()) {
        (0, 0) => println!("{} every reference resolves", green("✓")),
        (u, 0) => println!("{} {u} unresolved reference(s)", red("✗")),
        (0, h) => println!("{} {h} access error(s)", red("✗")),
        (u, h) => println!("{} {u} unresolved reference(s), {h} access error(s)", red("✗")),
    }
    if !hidden.is_empty() || !qual_errs.is_empty() {
        std::process::exit(1);
    }
}

// ---------------------------------------------------------------------------
// expand — the declared META tier, materialized
// ---------------------------------------------------------------------------

fn cmd_expand(args: &Args) {
    let grammar_path = args.at(0, "grammar.rg");
    let doc_path = args.at(1, "file");
    let lang = load(grammar_path);
    if !lang.def.macros.declared() {
        println!("{} declares no macro tier (@macro) — nothing to expand", grammar_path);
        return;
    }
    let src = read_file(doc_path);
    let depth: u32 = args.val("depth").map(|v| v.parse().unwrap_or(8)).unwrap_or(8);
    let out = rantlr_rg::expand::expand_document(&lang.lexer, &lang.def, &lang.tables, &src, depth)
        .unwrap_or_else(|e| die(&e));
    for d in &out.diags {
        eprintln!("{} {}..{}: {}", red("macro"), d.span.0, d.span.1, d.msg);
    }
    if out.repairs > 0 {
        eprintln!(
            "{} expansion produced text with {} parse repair(s) — the materialized file will show them",
            red("warning"),
            out.repairs
        );
    }

    // Deterministic naming: demo.c → demo.exp.c (extensionless: demo.exp).
    let p = std::path::Path::new(doc_path);
    let exp_path = match (p.file_stem().and_then(|s| s.to_str()), p.extension().and_then(|s| s.to_str())) {
        (Some(stem), Some(ext)) => p.with_file_name(format!("{stem}.exp.{ext}")),
        _ => p.with_file_name(format!(
            "{}.exp",
            p.file_name().and_then(|s| s.to_str()).unwrap_or("out")
        )),
    };
    let prov_path = exp_path.with_file_name(format!(
        "{}.prov.json",
        exp_path.file_name().and_then(|s| s.to_str()).unwrap()
    ));
    let prov = rantlr_rg::expand::provenance_json(doc_path, &src, &out);

    if args.has("print") {
        print!("{}", out.text);
    }
    if args.has("check") {
        // The read-only drift gate: the materialized pair must be
        // byte-identical to a fresh expansion.
        let disk_exp = std::fs::read_to_string(&exp_path).unwrap_or_default();
        let disk_prov = std::fs::read_to_string(&prov_path).unwrap_or_default();
        if disk_exp == out.text && disk_prov == prov {
            println!(
                "{} {} is current ({} substitution(s), {} pass(es))",
                green("✓"),
                exp_path.display(),
                out.substitutions,
                out.passes
            );
        } else {
            println!(
                "{} {} drifted from its source — regenerate with `rantlr expand`",
                red("✗"),
                exp_path.display()
            );
            std::process::exit(1);
        }
        return;
    }

    // Write-if-changed: unchanged expansions do not touch mtimes.
    let mut wrote = 0;
    for (path, content) in [(&exp_path, &out.text), (&prov_path, &prov)] {
        if std::fs::read_to_string(path).ok().as_deref() != Some(content.as_str()) {
            std::fs::write(path, content).unwrap_or_else(|e| {
                die(&format!("cannot write {}: {e}", path.display()));
            });
            wrote += 1;
        }
    }
    println!(
        "{} {} — {} substitution(s), {} pass(es), {} segment(s){}",
        green("✓"),
        exp_path.display(),
        out.substitutions,
        out.passes,
        out.segs.len(),
        if wrote == 0 { " (unchanged)" } else { "" }
    );
}

// ---------------------------------------------------------------------------
// types — the declared type tier
// ---------------------------------------------------------------------------

fn cmd_types(args: &Args) {
    let lang = load(args.at(0, "grammar.rg"));
    let doc_path = args.at(1, "file");
    let text = read_file(doc_path);
    let session = session_for(&lang, &text);
    let s = Src::new(doc_path, &text);

    if lang.def.types.rules.is_empty() {
        println!(
            "{}",
            dim("this grammar declares no type tier — add @type(…) annotations to rules")
        );
        return;
    }

    let mut db = SemDb::new(lang.def.binding.clone());
    db.set_types(lang.def.types.clone());
    load_siblings(&lang, &mut db, doc_path);
    db.set_tree(doc_path, session.tree().expect("total").clone());
    let report = db.types(doc_path);

    // Vocabulary line: grammar atoms, then the types THIS document
    // introduces. Arrow entries and foreign types display where they
    // are used — on the defs below — not as vocabulary.
    let ga = report.grammar_atoms.min(report.atoms.len());
    print!("{} {}", bold("vocabulary"), report.atoms[..ga].join(", "));
    if report.local_doc_types.is_empty() {
        println!();
    } else {
        let names: Vec<&str> = report
            .local_doc_types
            .iter()
            .filter_map(|&t| report.atoms.get(t as usize).map(|s| s.as_str()))
            .collect();
        println!("  {} {}", dim("+ document types:"), cyan(&names.join(", ")));
    }
    println!();
    println!("{}", bold("typed definitions"));
    if report.def_types.is_empty() {
        println!("  {}", dim("none"));
    }
    for &(span, t) in &report.def_types {
        let (line, col) = s.line_col(span.0);
        let name = &text[span.0 as usize..span.1 as usize];
        println!(
            "  {:<24} {:<10} {}",
            green(name),
            cyan(&report.atoms[t as usize]),
            dim(&format!("{line}:{col}"))
        );
    }

    if args.has("all") {
        println!();
        println!("{}", bold("typed nodes"));
        for &((a, b), t) in &report.types {
            let (line, col) = s.line_col(a);
            println!(
                "  {:<10} {:<10} {}",
                cyan(&report.atoms[t as usize]),
                dim(&format!("{line}:{col}")),
                render::escape(text[a as usize..(b as usize).min(text.len())].trim_end())
            );
        }
    }

    println!();
    if report.diags.is_empty() {
        println!("{} no type errors ({} nodes typed)", green("✓"), report.types.len());
    } else {
        for d in &report.diags {
            render::diagnostic(&s, 1, d.span, &d.msg);
        }
        println!("{} {} type error(s)", red("✗"), report.diags.len());
        std::process::exit(1);
    }
}

// ---------------------------------------------------------------------------
// edit — the incremental reparse
// ---------------------------------------------------------------------------

fn cmd_edit(args: &Args) {
    let lang = load(args.at(0, "grammar.rg"));
    let doc_path = args.at(1, "file");
    let text = read_file(doc_path);
    let line_no: usize = args
        .val("line")
        .unwrap_or_else(|| die("edit needs --line N"))
        .parse()
        .unwrap_or_else(|_| die("--line must be a number"));
    let new_text = args.val("text").unwrap_or_else(|| die("edit needs --text \"...\""));

    let mut session = session_for(&lang, &text);
    if line_no == 0 || line_no > session.buf.lines.len() {
        die(&format!("--line {line_no} is outside 1..={}", session.buf.lines.len()));
    }
    let li = line_no - 1;
    let term = session.buf.lines[li].term;
    let before = session.buf.lines[li].text.clone();

    println!("{} {}", dim("- "), render::escape(&before));
    println!("{} {}", dim("+ "), render::escape(new_text));
    println!();

    let edits =
        [LineEdit { start: li, end: li + 1, replacement: vec![Line::new(new_text, term)] }];
    let t0 = Instant::now();
    let outcome = match session.edit(&lang.def.sg, &lang.tables, &edits) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("rantlr: incremental parse failed: {e:?}");
            std::process::exit(1);
        }
    };
    let incremental = t0.elapsed();

    // The honest comparison: the same final text, parsed from scratch.
    let final_text = session.buf.reproduce();
    let t1 = Instant::now();
    let fresh = session_for(&lang, &final_text);
    let batch = t1.elapsed();

    let st = outcome.stats;
    let pct = if st.total_terms == 0 {
        100.0
    } else {
        100.0 * st.reused_terms as f64 / st.total_terms as f64
    };

    println!("  {}", bold("damage"));
    row("edit sites", &outcome.damage.sites.to_string());
    row("lines relexed", &outcome.damage.relexed_lines.to_string());
    println!();
    println!("  {}", bold("reuse"));
    row("terminals reused", &format!("{} of {}  ({pct:.1}%)", st.reused_terms, st.total_terms));
    row("subtree splices", &st.splices.to_string());
    row("breakdowns", &st.breakdowns.to_string());
    println!();
    println!("  {}", bold("time"));
    row("incremental", &format!("{incremental:?}"));
    row("full reparse", &format!("{batch:?}  {}", dim("(same final text)")));
    println!();

    // Differential gate, run on every invocation: incremental must agree
    // with batch, structurally and byte for byte.
    let same_text = session.buf.reproduce() == fresh.buf.reproduce();
    let same_tree = session.tree() == fresh.tree();
    if same_text && same_tree {
        println!("{} incremental result is identical to a full reparse", green("✓"));
    } else {
        println!("{} incremental and batch DISAGREE — please report this", red("✗"));
        std::process::exit(1);
    }
    if !outcome.repairs.is_empty() {
        println!("{}", dim(&format!("{} repair(s) in the new text", outcome.repairs.len())));
    }
}

// ---------------------------------------------------------------------------
// ts / ast — exports
// ---------------------------------------------------------------------------

fn cmd_ts(args: &Args) {
    let lang = load(args.at(0, "grammar.rg"));
    let outdir = args.at(1, "outdir");
    let out = match emit_tree_sitter(
        &lang.def.lex,
        &lang.def.sg,
        &lang.tables,
        &lang.def.styles,
        &lang.def.binding,
    ) {
        Ok(o) => o,
        Err(errors) => {
            for e in errors {
                eprintln!("{}: {e}", red("error"));
            }
            std::process::exit(1);
        }
    };
    for w in &out.warnings {
        eprintln!("{}: {w}", render::yellow("warning"));
    }
    let root = std::path::Path::new(outdir);
    std::fs::create_dir_all(root.join("queries")).unwrap_or_else(|e| {
        eprintln!("rantlr: cannot create {outdir}: {e}");
        std::process::exit(2);
    });
    write_file(&root.join("grammar.js"), &out.grammar_js);
    write_file(&root.join("queries/highlights.scm"), &out.highlights_scm);
    println!("{} {outdir}/grammar.js and {outdir}/queries/highlights.scm", green("wrote"));
}

fn cmd_ast(args: &Args) {
    let lang = load(args.at(0, "grammar.rg"));
    print!("{}", generate_with_paths(&lang.def.sg, &lang.tables, "rantlr_grammar"));
}
