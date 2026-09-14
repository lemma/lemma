//! Root explanation type and formatting.
//!
//! The root `Explanation` (with `result: OperationResult`) is built once per
//! requested rule from its narrated Rule node. The tree types (`ExplanationNode`,
//! `Cause`, `SerializedConversionTraceStep`) live in `planning::explanation`;
//! `evaluation::narration` builds them from the filled value table.

use crate::computation::{OperationResult, VetoType};
use crate::planning::semantics::{LemmaType, RulePath};
use serde::Serialize;
use std::sync::Arc;

// Re-export tree types for use within the evaluation module
pub use crate::planning::explanation::{
    Cause, ConversionTraceRole, ExplanationNode, SerializedConversionTraceStep,
};

#[derive(Debug, Clone)]
pub struct Explanation {
    pub name: RulePath,
    pub result: OperationResult,
    /// Type of [`Self::result`] for measure/ratio display (binding unit, decimals).
    pub result_type: Arc<LemmaType>,
    pub body: String,
    pub causes: Vec<Cause>,
    pub children: Vec<ExplanationNode>,
}

impl Serialize for Explanation {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ExplanationNode::Rule {
            name: self.name.clone(),
            result: Some(format_operation_result(
                &self.result,
                self.result_type.as_ref(),
            )),
            body: self.body.clone(),
            causes: self.causes.clone(),
            children: self.children.clone(),
        }
        .serialize(serializer)
    }
}

pub(crate) fn format_operation_result(result: &OperationResult, result_type: &LemmaType) -> String {
    match result {
        OperationResult::Value(value) => value.display_value_with_type(result_type),
        OperationResult::Veto(VetoType::UserDefined { message: None }) => String::new(),
        OperationResult::Veto(veto) => veto.to_string(),
    }
}

pub fn format_explanation(explanation: &Explanation) -> String {
    let mut lines = Vec::new();
    let result_display =
        format_operation_result(&explanation.result, explanation.result_type.as_ref());
    lines.push(format!("{}: {}", explanation.name.rule, result_display));
    let mut ctx = FormatContext {
        lines: &mut lines,
        indent: String::new(),
    };
    ctx.render_rule_contents(
        &result_display,
        &explanation.body,
        &explanation.causes,
        &explanation.children,
    );
    lines.join("\n")
}

#[derive(Copy, Clone)]
enum Connector {
    Branch,
    Last,
}

struct FormatContext<'a> {
    lines: &'a mut Vec<String>,
    indent: String,
}

impl<'a> FormatContext<'a> {
    fn push_line(&mut self, connector: Connector, text: &str) {
        self.lines.push(format!(
            "{}{} {text}",
            self.indent,
            connector_str(connector)
        ));
    }

    fn child_indent(&self, connector: Connector) -> String {
        match connector {
            Connector::Branch => format!("{}│  ", self.indent),
            Connector::Last => format!("{}   ", self.indent),
        }
    }

    fn render_rule_contents(
        &mut self,
        result_display: &str,
        body: &str,
        causes: &[Cause],
        children: &[ExplanationNode],
    ) {
        let body_shown = !body.is_empty() && body != result_display;
        let total = causes.len() + usize::from(body_shown);
        let mut index = 0;

        for cause in causes {
            index += 1;
            let connector = if index == total {
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
            self.push_line(connector, &line);
            let child_indent = self.child_indent(connector);
            let mut child_ctx = FormatContext {
                lines: self.lines,
                indent: child_indent,
            };
            child_ctx.render_cause_children(cause, value, &line);
        }

        if body_shown {
            self.push_line(Connector::Last, body);
            let child_indent = self.child_indent(Connector::Last);
            let mut child_ctx = FormatContext {
                lines: self.lines,
                indent: child_indent,
            };
            child_ctx.render_nodes(children, Some(body));
        } else if !children.is_empty() {
            self.render_nodes(children, None);
        }
    }

    /// Cause children for ASCII: omit bare literals and Data that only restates
    /// a `name is display` cause line (JSON keeps the structured child).
    fn render_cause_children(&mut self, cause: &Cause, value: &str, line: &str) {
        let visible: Vec<&ExplanationNode> = cause
            .children
            .iter()
            .filter(|node| !is_bare_literal_compose(node))
            .filter(|node| !data_restates_true_cause_line(node, value, line))
            .collect();
        let len = visible.len();
        for (i, node) in visible.into_iter().enumerate() {
            let connector = if i + 1 == len {
                Connector::Last
            } else {
                Connector::Branch
            };
            self.render_node(node, connector, None);
        }
    }

    fn render_nodes(&mut self, nodes: &[ExplanationNode], parent_body: Option<&str>) {
        let visible: Vec<&ExplanationNode> = nodes
            .iter()
            .filter(|node| !is_bare_literal_compose(node))
            .collect();
        let len = visible.len();
        for (i, node) in visible.into_iter().enumerate() {
            let connector = if i + 1 == len {
                Connector::Last
            } else {
                Connector::Branch
            };
            self.render_node(node, connector, parent_body);
        }
    }

    fn render_conversion_contents(
        &mut self,
        steps: &[SerializedConversionTraceStep],
        operands: &[ExplanationNode],
    ) {
        let visible_operands: Vec<&ExplanationNode> = operands
            .iter()
            .filter(|node| !is_bare_literal_compose(node))
            .collect();
        let total = steps.len() + visible_operands.len();
        let mut index = 0;
        for step in steps {
            index += 1;
            let connector = if index == total {
                Connector::Last
            } else {
                Connector::Branch
            };
            self.push_line(connector, &step.text);
        }
        for operand in visible_operands {
            index += 1;
            let connector = if index == total {
                Connector::Last
            } else {
                Connector::Branch
            };
            self.render_node(operand, connector, None);
        }
    }

    fn render_node(
        &mut self,
        node: &ExplanationNode,
        connector: Connector,
        parent_body: Option<&str>,
    ) {
        match node {
            ExplanationNode::Rule {
                name,
                result,
                body,
                causes,
                children,
            } => {
                let result_str = result
                    .as_deref()
                    .expect("BUG: ExplanationNode::Rule.result not filled by eval");
                self.push_line(connector, &format!("{}: {result_str}", name.rule));
                let child_indent = self.child_indent(connector);
                let mut child_ctx = FormatContext {
                    lines: self.lines,
                    indent: child_indent,
                };
                child_ctx.render_rule_contents(result_str, body, causes, children);
            }
            ExplanationNode::Compose {
                expression,
                operands,
            } => {
                if parent_body.is_some_and(|body| body == expression) {
                    self.render_nodes(operands, None);
                } else {
                    self.push_line(connector, expression);
                    let child_indent = self.child_indent(connector);
                    let mut child_ctx = FormatContext {
                        lines: self.lines,
                        indent: child_indent,
                    };
                    child_ctx.render_nodes(operands, None);
                }
            }
            ExplanationNode::Data { name, display } => {
                if name.data.is_empty() {
                    self.push_line(connector, display);
                } else {
                    self.push_line(connector, &format!("{name}: {display}"));
                }
            }
            ExplanationNode::DataUnused { name } => {
                self.push_line(connector, &name.to_string());
            }
            ExplanationNode::Conversion {
                expression,
                steps,
                operands,
            } => {
                let expression_is_parent_body = parent_body.is_some_and(|body| body == expression);
                if expression_is_parent_body {
                    let steps_without_outcome: Vec<SerializedConversionTraceStep> = steps
                        .iter()
                        .filter(|step| !matches!(step.role, ConversionTraceRole::Outcome))
                        .cloned()
                        .collect();
                    self.render_conversion_contents(&steps_without_outcome, operands);
                } else {
                    self.push_line(connector, expression);
                    let child_indent = self.child_indent(connector);
                    let mut child_ctx = FormatContext {
                        lines: self.lines,
                        indent: child_indent,
                    };
                    child_ctx.render_conversion_contents(steps, operands);
                }
            }
            ExplanationNode::Veto { message } => {
                let text = match message.as_deref() {
                    Some(msg) if !msg.is_empty() => format!("veto \"{msg}\""),
                    _ => "veto".to_string(),
                };
                self.push_line(connector, &text);
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

/// Bare literal compose: expression text already names it; ASCII omits the node.
fn is_bare_literal_compose(node: &ExplanationNode) -> bool {
    matches!(node, ExplanationNode::Compose { operands, .. } if operands.is_empty())
}

/// `code is NL` already states the binding; ASCII skips child `code: NL`.
/// Flipped / inequality cause lines do not match `{key} is {display}`.
fn data_restates_true_cause_line(node: &ExplanationNode, value: &str, line: &str) -> bool {
    if value != "true" {
        return false;
    }
    match node {
        ExplanationNode::Data { name, display } => {
            line == format!("{} is {display}", name.input_key())
        }
        _ => false,
    }
}
