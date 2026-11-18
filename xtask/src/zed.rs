//! Zed extension (`engine/lsp/editors/zed`): precommit gate, grammar `rev` pinning, and
//! release-version mirrors.
//!
//! Zed's registry loads the tree-sitter grammar from `[grammars.lemma]` in
//! `extension.toml` at a fixed git `rev`. A commit cannot name its own SHA, so `rev` is the
//! last commit that touched the grammar directory: `zed grammar-rev` writes it, and
//! [`verify_grammar_rev`] fails while the grammar at `HEAD` differs from the grammar at `rev`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use toml_edit::{value, DocumentMut};

use crate::versions_diff::git_output;

pub const ZED_EXTENSION_REL: &str = "engine/lsp/editors/zed";
pub const EXTENSION_TOML_REL: &str = "engine/lsp/editors/zed/extension.toml";
pub const EXTENSION_CARGO_REL: &str = "engine/lsp/editors/zed/Cargo.toml";
pub const GRAMMAR_REL: &str = "engine/lsp/editors/zed/grammars/tree-sitter-lemma";
pub const TREE_SITTER_JSON_REL: &str =
    "engine/lsp/editors/zed/grammars/tree-sitter-lemma/tree-sitter.json";

/// Exact tree-sitter CLI version. The committed `src/parser.c` is byte-compared against a
/// fresh `tree-sitter generate`, so every environment must run the same generator.
/// Keep `flake.nix` (nixpkgs `tree-sitter`) and workflow `TREE_SITTER_VERSION` in sync.
pub const TREE_SITTER_VERSION: &str = "0.27.1";

/// Generated files under the grammar `src/` that must match a fresh `tree-sitter generate`.
const GENERATED_FILES: &[&str] = &[
    "parser.c",
    "grammar.json",
    "node-types.json",
    "tree_sitter/parser.h",
    "tree_sitter/alloc.h",
    "tree_sitter/array.h",
];

const GRAMMAR_REV_HINT: &str =
    "commit the grammar, then run `cargo run -p xtask -- zed grammar-rev` and commit extension.toml. A rev that is not an ancestor of HEAD usually means the branch was squash- or rebase-merged; merge release pull requests with a merge commit";

fn run_checked(cmd: &mut Command, label: &str) -> Result<(), String> {
    let output = cmd
        .output()
        .map_err(|e| format!("failed to run {label}: {e}"))?;
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    print!("{combined}");
    if !output.status.success() {
        return Err(format!(
            "{label} exited with status {:?}",
            output.status.code()
        ));
    }
    crate::warnings::reject_warnings_in_output(label, &combined)
}

fn require_tree_sitter() -> Result<(), String> {
    let output = Command::new("tree-sitter")
        .arg("--version")
        .output()
        .map_err(|e| {
            format!("tree-sitter not found on PATH ({e}). Use `nix develop` (tree-sitter {TREE_SITTER_VERSION})")
        })?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout
        .lines()
        .next()
        .expect("tree-sitter --version printed no output")
        .trim();
    let expected_prefix = format!("tree-sitter {TREE_SITTER_VERSION}");
    let matches = line == expected_prefix || line.starts_with(&format!("{expected_prefix} "));
    if !output.status.success() || !matches {
        return Err(format!(
            "tree-sitter version mismatch: got {line:?}, expected {expected_prefix:?}"
        ));
    }
    Ok(())
}

/// Regenerate the parser into a scratch directory and require the committed `src/` to match.
fn check_generated_parser(grammar_dir: &Path) -> Result<(), String> {
    let scratch = std::env::temp_dir().join(format!("lemma-zed-generate-{}", std::process::id()));
    if scratch.exists() {
        fs::remove_dir_all(&scratch).map_err(|e| format!("{}: {e}", scratch.display()))?;
    }
    fs::create_dir_all(&scratch).map_err(|e| format!("{}: {e}", scratch.display()))?;
    let scratch_arg = scratch
        .to_str()
        .expect("BUG: temp_dir path must be valid UTF-8");
    let generated = run_checked(
        Command::new("tree-sitter")
            .args(["generate", "--output", scratch_arg])
            .current_dir(grammar_dir),
        "tree-sitter generate",
    );
    let compared = generated.and_then(|()| {
        let mut stale = Vec::new();
        for rel in GENERATED_FILES {
            let committed = grammar_dir.join("src").join(rel);
            let fresh = scratch.join(rel);
            let fresh_bytes = fs::read(&fresh).map_err(|e| format!("{}: {e}", fresh.display()))?;
            match fs::read(&committed) {
                Ok(bytes) if bytes == fresh_bytes => {}
                Ok(_) | Err(_) => stale.push(committed.display().to_string()),
            }
        }
        if stale.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "generated grammar sources are stale: {}. Run `tree-sitter generate` in {}",
                stale.join(", "),
                grammar_dir.display()
            ))
        }
    });
    fs::remove_dir_all(&scratch).map_err(|e| format!("{}: {e}", scratch.display()))?;
    compared
}

/// Every `.lemma` file under `roots` must parse without ERROR or MISSING nodes.
fn parse_fixtures(root: &Path, grammar_dir: &Path, roots: &[&str]) -> Result<(), String> {
    let mut files: Vec<PathBuf> = Vec::new();
    let mut stack: Vec<PathBuf> = roots.iter().map(|r| root.join(r)).collect();
    while let Some(dir) = stack.pop() {
        let entries = fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for entry in entries {
            let path = entry.map_err(|e| format!("{}: {e}", dir.display()))?.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "lemma") {
                files.push(path);
            }
        }
    }
    files.sort();
    if files.is_empty() {
        panic!("BUG: fixture roots {roots:?} must contain .lemma files");
    }
    let list = std::env::temp_dir().join(format!("lemma-zed-parse-{}.txt", std::process::id()));
    let listing: String = files.iter().map(|p| format!("{}\n", p.display())).collect();
    fs::write(&list, listing).map_err(|e| format!("{}: {e}", list.display()))?;
    let list_arg = list
        .to_str()
        .expect("BUG: temp_dir path must be valid UTF-8");
    let grammar_arg = grammar_dir
        .to_str()
        .expect("BUG: workspace path must be valid UTF-8");
    let output = Command::new("tree-sitter")
        .args([
            "parse",
            "--quiet",
            "--grammar-path",
            grammar_arg,
            "--paths",
            list_arg,
        ])
        .current_dir(root)
        .output()
        .map_err(|e| format!("failed to run tree-sitter parse: {e}"));
    fs::remove_file(&list).map_err(|e| format!("{}: {e}", list.display()))?;
    let output = output?;
    if !output.status.success() {
        return Err(format!(
            "tree-sitter parse found ERROR/MISSING nodes in fixtures:\n{}",
            String::from_utf8_lossy(&output.stdout)
        ));
    }
    eprintln!("xtask: zed grammar parsed {} fixture files", files.len());
    Ok(())
}

fn cargo_in_extension(ext_dir: &Path, args: &[&str]) -> Result<(), String> {
    let manifest = ext_dir.join("Cargo.toml");
    let manifest_arg = manifest
        .to_str()
        .expect("BUG: workspace path must be valid UTF-8");
    let mut full: Vec<&str> = vec![args[0], "--manifest-path", manifest_arg];
    full.extend_from_slice(&args[1..]);
    let label = format!("cargo {}", args.join(" "));
    eprintln!("xtask: zed {label}");
    run_checked(
        Command::new(crate::cargo_bin())
            .args(&full)
            .current_dir(ext_dir),
        &label,
    )
}

/// `cargo precommit` step: grammar generate/test/parse, extension fmt/clippy/test/build.
pub fn precommit(root: &Path, fixture_roots: &[&str]) -> Result<(), String> {
    require_tree_sitter()?;
    let grammar_dir = root.join(GRAMMAR_REL);
    let ext_dir = root.join(ZED_EXTENSION_REL);

    eprintln!("xtask: zed tree-sitter generate (compare with committed src/)");
    check_generated_parser(&grammar_dir)?;
    eprintln!("xtask: zed tree-sitter test");
    run_checked(
        Command::new("tree-sitter")
            .arg("test")
            .current_dir(&grammar_dir),
        "tree-sitter test",
    )?;
    parse_fixtures(root, &grammar_dir, fixture_roots)?;

    cargo_in_extension(&ext_dir, &["fmt", "--", "--check"])?;
    cargo_in_extension(
        &ext_dir,
        &[
            "clippy",
            "--locked",
            "--target",
            "wasm32-wasip2",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ],
    )?;
    cargo_in_extension(&ext_dir, &["nextest", "run", "--locked"])?;
    cargo_in_extension(
        &ext_dir,
        &[
            "build",
            "--locked",
            "--release",
            "--target",
            "wasm32-wasip2",
        ],
    )?;
    Ok(())
}

fn read_doc(path: &Path) -> Result<DocumentMut, String> {
    let raw = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    raw.parse()
        .map_err(|e| format!("{}: parse error: {e}", path.display()))
}

fn grammar_entry_str(doc: &DocumentMut, key: &str) -> Result<String, String> {
    doc.get("grammars")
        .and_then(|g| g.get("lemma"))
        .and_then(|g| g.get(key))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| format!("{EXTENSION_TOML_REL}: missing [grammars.lemma] {key}"))
}

/// `[grammars.lemma]` must point at a commit on `HEAD`'s history whose grammar equals `HEAD`'s.
pub fn verify_grammar_rev(root: &Path) -> Result<(), String> {
    let doc = read_doc(&root.join(EXTENSION_TOML_REL))?;
    let path = grammar_entry_str(&doc, "path")?;
    if path != GRAMMAR_REL {
        return Err(format!(
            "{EXTENSION_TOML_REL}: [grammars.lemma] path is {path:?}, expected {GRAMMAR_REL:?}"
        ));
    }
    let rev = grammar_entry_str(&doc, "rev")?;
    git_output(&["cat-file", "-e", &format!("{rev}^{{commit}}")], root).map_err(|e| {
        format!("{EXTENSION_TOML_REL}: grammar rev {rev} is not a known commit ({e}). {GRAMMAR_REV_HINT}")
    })?;
    git_output(&["merge-base", "--is-ancestor", &rev, "HEAD"], root).map_err(|_| {
        format!("{EXTENSION_TOML_REL}: grammar rev {rev} is not an ancestor of HEAD. {GRAMMAR_REV_HINT}")
    })?;
    // Zed's registry builds the grammar from `rev`: it must contain the parser sources.
    for file in ["grammar.js", "src/parser.c"] {
        git_output(
            &["cat-file", "-e", &format!("{rev}:{GRAMMAR_REL}/{file}")],
            root,
        )
        .map_err(|_| {
            format!("{EXTENSION_TOML_REL}: grammar rev {rev} has no {GRAMMAR_REL}/{file}. {GRAMMAR_REV_HINT}")
        })?;
    }
    let changed = git_output(
        &["diff", "--name-only", &rev, "HEAD", "--", GRAMMAR_REL],
        root,
    )?;
    if !changed.is_empty() {
        return Err(format!(
            "{EXTENSION_TOML_REL}: grammar changed since rev {rev}:\n{}{GRAMMAR_REV_HINT}",
            String::from_utf8_lossy(&changed)
        ));
    }
    Ok(())
}

/// `zed grammar-rev`: set `[grammars.lemma] rev` to the last commit that touched the grammar.
pub fn set_grammar_rev(root: &Path) -> Result<(), String> {
    let dirty = git_output(&["status", "--porcelain", "--", GRAMMAR_REL], root)?;
    if !dirty.is_empty() {
        return Err(format!(
            "{GRAMMAR_REL} has uncommitted changes; commit them first:\n{}",
            String::from_utf8_lossy(&dirty)
        ));
    }
    let out = git_output(&["log", "-1", "--format=%H", "--", GRAMMAR_REL], root)?;
    let rev = String::from_utf8(out)
        .expect("BUG: git log --format=%H prints ASCII")
        .trim()
        .to_string();
    if rev.is_empty() {
        return Err(format!("{GRAMMAR_REL} has no commits yet; commit it first"));
    }
    let path = root.join(EXTENSION_TOML_REL);
    let mut doc = read_doc(&path)?;
    doc["grammars"]["lemma"]["rev"] = value(rev.as_str());
    fs::write(&path, doc.to_string()).map_err(|e| format!("{}: {e}", path.display()))?;
    eprintln!("zed grammar-rev: set {EXTENSION_TOML_REL} [grammars.lemma] rev = {rev}");
    Ok(())
}

/// Top-level `version` (extension.toml) or `[package] version` (Cargo.toml).
fn manifest_version(doc: &DocumentMut, package_table: bool) -> Option<&str> {
    if package_table {
        doc.get("package")?.get("version")?.as_str()
    } else {
        doc.get("version")?.as_str()
    }
}

const VERSIONED_TOMLS: &[(&str, bool)] =
    &[(EXTENSION_TOML_REL, false), (EXTENSION_CARGO_REL, true)];

pub fn bump_versions(root: &Path, old: &str, new: &str) -> Result<(), String> {
    for (rel, package_table) in VERSIONED_TOMLS {
        let path = root.join(rel);
        let mut doc = read_doc(&path)?;
        if manifest_version(&doc, *package_table) != Some(old) {
            return Err(format!("{}: expected version {old:?}", path.display()));
        }
        if *package_table {
            doc["package"]["version"] = value(new);
        } else {
            doc["version"] = value(new);
        }
        fs::write(&path, doc.to_string()).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(())
}

pub fn verify_versions(root: &Path, v: &str, errs: &mut Vec<String>) {
    for (rel, package_table) in VERSIONED_TOMLS {
        let path = root.join(rel);
        match read_doc(&path) {
            Ok(doc) => match manifest_version(&doc, *package_table) {
                Some(found) if found == v => {}
                found => errs.push(format!(
                    "{}: version is {found:?}, expected {v:?}",
                    path.display()
                )),
            },
            Err(e) => errs.push(e),
        }
    }
}

pub fn run(root: &Path, rest: &[String], fixture_roots: &[&str]) -> Result<(), String> {
    match rest.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["check"] => precommit(root, fixture_roots),
        ["grammar-rev"] => set_grammar_rev(root),
        ["verify-grammar-rev"] => verify_grammar_rev(root),
        other => Err(format!(
            "usage: cargo run -p xtask -- zed <check|grammar-rev|verify-grammar-rev> (got {other:?})"
        )),
    }
}
