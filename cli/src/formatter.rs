use lemma::{format_explanation, Response, RuleResult};
use super_table::{presets, Cell, CellAlignment, Table};

pub struct RepositorySpecGroup<'a> {
    pub repository: Option<&'a str>,
    pub specs: &'a [String],
}

pub struct Formatter;

impl Default for Formatter {
    fn default() -> Self {
        Self
    }
}

impl Formatter {
    /// Format evaluation response. When `explain` is false: one line for a single rule, or one table
    /// for multiple rules. When true: reasoning tables per rule.
    pub fn format_response(&self, response: &Response, explain: bool) -> String {
        if response.results.is_empty() {
            return String::new();
        }

        if explain {
            return self.format_response_explain(response);
        }

        if response.results.len() == 1 {
            let result = response
                .results
                .values()
                .next()
                .expect("BUG: len==1 but no values");
            return format!("{}\n", self.format_rule_result_line(result));
        }

        let mut table = Table::new();
        table.load_preset(presets::UTF8_FULL);
        table.set_style(super_table::TableComponent::MiddleIntersections, '┼');
        table.set_style(super_table::TableComponent::HorizontalLines, '─');
        for result in response.results.values() {
            table.add_row(vec![
                Cell::new(&result.rule.name).set_alignment(CellAlignment::Left),
                Cell::new(self.format_rule_result_line(result)).set_alignment(CellAlignment::Left),
            ]);
        }
        format!("{}\n", table)
    }

    fn format_response_explain(&self, response: &Response) -> String {
        let mut output = String::new();
        for result in response.results.values() {
            output.push_str(&self.format_rule_result(result));
            output.push('\n');
        }
        output
    }

    /// JSON for a run response. When `include_explanations` is false, strip
    /// per-rule `explanation` trees.
    pub fn response_json_value(
        &self,
        response: &Response,
        include_explanations: bool,
    ) -> serde_json::Value {
        let mut value = serde_json::to_value(lemma::api::Response::from(response))
            .expect("BUG: failed to serialize response JSON");
        if !include_explanations {
            if let Some(results) = value.get_mut("results").and_then(|r| r.as_object_mut()) {
                for rule in results.values_mut() {
                    if let Some(obj) = rule.as_object_mut() {
                        obj.remove("explanation");
                    }
                }
            }
        }
        value
    }

    pub fn serialize_response_json(
        &self,
        response: &Response,
        include_explanations: bool,
    ) -> String {
        serde_json::to_string_pretty(&self.response_json_value(response, include_explanations))
            .expect("BUG: failed to serialize response JSON")
    }

    pub fn format_repository_spec_list(&self, groups: &[RepositorySpecGroup<'_>]) -> String {
        let mut output = String::new();
        for (index, group) in groups.iter().enumerate() {
            if index > 0 {
                output.push('\n');
            }
            match group.repository {
                None => {
                    for spec in group.specs {
                        output.push_str(spec);
                        output.push('\n');
                    }
                }
                Some(repository) => {
                    output.push_str(repository);
                    output.push('\n');
                    for spec in group.specs {
                        output.push_str("  ");
                        output.push_str(spec);
                        output.push('\n');
                    }
                }
            }
        }
        output
    }

    fn format_rule_result(&self, result: &RuleResult) -> String {
        let mut table = Table::new();
        table.load_preset(presets::UTF8_FULL);
        table.set_style(super_table::TableComponent::MiddleIntersections, '┼');
        table.set_style(super_table::TableComponent::HorizontalLines, '─');

        if let Some(explanation) = &result.explanation {
            table.add_row(vec![
                Cell::new(format_explanation(explanation)).set_alignment(CellAlignment::Left)
            ]);
        } else {
            let header = format!(
                "{}: {}",
                result.rule.name,
                self.highlight_value(&self.format_rule_result_line(result))
            );
            table.add_row(vec![Cell::new(&header).set_alignment(CellAlignment::Left)]);
        }

        let source = &result.rule.source_location;
        let location = format!("Source: {}:{}", source.source_type, source.span.line);
        table.add_row(vec![
            Cell::new(self.gray(&location)).set_alignment(CellAlignment::Left)
        ]);

        table.to_string()
    }

    fn format_rule_result_line(&self, result: &RuleResult) -> String {
        if result.vetoed {
            return result
                .veto_reason
                .clone()
                .expect("BUG: vetoed rule result must have veto_reason");
        }
        result
            .result()
            .expect("BUG: non-veto rule result must have result")
            .to_string()
    }

    fn gray(&self, text: &str) -> String {
        format!("\x1b[90m{}\x1b[0m", text)
    }

    fn highlight_value(&self, text: &str) -> String {
        format!("\x1b[38;2;80;180;220m{}\x1b[0m", text)
    }
}
