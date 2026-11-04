//! Treat tool warnings as build failures (npm, Maven, etc.).

pub fn line_is_warning(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("[WARNING]")
        || trimmed.starts_with("WARNING")
        || trimmed.starts_with("Warning:")
        || trimmed.starts_with("warning:")
        || trimmed.starts_with("npm warn")
        || trimmed.starts_with("npm WARN")
}

pub fn reject_warnings_in_output(label: &str, combined: &str) -> Result<(), String> {
    for line in combined.lines() {
        if line_is_warning(line) {
            return Err(format!(
                "{label} emitted warning (warnings are errors): {line}"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_npm_warn_and_warning() {
        assert!(line_is_warning(
            "npm warn deprecated whatwg-encoding@3.1.1: Use @exodus/bytes"
        ));
        assert!(line_is_warning(
            "npm WARN deprecated prebuild-install@7.1.3"
        ));
        assert!(line_is_warning("WARNING Something went wrong"));
        assert!(line_is_warning(
            "Warning: Unable to auto-format lemma_dotnet.cs"
        ));
        assert!(line_is_warning(
            "warning: ignoring benchmark `apply` as `benches/apply.rs` is not included in the published package"
        ));
        assert!(line_is_warning("  npm warn indented"));
        assert!(!line_is_warning(
            " DONE  Packaged: /tmp/lemma-language-0.9.4.vsix"
        ));
        assert!(!line_is_warning("INFO  Files included in the VSIX:"));
    }

    #[test]
    fn rejects_maven_warning_lines() {
        assert!(line_is_warning(
            "[WARNING] jackson-core-2.22.1.jar, lemma-engine-0.9.6.jar define overlapping resource: META-INF/MANIFEST.MF"
        ));
        assert!(line_is_warning(
            "  [WARNING] Usually this is not harmful and you can skip these warnings,"
        ));
    }

    #[test]
    fn reject_warnings_in_output_fails_on_npm_warn() {
        let err = reject_warnings_in_output(
            "npm ci",
            "added 309 packages\nnpm warn deprecated foo@1.0.0: bad\n",
        )
        .unwrap_err();
        assert!(err.contains("warnings are errors"));
        assert!(err.contains("npm warn"));
    }
}
