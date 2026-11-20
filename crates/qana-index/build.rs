//! The fingerprint of the code that summarizes a file, behind
//! `summary_key`.
//!
//! A kept summary is good exactly while its file's text, its grammar's
//! artifact and the code that summarized it are the same. The artifact's
//! key already names the grammar's text and the crates that certify and
//! parse it (qana-lang's `build.rs`); this names this crate: its sources
//! and manifest, hashed, so that an edit which bumps no version still
//! changes it, and its version.

use std::path::{Path, PathBuf};
use std::{env, fs};

fn main() {
    let here =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"));
    let mut hash = Fnv::new();
    hash.chunk(env::var("CARGO_PKG_VERSION").expect("cargo sets CARGO_PKG_VERSION").as_bytes());
    for part in ["Cargo.toml", "src"] {
        let path = here.join(part);
        println!("cargo:rerun-if-changed={}", path.display());
        hash_tree(&path, Path::new(part), &mut hash);
    }
    let out = PathBuf::from(env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("summarizer.rs");
    let source = format!(
        "/// The fingerprint of the code that summarized this build's summaries\n\
         /// (see `build.rs`).\n\
         const SUMMARIZER: u64 = {:#018x};\n",
        hash.finish()
    );
    fs::write(&out, source).expect("the fingerprint is written to OUT_DIR");
}

/// Feed the file or directory at `path`, named `name` within the crate, to
/// `hash`: a file's name and contents, a directory's entries in name order.
fn hash_tree(path: &Path, name: &Path, hash: &mut Fnv) {
    let name_text: Vec<String> =
        name.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    if path.is_file() {
        hash.chunk(name_text.join("/").as_bytes());
        hash.chunk(&fs::read(path).expect("the crate's source is readable"));
    } else if path.is_dir() {
        let mut entries: Vec<_> = fs::read_dir(path)
            .expect("the crate's source directory is readable")
            .map(|entry| entry.expect("a directory entry is readable").file_name())
            .collect();
        entries.sort();
        for entry in entries {
            hash_tree(&path.join(&entry), &name.join(&entry), hash);
        }
    }
}

/// 64-bit FNV-1a: fixed, so that a fingerprint does not depend on the
/// compiler that computed it.
struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }

    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 ^= u64::from(byte);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    /// Feed `bytes` as one delimited piece: its length first.
    fn chunk(&mut self, bytes: &[u8]) {
        self.write(&(bytes.len() as u64).to_le_bytes());
        self.write(bytes);
    }

    fn finish(&self) -> u64 {
        self.0
    }
}
