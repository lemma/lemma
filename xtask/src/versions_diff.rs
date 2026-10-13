//! `git diff` / `git log` between release tags.
//!
//! The umbrella GitHub release tag is `lemma-v{version}`. Releases before the rename used
//! `cli-v{version}`; both prefixes are accepted so version-diffing spans the transition.
//!
//! Runs `git fetch --tags` first so local tag refs match the remote (e.g. CI-created release tags)
//! before resolving tags.
//!
//! **No version argument:** `git diff` / `git diff --stat` compare the latest release tag to the
//! **working tree** (including uncommitted changes). `git log` is still `tag..HEAD` (commits only).
//!
//! **`versions-diff <semver>`:** compares the previous release tag to the requested version's tag
//! on history (two commits; no working tree).

use std::io::{self, Write};
use std::path::Path;
use std::process::Command;

use semver::Version;

/// Release tag prefixes, newest convention first. `cli-v` is the legacy prefix kept for history.
const RELEASE_TAG_PREFIXES: [&str; 2] = ["lemma-v", "cli-v"];

/// Parse the semver from a release tag, e.g. `lemma-v0.8.20` or `cli-v0.8.4`.
pub(crate) fn parse_release_tag_version(tag: &str) -> Option<Version> {
    RELEASE_TAG_PREFIXES
        .iter()
        .find_map(|prefix| tag.strip_prefix(prefix))?
        .parse()
        .ok()
}

fn git_fetch_tags(root: &Path) -> Result<(), String> {
    let o = Command::new("git")
        .args(["fetch", "--tags", "--quiet", "-f"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("failed to run git fetch --tags: {e}"))?;
    if !o.status.success() {
        let err = String::from_utf8_lossy(&o.stderr);
        let err = err.trim();
        if err.is_empty() {
            return Err("git fetch --tags failed".into());
        }
        return Err(format!("git fetch --tags failed: {err}"));
    }
    Ok(())
}

fn git_output(args: &[&str], cwd: &Path) -> Result<Vec<u8>, String> {
    let o = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|e| format!("failed to run git: {e}"))?;
    if !o.status.success() {
        let err = String::from_utf8_lossy(&o.stderr);
        let err = err.trim();
        if err.is_empty() {
            return Err("git command failed".into());
        }
        return Err(err.to_string());
    }
    Ok(o.stdout)
}

fn list_release_tags_sorted(root: &Path) -> Result<Vec<(Version, String)>, String> {
    let mut tags: Vec<(Version, String)> = Vec::new();
    for prefix in RELEASE_TAG_PREFIXES {
        let out = git_output(&["tag", "-l", &format!("{prefix}*")], root)?;
        for line in String::from_utf8_lossy(&out).lines() {
            if line.is_empty() {
                continue;
            }
            let Some(ver) = parse_release_tag_version(line) else {
                continue;
            };
            // Prefer the newest-convention tag when a version exists under multiple prefixes.
            if tags.iter().any(|(v, _)| v == &ver) {
                continue;
            }
            tags.push((ver, line.to_string()));
        }
    }
    tags.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(tags)
}

fn write_stdout(bytes: &[u8]) -> Result<(), String> {
    io::stdout().write_all(bytes).map_err(|e| e.to_string())?;
    Ok(())
}

/// True if index or working tree differs from `HEAD` (uncommitted or unstaged changes).
fn worktree_differs_from_head(root: &Path) -> bool {
    let status = Command::new("git")
        .args(["diff-index", "--quiet", "HEAD", "--"])
        .current_dir(root)
        .status()
        .unwrap_or_else(|error| panic!("failed to run git diff-index: {error}"));
    !status.success()
}

/// Print `git diff --stat`, `git log`, then `git diff` for the resolved range or tag → worktree.
pub fn run_versions_diff(root: &Path, version_arg: Option<&str>) -> Result<(), String> {
    git_fetch_tags(root)?;
    let tags = list_release_tags_sorted(root)?;
    if tags.is_empty() {
        return Err("no release tags found (lemma-v* or cli-v*)".into());
    }

    match version_arg {
        None => {
            let tag = tags.last().expect("non-empty").1.clone();
            if worktree_differs_from_head(root) {
                eprintln!(
                    "versions-diff: working tree differs from HEAD; diff includes uncommitted changes; log is tag..HEAD only."
                );
            }

            let stat = git_output(&["diff", "--stat", &tag], root)?;
            write_stdout(&stat)?;
            write_stdout(b"\n")?;

            let log_range = format!("{tag}..HEAD");
            let log = git_output(&["log", "--no-merges", &log_range, "--oneline"], root)?;
            write_stdout(&log)?;
            write_stdout(b"\n")?;

            let diff = git_output(&["diff", &tag], root)?;
            write_stdout(&diff)?;
        }
        Some(v) => {
            let want = Version::parse(v).map_err(|e| format!("invalid semver {v:?}: {e}"))?;
            let idx = tags
                .iter()
                .position(|(ver, _)| ver == &want)
                .ok_or_else(|| format!("no release tag for version {want}"))?;
            if idx == 0 {
                return Err(format!("no previous release tag before version {want}"));
            }
            let prev = tags[idx - 1].1.clone();
            let end = tags[idx].1.clone();
            let range = format!("{prev}..{end}");

            let stat = git_output(&["diff", "--stat", &range], root)?;
            write_stdout(&stat)?;
            write_stdout(b"\n")?;

            let log = git_output(&["log", "--no-merges", &range, "--oneline"], root)?;
            write_stdout(&log)?;
            write_stdout(b"\n")?;

            let diff = git_output(&["diff", &range], root)?;
            write_stdout(&diff)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_release_tag_version_accepts() {
        assert_eq!(
            parse_release_tag_version("lemma-v0.8.20"),
            Some(Version::new(0, 8, 20))
        );
        assert_eq!(
            parse_release_tag_version("cli-v0.8.4"),
            Some(Version::new(0, 8, 4))
        );
        assert_eq!(parse_release_tag_version("v0.8.4"), None);
        assert_eq!(parse_release_tag_version("lemma-engine-v0.8.4"), None);
    }
}
