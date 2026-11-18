//! Embed real `.lemma` documents for the structure-preserving mutator.
//!
//! Directory list and size cap match `FIXTURE_ROOTS` / `FIXTURE_MAX_BYTES` in
//! `xtask/src/main.rs`. `engine/benches/specs/iso_countries.lemma` is about 775KiB;
//! a campaign that restarts from it spends the run inside that catalog.

use std::path::{Path, PathBuf};

const FIXTURE_MAX_BYTES: u64 = 32 * 1024;

const FIXTURE_ROOTS: &[&str] = &[
    "../tests/fixtures",
    "../documentation/examples",
    "../benches/specs",
    "../../cli/tests/integrations/examples",
];

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let mut files = Vec::new();
    for root in FIXTURE_ROOTS {
        let dir = manifest.join(root);
        println!("cargo:rerun-if-changed={}", dir.display());
        collect(&dir, &mut files);
    }
    files.sort();
    let mut rust = String::from("pub static FIXTURES: &[&str] = &[\n");
    for path in &files {
        println!("cargo:rerun-if-changed={}", path.display());
        let text = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        rust.push_str("    ");
        rust.push_str(&rust_string(&text));
        rust.push_str(",\n");
    }
    rust.push_str("];\n");
    std::fs::write(out_dir.join("fixtures.rs"), rust)
        .unwrap_or_else(|e| panic!("write fixtures.rs: {e}"));
}

fn collect(dir: &Path, files: &mut Vec<PathBuf>) {
    let entries =
        std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read dir {}: {e}", dir.display()));
    for entry in entries {
        let entry = entry.unwrap_or_else(|e| panic!("read dir entry in {}: {e}", dir.display()));
        let path = entry.path();
        let kind = entry
            .file_type()
            .unwrap_or_else(|e| panic!("file type {}: {e}", path.display()));
        if kind.is_dir() {
            collect(&path, files);
            continue;
        }
        if !kind.is_file() || path.extension().and_then(|ext| ext.to_str()) != Some("lemma") {
            continue;
        }
        let len = entry
            .metadata()
            .unwrap_or_else(|e| panic!("metadata {}: {e}", path.display()))
            .len();
        if len <= FIXTURE_MAX_BYTES {
            files.push(path);
        }
    }
}

fn rust_string(text: &str) -> String {
    let mut out = String::from("\"");
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
