//! Bump and verify the workspace release version across Rust, Elixir, Maven, docs, and VS Code.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Paths and expectations shared by [`versions_bump`] and [`versions_verify`].
mod tracked {
    pub const WORKSPACE_CARGO: &str = "Cargo.toml";

    pub const PATH_DEP_MANIFESTS: &[&str] = &[
        "cli/Cargo.toml",
        "openapi/Cargo.toml",
        "engine/lsp/Cargo.toml",
    ];

    pub const HEX_MIX: &str = "engine/packages/hex/mix.exs";
    pub const MAVEN_POM: &str = "engine/packages/maven/pom.xml";
    pub const NUGET_CSPROJ: &str =
        "engine/packages/nuget/Lemmabase.Lemma.Engine/Lemmabase.Lemma.Engine.csproj";
    pub const NUGET_ENGINE_VERSION: &str =
        "engine/packages/nuget/Lemmabase.Lemma.Engine/engine.version";
    pub const ENGINE_README: &str = "engine/README.md";
    pub const VSCODE_PACKAGE_JSON: &str = "engine/lsp/editors/vscode/package.json";
    pub const QUALITY_YML: &str = ".github/workflows/quality.yml";
    pub const RELEASE_YML: &str = ".github/workflows/release.yml";

    /// Doc/README snippets that embed the Maven artifact version (XML and/or Gradle).
    pub const MAVEN_VERSION_DOCS: &[&str] = &[
        "README.md",
        "engine/README.md",
        "cli/documentation/tools/java.md",
        "engine/packages/maven/README.md",
    ];

    /// Doc snippets that embed `dotnet add package Lemmabase.Lemma.Engine --version {v}`.
    pub const NUGET_VERSION_DOCS: &[&str] = &[
        "README.md",
        "engine/README.md",
        "cli/documentation/tools/dotnet.md",
        "engine/packages/nuget/Lemmabase.Lemma.Engine/README.md",
    ];
}

/// Exact wasm-pack version required by precommit / CI. Keep workflow `WASM_PACK_VERSION` in sync.
pub const WASM_PACK_VERSION: &str = "0.15.0";

/// Exact uniffi crate version for lemma_dotnet. Keep with `UNIFFI_BINDGEN_CS_TAG` in nuget_natives.
pub const UNIFFI_VERSION: &str = "0.31.0";

fn dep_pin_needle(v: &str) -> String {
    format!(r#"version = "={v}""#)
}

fn mix_needle(v: &str) -> String {
    format!(r#"@version "{v}""#)
}

fn readme_needle(v: &str) -> String {
    format!(r#"lemma-engine = "{v}""#)
}

fn pom_project_version_block(v: &str) -> String {
    format!("<artifactId>lemma-engine</artifactId>\n  <version>{v}</version>")
}

fn maven_gradle_coord(v: &str) -> String {
    format!("lemma-engine:{v}")
}

fn replace_maven_doc_versions(content: &str, old: &str, new: &str) -> Result<String, String> {
    let from_xml = pom_project_version_block(old);
    let to_xml = pom_project_version_block(new);
    let from_gradle = maven_gradle_coord(old);
    let to_gradle = maven_gradle_coord(new);
    if !content.contains(&from_xml) && !content.contains(&from_gradle) {
        return Err(format!("expected `{from_xml}` and/or `{from_gradle}`"));
    }
    Ok(content
        .replace(&from_xml, &to_xml)
        .replace(&from_gradle, &to_gradle))
}

fn verify_maven_doc_versions(content: &str, v: &str) -> Result<(), String> {
    let xml = pom_project_version_block(v);
    let gradle = maven_gradle_coord(v);
    if content.contains(&xml) || content.contains(&gradle) {
        Ok(())
    } else {
        Err(format!("expected `{xml}` and/or `{gradle}`"))
    }
}

fn nuget_package_needle(v: &str) -> String {
    format!("Lemmabase.Lemma.Engine --version {v}")
}

fn nuget_csproj_version_line(v: &str) -> String {
    format!("<Version>{v}</Version>")
}

fn replace_nuget_doc_versions(content: &str, old: &str, new: &str) -> Result<String, String> {
    let from = nuget_package_needle(old);
    let to = nuget_package_needle(new);
    if !content.contains(&from) {
        return Err(format!("expected `{from}`"));
    }
    Ok(content.replace(&from, &to))
}

fn verify_nuget_doc_versions(content: &str, v: &str) -> Result<(), String> {
    let needle = nuget_package_needle(v);
    if content.contains(&needle) {
        Ok(())
    } else {
        Err(format!("expected `{needle}`"))
    }
}

fn replace_nuget_csproj_version(content: &str, old: &str, new: &str) -> Result<String, String> {
    let from = nuget_csproj_version_line(old);
    let to = nuget_csproj_version_line(new);
    if !content.contains(&from) {
        return Err(format!("expected `{from}`"));
    }
    Ok(content.replacen(&from, &to, 1))
}

/// Paths relative to workspace root that must carry the same release version as `[workspace.package]`.
pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask crate must live under workspace root")
        .to_path_buf()
}

pub fn read_workspace_version(root: &Path) -> Result<String, String> {
    let cargo = root.join(tracked::WORKSPACE_CARGO);
    let content = fs::read_to_string(&cargo).map_err(|e| format!("{}: {e}", cargo.display()))?;
    parse_workspace_version(&content)
}

fn parse_workspace_version(content: &str) -> Result<String, String> {
    let mut in_workspace_package = false;
    for line in content.lines() {
        let t = line.trim();
        if t == "[workspace.package]" {
            in_workspace_package = true;
            continue;
        }
        if t.starts_with('[') && t.ends_with(']') && in_workspace_package {
            break;
        }
        if !in_workspace_package {
            continue;
        }
        if let Some(rest) = t.strip_prefix("version") {
            let rest = rest.trim_start();
            if let Some(rest) = rest.strip_prefix('=') {
                let rest = rest.trim();
                if let Some(inner) = rest.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
                    return Ok(inner.to_string());
                }
            }
        }
    }
    Err("no version in [workspace.package] in Cargo.toml".into())
}

fn replace_workspace_package_version(
    content: &str,
    old: &str,
    new: &str,
) -> Result<String, String> {
    let old_line = format!(r#"version = "{old}""#);
    let new_line = format!(r#"version = "{new}""#);
    let mut in_workspace_package = false;
    let mut replaced = false;
    let mut out = String::new();
    for line in content.lines() {
        let t = line.trim();
        if t == "[workspace.package]" {
            in_workspace_package = true;
        } else if t.starts_with('[') && t.ends_with(']') && in_workspace_package {
            in_workspace_package = false;
        }
        if in_workspace_package && line.trim() == old_line {
            out.push_str(&new_line);
            replaced = true;
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    if !replaced {
        return Err(format!(
            "root Cargo.toml: expected `{old_line}` inside [workspace.package]"
        ));
    }
    Ok(out)
}

fn replace_dep_pins(content: &str, old: &str, new: &str) -> String {
    let from = dep_pin_needle(old);
    let to = dep_pin_needle(new);
    content.replace(&from, &to)
}

fn replace_mix_version(content: &str, old: &str, new: &str) -> String {
    let from = mix_needle(old);
    let to = mix_needle(new);
    content.replace(&from, &to)
}

fn replace_readme_engine_line(content: &str, old: &str, new: &str) -> String {
    let from = readme_needle(old);
    let to = readme_needle(new);
    content.replace(&from, &to)
}

fn replace_pom_project_version(content: &str, old: &str, new: &str) -> Result<String, String> {
    let from = pom_project_version_block(old);
    let to = pom_project_version_block(new);
    if !content.contains(&from) {
        return Err(format!("expected `{from}`"));
    }
    Ok(content.replacen(&from, &to, 1))
}

fn bump_package_json_version(path: &Path, old: &str, new: &str) -> Result<(), String> {
    let raw = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let from = format!(r#""version": "{old}""#);
    if !raw.contains(&from) {
        return Err(format!(
            "{}: expected top-level `\"version\": \"{old}\"`",
            path.display()
        ));
    }
    let to = format!(r#""version": "{new}""#);
    let updated = raw.replacen(&from, &to, 1);
    fs::write(path, updated).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(())
}

/// `cargo bump <new>` — set workspace release to `new` everywhere and refresh lockfile metadata.
pub fn versions_bump(root: &Path, new: &str) -> Result<(), String> {
    semver::Version::parse(new).map_err(|e| format!("invalid semver {new:?}: {e}"))?;
    let old = read_workspace_version(root)?;
    if old == new {
        return Err(format!("already at version {new}"));
    }

    let root_cargo = root.join(tracked::WORKSPACE_CARGO);
    let raw =
        fs::read_to_string(&root_cargo).map_err(|e| format!("{}: {e}", root_cargo.display()))?;
    let updated = replace_workspace_package_version(&raw, &old, new)?;
    fs::write(&root_cargo, updated).map_err(|e| format!("{}: {e}", root_cargo.display()))?;

    for rel in tracked::PATH_DEP_MANIFESTS {
        let p = root.join(rel);
        let c = fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        let c2 = replace_dep_pins(&c, &old, new);
        if c2 == c {
            return Err(format!(
                "{}: expected `{}` dependency pins",
                p.display(),
                dep_pin_needle(&old)
            ));
        }
        fs::write(&p, c2).map_err(|e| format!("{}: {e}", p.display()))?;
    }

    let mix = root.join(tracked::HEX_MIX);
    let mix_raw = fs::read_to_string(&mix).map_err(|e| format!("{}: {e}", mix.display()))?;
    let mix2 = replace_mix_version(&mix_raw, &old, new);
    if mix2 == mix_raw {
        return Err(format!(
            "{}: expected `{}`",
            mix.display(),
            mix_needle(&old)
        ));
    }
    fs::write(&mix, mix2).map_err(|e| format!("{}: {e}", mix.display()))?;

    let pom = root.join(tracked::MAVEN_POM);
    let pom_raw = fs::read_to_string(&pom).map_err(|e| format!("{}: {e}", pom.display()))?;
    let pom2 = replace_pom_project_version(&pom_raw, &old, new)
        .map_err(|e| format!("{}: {e}", pom.display()))?;
    fs::write(&pom, pom2).map_err(|e| format!("{}: {e}", pom.display()))?;

    let csproj = root.join(tracked::NUGET_CSPROJ);
    let csproj_raw =
        fs::read_to_string(&csproj).map_err(|e| format!("{}: {e}", csproj.display()))?;
    let csproj2 = replace_nuget_csproj_version(&csproj_raw, &old, new)
        .map_err(|e| format!("{}: {e}", csproj.display()))?;
    fs::write(&csproj, csproj2).map_err(|e| format!("{}: {e}", csproj.display()))?;

    let engine_version = root.join(tracked::NUGET_ENGINE_VERSION);
    fs::write(&engine_version, format!("{new}\n"))
        .map_err(|e| format!("{}: {e}", engine_version.display()))?;

    for rel in tracked::MAVEN_VERSION_DOCS {
        let p = root.join(rel);
        let raw = fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        let updated = replace_maven_doc_versions(&raw, &old, new)
            .map_err(|e| format!("{}: {e}", p.display()))?;
        fs::write(&p, updated).map_err(|e| format!("{}: {e}", p.display()))?;
    }

    for rel in tracked::NUGET_VERSION_DOCS {
        let p = root.join(rel);
        let raw = fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        let updated = replace_nuget_doc_versions(&raw, &old, new)
            .map_err(|e| format!("{}: {e}", p.display()))?;
        fs::write(&p, updated).map_err(|e| format!("{}: {e}", p.display()))?;
    }

    let readme = root.join(tracked::ENGINE_README);
    let rm = fs::read_to_string(&readme).map_err(|e| format!("{}: {e}", readme.display()))?;
    let rm2 = replace_readme_engine_line(&rm, &old, new);
    if rm2 == rm {
        return Err(format!(
            "{}: expected `{}`",
            readme.display(),
            readme_needle(&old)
        ));
    }
    fs::write(&readme, rm2).map_err(|e| format!("{}: {e}", readme.display()))?;

    let pkg = root.join(tracked::VSCODE_PACKAGE_JSON);
    bump_package_json_version(&pkg, &old, new)?;

    run_cargo_generate_lockfile(root)?;
    run_mix_deps_get(root)?;
    run_npm_package_lock_only(root)?;
    Ok(())
}

fn run_cargo_generate_lockfile(root: &Path) -> Result<(), String> {
    eprintln!("xtask: cargo generate-lockfile");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let st = Command::new(&cargo)
        .args(["generate-lockfile"])
        .current_dir(root)
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| format!("failed to run {cargo} generate-lockfile: {e}"))?;
    if !st.success() {
        return Err("cargo generate-lockfile failed".into());
    }
    Ok(())
}

fn run_mix_deps_get(root: &Path) -> Result<(), String> {
    eprintln!("xtask: mix deps.get (hex)");
    let hex_dir = root.join(
        Path::new(tracked::HEX_MIX)
            .parent()
            .expect("HEX_MIX must have a parent directory"),
    );
    let st = Command::new("mix")
        .args(["deps.get"])
        .current_dir(&hex_dir)
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| format!("failed to run mix deps.get in {}: {e}", hex_dir.display()))?;
    if !st.success() {
        return Err("mix deps.get failed".into());
    }
    Ok(())
}

fn run_npm_package_lock_only(root: &Path) -> Result<(), String> {
    eprintln!("xtask: npm install --package-lock-only (vscode extension)");
    let pkg_json = root.join(tracked::VSCODE_PACKAGE_JSON);
    let vscode_dir = pkg_json
        .parent()
        .expect("VSCODE_PACKAGE_JSON must have a parent directory");
    let output = Command::new("npm")
        .args(["install", "--package-lock-only"])
        .current_dir(vscode_dir)
        .output()
        .map_err(|e| format!("failed to run npm in {}: {e}", vscode_dir.display()))?;
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    print!("{combined}");
    let label = "npm install --package-lock-only";
    if !output.status.success() {
        return Err("npm install --package-lock-only failed".into());
    }
    crate::warnings::reject_warnings_in_output(label, &combined)?;
    Ok(())
}

/// `cargo verify` — ensure every tracked location matches `[workspace.package] version`.
pub fn versions_verify(root: &Path) -> Result<(), String> {
    let v = read_workspace_version(root)?;
    let mut errs: Vec<String> = Vec::new();

    let root_cargo = root.join(tracked::WORKSPACE_CARGO);
    match fs::read_to_string(&root_cargo) {
        Ok(s) => {
            if parse_workspace_version(&s).as_ref() != Ok(&v) {
                errs.push(format!(
                    "{}: [workspace.package] version does not match canonical {v}",
                    root_cargo.display()
                ));
            }
        }
        Err(e) => errs.push(format!("{}: {e}", root_cargo.display())),
    }

    for rel in tracked::PATH_DEP_MANIFESTS {
        let p = root.join(rel);
        match fs::read_to_string(&p) {
            Ok(s) => {
                let needle = dep_pin_needle(&v);
                if !s.contains(&needle) {
                    errs.push(format!(
                        "{}: expected path deps to contain `{needle}`",
                        p.display()
                    ));
                }
            }
            Err(e) => errs.push(format!("{}: {e}", p.display())),
        }
    }

    let mix = root.join(tracked::HEX_MIX);
    match fs::read_to_string(&mix) {
        Ok(s) => {
            let needle = mix_needle(&v);
            if !s.contains(&needle) {
                errs.push(format!("{}: expected `{needle}`", mix.display()));
            }
        }
        Err(e) => errs.push(format!("{}: {e}", mix.display())),
    }

    let pom = root.join(tracked::MAVEN_POM);
    match fs::read_to_string(&pom) {
        Ok(s) => {
            let needle = pom_project_version_block(&v);
            if !s.contains(&needle) {
                errs.push(format!("{}: expected `{needle}`", pom.display()));
            }
        }
        Err(e) => errs.push(format!("{}: {e}", pom.display())),
    }

    let csproj = root.join(tracked::NUGET_CSPROJ);
    match fs::read_to_string(&csproj) {
        Ok(s) => {
            let needle = nuget_csproj_version_line(&v);
            if !s.contains(&needle) {
                errs.push(format!("{}: expected `{needle}`", csproj.display()));
            }
        }
        Err(e) => errs.push(format!("{}: {e}", csproj.display())),
    }

    let engine_version = root.join(tracked::NUGET_ENGINE_VERSION);
    match fs::read_to_string(&engine_version) {
        Ok(s) => {
            if s.trim() != v {
                errs.push(format!(
                    "{}: expected `{v}`, got {:?}",
                    engine_version.display(),
                    s.trim()
                ));
            }
        }
        Err(e) => errs.push(format!("{}: {e}", engine_version.display())),
    }

    for rel in tracked::MAVEN_VERSION_DOCS {
        let p = root.join(rel);
        match fs::read_to_string(&p) {
            Ok(s) => {
                if let Err(e) = verify_maven_doc_versions(&s, &v) {
                    errs.push(format!("{}: {e}", p.display()));
                }
            }
            Err(e) => errs.push(format!("{}: {e}", p.display())),
        }
    }

    for rel in tracked::NUGET_VERSION_DOCS {
        let p = root.join(rel);
        match fs::read_to_string(&p) {
            Ok(s) => {
                if let Err(e) = verify_nuget_doc_versions(&s, &v) {
                    errs.push(format!("{}: {e}", p.display()));
                }
            }
            Err(e) => errs.push(format!("{}: {e}", p.display())),
        }
    }

    let readme = root.join(tracked::ENGINE_README);
    match fs::read_to_string(&readme) {
        Ok(s) => {
            let needle = readme_needle(&v);
            if !s.contains(&needle) {
                errs.push(format!(
                    "{}: expected `{needle}` in Quick start example",
                    readme.display()
                ));
            }
        }
        Err(e) => errs.push(format!("{}: {e}", readme.display())),
    }

    let pkg = root.join(tracked::VSCODE_PACKAGE_JSON);
    match fs::read_to_string(&pkg) {
        Ok(s) => match serde_json::from_str::<serde_json::Value>(&s) {
            Ok(j) => match j.get("version").and_then(|x| x.as_str()) {
                Some(pv) if pv == v => {}
                Some(pv) => errs.push(format!(
                    "{}: version is {pv:?}, expected {v:?}",
                    pkg.display()
                )),
                None => errs.push(format!("{}: missing top-level \"version\"", pkg.display())),
            },
            Err(e) => errs.push(format!("{}: {e}", pkg.display())),
        },
        Err(e) => errs.push(format!("{}: {e}", pkg.display())),
    }

    let wasm_pack_needle = format!("WASM_PACK_VERSION: {WASM_PACK_VERSION}");
    for rel in [tracked::QUALITY_YML, tracked::RELEASE_YML] {
        let p = root.join(rel);
        match fs::read_to_string(&p) {
            Ok(s) => {
                if !s.contains(&wasm_pack_needle) {
                    errs.push(format!(
                        "{}: expected `{wasm_pack_needle}` (must match xtask WASM_PACK_VERSION)",
                        p.display()
                    ));
                }
            }
            Err(e) => errs.push(format!("{}: {e}", p.display())),
        }
    }

    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_workspace_version_finds_package_version() {
        let t = r#"
[workspace]
members = []

[workspace.package]
version = "0.8.4"
edition = "2021"
"#;
        assert_eq!(parse_workspace_version(t).unwrap(), "0.8.4");
    }

    #[test]
    fn replace_workspace_package_version_replaces() {
        let t = r#"[workspace.package]
version = "0.8.4"
"#;
        let out = replace_workspace_package_version(t, "0.8.4", "0.8.5").unwrap();
        assert!(out.contains("version = \"0.8.5\""));
        assert!(!out.contains("0.8.4"));
    }

    #[test]
    fn replace_dep_pins_replaces() {
        let s = r#"lemma = { version = "=0.8.4", path = ".." }"#;
        let out = replace_dep_pins(s, "0.8.4", "0.8.5");
        assert!(out.contains("=0.8.5"));
    }

    #[test]
    fn replace_pom_project_version_replaces_once() {
        let t = r#"  <artifactId>lemma-engine</artifactId>
  <version>0.9.0</version>
  <dependency>
    <version>2.18.2</version>
  </dependency>
"#;
        let out = replace_pom_project_version(t, "0.9.0", "0.9.1").unwrap();
        assert!(out.contains("<version>0.9.1</version>"));
        assert!(out.contains("<version>2.18.2</version>"));
        assert!(!out.contains("lemma-engine</artifactId>\n  <version>0.9.0</version>"));
    }

    #[test]
    fn replace_maven_doc_versions_updates_xml_and_gradle() {
        let t = r#"  <artifactId>lemma-engine</artifactId>
  <version>0.9.0</version>
implementation("com.lemmabase:lemma-engine:0.9.0")
"#;
        let out = replace_maven_doc_versions(t, "0.9.0", "0.9.1").unwrap();
        assert!(out.contains("<version>0.9.1</version>"));
        assert!(out.contains("lemma-engine:0.9.1"));
        assert!(!out.contains("0.9.0"));
    }
}
