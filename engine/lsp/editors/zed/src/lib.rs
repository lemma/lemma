//! Zed extension for Lemma: resolves the `lemma` binary and starts `lemma lsp` over stdio.
//!
//! Lookup order mirrors `resolveServerCommand` in the VS Code extension
//! (`engine/lsp/editors/vscode/src/extension.ts`):
//! 1. `lsp.lemma.binary.path` from Zed settings (with `arguments` / `env` if set)
//! 2. `<worktree>/target/release/lemma`, then `target/debug/lemma`, preferring the
//!    binary whose `--version` matches this extension's version
//! 3. `lemma` on the worktree shell `$PATH`

use zed_extension_api::{
    self as zed, process, settings::LspSettings, LanguageServerId, Result, Worktree,
};

const EXTENSION_VERSION: &str = env!("CARGO_PKG_VERSION");
const LSP_SUBCOMMAND: &str = "lsp";
const INSTALL_HINT: &str = "Lemma LSP requires the `lemma` binary. Install via: npm install -g lemma or cargo install lemma, or set lsp.lemma.binary.path in Zed settings.";

struct LemmaExtension;

/// Last whitespace-separated word of the first `--version` line (`lemma 0.11.4` -> `0.11.4`).
fn version_from_output(stdout: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(stdout);
    let line = text.trim().lines().next()?;
    line.split_whitespace().last().map(str::to_string)
}

/// `Some(version)` when `path` runs and exits 0 for `--version`; `None` when it is absent or fails.
fn lemma_binary_version(path: &str) -> Option<String> {
    let output = process::Command::new(path).arg("--version").output().ok()?;
    if output.status != Some(0) {
        return None;
    }
    version_from_output(&output.stdout)
}

fn binary_file_name() -> &'static str {
    match zed::current_platform().0 {
        zed::Os::Windows => "lemma.exe",
        zed::Os::Mac | zed::Os::Linux => "lemma",
    }
}

/// Workspace `target/{release,debug}` build: a version match wins, else the first that runs.
fn workspace_binary(worktree: &Worktree) -> Option<String> {
    let root = worktree.root_path();
    let name = binary_file_name();
    let runnable: Vec<(String, String)> = ["release", "debug"]
        .iter()
        .map(|profile| format!("{root}/target/{profile}/{name}"))
        .filter_map(|path| lemma_binary_version(&path).map(|version| (path, version)))
        .collect();
    if let Some((path, _)) = runnable
        .iter()
        .find(|(_, version)| version == EXTENSION_VERSION)
    {
        return Some(path.clone());
    }
    runnable.into_iter().next().map(|(path, _)| path)
}

impl zed::Extension for LemmaExtension {
    fn new() -> Self {
        LemmaExtension
    }

    fn language_server_command(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &Worktree,
    ) -> Result<zed::Command> {
        let mut env = worktree.shell_env();
        let binary_settings =
            LspSettings::for_worktree(language_server_id.as_ref(), worktree)?.binary;

        if let Some(settings) = binary_settings {
            if let Some(extra_env) = settings.env {
                env.extend(extra_env);
            }
            if let Some(path) = settings.path {
                let args = settings
                    .arguments
                    .unwrap_or_else(|| vec![LSP_SUBCOMMAND.to_string()]);
                return Ok(zed::Command {
                    command: path,
                    args,
                    env,
                });
            }
        }

        let command = workspace_binary(worktree)
            .or_else(|| worktree.which("lemma"))
            .ok_or_else(|| INSTALL_HINT.to_string())?;
        Ok(zed::Command {
            command,
            args: vec![LSP_SUBCOMMAND.to_string()],
            env,
        })
    }
}

zed::register_extension!(LemmaExtension);

#[cfg(test)]
mod tests {
    use super::version_from_output;

    #[test]
    fn version_is_last_word_of_first_line() {
        assert_eq!(
            version_from_output(b"lemma 0.11.4\nbuild info\n"),
            Some("0.11.4".to_string())
        );
    }

    #[test]
    fn empty_output_has_no_version() {
        assert_eq!(version_from_output(b"  \n"), None);
    }
}
