//! Drift check: VS Code TextMate grammar must cover lexer/constraint catalogs.

#[cfg(test)]
mod tests {
    use lemma::{TypeConstraintCommand, COMPLETION_KEYWORD_LABELS};
    use serde_json::Value;

    const GRAMMAR: &str = include_str!("../editors/vscode/syntaxes/lemma.tmLanguage.json");

    fn grammar_json() -> Value {
        serde_json::from_str(GRAMMAR).expect("BUG: lemma.tmLanguage.json must be valid JSON")
    }

    fn collect_match_patterns(value: &Value, out: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                if let Some(Value::String(m)) = map.get("match") {
                    out.push(m.clone());
                }
                for child in map.values() {
                    collect_match_patterns(child, out);
                }
            }
            Value::Array(items) => {
                for child in items {
                    collect_match_patterns(child, out);
                }
            }
            _ => {}
        }
    }

    fn all_match_patterns() -> Vec<String> {
        let mut patterns = Vec::new();
        collect_match_patterns(&grammar_json(), &mut patterns);
        patterns
    }

    #[test]
    fn textmate_covers_completion_keywords() {
        let lower = GRAMMAR.to_ascii_lowercase();
        let mut missing = Vec::new();
        for label in COMPLETION_KEYWORD_LABELS {
            if !lower.contains(&label.to_ascii_lowercase()) {
                missing.push(*label);
            }
        }
        assert!(
            missing.is_empty(),
            "lemma.tmLanguage.json missing COMPLETION_KEYWORD_LABELS: {missing:?}"
        );
    }

    #[test]
    fn textmate_covers_type_constraint_commands() {
        let lower = GRAMMAR.to_ascii_lowercase();
        let mut missing = Vec::new();
        for cmd in TypeConstraintCommand::ALL {
            let spelling = cmd.to_string();
            if !lower.contains(&spelling.to_ascii_lowercase()) {
                missing.push(spelling);
            }
        }
        assert!(
            missing.is_empty(),
            "lemma.tmLanguage.json missing TypeConstraintCommand spellings: {missing:?}"
        );
    }

    #[test]
    fn textmate_comparison_patterns_forbid_equals_ops() {
        let patterns = all_match_patterns();
        let bad: Vec<&str> = patterns
            .iter()
            .filter(|p| p.contains("==") || p.contains("!="))
            .map(String::as_str)
            .collect();
        assert!(
            bad.is_empty(),
            "TextMate match patterns must not contain == or !=: {bad:?}"
        );
    }

    #[test]
    fn textmate_type_keyword_pattern_matches_primitives_and_range() {
        let patterns = all_match_patterns();
        let type_pat = patterns
            .iter()
            .find(|p| {
                p.contains("boolean")
                    && p.contains("measure")
                    && p.contains("number")
                    && p.contains("ratio")
                    && p.contains("text")
                    && p.contains("date")
                    && p.contains("time")
            })
            .expect("BUG: TextMate grammar must have a primitive type match pattern");

        assert!(
            type_pat.contains("range"),
            "type-keyword pattern must include soft suffix range: {type_pat}"
        );
        assert!(
            !type_pat.contains("percent"),
            "type-keyword pattern must not list percent: {type_pat}"
        );
        assert!(
            !type_pat.contains("duration"),
            "type-keyword pattern must not list duration: {type_pat}"
        );
    }

    #[test]
    fn textmate_forbids_outdated_constraint_minimal() {
        let patterns = all_match_patterns();
        let bad: Vec<&str> = patterns
            .iter()
            .filter(|p| p.to_ascii_lowercase().contains("minimal"))
            .map(String::as_str)
            .collect();
        assert!(
            bad.is_empty(),
            "TextMate match patterns must not contain removed constraint minimal: {bad:?}"
        );
    }
}
