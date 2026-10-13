//! Rewrite `engine/packages/hex/native/lemma_hex/Cargo.toml` into a self-contained
//! manifest suitable for the published Hex tarball.
//!
//! End users of the `lemma_engine` Hex package on platforms without a precompiled
//! NIF binary fall back to compiling the bundled Rust source. The bundle does not
//! contain the workspace root or sibling crates, so any workspace inheritance or
//! path dependency in `lemma_hex/Cargo.toml` would prevent compilation. The same
//! manifest is also what `mix hex.publish` compiles for verification, so a
//! mismatch between path and registry deps there causes the dep graph to carry
//! two distinct copies of `lemma-engine` and explodes at type-check time.
//!
//! This rewrite:
//!
//! 1. Inlines every `[package].X.workspace = true` from `[workspace.package].X`.
//! 2. Inlines every `[dependencies].X.workspace = true` from `[workspace.dependencies].X`.
//! 3. Converts every `[dependencies].X = { path = ... }` into a registry version
//!    pin `X = "=VERSION"` (or, if the dep table carries other keys like
//!    `features`, drops the `path` and inserts `version = "=VERSION"`).
//! 4. Copies the workspace `[profile.release]` (e.g. `overflow-checks = true`), which a
//!    standalone build would otherwise not see.
//!
//! Post-conditions: no `.workspace = true` and no `path = ...` survives.

use std::fs;
use std::path::Path;

use toml_edit::{DocumentMut, Item, Value};

const HEX_CARGO_REL: &str = "engine/packages/hex/native/lemma_hex/Cargo.toml";
const ROOT_CARGO_REL: &str = "Cargo.toml";

pub fn run(root: &Path) -> Result<(), String> {
    let hex_path = root.join(HEX_CARGO_REL);
    let root_path = root.join(ROOT_CARGO_REL);

    let hex_raw =
        fs::read_to_string(&hex_path).map_err(|e| format!("{}: {e}", hex_path.display()))?;
    let root_raw =
        fs::read_to_string(&root_path).map_err(|e| format!("{}: {e}", root_path.display()))?;

    let updated = rewrite(&hex_raw, &root_raw)?;

    fs::write(&hex_path, updated).map_err(|e| format!("{}: {e}", hex_path.display()))?;
    eprintln!("hex-standalone: rewrote {}", hex_path.display());
    Ok(())
}

fn rewrite(hex_raw: &str, root_raw: &str) -> Result<String, String> {
    let root_doc: DocumentMut = root_raw
        .parse()
        .map_err(|e| format!("workspace Cargo.toml: parse error: {e}"))?;
    let mut hex_doc: DocumentMut = hex_raw
        .parse()
        .map_err(|e| format!("hex Cargo.toml: parse error: {e}"))?;

    let workspace_version = workspace_version(&root_doc)?;

    inline_package_inheritance(&mut hex_doc, &root_doc)?;
    inline_dep_inheritance(&mut hex_doc, &root_doc)?;
    pin_path_deps_to_workspace_version(&mut hex_doc, &workspace_version);
    copy_release_profile(&mut hex_doc, &root_doc)?;

    assert_no_workspace_inheritance(&hex_doc)?;
    assert_no_path_deps(&hex_doc)?;

    Ok(hex_doc.to_string())
}

fn workspace_version(root: &DocumentMut) -> Result<String, String> {
    root.get("workspace")
        .and_then(|w| w.get("package"))
        .and_then(|p| p.get("version"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "workspace Cargo.toml: missing [workspace.package].version".to_string())
}

fn inline_package_inheritance(hex: &mut DocumentMut, root: &DocumentMut) -> Result<(), String> {
    let workspace_pkg = root
        .get("workspace")
        .and_then(|w| w.get("package"))
        .and_then(Item::as_table_like)
        .ok_or_else(|| "workspace Cargo.toml: missing [workspace.package]".to_string())?;

    let pkg = hex
        .get_mut("package")
        .and_then(Item::as_table_like_mut)
        .ok_or_else(|| "hex Cargo.toml: missing [package]".to_string())?;

    let inherited_keys: Vec<String> = pkg
        .iter()
        .filter(|(_, v)| value_is_workspace_inheritance(v))
        .map(|(k, _)| k.to_string())
        .collect();

    for key in inherited_keys {
        let workspace_value = workspace_pkg.get(&key).cloned().ok_or_else(|| {
            format!("[package].{key}.workspace = true but [workspace.package].{key} is missing")
        })?;
        pkg.insert(&key, workspace_value);
    }

    Ok(())
}

fn inline_dep_inheritance(hex: &mut DocumentMut, root: &DocumentMut) -> Result<(), String> {
    let workspace_deps = root
        .get("workspace")
        .and_then(|w| w.get("dependencies"))
        .and_then(Item::as_table_like);

    let Some(deps) = hex
        .get_mut("dependencies")
        .and_then(Item::as_table_like_mut)
    else {
        return Ok(());
    };

    let inherited_keys: Vec<String> = deps
        .iter()
        .filter(|(_, v)| value_is_workspace_inheritance(v))
        .map(|(k, _)| k.to_string())
        .collect();

    for key in inherited_keys {
        let workspace_dep = workspace_deps
            .and_then(|d| d.get(&key))
            .cloned()
            .ok_or_else(|| {
                format!(
                    "[dependencies].{key}.workspace = true but \
                     [workspace.dependencies].{key} is missing"
                )
            })?;
        deps.insert(&key, workspace_dep);
    }

    Ok(())
}

fn copy_release_profile(hex: &mut DocumentMut, root: &DocumentMut) -> Result<(), String> {
    let release = root
        .get("profile")
        .and_then(|p| p.get("release"))
        .cloned()
        .ok_or_else(|| "workspace Cargo.toml: missing [profile.release]".to_string())?;
    let profile = hex
        .entry("profile")
        .or_insert(toml_edit::table())
        .as_table_like_mut()
        .ok_or_else(|| "hex Cargo.toml: [profile] is not a table".to_string())?;
    profile.insert("release", release);
    Ok(())
}

fn pin_path_deps_to_workspace_version(hex: &mut DocumentMut, version: &str) {
    let Some(deps) = hex
        .get_mut("dependencies")
        .and_then(Item::as_table_like_mut)
    else {
        return;
    };

    let pin = format!("={version}");

    let path_only: Vec<String> = deps
        .iter()
        .filter(|(_, v)| value_is_path_only_dep(v))
        .map(|(k, _)| k.to_string())
        .collect();
    for key in path_only {
        deps.insert(&key, Item::Value(Value::from(pin.as_str())));
    }

    let path_with_extras: Vec<String> = deps
        .iter()
        .filter(|(_, v)| value_has_path_key(v))
        .map(|(k, _)| k.to_string())
        .collect();
    for key in path_with_extras {
        let table = deps
            .get_mut(&key)
            .and_then(Item::as_table_like_mut)
            .expect("BUG: filter matched key whose value is no longer a table");
        table.remove("path");
        table.insert("version", Item::Value(Value::from(pin.as_str())));
    }
}

fn assert_no_workspace_inheritance(hex: &DocumentMut) -> Result<(), String> {
    let mut offenders = Vec::new();
    if let Some(pkg) = hex.get("package").and_then(Item::as_table_like) {
        for (k, v) in pkg.iter() {
            if value_is_workspace_inheritance(v) {
                offenders.push(format!("[package].{k}.workspace"));
            }
        }
    }
    if let Some(deps) = hex.get("dependencies").and_then(Item::as_table_like) {
        for (k, v) in deps.iter() {
            if value_is_workspace_inheritance(v) {
                offenders.push(format!("[dependencies].{k}.workspace"));
            }
        }
    }
    if offenders.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "rewrite did not eliminate all workspace inheritance: {}",
            offenders.join(", ")
        ))
    }
}

fn assert_no_path_deps(hex: &DocumentMut) -> Result<(), String> {
    let Some(deps) = hex.get("dependencies").and_then(Item::as_table_like) else {
        return Ok(());
    };
    let offenders: Vec<String> = deps
        .iter()
        .filter(|(_, v)| value_has_path_key(v))
        .map(|(k, _)| format!("[dependencies].{k}"))
        .collect();
    if offenders.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "rewrite did not eliminate all path deps: {}",
            offenders.join(", ")
        ))
    }
}

fn value_is_workspace_inheritance(item: &Item) -> bool {
    let Some(t) = item.as_table_like() else {
        return false;
    };
    t.get("workspace")
        .and_then(|v| v.as_value())
        .and_then(|v| v.as_bool())
        == Some(true)
}

fn value_is_path_only_dep(item: &Item) -> bool {
    let Some(t) = item.as_table_like() else {
        return false;
    };
    t.len() == 1 && t.contains_key("path")
}

fn value_has_path_key(item: &Item) -> bool {
    let Some(t) = item.as_table_like() else {
        return false;
    };
    t.contains_key("path")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace_fixture() -> &'static str {
        r#"
[workspace]
members = ["engine"]

[workspace.package]
version = "0.8.12"
edition = "2021"
authors = ["Test <test@example.com>"]
license = "Apache-2.0"
repository = "https://github.com/lemma/lemma"

[workspace.dependencies]
serde_json = "1.0"

[profile.release]
overflow-checks = true
"#
    }

    fn hex_fixture() -> &'static str {
        r#"[package]
name = "lemma_hex"
version.workspace = true
edition.workspace = true
authors.workspace = true
license.workspace = true
repository.workspace = true
description = "Lemma engine NIF crate for Elixir/Erlang"
publish = false

[lib]
name = "lemma_hex"
path = "src/lib.rs"
crate-type = ["cdylib"]

[dependencies]
lemma-engine = { path = "../../../../" }
lemma-openapi = { path = "../../../../../openapi" }
rustler = "0.37"
rust_decimal = "1"
serde_json.workspace = true
"#
    }

    #[test]
    fn inlines_package_workspace_inheritance() {
        let out = rewrite(hex_fixture(), workspace_fixture()).unwrap();
        assert!(
            out.contains(r#"version = "0.8.12""#),
            "version inlined:\n{out}"
        );
        assert!(
            out.contains(r#"edition = "2021""#),
            "edition inlined:\n{out}"
        );
        assert!(
            out.contains("Test <test@example.com>"),
            "authors inlined:\n{out}"
        );
        assert!(out.contains("Apache-2.0"), "license inlined:\n{out}");
        assert!(
            out.contains("https://github.com/lemma/lemma"),
            "repository inlined:\n{out}"
        );
    }

    #[test]
    fn inlines_dep_workspace_inheritance() {
        let out = rewrite(hex_fixture(), workspace_fixture()).unwrap();
        assert!(
            out.contains(r#"serde_json = "1.0""#),
            "serde_json inlined to literal version:\n{out}"
        );
    }

    #[test]
    fn pins_path_deps_to_workspace_version() {
        let out = rewrite(hex_fixture(), workspace_fixture()).unwrap();
        assert!(
            out.contains(r#"lemma-engine = "=0.8.12""#),
            "lemma-engine pinned:\n{out}"
        );
        assert!(
            out.contains(r#"lemma-openapi = "=0.8.12""#),
            "lemma-openapi pinned:\n{out}"
        );
    }

    #[test]
    fn no_workspace_inheritance_or_path_deps_remain() {
        let out = rewrite(hex_fixture(), workspace_fixture()).unwrap();
        assert!(
            !out.contains(".workspace"),
            "no .workspace = true survives:\n{out}"
        );
        let parsed: DocumentMut = out.parse().expect("output is valid TOML");
        assert!(
            assert_no_path_deps(&parsed).is_ok(),
            "no path deps survive in [dependencies]:\n{out}"
        );
    }

    #[test]
    fn errors_when_workspace_dep_is_missing() {
        let workspace = r#"
[workspace.package]
version = "0.8.12"
edition = "2021"
authors = ["A"]
license = "A"
repository = "R"
"#;
        let hex = r#"[package]
name = "x"
version.workspace = true
edition.workspace = true
authors.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
serde_json.workspace = true
"#;
        let err = rewrite(hex, workspace).unwrap_err();
        assert!(err.contains("serde_json"), "missing dep is reported: {err}");
    }

    #[test]
    fn errors_when_workspace_package_field_is_missing() {
        let workspace = r#"
[workspace.package]
version = "0.8.12"
"#;
        let hex = r#"[package]
name = "x"
version.workspace = true
edition.workspace = true
"#;
        let err = rewrite(hex, workspace).unwrap_err();
        assert!(
            err.contains("edition"),
            "missing package key is reported: {err}"
        );
    }

    #[test]
    fn errors_when_workspace_version_is_missing() {
        let workspace = r#"
[workspace.package]
edition = "2021"
"#;
        let hex = hex_fixture();
        let err = rewrite(hex, workspace).unwrap_err();
        assert!(
            err.contains("version"),
            "missing version is reported: {err}"
        );
    }

    #[test]
    fn pins_path_dep_with_extra_fields() {
        let workspace = workspace_fixture();
        let hex = r#"[package]
name = "x"
version.workspace = true
edition.workspace = true
authors.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
lemma-engine = { path = "../../../../", features = ["wasm"] }
"#;
        let out = rewrite(hex, workspace).unwrap();
        assert!(!out.contains("path"), "path key dropped:\n{out}");
        assert!(
            out.contains(r#"version = "=0.8.12""#),
            "version inserted alongside features:\n{out}"
        );
        assert!(out.contains("features"), "features preserved:\n{out}");
    }

    #[test]
    fn copies_workspace_release_profile() {
        let out = rewrite(hex_fixture(), workspace_fixture()).unwrap();
        let parsed: DocumentMut = out.parse().expect("output is valid TOML");
        assert_eq!(
            parsed["profile"]["release"]["overflow-checks"].as_bool(),
            Some(true),
            "release profile copied:\n{out}"
        );
    }

    #[test]
    fn errors_when_workspace_release_profile_is_missing() {
        let workspace = r#"
[workspace.package]
version = "0.8.12"
edition = "2021"
authors = ["A"]
license = "A"
repository = "R"

[workspace.dependencies]
serde_json = "1.0"
"#;
        let err = rewrite(hex_fixture(), workspace).unwrap_err();
        assert!(
            err.contains("[profile.release]"),
            "missing release profile is reported: {err}"
        );
    }

    #[test]
    fn rewrite_is_idempotent() {
        let once = rewrite(hex_fixture(), workspace_fixture()).unwrap();
        let twice = rewrite(&once, workspace_fixture()).unwrap();
        assert_eq!(once, twice);
    }

    #[test]
    fn preserves_unrelated_keys() {
        let out = rewrite(hex_fixture(), workspace_fixture()).unwrap();
        assert!(
            out.contains(r#"description = "Lemma engine NIF crate for Elixir/Erlang""#),
            "description preserved:\n{out}"
        );
        assert!(
            out.contains("publish = false"),
            "publish flag preserved:\n{out}"
        );
        assert!(
            out.contains(r#"name = "lemma_hex""#),
            "name preserved:\n{out}"
        );
        assert!(out.contains("crate-type"), "[lib] preserved:\n{out}");
        assert!(
            out.contains(r#"rustler = "0.37""#),
            "rustler preserved:\n{out}"
        );
        assert!(
            out.contains(r#"rust_decimal = "1""#),
            "rust_decimal preserved:\n{out}"
        );
    }
}
