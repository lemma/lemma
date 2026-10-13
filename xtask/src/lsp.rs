//! Build the Lemma CLI and prepare / package / publish the VS Code / Cursor extension.

use std::path::{Path, PathBuf};
use std::process::Command;

pub const VSCODE_EXTENSION_REL: &str = "engine/lsp/editors/vscode";

const VSCE_BIN: &str = "vsce";
const OVSX_BIN: &str = "ovsx";

/// Run `npm <args>` in `dir`; non-zero exit or any warning line in its output is an error.
pub(crate) fn run_npm(dir: &Path, args: &[&str]) -> Result<(), String> {
    let output = Command::new("npm")
        .current_dir(dir)
        .args(args)
        .output()
        .map_err(|e| {
            format!(
                "failed to run npm {} in {}: {e}",
                args.join(" "),
                dir.display()
            )
        })?;
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    print!("{combined}");
    let label = format!("npm {}", args.join(" "));
    if !output.status.success() {
        return Err(format!(
            "{label} exited with status {:?}",
            output.status.code()
        ));
    }
    crate::warnings::reject_warnings_in_output(&label, &combined)?;
    Ok(())
}

fn run_npm_exec_capture(vscode_dir: &Path, bin: &str, args: &[&str]) -> Result<String, String> {
    let mut cmd_args = vec!["exec", "--no", "--", bin];
    cmd_args.extend_from_slice(args);
    let output = Command::new("npm")
        .current_dir(vscode_dir)
        .args(&cmd_args)
        .output()
        .map_err(|e| {
            format!(
                "failed to run npm exec --no -- {bin} {} in {}: {e}",
                args.join(" "),
                vscode_dir.display()
            )
        })?;
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    print!("{combined}");
    let label = format!("npm exec --no -- {bin} {}", args.join(" "));
    if !output.status.success() {
        return Err(format!(
            "{label} exited with status {:?}",
            output.status.code()
        ));
    }
    crate::warnings::reject_warnings_in_output(&label, &combined)?;
    Ok(combined)
}

fn build_release_lemma(root: &Path) -> Result<(), String> {
    let status = Command::new(crate::cargo_bin())
        .current_dir(root)
        .args(["build", "--release", "-p", "lemma"])
        .status()
        .map_err(|e| format!("failed to run cargo build --release -p lemma: {e}"))?;
    if !status.success() {
        return Err(format!(
            "cargo build --release -p lemma exited with status {:?}",
            status.code()
        ));
    }
    Ok(())
}

fn prepare_extension(root: &Path, vscode_dir: &Path) -> Result<(), String> {
    eprintln!("xtask: cargo build --release -p lemma");
    build_release_lemma(root)?;
    eprintln!("xtask: npm ci (vscode extension)");
    run_npm(vscode_dir, &["ci"])?;
    eprintln!("xtask: npm run compile (vscode extension)");
    run_npm(vscode_dir, &["run", "compile"])?;
    Ok(())
}

fn package_vsix(vscode_dir: &Path) -> Result<(), String> {
    eprintln!("xtask: npm exec --no -- {VSCE_BIN} package");
    run_npm_exec_capture(vscode_dir, VSCE_BIN, &["package"])?;
    match newest_vsix(vscode_dir) {
        Some(p) => eprintln!("xtask: VSIX: {}", p.display()),
        None => {
            return Err(format!(
                "npm exec --no -- {VSCE_BIN} package finished but no .vsix under {}",
                vscode_dir.display()
            ));
        }
    }
    Ok(())
}

/// `npm ci` + compile + package `.vsix` (no lemma binary build). Used by precommit and release.
pub fn ci_compile_package(vscode_dir: &Path) -> Result<(), String> {
    eprintln!("xtask: npm ci (vscode extension)");
    run_npm(vscode_dir, &["ci"])?;
    eprintln!("xtask: npm run compile (vscode extension)");
    run_npm(vscode_dir, &["run", "compile"])?;
    package_vsix(vscode_dir)?;
    smoke_publish_clis(vscode_dir)?;
    Ok(())
}

const PUBLISH_CLI_SMOKE: &str =
    r#"require("@node-rs/crc32"); require("ovsx"); require("@vscode/vsce");"#;

fn smoke_publish_clis(vscode_dir: &Path) -> Result<(), String> {
    eprintln!("xtask: smoke require publish CLIs (@node-rs/crc32, ovsx, vsce)");
    let output = Command::new("node")
        .current_dir(vscode_dir)
        .args(["-e", PUBLISH_CLI_SMOKE])
        .output()
        .map_err(|e| {
            format!(
                "failed to run node publish CLI smoke in {}: {e}",
                vscode_dir.display()
            )
        })?;
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    print!("{combined}");
    let label = "node publish CLI smoke";
    if !output.status.success() {
        return Err(format!(
            "{label} exited with status {:?}",
            output.status.code()
        ));
    }
    crate::warnings::reject_warnings_in_output(label, &combined)?;
    Ok(())
}

fn require_env(name: &str) -> Result<String, String> {
    std::env::var(name).map_err(|_| format!("missing required env var {name}"))
}

fn publish_marketplace(vscode_dir: &Path) -> Result<(), String> {
    let token = require_env("VSCE_PAT")?;
    let vsix = newest_vsix(vscode_dir).ok_or_else(|| {
        format!(
            "no .vsix under {} — run `cargo lsp package` first",
            vscode_dir.display()
        )
    })?;
    let vsix_name = vsix
        .file_name()
        .and_then(|s| s.to_str())
        .expect("BUG: vsix path must be UTF-8");
    eprintln!("xtask: npm exec --no -- {VSCE_BIN} publish --packagePath {vsix_name}");
    run_npm_exec_capture(
        vscode_dir,
        VSCE_BIN,
        &["publish", "--packagePath", vsix_name, "-p", &token],
    )?;
    Ok(())
}

fn publish_openvsx(vscode_dir: &Path) -> Result<(), String> {
    let token = require_env("OPEN_VSX_TOKEN")?;
    let vsix = newest_vsix(vscode_dir).ok_or_else(|| {
        format!(
            "no .vsix under {} — run `cargo lsp package` first",
            vscode_dir.display()
        )
    })?;
    let vsix_name = vsix
        .file_name()
        .and_then(|s| s.to_str())
        .expect("BUG: vsix path must be UTF-8");
    eprintln!("xtask: npm exec --no -- {OVSX_BIN} publish {vsix_name}");
    run_npm_exec_capture(vscode_dir, OVSX_BIN, &["publish", vsix_name, "-p", &token])?;
    Ok(())
}

fn newest_vsix(vscode_dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(vscode_dir).ok()?;
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(ext) = path.extension() else {
            continue;
        };
        let Some(ext) = ext.to_str() else {
            continue;
        };
        if !ext.eq_ignore_ascii_case("vsix") {
            continue;
        }
        let modified = entry.metadata().ok()?.modified().ok()?;
        match &best {
            None => best = Some((modified, path)),
            Some((t, _)) if modified > *t => best = Some((modified, path)),
            _ => {}
        }
    }
    best.map(|(_, p)| p)
}

fn print_paths(root: &Path, vscode_dir: &Path) {
    let binary = root.join("target").join("release").join("lemma");
    eprintln!("xtask: lemma binary: {}", binary.display());
    eprintln!("xtask: vscode extension dir: {}", vscode_dir.display());
}

/// `rest`: optional subcommand (`vsix`, `prepare`, `package`, publish-*); empty means default prepare.
pub fn run(root: &Path, rest: &[String]) -> Result<(), String> {
    let vscode_dir = root.join(VSCODE_EXTENSION_REL);

    match rest.first().map(|s| s.as_str()) {
        None => {
            prepare_extension(root, &vscode_dir)?;
            print_paths(root, &vscode_dir);
        }
        Some("prepare") => {
            if rest.len() != 1 {
                return Err(format!(
                    "`prepare` takes no extra arguments (got: {})",
                    rest[1..].join(" ")
                ));
            }
            prepare_extension(root, &vscode_dir)?;
            print_paths(root, &vscode_dir);
        }
        Some("package") => {
            if rest.len() != 1 {
                return Err(format!(
                    "`package` takes no extra arguments (got: {})",
                    rest[1..].join(" ")
                ));
            }
            ci_compile_package(&vscode_dir)?;
            print_paths(root, &vscode_dir);
        }
        Some("vsix") => {
            if rest.len() != 1 {
                return Err(format!(
                    "extra arguments after vsix: {}",
                    rest[1..].join(" ")
                ));
            }
            prepare_extension(root, &vscode_dir)?;
            package_vsix(&vscode_dir)?;
            print_paths(root, &vscode_dir);
        }
        Some("publish-marketplace") => {
            if rest.len() != 1 {
                return Err(format!(
                    "`publish-marketplace` takes no extra arguments (got: {})",
                    rest[1..].join(" ")
                ));
            }
            publish_marketplace(&vscode_dir)?;
        }
        Some("publish-openvsx") => {
            if rest.len() != 1 {
                return Err(format!(
                    "`publish-openvsx` takes no extra arguments (got: {})",
                    rest[1..].join(" ")
                ));
            }
            publish_openvsx(&vscode_dir)?;
        }
        Some("-h" | "--help" | "help") => {
            if rest.len() != 1 {
                return Err(format!(
                    "unexpected arguments after help: {}",
                    rest[1..].join(" ")
                ));
            }
            eprintln!(
                "cargo lsp                    — release-build `lemma` + npm ci && npm run compile in {}",
                vscode_dir.display()
            );
            eprintln!(
                "cargo lsp package            — npm ci + compile + npm exec --no -- {VSCE_BIN} package (no lemma build)"
            );
            eprintln!(
                "cargo lsp vsix               — prepare + package .vsix (install via Extensions → Install from VSIX)"
            );
            eprintln!(
                "cargo lsp publish-marketplace — npm exec --no -- {VSCE_BIN} publish (needs VSCE_PAT)"
            );
            eprintln!(
                "cargo lsp publish-openvsx    — npm exec --no -- {OVSX_BIN} publish (needs OPEN_VSX_TOKEN)"
            );
        }
        Some(other) => {
            return Err(format!(
                "unknown subcommand {other:?}; try `cargo lsp --help`"
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepare_rejects_extra_arguments() {
        assert!(run(
            Path::new("/tmp"),
            &["prepare".to_string(), "nope".to_string()]
        )
        .unwrap_err()
        .contains("prepare"));
    }

    #[test]
    fn package_rejects_extra_arguments() {
        assert!(run(
            Path::new("/tmp"),
            &["package".to_string(), "nope".to_string()]
        )
        .unwrap_err()
        .contains("package"));
    }

    #[test]
    fn rejects_unknown_subcommand() {
        assert!(run(Path::new("/tmp"), &["not-a-command".to_string()])
            .unwrap_err()
            .contains("unknown"));
    }
}
