//! Build host native library, generate UniFFI C#, and place under nuget package.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub use crate::versions::UNIFFI_VERSION;
pub const UNIFFI_BINDGEN_CS_TAG: &str = "v0.11.0+v0.31.0";
const CSHARPIER_VERSION: &str = "1.3.0";

pub fn run(root: &Path) -> Result<(), String> {
    require_uniffi_bindgen_cs()?;
    verify_uniffi_crate_pin(root)?;
    let csharpier_dir = ensure_csharpier(root)?;

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
    let output = Command::new("uniffi-bindgen-cs")
        .args([
            "--library",
            built.to_str().ok_or("library path is not UTF-8")?,
            "--out-dir",
            generated_dir.to_str().ok_or("generated dir is not UTF-8")?,
            "--config",
            config.to_str().ok_or("config path is not UTF-8")?,
            "--no-format",
        ])
        .output()
        .map_err(|e| format!("failed to run uniffi-bindgen-cs: {e}"))?;
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    print!("{combined}");
    if !output.status.success() {
        return Err("uniffi-bindgen-cs failed".to_string());
    }
    crate::warnings::reject_warnings_in_output("uniffi-bindgen-cs", &combined)?;
    format_generated_csharp(&csharpier_dir, &generated_dir)?;

    let version = crate::versions::read_workspace_version(root)?;
    let version_file = nuget_pkg.join("engine.version");
    fs::write(&version_file, format!("{version}\n"))
        .map_err(|e| format!("failed to write {}: {e}", version_file.display()))?;

    Ok(())
}

fn ensure_csharpier(root: &Path) -> Result<PathBuf, String> {
    let tool_dir = root.join("target").join("csharpier");
    fs::create_dir_all(&tool_dir)
        .map_err(|e| format!("failed to create {}: {e}", tool_dir.display()))?;
    let bin = tool_dir.join(csharpier_bin_name());
    if bin.is_file() {
        match csharpier_version(&bin) {
            Ok(version) if version == CSHARPIER_VERSION => return Ok(tool_dir),
            Ok(_) | Err(_) => {
                let status = Command::new("dotnet")
                    .args(["tool", "uninstall", "csharpier", "--tool-path"])
                    .arg(&tool_dir)
                    .status()
                    .map_err(|e| format!("failed to run dotnet tool uninstall csharpier: {e}"))?;
                if !status.success() {
                    return Err(format!(
                        "dotnet tool uninstall csharpier failed in {}",
                        tool_dir.display()
                    ));
                }
            }
        }
    }
    eprintln!("nuget-natives: dotnet tool install csharpier {CSHARPIER_VERSION}");
    let status = Command::new("dotnet")
        .args([
            "tool",
            "install",
            "csharpier",
            "--version",
            CSHARPIER_VERSION,
            "--tool-path",
        ])
        .arg(&tool_dir)
        .status()
        .map_err(|e| format!("failed to run dotnet tool install csharpier: {e}"))?;
    if !status.success() {
        return Err(format!(
            "dotnet tool install csharpier {CSHARPIER_VERSION} failed"
        ));
    }
    if !bin.is_file() {
        return Err(format!(
            "csharpier {CSHARPIER_VERSION} installed but {} is missing",
            bin.display()
        ));
    }
    Ok(tool_dir)
}

fn csharpier_bin_name() -> &'static str {
    if cfg!(windows) {
        "csharpier.exe"
    } else {
        "csharpier"
    }
}

fn csharpier_version(bin: &Path) -> Result<String, String> {
    let output = Command::new(bin)
        .arg("--version")
        .output()
        .map_err(|e| format!("failed to run {}: {e}", bin.display()))?;
    if !output.status.success() {
        return Err(format!("{} --version failed", bin.display()));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.lines().next().unwrap_or("").trim().to_string())
}

fn format_generated_csharp(csharpier_dir: &Path, generated_dir: &Path) -> Result<(), String> {
    let mut found = false;
    let entries = fs::read_dir(generated_dir)
        .map_err(|e| format!("failed to read {}: {e}", generated_dir.display()))?;
    for entry in entries {
        let path = entry
            .map_err(|e| format!("failed to read {}: {e}", generated_dir.display()))?
            .path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("cs") {
            continue;
        }
        found = true;
        // CSharpier skips paths matched by gitignore. Generated/ is gitignored, so format a copy outside the repo.
        let temp_dir = std::env::temp_dir().join(format!("lemma-csharpier-{}", std::process::id()));
        fs::create_dir_all(&temp_dir)
            .map_err(|e| format!("failed to create {}: {e}", temp_dir.display()))?;
        let file_name = path
            .file_name()
            .expect("BUG: C# binding path has no file name");
        let temp_file = temp_dir.join(file_name);
        fs::copy(&path, &temp_file).map_err(|e| {
            format!(
                "failed to copy {} to {}: {e}",
                path.display(),
                temp_file.display()
            )
        })?;
        let bin = csharpier_dir.join(csharpier_bin_name());
        eprintln!("nuget-natives: csharpier format {}", path.display());
        let output = Command::new(&bin)
            .args(["format", "--include-generated", "--no-cache"])
            .arg(&temp_file)
            .output()
            .map_err(|e| format!("failed to run {}: {e}", bin.display()))?;
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        print!("{combined}");
        if !output.status.success() {
            return Err(format!("csharpier failed on {}", path.display()));
        }
        crate::warnings::reject_warnings_in_output("csharpier", &combined)?;
        if !combined.contains("Formatted 1 ") {
            return Err(format!(
                "csharpier did not format {}:\n{combined}",
                path.display()
            ));
        }
        fs::copy(&temp_file, &path).map_err(|e| {
            format!(
                "failed to copy {} to {}: {e}",
                temp_file.display(),
                path.display()
            )
        })?;
        fs::remove_dir_all(&temp_dir)
            .map_err(|e| format!("failed to remove {}: {e}", temp_dir.display()))?;
    }
    if !found {
        return Err(format!(
            "uniffi-bindgen-cs wrote no C# files in {}",
            generated_dir.display()
        ));
    }
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
