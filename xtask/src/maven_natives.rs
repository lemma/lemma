//! Build and install host native library for Maven package.

use std::fs;
use std::path::Path;
use std::process::Command;

pub fn run(root: &Path) -> Result<(), String> {
    let cargo = crate::cargo_bin();
    let target_dir = std::env::var("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| root.join("target"));

    let triple = host_triple()?;
    let lib_name = library_name();

    let src = build_release_library(&cargo, &target_dir, &lib_name)?;

    let maven_pkg = root.join("engine/packages/maven");
    let dest_dir = maven_pkg.join("src/main/resources/native").join(&triple);
    fs::create_dir_all(&dest_dir)
        .map_err(|e| format!("failed to create {}: {e}", dest_dir.display()))?;

    let dest = dest_dir.join(&lib_name);
    fs::copy(&src, &dest).map_err(|e| {
        format!(
            "failed to copy {} to {}: {e}",
            src.display(),
            dest.display()
        )
    })?;

    eprintln!("maven-natives: {} -> {}", src.display(), dest.display());
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
        "liblemma_jni.dylib".to_string()
    } else if cfg!(target_os = "windows") {
        "lemma_jni.dll".to_string()
    } else {
        "liblemma_jni.so".to_string()
    }
}

/// Always builds the release profile so the packaged library is never a debug
/// artifact; cargo makes the rebuild a no-op when nothing changed.
fn build_release_library(
    cargo: &str,
    target_dir: &Path,
    lib_name: &str,
) -> Result<std::path::PathBuf, String> {
    eprintln!("maven-natives: cargo build --release -p lemma_jni");

    let status = Command::new(cargo)
        .args(["build", "--release", "-p", "lemma_jni"])
        .status()
        .map_err(|e| format!("failed to run cargo build: {e}"))?;

    if !status.success() {
        return Err("cargo build --release -p lemma_jni failed".to_string());
    }

    let built = target_dir.join("release").join(lib_name);
    if built.is_file() {
        Ok(built)
    } else {
        Err(format!(
            "cargo build --release succeeded but {} not found",
            built.display()
        ))
    }
}
