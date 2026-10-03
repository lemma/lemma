//! Build host native library, generate UniFFI C#, and place under nuget package.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub use crate::versions::UNIFFI_VERSION;
pub const UNIFFI_BINDGEN_CS_TAG: &str = "v0.11.0+v0.31.0";

pub fn run(root: &Path) -> Result<(), String> {
    require_uniffi_bindgen_cs()?;
    verify_uniffi_crate_pin(root)?;

    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let target_dir = std::env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| root.join("target"));

    let triple = host_triple()?;
    let lib_name = library_name();

    eprintln!("nuget-natives: cargo build --release -p lemma_dotnet");
    let status = Command::new(&cargo)
        .args(["build", "--release", "-p", "lemma_dotnet"])
        .status()
        .map_err(|e| format!("failed to run cargo build: {e}"))?;
    if !status.success() {
        return Err("cargo build --release -p lemma_dotnet failed".to_string());
    }

    let built = target_dir.join("release").join(&lib_name);
    if !built.is_file() {
        return Err(format!(
            "cargo build --release succeeded but {} not found",
            built.display()
        ));
    }

    let nuget_pkg = root.join("engine/packages/nuget/Lemmabase.Lemma.Engine");
    let natives_dir = nuget_pkg.join("natives").join(&triple);
    fs::create_dir_all(&natives_dir)
        .map_err(|e| format!("failed to create {}: {e}", natives_dir.display()))?;
    let dest = natives_dir.join(&lib_name);
    fs::copy(&built, &dest).map_err(|e| {
        format!(
            "failed to copy {} to {}: {e}",
            built.display(),
            dest.display()
        )
    })?;
    eprintln!("nuget-natives: {} -> {}", built.display(), dest.display());

    let generated_dir = nuget_pkg.join("Generated");
    fs::create_dir_all(&generated_dir)
        .map_err(|e| format!("failed to create {}: {e}", generated_dir.display()))?;
    let config = root.join("engine/packages/nuget/native/lemma_dotnet/uniffi.toml");
    eprintln!(
        "nuget-natives: uniffi-bindgen-cs --library {}",
        built.display()
    );
    let status = Command::new("uniffi-bindgen-cs")
        .args([
            "--library",
            built.to_str().ok_or("library path is not UTF-8")?,
            "--out-dir",
            generated_dir.to_str().ok_or("generated dir is not UTF-8")?,
            "--config",
            config.to_str().ok_or("config path is not UTF-8")?,
        ])
        .status()
        .map_err(|e| format!("failed to run uniffi-bindgen-cs: {e}"))?;
    if !status.success() {
        return Err("uniffi-bindgen-cs failed".to_string());
    }

    let version = crate::versions::read_workspace_version(root)?;
    let version_file = nuget_pkg.join("engine.version");
    fs::write(&version_file, format!("{version}\n"))
        .map_err(|e| format!("failed to write {}: {e}", version_file.display()))?;

    Ok(())
}

fn verify_uniffi_crate_pin(root: &Path) -> Result<(), String> {
    let manifest = root.join("engine/packages/nuget/native/lemma_dotnet/Cargo.toml");
    let content =
        fs::read_to_string(&manifest).map_err(|e| format!("{}: {e}", manifest.display()))?;
    let needle = format!("uniffi = \"={UNIFFI_VERSION}\"");
    if !content.contains(&needle) {
        return Err(format!(
            "{}: expected `{needle}` (keep in sync with versions::UNIFFI_VERSION)",
            manifest.display()
        ));
    }
    Ok(())
}

fn require_uniffi_bindgen_cs() -> Result<(), String> {
    let output = Command::new("uniffi-bindgen-cs")
        .arg("--version")
        .output()
        .map_err(|e| {
            format!(
                "uniffi-bindgen-cs not found on PATH. Install: cargo install uniffi-bindgen-cs --git https://github.com/NordSecurity/uniffi-bindgen-cs --tag {UNIFFI_BINDGEN_CS_TAG} ({e})"
            )
        })?;
    if !output.status.success() {
        return Err(format!(
            "uniffi-bindgen-cs --version failed. Install: cargo install uniffi-bindgen-cs --git https://github.com/NordSecurity/uniffi-bindgen-cs --tag {UNIFFI_BINDGEN_CS_TAG}"
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout.lines().next().unwrap_or("").trim();
    // uniffi-bindgen-cs --version prints like "uniffi-bindgen-cs 0.11.0"
    if !line.contains("0.11.0") {
        return Err(format!(
            "uniffi-bindgen-cs version mismatch: got {line:?}, expected tag {UNIFFI_BINDGEN_CS_TAG}. Install: cargo install uniffi-bindgen-cs --git https://github.com/NordSecurity/uniffi-bindgen-cs --tag {UNIFFI_BINDGEN_CS_TAG}"
        ));
    }
    Ok(())
}

fn host_triple() -> Result<String, String> {
    let output = Command::new("rustc")
        .args(["-vV"])
        .output()
        .map_err(|e| format!("failed to run rustc: {e}"))?;
    if !output.status.success() {
        return Err("rustc -vV failed".to_string());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if let Some(host) = line.strip_prefix("host: ") {
            return Ok(host.trim().to_string());
        }
    }
    Err("could not determine host triple from rustc -vV".to_string())
}

fn library_name() -> String {
    if cfg!(target_os = "macos") {
        "liblemma_dotnet.dylib".to_string()
    } else if cfg!(target_os = "windows") {
        "lemma_dotnet.dll".to_string()
    } else {
        "liblemma_dotnet.so".to_string()
    }
}
