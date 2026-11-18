//! Drift check: the Zed extension's tree-sitter queries and semantic token rules must
//! cover the lexer keyword catalog, constraint commands, and LSP custom token types.

#[cfg(test)]
mod tests {
    use crate::semantic_tokens::TOKEN_TYPES;
    use lemma::{TypeConstraintCommand, COMPLETION_KEYWORD_LABELS};
    use serde_json::Value;
    use std::collections::BTreeSet;

    const HIGHLIGHTS: &str = include_str!("../editors/zed/languages/lemma/highlights.scm");
    const SEMANTIC_TOKEN_RULES: &str =
        include_str!("../editors/zed/languages/lemma/semantic_token_rules.json");

    /// Token types defined by the LSP 3.17 specification. Zed styles these natively;
    /// every other entry in [`TOKEN_TYPES`] needs a rule in `semantic_token_rules.json`.
    const LSP_STANDARD_TOKEN_TYPES: &[&str] = &[
        "namespace",
        "type",
        "class",
        "enum",
        "interface",
        "struct",
        "typeParameter",
        "parameter",
        "variable",
        "property",
        "enumMember",
        "event",
        "function",
        "method",
        "macro",
        "keyword",
        "modifier",
        "comment",
        "string",
        "number",
        "regexp",
        "operator",
        "decorator",
    ];

    /// Lowercased words in highlights.scm: anonymous node names and `#match?` alternatives.
    fn highlight_words() -> BTreeSet<String> {
        HIGHLIGHTS
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .filter(|w| !w.is_empty())
            .map(str::to_ascii_lowercase)
            .collect()
    }

    #[test]
    fn zed_highlights_cover_completion_keywords() {
        let words = highlight_words();
        let missing: Vec<&str> = COMPLETION_KEYWORD_LABELS
            .iter()
            .copied()
            .filter(|label| !words.contains(&label.to_ascii_lowercase()))
            .collect();
        assert!(
            missing.is_empty(),
            "zed highlights.scm missing COMPLETION_KEYWORD_LABELS: {missing:?}"
        );
    }

    #[test]
    fn zed_highlights_cover_type_constraint_commands() {
        let words = highlight_words();
        let missing: Vec<String> = TypeConstraintCommand::ALL
            .iter()
            .map(ToString::to_string)
            .filter(|spelling| !words.contains(&spelling.to_ascii_lowercase()))
            .collect();
        assert!(
            missing.is_empty(),
            "zed highlights.scm missing TypeConstraintCommand spellings: {missing:?}"
        );
    }

    #[test]
    fn zed_semantic_token_rules_match_custom_token_types() {
        let custom: BTreeSet<String> = TOKEN_TYPES
            .iter()
            .map(|t| t.as_str().to_string())
            .filter(|t| !LSP_STANDARD_TOKEN_TYPES.contains(&t.as_str()))
            .collect();
        let rules: Value = serde_json::from_str(SEMANTIC_TOKEN_RULES)
            .expect("BUG: zed semantic_token_rules.json must be valid JSON");
        let ruled: BTreeSet<String> = rules
            .as_array()
            .expect("BUG: zed semantic_token_rules.json must be a JSON array")
            .iter()
            .map(|rule| {
                rule.get("token_type")
                    .and_then(Value::as_str)
                    .expect("BUG: every zed semantic token rule must name a token_type")
                    .to_string()
            })
            .collect();
        assert_eq!(
            ruled, custom,
            "zed semantic_token_rules.json token types must equal the LSP custom token types"
        );
    }
}
