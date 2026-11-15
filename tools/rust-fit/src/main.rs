//! How much of real Rust does a `.qana` lexical grammar lex the way
//! rustc_lexer does?
//!
//! `compare` runs both lexers over every `.rs` file under the roots it is
//! given. Each byte gets a class from each lexer (see `classes.rs`), and a
//! line agrees when every byte of it has the same class from both.
//!
//! `record` writes rustc_lexer's reading of `examples/rust-lex/sample.rs`
//! to `sample.classes`, so that the workspace's own tests can hold qana to
//! it without depending on rustc_lexer. `record --check` fails when the
//! committed record is not what rustc_lexer says now.

mod classes;

use classes::{encode, line_classes, token_classes, Class};
use qana_engine::{LexedBuffer, LineTerm};
use qana_grammar::CompiledLexer;
use qana_lang::EmbeddedLang;
use ra_ap_rustc_lexer::{strip_shebang, tokenize, FrontmatterAllowed, LiteralKind, TokenKind};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

const USAGE: &str = "\
usage:
  rust-fit compare [--grammar <file.qana>] <root>...
      Lex every .rs file under the roots with both lexers and report the
      lines they read differently. Fails if there are any.
  rust-fit record
      Write rustc_lexer's reading of examples/rust-lex/sample.rs to
      examples/rust-lex/sample.classes.
  rust-fit record --check
      Fail if the committed record is not what rustc_lexer says now.";

/// `examples/rust-lex`, found from this crate's place in the repository.
fn example_dir() -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/rust-lex");
    // The short form reads better in a message; the long one still works
    // if the directory cannot be resolved, and the caller reports that.
    dir.canonicalize().unwrap_or(dir)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("compare") => compare(&args[1..]),
        Some("record") => match &args[1..] {
            [] => record(false),
            [flag] if flag == "--check" => record(true),
            _ => Err(USAGE.to_string()),
        },
        _ => Err(USAGE.to_string()),
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::from(2)
        }
    }
}

fn load_grammar(path: &Path) -> Result<&'static CompiledLexer, String> {
    let source = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read the grammar {}: {e}", path.display()))?;
    let lang = EmbeddedLang::from_qana_source(&source).map_err(|diags| {
        let mut message = format!("the grammar {} does not certify:", path.display());
        for diag in diags {
            message.push_str(&format!("\n  at {:?}: {}", diag.span, diag.msg));
        }
        message
    })?;
    Ok(lang.lexer)
}

/// rustc_lexer's class for a token, and its name for a report.
fn rustc_class(kind: &TokenKind) -> (Class, String) {
    let hashes = |n: &Option<u8>| match n {
        Some(n) => n.to_string(),
        None => "invalid".to_string(),
    };
    match kind {
        TokenKind::Whitespace => (Class::Whitespace, "Whitespace".into()),
        TokenKind::LineComment { .. } => (Class::Comment, "LineComment".into()),
        TokenKind::BlockComment { .. } => (Class::Comment, "BlockComment".into()),
        TokenKind::Frontmatter { .. } => (Class::Comment, "Frontmatter".into()),
        TokenKind::Ident => (Class::Word, "Ident".into()),
        TokenKind::RawIdent => (Class::Word, "RawIdent".into()),
        TokenKind::InvalidIdent => (Class::Word, "InvalidIdent".into()),
        TokenKind::UnknownPrefix => (Class::Word, "UnknownPrefix".into()),
        TokenKind::Lifetime { .. } => (Class::Lifetime, "Lifetime".into()),
        TokenKind::RawLifetime => (Class::Lifetime, "RawLifetime".into()),
        TokenKind::UnknownPrefixLifetime => (Class::Lifetime, "UnknownPrefixLifetime".into()),
        TokenKind::Literal { kind, .. } => match kind {
            LiteralKind::Int { .. } => (Class::Number, "Int".into()),
            LiteralKind::Float { .. } => (Class::Number, "Float".into()),
            LiteralKind::Char { .. } => (Class::Text, "Char".into()),
            LiteralKind::Byte { .. } => (Class::Text, "Byte".into()),
            LiteralKind::Str { .. } => (Class::Text, "Str".into()),
            LiteralKind::ByteStr { .. } => (Class::Text, "ByteStr".into()),
            LiteralKind::CStr { .. } => (Class::Text, "CStr".into()),
            LiteralKind::RawStr { n_hashes } => {
                (Class::Text, format!("RawStr#{}", hashes(n_hashes)))
            }
            LiteralKind::RawByteStr { n_hashes } => {
                (Class::Text, format!("RawByteStr#{}", hashes(n_hashes)))
            }
            LiteralKind::RawCStr { n_hashes } => {
                (Class::Text, format!("RawCStr#{}", hashes(n_hashes)))
            }
        },
        TokenKind::Unknown => (Class::Unknown, "Unknown".into()),
        TokenKind::Eof => (Class::Whitespace, "Eof".into()),
        // Every remaining kind is one character of punctuation.
        other => (Class::Punct, format!("{other:?}")),
    }
}

/// rustc_lexer's reading of a whole file: for every byte, its class and
/// the index in `kinds` of the token kind it belongs to.
struct RustcReading {
    classes: Vec<Class>,
    kind_of: Vec<u32>,
    /// Bytes of a leading `#!` line, which is not Rust.
    shebang: usize,
}

struct KindNames {
    names: Vec<String>,
    index: HashMap<String, u32>,
}

impl KindNames {
    fn new() -> Self {
        KindNames { names: Vec::new(), index: HashMap::new() }
    }

    fn id(&mut self, name: String) -> u32 {
        if let Some(&id) = self.index.get(&name) {
            return id;
        }
        let id = self.names.len() as u32;
        self.names.push(name.clone());
        self.index.insert(name, id);
        id
    }
}

fn read_with_rustc(src: &str, kinds: &mut KindNames) -> RustcReading {
    let shebang = strip_shebang(src).unwrap_or(0);
    let mut classes = vec![Class::Whitespace; src.len()];
    let mut kind_of = vec![0u32; src.len()];
    let mut at = shebang;
    for token in tokenize(&src[shebang..], FrontmatterAllowed::No) {
        let len = token.len as usize;
        let (class, name) = rustc_class(&token.kind);
        let id = kinds.id(name);
        classes[at..at + len].fill(class);
        kind_of[at..at + len].fill(id);
        at += len;
    }
    RustcReading { classes, kind_of, shebang }
}

fn term_len(term: LineTerm) -> usize {
    match term {
        LineTerm::Lf | LineTerm::Cr => 1,
        LineTerm::CrLf => 2,
        LineTerm::None => 0,
    }
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if name != "target" && !name.starts_with('.') {
                collect(&path, out);
            }
        } else if name.ends_with(".rs") {
            out.push(path);
        }
    }
}

#[derive(Default)]
struct Category {
    count: usize,
    samples: Vec<String>,
}

fn compare(args: &[String]) -> Result<bool, String> {
    let mut grammar = example_dir().join("rust_lex.qana");
    let mut roots: Vec<&str> = Vec::new();
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        if arg == "--grammar" {
            grammar = PathBuf::from(rest.next().ok_or(USAGE)?);
        } else {
            roots.push(arg);
        }
    }
    if roots.is_empty() {
        return Err(USAGE.to_string());
    }

    let lexer = load_grammar(&grammar)?;
    let qana_class = token_classes(lexer);

    let mut files = Vec::new();
    for root in roots {
        let root = Path::new(root);
        if !root.exists() {
            return Err(format!("no such root: {}", root.display()));
        }
        if root.is_file() {
            files.push(root.to_path_buf());
        } else {
            collect(root, &mut files);
        }
    }
    files.sort();

    let (mut n_files, mut n_bytes, mut n_lines) = (0usize, 0usize, 0usize);
    let (mut bad_lines, mut bad_files) = (0usize, 0usize);
    // Keyed by what each lexer said at the first byte where they part.
    let mut by_line: HashMap<String, Category> = HashMap::new();
    let mut by_file: HashMap<String, Category> = HashMap::new();
    let (mut rustc_time, mut qana_time) = (0f64, 0f64);
    let mut kinds = KindNames::new();

    for path in &files {
        // A file that is not UTF-8 is not Rust source.
        let Ok(src) = std::fs::read_to_string(path) else { continue };
        n_files += 1;
        n_bytes += src.len();

        let started = Instant::now();
        let theirs = read_with_rustc(&src, &mut kinds);
        rustc_time += started.elapsed().as_secs_f64();

        // qana: line by line, the mode stack carried from each to the next.
        let started = Instant::now();
        let buffer = LexedBuffer::new(lexer, &src);
        qana_time += started.elapsed().as_secs_f64();

        let mut offset = 0usize;
        let mut file_is_bad = false;
        for (line_no, (line, lexed)) in buffer.lines.iter().zip(&buffer.lexed).enumerate() {
            let start = offset;
            offset += line.text.len() + term_len(line.term);
            n_lines += 1;
            if start < theirs.shebang {
                continue;
            }
            let ours = line_classes(&lexed.tokens, &qana_class);
            let Some(col) = (0..ours.len()).find(|&i| ours[i] != theirs.classes[start + i])
            else {
                continue;
            };
            bad_lines += 1;

            // Name the qana token that holds the first differing byte.
            let mut end = 0usize;
            let token = lexed
                .tokens
                .iter()
                .find(|t| {
                    end += t.len as usize;
                    col < end
                })
                .expect("a byte of the line is in one of its tokens");
            let key = format!(
                "rustc_lexer {} ({:?})  vs  qana {} ({:?})",
                kinds.names[theirs.kind_of[start + col] as usize],
                theirs.classes[start + col],
                lexer.vocab.names[token.id as usize],
                ours[col],
            );
            let sample = || {
                let shown: String = line.text.chars().take(140).collect();
                format!("{}:{}:{}  {}", path.display(), line_no + 1, col + 1, shown.trim_end())
            };
            let entry = by_line.entry(key.clone()).or_default();
            entry.count += 1;
            if entry.samples.len() < 3 {
                entry.samples.push(sample());
            }
            if !file_is_bad {
                file_is_bad = true;
                bad_files += 1;
                let entry = by_file.entry(key).or_default();
                entry.count += 1;
                if entry.samples.len() < 4 {
                    entry.samples.push(sample());
                }
            }
        }
    }

    let mb = n_bytes as f64 / 1e6;
    println!("{n_files} files, {mb:.1} MB, {n_lines} lines");
    println!(
        "rustc_lexer {:.0} MB/s    qana {:.0} MB/s ({:.1} M lines/s)",
        mb / rustc_time,
        mb / qana_time,
        n_lines as f64 / 1e6 / qana_time
    );
    println!(
        "lines that differ: {bad_lines} of {n_lines} ({:.4}%)    \
         files with a difference: {bad_files} of {n_files} ({:.2}%)",
        100.0 * bad_lines as f64 / n_lines.max(1) as f64,
        100.0 * bad_files as f64 / n_files.max(1) as f64
    );
    if bad_lines == 0 {
        return Ok(true);
    }

    // A difference can spoil the lines after it (a string taken for code
    // runs on), so the first one in each file is the cause to look at.
    println!("\nThe first difference in each file, by files:");
    let mut causes: Vec<(&String, &Category)> = by_file.iter().collect();
    causes.sort_by(|a, b| b.1.count.cmp(&a.1.count).then(a.0.cmp(b.0)));
    for (key, category) in causes.iter().take(40) {
        println!("{:>7} files  {key}", category.count);
        for sample in &category.samples {
            println!("            {sample}");
        }
    }

    println!("\nEvery differing line, by lines:");
    let mut all: Vec<(&String, &Category)> = by_line.iter().collect();
    all.sort_by(|a, b| b.1.count.cmp(&a.1.count).then(a.0.cmp(b.0)));
    for (key, category) in all.iter().take(25) {
        println!("{:>9} lines  {key}", category.count);
    }
    Ok(false)
}

/// The record: a header, then one line for each line of the sample.
fn record_text(src: &str) -> String {
    let mut kinds = KindNames::new();
    let reading = read_with_rustc(src, &mut kinds);
    let mut out = String::from(
        "# How rustc_lexer reads sample.rs: one line here for each line there.\n\
         # A line is runs of one class, each a letter and a count of bytes:\n\
         #   _ whitespace   c comment   w word   l lifetime   n number\n\
         #   s string, char or byte literal   p punctuation   ? unknown\n\
         # Written by `rust-fit record` (tools/rust-fit); do not edit by hand.\n",
    );
    let mut offset = 0usize;
    for line in qana_engine::split_lines(src) {
        let end = offset + line.text.len();
        out.push_str(&encode(&reading.classes[offset..end]));
        out.push('\n');
        offset = end + term_len(line.term);
    }
    out
}

fn record(check: bool) -> Result<bool, String> {
    let dir = example_dir();
    let sample_path = dir.join("sample.rs");
    let record_path = dir.join("sample.classes");
    let src = std::fs::read_to_string(&sample_path)
        .map_err(|e| format!("cannot read {}: {e}", sample_path.display()))?;
    let text = record_text(&src);

    if !check {
        std::fs::write(&record_path, &text)
            .map_err(|e| format!("cannot write {}: {e}", record_path.display()))?;
        println!("wrote {} ({} lines)", record_path.display(), text.lines().count());
        return Ok(true);
    }

    let committed = std::fs::read_to_string(&record_path)
        .map_err(|e| format!("cannot read {}: {e}", record_path.display()))?;
    if committed == text {
        println!("{} is what rustc_lexer says", record_path.display());
        return Ok(true);
    }
    let line = committed
        .lines()
        .zip(text.lines())
        .position(|(a, b)| a != b)
        .unwrap_or_else(|| committed.lines().count().min(text.lines().count()));
    println!(
        "{} is out of date: line {} is not what rustc_lexer says now.\n\
         Run `rust-fit record` and commit the result.",
        record_path.display(),
        line + 1
    );
    Ok(false)
}
