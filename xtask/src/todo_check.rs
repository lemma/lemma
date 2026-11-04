//! **`todo-check`:** fails when any `.rs` source contains a `todo!` macro invocation.

use std::fs;
use std::path::{Path, PathBuf};

const SKIP_DIRS: &[&str] = &["target", ".git", "node_modules"];

pub fn run(workspace: &Path) -> Result<(), String> {
    let files = rust_files(workspace)?;
    let mut hits = Vec::new();
    for file in &files {
        let text = fs::read_to_string(file).map_err(|e| format!("read {}: {e}", file.display()))?;
        let display = file.strip_prefix(workspace).unwrap_or(file);
        for (i, line) in text.lines().enumerate() {
            if contains_todo_macro(line) {
                hits.push(format!("{}:{}: {}", display.display(), i + 1, line.trim()));
            }
        }
    }
    if hits.is_empty() {
        println!(
            "todo-check: no todo! macros among {} Rust source files",
            files.len()
        );
        return Ok(());
    }
    let mut message = format!("todo-check: {} todo! macro(s) found:", hits.len());
    for hit in &hits {
        message.push('\n');
        message.push_str(hit);
    }
    Err(message)
}

fn rust_files(workspace: &Path) -> Result<Vec<PathBuf>, String> {
    let mut pending = vec![workspace.to_path_buf()];
    let mut files = Vec::new();
    while let Some(next) = pending.pop() {
        let entries =
            fs::read_dir(&next).map_err(|e| format!("read_dir {}: {e}", next.display()))?;
        for entry in entries {
            let path = entry
                .map_err(|e| format!("read_dir {}: {e}", next.display()))?
                .path();
            if path.is_dir() {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if SKIP_DIRS.contains(&name) {
                    continue;
                }
                pending.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

fn contains_todo_macro(line: &str) -> bool {
    let mut search = line;
    while let Some(idx) = search.find("todo!") {
        let before_ok = if idx == 0 {
            true
        } else {
            let c = search[..idx]
                .chars()
                .next_back()
                .expect("BUG: idx > 0 so preceding char exists");
            !c.is_ascii_alphanumeric() && c != '_'
        };
        let after = &search[idx + "todo!".len()..];
        if before_ok && after.trim_start().starts_with('(') {
            return true;
        }
        search = &search[idx + "todo!".len()..];
    }
    false
}

#[cfg(test)]
mod tests {
    use super::contains_todo_macro;

    fn call() -> String {
        format!("{}{}", "todo!", "(")
    }

    #[test]
    fn detects_todo_macro_call() {
        assert!(contains_todo_macro(&format!("{}{}", call(), ")")));
        assert!(contains_todo_macro(&format!("{} {}", "todo!", "(")));
        assert!(contains_todo_macro(&format!("    {}{})", call(), "\"x\"")));
    }

    #[test]
    fn rejects_non_calls() {
        assert!(!contains_todo_macro("todo!"));
        assert!(!contains_todo_macro("a todo! comment without call"));
        assert!(!contains_todo_macro(&format!("not_a_{}", call())));
        assert!(!contains_todo_macro("something else"));
    }
}
