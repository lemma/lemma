//! Root explanation type and formatting.
//!
//! The root `Explanation` (with `result: OperationResult`) is built once per
//! requested rule from its narrated arena entry. Nested rule uses are
//! [`ExplanationNode::RuleRef`] into [`Explanation::rules`]. The tree types
//! live in `planning::explanation`; `evaluation::narration` builds them from
//! the filled value table.

use crate::computation::{OperationResult, VetoType};
use crate::planning::explanation::{ExplainedRule, ExplainedRuleId};
use crate::planning::semantics::{LemmaType, RulePath};
use crate::planning::unit_family::FamilyUnitCatalog;
use crate::result_value::{
    rule_result_value_failure_message, rule_result_value_from_literal, RuleResultValue,
    RuleResultValueFailure,
};
use serde::Serialize;
use std::collections::HashSet;
use std::sync::Arc;

// Re-export tree types for use within the evaluation module
pub use crate::planning::explanation::{
    Cause, ConversionTraceRole, ExplanationNode, SerializedConversionTraceStep,
};

#[derive(Debug, Clone)]
pub struct Explanation {
    pub name: RulePath,
    pub result: OperationResult,
    /// Type of [`Self::result`] for measure/ratio rendering (binding unit).
    pub result_type: Arc<LemmaType>,
    /// Family catalog needed to build the rule-node [`RuleResultValue`] on serialize.
    pub(crate) family_units: Arc<FamilyUnitCatalog>,
    pub body: String,
    pub causes: Vec<Cause>,
    pub children: Vec<ExplanationNode>,
    /// Reachable narrated rules, ids in first-reach order from this root.
    pub rules: Vec<ExplainedRule>,
}

#[derive(Serialize)]
struct ExplanationSer<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(serialize_with = "crate::planning::explanation::serialize_rule_path_as_name")]
    name: &'a RulePath,
    #[serde(flatten)]
    result: RuleResultValue,
    body: &'a str,
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    causes: &'a [Cause],
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    children: &'a [ExplanationNode],
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    rules: &'a [ExplainedRule],
}

impl Serialize for Explanation {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ExplanationSer {
            kind: "rule",
            name: &self.name,
            result: explanation_result_value(
                &self.result,
                self.result_type.as_ref(),
                &self.family_units,
            ),
            body: &self.body,
            causes: self.causes.as_slice(),
            children: self.children.as_slice(),
            rules: self.rules.as_slice(),
        }
        .serialize(serializer)
    }
}

pub(crate) fn format_operation_result(result: &OperationResult, result_type: &LemmaType) -> String {
    match result {
        OperationResult::Value(bound) => bound.to_literal().display_value_with_type(result_type),
        OperationResult::Veto(VetoType::UserDefined { message: None }) => String::new(),
        OperationResult::Veto(veto) => veto.to_string(),
    }
}

/// Build a [`RuleResultValue`] for an explanation node from a settled operation result.
pub(crate) fn explanation_result_value(
    result: &OperationResult,
    lemma_type: &LemmaType,
    family_units: &FamilyUnitCatalog,
) -> RuleResultValue {
    match result {
        OperationResult::Value(bound) => {
            match rule_result_value_from_literal(&bound.to_literal(), lemma_type, family_units) {
                Ok(value) => value,
                // Same veto message RuleResult attaches; result is veto text, not magnitude.
                Err(RuleResultValueFailure::DecimalLimit) => RuleResultValue {
                    result: Some(
                        rule_result_value_failure_message(RuleResultValueFailure::DecimalLimit)
                            .to_string(),
                    ),
                    ..RuleResultValue::default()
                },
                Err(failure) => panic!(
                    "BUG: settled explanation value failed rule_result_value_from_literal: {}",
                    rule_result_value_failure_message(failure)
                ),
            }
        }
        OperationResult::Veto(VetoType::UserDefined { message: None }) => RuleResultValue {
            result: Some(String::new()),
            ..RuleResultValue::default()
        },
        OperationResult::Veto(veto) => RuleResultValue {
            result: Some(veto.to_string()),
            ..RuleResultValue::default()
        },
    }
}

pub fn format_explanation(explanation: &Explanation) -> String {
    let mut lines = Vec::new();
    let result_line = explanation_result_value(
        &explanation.result,
        explanation.result_type.as_ref(),
        &explanation.family_units,
    )
    .result
    .expect("BUG: root explanation must carry result");
    lines.push(format!("{}: {}", explanation.name, result_line));
    let mut state = FormatState {
        lines: &mut lines,
        rules: &explanation.rules,
        expanded: HashSet::new(),
        stack: Vec::new(),
    };
    state.push_rule_contents(
        &result_line,
        &explanation.body,
        &explanation.causes,
        &explanation.children,
        String::new(),
    );
    state.run();
    lines.join("\n")
}

#[derive(Copy, Clone)]
enum Connector {
    Branch,
    Last,
}

enum ConversionSteps {
    Owned(Vec<SerializedConversionTraceStep>),
}

impl ConversionSteps {
    fn as_slice(&self) -> &[SerializedConversionTraceStep] {
        match self {
            ConversionSteps::Owned(steps) => steps.as_slice(),
        }
    }
}

/// Visible children of one parent, with tree connectors. `Filter` nodes are
/// flattened away so matched rows sit directly under the list pipeline.
struct Siblings<'a> {
    nodes: Vec<&'a ExplanationNode>,
    /// Pre-aligned comma headlines for [`ExplanationNode::Row`] entries, keyed by index.
    row_headlines: std::collections::HashMap<usize, String>,
}

impl<'a> Siblings<'a> {
    fn new(nodes: impl Iterator<Item = &'a ExplanationNode>) -> Self {
        let mut flat = Vec::new();
        for node in nodes {
            flatten_visible(node, &mut flat);
        }
        let row_headlines = aligned_row_headlines(&flat);
        Self {
            nodes: flat,
            row_headlines,
        }
    }

    fn len(&self) -> usize {
        self.nodes.len()
    }
}

fn flatten_visible<'a>(node: &'a ExplanationNode, out: &mut Vec<&'a ExplanationNode>) {
    let mut pending = vec![node];
    while let Some(node) = pending.pop() {
        if is_bare_literal_compose(node) || is_visually_empty_compose(node) {
            continue;
        }
        match node {
            ExplanationNode::Filter { children, .. } => {
                for child in children.iter().rev() {
                    pending.push(child);
                }
            }
            _ => out.push(node),
        }
    }
}

enum Work<'a> {
    Line {
        indent: String,
        connector: Connector,
        text: String,
    },
    RuleBody {
        id: ExplainedRuleId,
        result_line: String,
        indent: String,
    },
    Nodes {
        siblings: Siblings<'a>,
        parent_body: Option<&'a str>,
        indent: String,
        next: usize,
        elide_list_commands: bool,
    },
    CauseChildren {
        children: Vec<&'a ExplanationNode>,
        indent: String,
        next: usize,
    },
    Conversion {
        steps: ConversionSteps,
        siblings: Siblings<'a>,
        indent: String,
        next: usize,
    },
}

struct FormatState<'a> {
    lines: &'a mut Vec<String>,
    rules: &'a [ExplainedRule],
    expanded: HashSet<ExplainedRuleId>,
    stack: Vec<Work<'a>>,
}

impl<'a> FormatState<'a> {
    fn push_line(&mut self, indent: &str, connector: Connector, text: &str) {
        self.lines
            .push(format!("{indent}{} {text}", connector_str(connector)));
    }

    fn child_indent(indent: &str, connector: Connector) -> String {
        match connector {
            Connector::Branch => format!("{indent}│  "),
            Connector::Last => format!("{indent}   "),
        }
    }

    fn rule_by_id(&self, id: ExplainedRuleId) -> &'a ExplainedRule {
        self.rules
            .iter()
            .find(|rule| rule.id == id)
            .unwrap_or_else(|| panic!("BUG: explanation missing rules entry for id {}", id.0))
    }

    fn push_rule_contents(
        &mut self,
        result_line: &str,
        body: &'a str,
        causes: &'a [Cause],
        children: &'a [ExplanationNode],
        indent: String,
    ) {
        // Bare data-path rule bodies (`rule guardian: has_ward`) already appear as the
        // sole Data child (`has_ward: true`); printing the path again nests a duplicate.
        let body_restated_by_sole_data = matches!(
            children,
            [ExplanationNode::Data { name, .. }] if name.input_key() == body
        );
        let body_shown = !body.is_empty() && body != result_line && !body_restated_by_sole_data;
        let total = causes.len() + usize::from(body_shown);
        // LIFO: push later output first so first cause prints first.
        if body_shown {
            let child_indent = Self::child_indent(&indent, Connector::Last);
            self.stack.push(Work::Nodes {
                siblings: Siblings::new(children.iter()),
                parent_body: Some(body),
                indent: child_indent,
                next: 0,
                elide_list_commands: false,
            });
            self.stack.push(Work::Line {
                indent: indent.clone(),
                connector: Connector::Last,
                text: body.to_string(),
            });
        } else if !children.is_empty() {
            self.stack.push(Work::Nodes {
                siblings: Siblings::new(children.iter()),
                parent_body: None,
                indent: indent.clone(),
                next: 0,
                elide_list_commands: false,
            });
        }
        for (i, cause) in causes.iter().enumerate().rev() {
            let connector = if i + 1 == total {
                Connector::Last
            } else {
                Connector::Branch
            };
            let value = cause.value.as_str();
            let line = if value == "true" {
                cause.condition.clone()
            } else {
                format!("{} is {}", cause.condition, value)
            };
            let visible: Vec<&'a ExplanationNode> = cause
                .children
                .iter()
                .filter(|node| !is_bare_literal_compose(node))
                .filter(|node| !data_restates_true_cause_line(node, value, &line))
                .collect();
            let child_indent = Self::child_indent(&indent, connector);
            self.stack.push(Work::CauseChildren {
                children: visible,
                indent: child_indent,
                next: 0,
            });
            self.stack.push(Work::Line {
                indent: indent.clone(),
                connector,
                text: line,
            });
        }
    }

    fn run(&mut self) {
        while let Some(work) = self.stack.pop() {
            match work {
                Work::Line {
                    indent,
                    connector,
                    text,
                } => self.push_line(&indent, connector, &text),
                Work::RuleBody {
                    id,
                    result_line,
                    indent,
                } => {
                    let rule = self.rule_by_id(id);
                    self.push_rule_contents(
                        &result_line,
                        rule.body.as_str(),
                        rule.causes.as_slice(),
                        rule.children.as_slice(),
                        indent,
                    );
                }
                Work::Nodes {
                    siblings,
                    parent_body,
                    indent,
                    next,
                    elide_list_commands,
                } => self.step_nodes(siblings, parent_body, indent, next, elide_list_commands),
                Work::CauseChildren {
                    children,
                    indent,
                    next,
                } => self.step_cause_children(children, indent, next),
                Work::Conversion {
                    steps,
                    siblings,
                    indent,
                    next,
                } => self.step_conversion(steps, siblings, indent, next),
            }
        }
    }

    fn step_nodes(
        &mut self,
        siblings: Siblings<'a>,
        parent_body: Option<&'a str>,
        indent: String,
        next: usize,
        elide_list_commands: bool,
    ) {
        if next >= siblings.len() {
            return;
        }
        let connector = if next + 1 == siblings.len() {
            Connector::Last
        } else {
            Connector::Branch
        };
        let node = siblings.nodes[next];
        let row_headline = siblings.row_headlines.get(&next).cloned();
        self.stack.push(Work::Nodes {
            siblings,
            parent_body,
            indent: indent.clone(),
            next: next + 1,
            elide_list_commands,
        });
        self.emit_node(
            node,
            connector,
            parent_body,
            indent,
            elide_list_commands,
            row_headline,
        );
    }

    fn step_cause_children(
        &mut self,
        children: Vec<&'a ExplanationNode>,
        indent: String,
        next: usize,
    ) {
        if next >= children.len() {
            return;
        }
        let connector = if next + 1 == children.len() {
            Connector::Last
        } else {
            Connector::Branch
        };
        let node = children[next];
        self.stack.push(Work::CauseChildren {
            children,
            indent: indent.clone(),
            next: next + 1,
        });
        self.emit_node(node, connector, None, indent, false, None);
    }

    fn step_conversion(
        &mut self,
        steps: ConversionSteps,
        siblings: Siblings<'a>,
        indent: String,
        next: usize,
    ) {
        let sibling_len = siblings.len();
        let step_len = steps.as_slice().len();
        let total = step_len + sibling_len;
        if next >= total {
            return;
        }
        let connector = if next + 1 == total {
            Connector::Last
        } else {
            Connector::Branch
        };
        if next < sibling_len {
            let node = siblings.nodes[next];
            let row_headline = siblings.row_headlines.get(&next).cloned();
            self.stack.push(Work::Conversion {
                steps,
                siblings,
                indent: indent.clone(),
                next: next + 1,
            });
            self.emit_node(node, connector, None, indent, false, row_headline);
            return;
        }
        let step_index = next - sibling_len;
        let text = steps.as_slice()[step_index].text.clone();
        self.stack.push(Work::Conversion {
            steps,
            siblings,
            indent: indent.clone(),
            next: next + 1,
        });
        self.push_line(&indent, connector, &text);
    }

    fn emit_node(
        &mut self,
        node: &'a ExplanationNode,
        connector: Connector,
        parent_body: Option<&'a str>,
        indent: String,
        elide_list_commands: bool,
        row_headline: Option<String>,
    ) {
        match node {
            ExplanationNode::RuleRef { id, name, result } => {
                let result_str = result
                    .result
                    .as_deref()
                    .expect("BUG: ExplanationNode::RuleRef.result.result not filled by eval");
                if self.expanded.insert(*id) {
                    self.push_line(&indent, connector, &format!("{name}: {result_str}"));
                    self.stack.push(Work::RuleBody {
                        id: *id,
                        result_line: result_str.to_string(),
                        indent: Self::child_indent(&indent, connector),
                    });
                } else {
                    self.push_line(
                        &indent,
                        connector,
                        &format!("{name}: {result_str} (see above)"),
                    );
                }
            }
            ExplanationNode::Compose {
                expression,
                operands,
                result: _,
                ..
            } => {
                let siblings = Siblings::new(operands.iter());
                if parent_body.is_some_and(|body| body == expression.as_str()) {
                    self.stack.push(Work::Nodes {
                        siblings,
                        parent_body: None,
                        indent,
                        next: 0,
                        elide_list_commands,
                    });
                } else {
                    self.push_line(&indent, connector, expression);
                    let child_indent = Self::child_indent(&indent, connector);
                    self.stack.push(Work::Nodes {
                        siblings,
                        parent_body: None,
                        indent: child_indent,
                        next: 0,
                        elide_list_commands,
                    });
                }
            }
            ExplanationNode::ListCommand {
                expression,
                operands,
                result: _,
                ..
            } => {
                let elide = elide_list_commands
                    || parent_body.is_some_and(|body| body == expression.as_str());
                if elide {
                    self.stack.push(Work::Nodes {
                        siblings: Siblings::new(operands.iter()),
                        parent_body: None,
                        indent,
                        next: 0,
                        elide_list_commands: true,
                    });
                } else {
                    self.push_line(&indent, connector, expression);
                    let child_indent = Self::child_indent(&indent, connector);
                    self.stack.push(Work::Nodes {
                        siblings: Siblings::new(operands.iter()),
                        parent_body: None,
                        indent: child_indent,
                        next: 0,
                        elide_list_commands: true,
                    });
                }
            }
            ExplanationNode::Data { name, result } => {
                let result_str = result
                    .result
                    .as_deref()
                    .expect("BUG: ExplanationNode::Data.result.result not filled by eval");
                if name.data.is_empty() {
                    self.push_line(&indent, connector, result_str);
                } else {
                    self.push_line(&indent, connector, &format!("{name}: {result_str}"));
                }
            }
            ExplanationNode::DataUnused { name } => {
                self.push_line(&indent, connector, &name.to_string());
            }
            ExplanationNode::Conversion {
                expression,
                steps,
                operands,
                result: _,
            } => {
                let expression_is_parent_body =
                    parent_body.is_some_and(|body| body == expression.as_str());
                let rule_steps: Vec<SerializedConversionTraceStep> = steps
                    .iter()
                    .filter(|step| !matches!(step.role, ConversionTraceRole::Outcome))
                    .cloned()
                    .collect();
                // Rule 2: when the single operand is a Compose, skip the compose line
                // (its expression is already on this conversion line) and promote the
                // compose's operands to direct children.
                let siblings = if let [ExplanationNode::Compose {
                    operands: compose_operands,
                    ..
                }] = operands.as_slice()
                {
                    Siblings::new(compose_operands.iter())
                } else {
                    Siblings::new(operands.iter())
                };
                if expression_is_parent_body {
                    self.stack.push(Work::Conversion {
                        steps: ConversionSteps::Owned(rule_steps),
                        siblings,
                        indent,
                        next: 0,
                    });
                } else {
                    self.push_line(&indent, connector, expression);
                    let child_indent = Self::child_indent(&indent, connector);
                    self.stack.push(Work::Conversion {
                        steps: ConversionSteps::Owned(rule_steps),
                        siblings,
                        indent: child_indent,
                        next: 0,
                    });
                }
            }
            ExplanationNode::Veto { message } => {
                let text = match message.as_deref() {
                    Some(msg) if !msg.is_empty() => format!("veto \"{msg}\""),
                    _ => "veto".to_string(),
                };
                self.push_line(&indent, connector, &text);
            }
            ExplanationNode::Row {
                number,
                appended,
                key,
                fields,
            } => {
                let tree_fields = row_tree_fields(key, fields);
                // Numbered row whose only field is a RuleRef: `2. bonus: 950 eur`.
                if key.is_empty()
                    && row_comma_columns(key, fields).is_empty()
                    && tree_fields.len() == 1
                {
                    if let ExplanationNode::RuleRef { id, name, result } = tree_fields[0] {
                        let result_str = result
                            .result
                            .as_deref()
                            .expect("BUG: RuleRef.result.result not filled by eval");
                        let prefix = if *appended {
                            format!("appended {number}.")
                        } else {
                            format!("{number}.")
                        };
                        let child_indent = Self::child_indent(&indent, connector);
                        if self.expanded.insert(*id) {
                            self.push_line(
                                &indent,
                                connector,
                                &format!("{prefix} {name}: {result_str}"),
                            );
                            self.stack.push(Work::RuleBody {
                                id: *id,
                                result_line: result_str.to_string(),
                                indent: child_indent,
                            });
                        } else {
                            self.push_line(
                                &indent,
                                connector,
                                &format!("{prefix} {name}: {result_str} (see above)"),
                            );
                        }
                        return;
                    }
                }
                let headline = row_headline
                    .unwrap_or_else(|| row_headline_for(*number, *appended, key, fields));
                self.push_line(&indent, connector, &headline);
                if !tree_fields.is_empty() {
                    let child_indent = Self::child_indent(&indent, connector);
                    self.stack.push(Work::Nodes {
                        siblings: Siblings::new(tree_fields.into_iter()),
                        parent_body: None,
                        indent: child_indent,
                        next: 0,
                        elide_list_commands,
                    });
                }
            }
            ExplanationNode::Filter { .. } => {
                unreachable!("BUG: Filter nodes are flattened in Siblings::new");
            }
        }
    }
}

fn connector_str(connector: Connector) -> &'static str {
    match connector {
        Connector::Branch => "├─",
        Connector::Last => "└─",
    }
}

fn aligned_row_headlines(nodes: &[&ExplanationNode]) -> std::collections::HashMap<usize, String> {
    let mut out = std::collections::HashMap::new();
    let mut index = 0;
    while index < nodes.len() {
        if !matches!(nodes[index], ExplanationNode::Row { .. }) {
            index += 1;
            continue;
        }
        let start = index;
        while index < nodes.len() && matches!(nodes[index], ExplanationNode::Row { .. }) {
            index += 1;
        }
        let rows = &nodes[start..index];
        let branched = rows.iter().any(|node| {
            let ExplanationNode::Row { key, fields, .. } = node else {
                unreachable!("BUG: row run contains non-Row");
            };
            !row_tree_fields(key, fields).is_empty()
        });
        let columns: Vec<Vec<RowColumn>> = rows
            .iter()
            .map(|node| {
                let ExplanationNode::Row { key, fields, .. } = node else {
                    unreachable!("BUG: row run contains non-Row");
                };
                row_comma_columns(key, fields)
            })
            .collect();
        let col_widths = if branched {
            Vec::new()
        } else {
            let width = columns.iter().map(|cols| cols.len()).max().unwrap_or(0);
            let mut widths = vec![0; width];
            for cols in &columns {
                for (i, column) in cols.iter().enumerate() {
                    widths[i] = widths[i].max(column.text.len());
                }
            }
            widths
        };
        for (offset, (node, cols)) in rows.iter().zip(columns.iter()).enumerate() {
            let ExplanationNode::Row {
                number, appended, ..
            } = node
            else {
                unreachable!("BUG: row run contains non-Row");
            };
            let line = if branched {
                cols.iter()
                    .map(|column| column.text.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            } else {
                let mut line = String::new();
                for (i, column) in cols.iter().enumerate() {
                    let text = &column.text;
                    if i + 1 == cols.len() {
                        line.push_str(text);
                    } else {
                        line.push_str(text);
                        line.push(',');
                        let pad = col_widths[i].saturating_sub(text.len()) + 1;
                        line.push_str(&" ".repeat(pad));
                    }
                }
                line
            };
            let all_appended = rows
                .iter()
                .all(|row| matches!(row, ExplanationNode::Row { appended: true, .. }));
            let headline = if key_is_empty(node) && !all_appended {
                let prefix = if *appended {
                    format!("appended {number}.")
                } else {
                    format!("{number}.")
                };
                if line.is_empty() {
                    prefix
                } else {
                    format!("{prefix} {line}")
                }
            } else {
                line
            };
            out.insert(start + offset, headline);
        }
    }
    out
}

fn key_is_empty(node: &ExplanationNode) -> bool {
    matches!(node, ExplanationNode::Row { key, .. } if key.is_empty())
}

fn row_headline_for(
    number: usize,
    appended: bool,
    key: &[ExplanationNode],
    fields: &[ExplanationNode],
) -> String {
    let cols = row_comma_columns(key, fields);
    let comma = cols
        .iter()
        .map(|column| column.text.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    if key.is_empty() {
        let prefix = if appended {
            format!("appended {number}.")
        } else {
            format!("{number}.")
        };
        if comma.is_empty() {
            prefix
        } else {
            format!("{prefix} {comma}")
        }
    } else {
        comma
    }
}

/// One comma column of a row line.
struct RowColumn {
    name: String,
    text: String,
}

fn row_comma_columns(key: &[ExplanationNode], fields: &[ExplanationNode]) -> Vec<RowColumn> {
    let mut cols = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for node in key {
        let column = row_column(node);
        seen.insert(column.name.clone());
        cols.push(column);
    }
    for node in fields {
        if !is_comma_field(node) {
            continue;
        }
        let column = row_column(node);
        if seen.insert(column.name.clone()) {
            cols.push(column);
        }
    }
    cols
}

fn row_tree_fields<'a>(
    key: &'a [ExplanationNode],
    fields: &'a [ExplanationNode],
) -> Vec<&'a ExplanationNode> {
    let _ = key;
    fields.iter().filter(|node| !is_comma_field(node)).collect()
}

fn is_comma_field(node: &ExplanationNode) -> bool {
    matches!(
        node,
        ExplanationNode::Data { .. }
            | ExplanationNode::DataUnused { .. }
            | ExplanationNode::Veto { .. }
    )
}

fn row_column(node: &ExplanationNode) -> RowColumn {
    match node {
        ExplanationNode::Data { name, result } => {
            let value = result
                .result
                .as_deref()
                .expect("BUG: row Data must carry result");
            RowColumn {
                name: name.to_string(),
                text: format!("{name}: {value}"),
            }
        }
        ExplanationNode::DataUnused { name } => RowColumn {
            name: name.to_string(),
            text: name.to_string(),
        },
        ExplanationNode::Veto { message } => {
            let value = match message.as_deref() {
                Some(msg) if !msg.is_empty() => format!("veto \"{msg}\""),
                _ => "veto".to_string(),
            };
            RowColumn {
                name: String::new(),
                text: format!(": {value}"),
            }
        }
        other => panic!("BUG: comma column node is {other:?}"),
    }
}

/// Bare literal compose: expression text already names it; ASCII omits the node.
fn is_bare_literal_compose(node: &ExplanationNode) -> bool {
    matches!(node, ExplanationNode::Compose { operands, .. } if operands.is_empty())
}

/// Visually empty compose: a `Compose` whose every operand is itself omitted by
/// the formatter (bare literal, or recursively visually empty). Its line would
/// only restate a sub-expression already shown on the parent and Rule 7 strips
/// its value, so it is dropped from siblings before tree connectors are assigned.
fn is_visually_empty_compose(node: &ExplanationNode) -> bool {
    let ExplanationNode::Compose { operands, .. } = node else {
        return false;
    };
    operands.iter().all(operand_omitted_from_tree)
}

fn operand_omitted_from_tree(node: &ExplanationNode) -> bool {
    if is_bare_literal_compose(node) {
        return true;
    }
    is_visually_empty_compose(node)
}

/// `code is NL` already states the binding; ASCII skips child `code: NL`.
/// Flipped / inequality cause lines do not match `{key} is {result}`.
fn data_restates_true_cause_line(node: &ExplanationNode, value: &str, line: &str) -> bool {
    if value != "true" {
        return false;
    }
    match node {
        ExplanationNode::Data {
            name,
            result: node_result,
        } => {
            let result_str = node_result
                .result
                .as_deref()
                .expect("BUG: Data node must carry result");
            line == format!("{} is {result_str}", name.input_key())
        }
        _ => false,
    }
}
