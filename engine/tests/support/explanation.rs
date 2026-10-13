//! Helpers for walking shared-rule explanation graphs (typed).

use lemma::{ExplainedRule, ExplainedRuleId, Explanation, ExplanationNode};

/// Look up a rule in [`Explanation::rules`] by public id.
pub fn rule_by_id(explanation: &Explanation, id: ExplainedRuleId) -> &ExplainedRule {
    explanation
        .rules
        .iter()
        .find(|rule| rule.id == id)
        .unwrap_or_else(|| panic!("explanation missing rules entry for id {}", id.0))
}

/// Resolve a [`ExplanationNode::RuleRef`] against the explanation's rules table.
pub fn resolve_ref<'a>(explanation: &'a Explanation, node: &ExplanationNode) -> &'a ExplainedRule {
    match node {
        ExplanationNode::RuleRef { id, .. } => rule_by_id(explanation, *id),
        other => panic!("expected RuleRef, got {other:?}"),
    }
}

/// Collect rule names referenced (as RuleRef) under `nodes`, without expanding the table.
pub fn rule_ref_names(nodes: &[ExplanationNode]) -> Vec<String> {
    let mut names = Vec::new();
    fn walk(node: &ExplanationNode, names: &mut Vec<String>) {
        match node {
            ExplanationNode::RuleRef { name, .. } => names.push(name.rule.clone()),
            ExplanationNode::Compose { operands, .. }
            | ExplanationNode::Conversion { operands, .. } => {
                for child in operands {
                    walk(child, names);
                }
            }
            ExplanationNode::Row { key, fields, .. } => {
                for child in key.iter().chain(fields.iter()) {
                    walk(child, names);
                }
            }
            ExplanationNode::Filter { children, .. } => {
                for child in children {
                    walk(child, names);
                }
            }
            ExplanationNode::Data { .. }
            | ExplanationNode::DataUnused { .. }
            | ExplanationNode::Veto { .. } => {}
        }
    }
    for node in nodes {
        walk(node, &mut names);
    }
    names
}
