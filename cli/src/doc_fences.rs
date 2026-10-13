//! Extract and format `` ```lemma `` fences inside markdown / text documents.

use lemma::{format_source, Error, SourceType};

/// One visual `` ```lemma `` fence (not adjacent-grouped).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LemmaFence {
    /// 1-based line of the opening `` ```lemma `` marker.
    pub line: usize,
    /// Inclusive 1-based line of the closing `` ``` ``.
    pub end_line: usize,
    /// Fence body with outer whitespace trimmed.
    pub content: String,
}

/// Parse failure while formatting a fence body.
#[derive(Debug)]
pub struct FenceFormatError {
    /// 1-based line of the opening fence marker.
    pub fence_line: usize,
    /// Body that failed to format (trimmed).
    pub fence_source: String,
    pub error: Error,
}

/// Extract every non-empty `` ```lemma `` fence (`` ```lemma-skip `` ignored).
///
/// Does not merge whitespace-adjacent fences; callers that load multi-spec
/// examples as one workspace should call [`group_whitespace_adjacent_fences`].
pub fn extract_lemma_fences(content: &str) -> Vec<LemmaFence> {
    let mut blocks = Vec::new();
    let mut in_fence = false;
    let mut fence_line = 0usize;
    let mut current = String::new();
    for (line_index, line) in content.lines().enumerate() {
        let line_number = line_index + 1;
        let trimmed = line.trim();
        if trimmed == "```lemma" {
            in_fence = true;
            fence_line = line_number;
            current.clear();
            continue;
        }
        if in_fence && trimmed == "```" {
            in_fence = false;
            let block = current.trim().to_string();
            if !block.is_empty() {
                blocks.push(LemmaFence {
                    line: fence_line,
                    end_line: line_number,
                    content: block,
                });
            }
            continue;
        }
        if in_fence {
            current.push_str(line);
            current.push('\n');
        }
    }
    blocks
}

/// Merge fences that have only blank/whitespace lines between them into one load unit.
pub fn group_whitespace_adjacent_fences(file: &str, fences: Vec<LemmaFence>) -> Vec<LemmaFence> {
    let lines: Vec<&str> = file.lines().collect();
    let mut groups: Vec<LemmaFence> = Vec::new();
    for fence in fences {
        if let Some(prev) = groups.last_mut() {
            let between_start = prev.end_line;
            let between_end = fence.line.saturating_sub(1);
            let only_whitespace = (between_start..between_end).all(|idx| {
                lines
                    .get(idx)
                    .is_none_or(|line| line.chars().all(char::is_whitespace))
            });
            if only_whitespace {
                prev.content.push_str("\n\n");
                prev.content.push_str(&fence.content);
                prev.end_line = fence.end_line;
                continue;
            }
        }
        groups.push(fence);
    }
    groups
}

/// Rewrite every `` ```lemma `` fence body to canonical `format_source` output.
///
/// Prose, fence markers, and `` ```lemma-skip `` blocks are unchanged. Each visual
/// fence is formatted on its own (adjacent fences are not merged).
///
/// The returned document always ends with a trailing newline when it contains
/// at least one line (same as walking `lines()` and re-joining).
pub fn format_lemma_fences_in_text(content: &str) -> Result<String, FenceFormatError> {
    let lines: Vec<&str> = content.lines().collect();
    let mut out = String::new();
    let mut i = 0usize;
    while i < lines.len() {
        let trimmed = lines[i].trim();
        if trimmed == "```lemma" {
            out.push_str(lines[i]);
            out.push('\n');
            let fence_line = i + 1;
            i += 1;
            let mut body = String::new();
            while i < lines.len() && lines[i].trim() != "```" {
                body.push_str(lines[i]);
                body.push('\n');
                i += 1;
            }
            let trimmed_body = body.trim();
            if trimmed_body.is_empty() {
                out.push_str(&body);
            } else {
                let formatted =
                    format_source(trimmed_body, SourceType::Volatile).map_err(|error| {
                        FenceFormatError {
                            fence_line,
                            fence_source: trimmed_body.to_string(),
                            error,
                        }
                    })?;
                out.push_str(&formatted);
            }
            if i < lines.len() {
                out.push_str(lines[i]);
                out.push('\n');
                i += 1;
            }
            continue;
        }
        out.push_str(lines[i]);
        out.push('\n');
        i += 1;
    }
    if content.is_empty() {
        return Ok(String::new());
    }
    if !content.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_skips_lemma_skip_and_empty() {
        let file = "\
```lemma-skip
spec bad
```

```lemma
```

```lemma
spec ok
data x: 1
```
";
        let fences = extract_lemma_fences(file);
        assert_eq!(fences.len(), 1);
        assert_eq!(fences[0].line, 8);
        assert!(fences[0].content.contains("spec ok"));
    }

    #[test]
    fn group_merges_whitespace_adjacent() {
        let file = "\
```lemma
spec a
data x: 1
```

```lemma
spec b
uses a
rule r: a.x
```

prose

```lemma
spec c
data y: 2
```
";
        let groups = group_whitespace_adjacent_fences(file, extract_lemma_fences(file));
        assert_eq!(groups.len(), 2);
        assert!(groups[0].content.contains("spec a") && groups[0].content.contains("spec b"));
        assert!(groups[1].content.contains("spec c"));
    }

    #[test]
    fn format_rewrites_fence_body_preserves_prose() {
        let file = "\
# Title

```lemma
spec demo
data X: 1
rule Y: X
```

done
";
        let formatted = format_lemma_fences_in_text(file).expect("format");
        assert!(formatted.contains("# Title"));
        assert!(formatted.contains("done"));
        assert!(formatted.contains("```lemma\n"));
        assert!(formatted.contains("data x: 1"));
        assert!(formatted.contains("rule y:\n"));
        let again = format_lemma_fences_in_text(&formatted).expect("idempotent");
        assert_eq!(formatted, again);
    }

    #[test]
    fn format_leaves_lemma_skip_alone() {
        let file = "\
```lemma-skip
spec Weird
  data X: 1
```
";
        let formatted = format_lemma_fences_in_text(file).expect("format");
        assert_eq!(file, formatted);
    }

    #[test]
    fn format_reports_parse_error_with_fence_line() {
        let file = "\
prose

```lemma
this is not lemma
```
";
        let err = format_lemma_fences_in_text(file).expect_err("must fail");
        assert_eq!(err.fence_line, 3);
    }
}
