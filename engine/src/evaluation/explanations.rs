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
    /// Type of [`Self::result`] for measure/ratio rendering (binding unit, decimals).
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
    /// Row label: no tree connector; sits at the child indent.
    None,
}

enum ConversionSteps<'a> {
    Borrowed(&'a [SerializedConversionTraceStep]),
    Owned(Vec<SerializedConversionTraceStep>),
}

impl ConversionSteps<'_> {
    fn as_slice(&self) -> &[SerializedConversionTraceStep] {
        match self {
            ConversionSteps::Borrowed(steps) => steps,
            ConversionSteps::Owned(steps) => steps.as_slice(),
        }
    }
}

/// Visible children of one parent, split once: connected nodes (tree connectors)
/// first, then row labels. Rows always follow the connected nodes.
struct Siblings<'a> {
    connected: Vec<&'a ExplanationNode>,
    rows: Vec<&'a ExplanationNode>,
}

impl<'a> Siblings<'a> {
    fn new(nodes: impl Iterator<Item = &'a ExplanationNode>) -> Self {
        let mut connected = Vec::new();
        let mut rows = Vec::new();
        for node in nodes {
            if is_bare_literal_compose(node) {
                continue;
            }
            if matches!(node, ExplanationNode::Row { .. }) {
                rows.push(node);
            } else if !rows.is_empty() {
                panic!("BUG: non-row explanation node follows a Row sibling");
            } else {
                connected.push(node);
            }
        }
        Self { connected, rows }
    }

    fn len(&self) -> usize {
        self.connected.len() + self.rows.len()
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
    },
    CauseChildren {
        children: Vec<&'a ExplanationNode>,
        indent: String,
        next: usize,
    },
    Conversion {
        steps: ConversionSteps<'a>,
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
        match connector {
            Connector::None => self.lines.push(format!("{indent}{text}")),
            Connector::Branch | Connector::Last => self
                .lines
                .push(format!("{indent}{} {text}", connector_str(connector))),
        }
    }

    fn child_indent(indent: &str, connector: Connector) -> String {
        match connector {
            Connector::Branch => format!("{indent}│  "),
            Connector::Last => format!("{indent}   "),
            Connector::None => format!("{indent}   "),
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
        let body_shown = !body.is_empty() && body != result_line;
        let total = causes.len() + usize::from(body_shown);
        // LIFO: push later output first so first cause prints first.
        if body_shown {
            let child_indent = Self::child_indent(&indent, Connector::Last);
            self.stack.push(Work::Nodes {
                siblings: Siblings::new(children.iter()),
                parent_body: Some(body),
                indent: child_indent,
                next: 0,
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
                } => self.step_nodes(siblings, parent_body, indent, next),
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
    ) {
        if next >= siblings.len() {
            return;
        }
        let connected_len = siblings.connected.len();
        let (node, connector) = if next < connected_len {
            let connector = if next + 1 == connected_len {
                Connector::Last
            } else {
                Connector::Branch
            };
            (siblings.connected[next], connector)
        } else {
            (siblings.rows[next - connected_len], Connector::None)
        };
        self.stack.push(Work::Nodes {
            siblings,
            parent_body,
            indent: indent.clone(),
            next: next + 1,
        });
        self.emit_node(node, connector, parent_body, indent);
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
        self.emit_node(node, connector, None, indent);
    }

    fn step_conversion(
        &mut self,
        steps: ConversionSteps<'a>,
        siblings: Siblings<'a>,
        indent: String,
        next: usize,
    ) {
        let step_len = steps.as_slice().len();
        let total = step_len + siblings.len();
        if next >= total {
            return;
        }
        let connector = if next + 1 == total {
            Connector::Last
        } else {
            Connector::Branch
        };
        if next < step_len {
            let text = steps.as_slice()[next].text.clone();
            self.stack.push(Work::Conversion {
                steps,
                siblings,
                indent: indent.clone(),
                next: next + 1,
            });
            self.push_line(&indent, connector, &text);
            return;
        }
        let op_index = next - step_len;
        let connected_len = siblings.connected.len();
        let (node, connector) = if op_index < connected_len {
            (siblings.connected[op_index], connector)
        } else {
            (siblings.rows[op_index - connected_len], Connector::None)
        };
        self.stack.push(Work::Conversion {
            steps,
            siblings,
            indent: indent.clone(),
            next: next + 1,
        });
        self.emit_node(node, connector, None, indent);
    }

    fn emit_node(
        &mut self,
        node: &'a ExplanationNode,
        connector: Connector,
        parent_body: Option<&'a str>,
        indent: String,
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
                ..
            } => {
                if parent_body.is_some_and(|body| body == expression.as_str()) {
                    self.stack.push(Work::Nodes {
                        siblings: Siblings::new(operands.iter()),
                        parent_body: None,
                        indent,
                        next: 0,
                    });
                } else {
                    self.push_line(&indent, connector, expression);
                    let child_indent = Self::child_indent(&indent, connector);
                    self.stack.push(Work::Nodes {
                        siblings: Siblings::new(operands.iter()),
                        parent_body: None,
                        indent: child_indent,
                        next: 0,
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
            } => {
                let expression_is_parent_body =
                    parent_body.is_some_and(|body| body == expression.as_str());
                if expression_is_parent_body {
                    let owned: Vec<SerializedConversionTraceStep> = steps
                        .iter()
                        .filter(|step| !matches!(step.role, ConversionTraceRole::Outcome))
                        .cloned()
                        .collect();
                    self.stack.push(Work::Conversion {
                        steps: ConversionSteps::Owned(owned),
                        siblings: Siblings::new(operands.iter()),
                        indent,
                        next: 0,
                    });
                } else {
                    self.push_line(&indent, connector, expression);
                    let child_indent = Self::child_indent(&indent, connector);
                    self.stack.push(Work::Conversion {
                        steps: ConversionSteps::Borrowed(steps),
                        siblings: Siblings::new(operands.iter()),
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
                let label = row_label(*number, *appended, key);
                let child_indent = Self::child_indent(&indent, Connector::None);
                if fields.len() == 1 {
                    match &fields[0] {
                        ExplanationNode::RuleRef { id, name, result } => {
                            let result_str = result
                                .result
                                .as_deref()
                                .expect("BUG: RuleRef.result.result not filled by eval");
                            if self.expanded.insert(*id) {
                                self.push_line(
                                    &indent,
                                    Connector::None,
                                    &format!("{label} {name}: {result_str}"),
                                );
                                self.stack.push(Work::RuleBody {
                                    id: *id,
                                    result_line: result_str.to_string(),
                                    indent: child_indent,
                                });
                            } else {
                                self.push_line(
                                    &indent,
                                    Connector::None,
                                    &format!("{label} {name}: {result_str} (see above)"),
                                );
                            }
                        }
                        other => {
                            let field_line = field_headline(other);
                            self.push_line(
                                &indent,
                                Connector::None,
                                &format!("{label} {field_line}"),
                            );
                        }
                    }
                } else {
                    self.push_line(&indent, Connector::None, &label);
                    self.stack.push(Work::Nodes {
                        siblings: Siblings::new(fields.iter()),
                        parent_body: None,
                        indent: child_indent,
                        next: 0,
                    });
                }
            }
            ExplanationNode::Filter {
                condition,
                matched,
                total,
                children,
            } => {
                self.push_line(
                    &indent,
                    connector,
                    &format!("{matched} of {total} are {condition}"),
                );
                let child_indent = Self::child_indent(&indent, connector);
                self.stack.push(Work::Nodes {
                    siblings: Siblings::new(children.iter()),
                    parent_body: None,
                    indent: child_indent,
                    next: 0,
                });
            }
        }
    }
}

fn row_label(number: usize, appended: bool, key: &[ExplanationNode]) -> String {
    if key.is_empty() {
        if appended {
            format!("appended {number}.")
        } else {
            format!("{number}.")
        }
    } else {
        let values: Vec<String> = key.iter().map(key_value_display).collect();
        format!("[{}]", values.join(", "))
    }
}

/// Key values in brackets: text is quoted so it cannot be read as a row number.
fn key_value_display(node: &ExplanationNode) -> String {
    match node {
        ExplanationNode::Data { result, .. } => {
            if let Some(text) = &result.text {
                format!("\"{text}\"")
            } else {
                result
                    .result
                    .as_deref()
                    .expect("BUG: row key Data must carry result")
                    .to_string()
            }
        }
        other => panic!("BUG: row key node is {other:?}, not Data"),
    }
}

fn field_headline(field: &ExplanationNode) -> String {
    match field {
        ExplanationNode::RuleRef { name, result, .. } => {
            let result_str = result
                .result
                .as_deref()
                .expect("BUG: RuleRef.result.result not filled by eval");
            format!("{name}: {result_str}")
        }
        ExplanationNode::Data { name, result } => {
            let result_str = result
                .result
                .as_deref()
                .expect("BUG: Data.result.result not filled by eval");
            if name.data.is_empty() {
                result_str.to_string()
            } else {
                format!("{name}: {result_str}")
            }
        }
        ExplanationNode::DataUnused { name } => name.to_string(),
        ExplanationNode::Veto { message } => match message.as_deref() {
            Some(msg) if !msg.is_empty() => format!("veto \"{msg}\""),
            _ => "veto".to_string(),
        },
        other => panic!("BUG: row field headline for unexpected node {other:?}"),
    }
}

fn connector_str(connector: Connector) -> &'static str {
    match connector {
        Connector::Branch => "├─",
        Connector::Last => "└─",
        Connector::None => "",
    }
}

/// Bare literal compose: expression text already names it; ASCII omits the node.
fn is_bare_literal_compose(node: &ExplanationNode) -> bool {
    matches!(node, ExplanationNode::Compose { operands, .. } if operands.is_empty())
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
