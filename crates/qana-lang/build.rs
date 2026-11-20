//! The fingerprint of the code that certifies a grammar, behind
//! `GrammarArtifact::key`.
//!
//! A kept artifact is good exactly while its grammar's text and the code
//! that certified it are the same. That code is this crate and the qana
//! crates it certifies with. Built inside qana's workspace (a checkout, or
//! one patched into a host as a sibling), they are hashed from their
//! sources, so that an edit which bumps no version still changes the
//! fingerprint. Built from a registry, where their versions are exact
//! (`0.0.x`), this crate's version names all of them; its own sources are
//! hashed there too.

use std::path::{Path, PathBuf};
use std::{env, fs};

/// The crates that make an artifact beside this one: directories beside it
/// in qana's workspace.
const CERTIFIERS: [&str; 5] =
    ["qana-grammar", "qana-engine", "qana-sem", "qana-services", "linework"];

fn main() {
    let here =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"));
    let mut hash = Fnv::new();
    hash.chunk(env::var("CARGO_PKG_VERSION").expect("cargo sets CARGO_PKG_VERSION").as_bytes());

    let mut crates = vec![("qana-lang", here.clone())];
    for name in CERTIFIERS {
        let sibling = here.join("..").join(name);
        if sibling.join("Cargo.toml").is_file() {
            crates.push((name, sibling));
        }
    }
    for (name, dir) in &crates {
        hash.chunk(name.as_bytes());
        for part in ["Cargo.toml", "src"] {
            let path = dir.join(part);
            println!("cargo:rerun-if-changed={}", path.display());
            hash_tree(&path, Path::new(part), &mut hash);
        }
    }

    let out = PathBuf::from(env::var("OUT_DIR").expect("cargo sets OUT_DIR"))
        .join("certifier_fingerprint.rs");
    let source = format!(
        "/// The fingerprint of the code that certified this build's artifacts\n\
         /// (see `build.rs`).\n\
         const CERTIFIER: u64 = {:#018x};\n",
        hash.finish()
    );
    fs::write(&out, source).expect("the fingerprint is written to OUT_DIR");
}

/// Feed the file or directory at `path`, named `name` within its crate, to
/// `hash`: a file's name and contents, a directory's entries in name order.
fn hash_tree(path: &Path, name: &Path, hash: &mut Fnv) {
    let name_text: Vec<String> =
        name.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    if path.is_file() {
        hash.chunk(name_text.join("/").as_bytes());
        hash.chunk(&fs::read(path).expect("a certifying crate's source is readable"));
    } else if path.is_dir() {
        let mut entries: Vec<_> = fs::read_dir(path)
            .expect("a certifying crate's source directory is readable")
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

    /// Feed `bytes` as one delimited piece: its length first, so that two
    /// pieces never read as a different two.
    fn chunk(&mut self, bytes: &[u8]) {
        self.write(&(bytes.len() as u64).to_le_bytes());
        self.write(bytes);
    }

    fn finish(&self) -> u64 {
        self.0
    }
}
