use serde_json::Value;

use crate::documentation::{GuideTopic, EVALUATE_GUIDE};
use crate::engine::{resolve_effective as resolve_effective_datetime, Engine};
use crate::evaluation::explanations::format_explanation;
use crate::evaluation::response::Response;
use crate::mcp::error::ToolError;
use crate::parse_evaluate_data_object;
use crate::parsing::ast::DateTimeValue;
use crate::parsing::source::SourceType;
use crate::resolve_evaluate_rules;
use crate::spec_set_id::parse_spec_set_id;

/// Evaluate a spec. Always explains (`Engine::evaluate(..., true)`). No `explain` arg.
/// Success text is ASCII explanation trees plus a Missing data block when unbound.
pub fn evaluate(engine: &Engine, args: &Value) -> Result<String, ToolError> {
    require_object(args)?;
    reject_explain_arg(args)?;
    if args.get("rule").is_some() {
        return Err(ToolError::invalid_arguments(
            "Unknown field 'rule'. Use 'rules' (string or string array).",
        ));
    }

    let spec_set_id = required_string(args, "spec")?;
    if spec_set_id.is_empty() {
        return Err(ToolError::invalid_arguments("Spec set id cannot be empty"));
    }
    let spec_name = parse_spec_set_id(spec_set_id).map_err(engine_error_to_diagnostics)?;
    let repository = optional_nonempty_string(args, "repository")?;
    let now = resolve_effective(args)?;
    let data_values = parse_evaluate_data_object(&args.get("data").cloned())
        .map_err(|e| ToolError::invalid_arguments(e.to_string()))?;
    let rule_names = resolve_evaluate_rules(&args.get("rules").cloned())
        .map_err(|e| ToolError::invalid_arguments(e.to_string()))?;
    let rules = rule_names.as_deref();

    let response = engine
        .evaluate(repository, &spec_name, Some(&now), data_values, rules, true)
        .map_err(engine_error_to_diagnostics)?;

    Ok(format_evaluate_text(&response))
}

/// Deprecated alias of [`evaluate`]. Same args and formatted trees.
pub fn run(engine: &Engine, args: &Value) -> Result<String, ToolError> {
    evaluate(engine, args)
}

fn format_evaluate_text(response: &Response) -> String {
    let mut output = String::new();
    let missing: Vec<&str> = response
        .results
        .values()
        .filter(|result| result.is_missing_data())
        .flat_map(|result| result.missing_data().iter().map(String::as_str))
        .collect();
    if !missing.is_empty() {
        output.push_str("Missing data\n");
        for key in &missing {
            output.push_str("  ");
            output.push_str(key);
            output.push('\n');
        }
        output.push('\n');
    }
    let mut first = true;
    for result in response.results.values() {
        if !first {
            output.push('\n');
        }
        first = false;
        let explanation = result
            .explanation
            .as_ref()
            .expect("BUG: MCP evaluate always explains");
        output.push_str(&format_explanation(explanation));
        output.push('\n');
    }
    output
}

pub fn specs(engine: &Engine, args: &Value) -> Result<String, ToolError> {
    require_object(args)?;
    let text = optional_bool(args, "text")?;
    let all = optional_bool(args, "all")?;
    let mut repositories = engine.specs();
    if text {
        return Ok(crate::format_repositories(&repositories, all));
    }
    if !all {
        repositories.retain(|repository| !repository.from_dependency);
    }
    Ok(serde_json::to_string_pretty(&repositories)
        .unwrap_or_else(|error| panic!("BUG: engine specs must serialize: {error}")))
}

pub fn show(engine: &Engine, args: &Value) -> Result<String, ToolError> {
    let repository = optional_nonempty_string(args, "repository")?;
    let text = optional_bool(args, "text")?;
    let spec_set_id = required_string(args, "spec")?;
    if spec_set_id.is_empty() {
        return Err(ToolError::invalid_arguments("Spec set id cannot be empty"));
    }
    let spec_name = parse_spec_set_id(spec_set_id).map_err(engine_error_to_diagnostics)?;
    let now = resolve_effective(args)?;
    let show = engine
        .show(repository, &spec_name, Some(&now))
        .map_err(engine_error_to_diagnostics)?;
    if text {
        return Ok(format!("{show}\n"));
    }
    Ok(
        serde_json::to_string_pretty(&crate::api::show_document(engine, &show))
            .unwrap_or_else(|error| panic!("BUG: show response must serialize: {error}")),
    )
}

pub fn source(engine: &Engine, args: &Value) -> Result<String, ToolError> {
    require_object(args)?;
    let repository = optional_nonempty_string(args, "repository")?;
    let spec = optional_nonempty_string(args, "spec")?;
    match (repository, spec) {
        (Some(repo), None) => engine
            .source(Some(repo), None, None)
            .map_err(engine_error_to_diagnostics),
        (repo, Some(spec_set_id)) => {
            let spec_name = parse_spec_set_id(spec_set_id).map_err(engine_error_to_diagnostics)?;
            let now = resolve_effective(args)?;
            engine
                .source(repo, Some(&spec_name), Some(&now))
                .map_err(engine_error_to_diagnostics)
        }
        (None, None) => Err(ToolError::invalid_arguments(
            "Missing 'spec' or 'repository' field",
        )),
    }
}

pub fn check(args: &Value) -> Result<String, ToolError> {
    let text = optional_bool(args, "text")?;
    let sources_value = args
        .get("sources")
        .ok_or_else(|| ToolError::invalid_arguments("Missing 'sources' array field"))?;
    let sources_arr = sources_value
        .as_array()
        .ok_or_else(|| ToolError::invalid_arguments("Missing 'sources' array field"))?;
    if sources_arr.is_empty() {
        return Err(ToolError::invalid_arguments(
            "'sources' must be a non-empty array of [label, code] pairs",
        ));
    }

    let mut sources: Vec<(SourceType, String)> = Vec::with_capacity(sources_arr.len());
    for (i, entry) in sources_arr.iter().enumerate() {
        let pair = entry.as_array().ok_or_else(|| {
            ToolError::invalid_arguments(format!("sources[{i}] must be a [label, code] array"))
        })?;
        if pair.len() != 2 {
            return Err(ToolError::invalid_arguments(format!(
                "sources[{i}] must have exactly 2 elements [label, code]"
            )));
        }
        let label = pair[0].as_str().ok_or_else(|| {
            ToolError::invalid_arguments(format!("sources[{i}][0] (label) must be a string"))
        })?;
        let code = pair[1].as_str().ok_or_else(|| {
            ToolError::invalid_arguments(format!("sources[{i}][1] (code) must be a string"))
        })?;
        let source_type =
            SourceType::from_binding_label(label).map_err(ToolError::invalid_arguments)?;
        sources.push((source_type, code.to_string()));
    }

    let mut engine = Engine::new();
    if let Err(load_err) = engine.load(sources) {
        return Err(ToolError::diagnostics(&load_err.errors));
    }

    let recommendations = engine.quality();
    if text {
        return Ok(format_recommendations_text(&recommendations));
    }
    Ok(serde_json::to_string_pretty(&recommendations)
        .unwrap_or_else(|error| panic!("BUG: quality recommendations must serialize: {error}")))
}

fn format_recommendations_text(recommendations: &[crate::Recommendation]) -> String {
    if recommendations.is_empty() {
        return "No recommendations.\n".to_string();
    }
    let mut output = String::new();
    for recommendation in recommendations {
        output.push_str(&recommendation.to_string());
        output.push('\n');
    }
    output
}

pub fn guide(args: &Value) -> Result<String, ToolError> {
    require_object(args)?;
    match args.get("topic") {
        None => Ok(EVALUATE_GUIDE.to_string()),
        Some(value) => {
            let topic_name = value
                .as_str()
                .ok_or_else(|| ToolError::invalid_arguments("topic must be a string"))?;
            let topic = GuideTopic::parse(topic_name).ok_or_else(|| {
                ToolError::invalid_arguments(format!(
                    "Unknown guide topic '{topic_name}'. Valid: {}",
                    GuideTopic::VALID_LIST
                ))
            })?;
            Ok(topic.section_text().to_string())
        }
    }
}

fn engine_error_to_diagnostics(error: crate::Error) -> ToolError {
    ToolError::diagnostics(std::slice::from_ref(&error))
}

fn reject_explain_arg(args: &Value) -> Result<(), ToolError> {
    if args.get("explain").is_some() {
        return Err(ToolError::invalid_arguments(
            "MCP run always includes explanations; do not pass 'explain'",
        ));
    }
    Ok(())
}

fn require_object(args: &Value) -> Result<(), ToolError> {
    if args.is_object() || args.is_null() {
        Ok(())
    } else {
        Err(ToolError::invalid_arguments("arguments must be an object"))
    }
}

fn required_string<'a>(args: &'a Value, field: &str) -> Result<&'a str, ToolError> {
    match args.get(field) {
        Some(Value::String(value)) => Ok(value.trim()),
        Some(_) => Err(ToolError::invalid_arguments(format!(
            "'{field}' must be a string"
        ))),
        None => Err(ToolError::invalid_arguments(format!(
            "Missing '{field}' field"
        ))),
    }
}

fn optional_nonempty_string<'a>(
    args: &'a Value,
    field: &str,
) -> Result<Option<&'a str>, ToolError> {
    match args.get(field) {
        None => Ok(None),
        Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                Ok(Some(trimmed))
            }
        }
        Some(_) => Err(ToolError::invalid_arguments(format!(
            "'{field}' must be a string"
        ))),
    }
}

fn optional_bool(args: &Value, field: &str) -> Result<bool, ToolError> {
    match args.get(field) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(value)) => Ok(*value),
        Some(_) => Err(ToolError::invalid_arguments(format!(
            "'{field}' must be a boolean"
        ))),
    }
}

fn resolve_effective(args: &Value) -> Result<DateTimeValue, ToolError> {
    match args.get("effective") {
        None | Some(Value::Null) => {
            resolve_effective_datetime(None).map_err(engine_error_to_diagnostics)
        }
        Some(Value::String(raw)) => {
            resolve_effective_datetime(Some(raw)).map_err(engine_error_to_diagnostics)
        }
        Some(_) => Err(ToolError::invalid_arguments("'effective' must be a string")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn load_pricing() -> Engine {
        let mut engine = Engine::new();
        engine
            .load([(
                SourceType::Path(Arc::new(PathBuf::from("pricing.lemma"))),
                "spec pricing\ndata quantity: number\nrule total: quantity * 10\n".to_string(),
            )])
            .expect("load");
        engine
    }

    #[test]
    fn evaluate_returns_formatted_explanation_tree() {
        let engine = load_pricing();
        let text = run(
            &engine,
            &serde_json::json!({
                "spec": "pricing",
                "rules": "total",
                "data": { "quantity": "3" }
            }),
        )
        .expect("evaluate");
        assert!(
            text.contains("total: 30"),
            "expected formatted tree with total: 30, got: {text}"
        );
        assert!(
            text.contains("└─") || text.contains("quantity"),
            "expected tree connector or quantity in body, got: {text}"
        );
    }

    #[test]
    fn evaluate_rejects_json_number_data() {
        let engine = load_pricing();
        let err = run(
            &engine,
            &serde_json::json!({
                "spec": "pricing",
                "data": { "quantity": 3 }
            }),
        )
        .expect_err("json number");
        match err {
            ToolError::InvalidArguments(message) => {
                assert!(message.contains("pass as string: \"3\""), "{message}");
            }
            other => panic!("expected InvalidArguments, got {other}"),
        }
    }

    #[test]
    fn evaluate_rejects_explain_arg() {
        let engine = load_pricing();
        let err = run(
            &engine,
            &serde_json::json!({
                "spec": "pricing",
                "explain": false
            }),
        )
        .expect_err("explain forbidden");
        assert!(matches!(err, ToolError::InvalidArguments(_)));
    }

    #[test]
    fn evaluate_rejects_legacy_rule_field() {
        let engine = load_pricing();
        let err = run(
            &engine,
            &serde_json::json!({
                "spec": "pricing",
                "rule": "total"
            }),
        )
        .expect_err("rule forbidden");
        assert!(matches!(err, ToolError::InvalidArguments(_)));
    }

    #[test]
    fn evaluate_rules_array() {
        let engine = load_pricing();
        let text = run(
            &engine,
            &serde_json::json!({
                "spec": "pricing",
                "rules": ["total"],
                "data": { "quantity": "2" }
            }),
        )
        .expect("evaluate");
        assert!(
            text.contains("total: 20"),
            "expected formatted tree with total: 20, got: {text}"
        );
    }

    #[test]
    fn show_accepts_repository() {
        let engine = Engine::new();
        let text = show(
            &engine,
            &serde_json::json!({
                "repository": "lemma",
                "spec": "units"
            }),
        )
        .expect("show lemma units");
        let value: Value = serde_json::from_str(&text).expect("Show JSON");
        assert_eq!(value["title"], "units");
        assert_eq!(value["x-lemma-repository"], "lemma");
    }

    #[test]
    fn missing_spec_is_diagnostics() {
        let engine = Engine::new();
        let err =
            run(&engine, &serde_json::json!({ "spec": "nonexistent" })).expect_err("missing spec");
        match err {
            ToolError::Diagnostics(text) => {
                let value: Value = serde_json::from_str(&text).expect("EngineError JSON");
                assert!(value.is_array());
                assert!(!value.as_array().expect("array").is_empty());
            }
            other => panic!("expected Diagnostics, got {other}"),
        }
    }

    #[test]
    fn evaluate_aliases_run() {
        let engine = load_pricing();
        let a = run(
            &engine,
            &serde_json::json!({
                "spec": "pricing",
                "data": { "quantity": "1" }
            }),
        )
        .expect("evaluate");
        let b = evaluate(
            &engine,
            &serde_json::json!({
                "spec": "pricing",
                "data": { "quantity": "1" }
            }),
        )
        .expect("evaluate");
        assert_eq!(a, b);
    }

    #[test]
    fn check_success_is_quality_json() {
        let text = check(&serde_json::json!({
            "sources": [["ok.lemma", "spec ok\nrule r: 1\n"]]
        }))
        .expect("check");
        let value: Value = serde_json::from_str(&text).expect("quality JSON");
        assert!(value.is_array());
    }

    fn assert_text_must_be_boolean(result: Result<String, ToolError>) {
        match result.expect_err("non-boolean text") {
            ToolError::InvalidArguments(message) => {
                assert_eq!(message, "'text' must be a boolean")
            }
            other => panic!("expected InvalidArguments, got {other}"),
        }
    }

    #[test]
    fn specs_text_is_the_cli_view() {
        let engine = load_pricing();
        let text = specs(&engine, &serde_json::json!({ "text": true })).expect("specs text");
        assert_eq!(text, crate::format_repositories(&engine.specs(), false));
        assert_eq!(text, "pricing\n");
        let all_text =
            specs(&engine, &serde_json::json!({ "text": true, "all": true })).expect("specs all");
        assert_eq!(all_text, crate::format_repositories(&engine.specs(), true));
        assert!(
            all_text.starts_with("pricing\n\nlemma\n  units\n"),
            "{all_text}"
        );
        let json = specs(&engine, &serde_json::json!({ "text": false })).expect("specs JSON");
        let value: Value = serde_json::from_str(&json).expect("specs JSON parses");
        assert!(value.is_array());
        assert!(
            !value
                .as_array()
                .expect("array")
                .iter()
                .any(|g| g["repository"] == "lemma"),
            "default specs JSON omits lemma: {json}"
        );
        assert_eq!(json, specs(&engine, &serde_json::json!({})).expect("specs"));
        let all_json = specs(&engine, &serde_json::json!({ "all": true })).expect("specs all JSON");
        let all_value: Value = serde_json::from_str(&all_json).expect("all JSON");
        assert!(
            all_value
                .as_array()
                .expect("array")
                .iter()
                .any(|g| g["repository"] == "lemma"),
            "all: true JSON includes lemma: {all_json}"
        );
        assert_text_must_be_boolean(specs(&engine, &serde_json::json!({ "text": "true" })));
        assert_text_must_be_boolean(specs(&engine, &serde_json::json!({ "text": 1 })));
    }

    #[test]
    fn show_text_is_the_cli_view() {
        let engine = load_pricing();
        let now = DateTimeValue::now();
        let expected = format!(
            "{}\n",
            engine.show(None, "pricing", Some(&now)).expect("show")
        );
        let text = show(
            &engine,
            &serde_json::json!({ "spec": "pricing", "text": true }),
        )
        .expect("show text");
        assert_eq!(text, expected);
        let json = show(
            &engine,
            &serde_json::json!({ "spec": "pricing", "text": null }),
        )
        .expect("show JSON");
        let value: Value = serde_json::from_str(&json).expect("Show JSON");
        assert_eq!(
            value["$schema"],
            "https://json-schema.org/draft/2020-12/schema"
        );
        assert_text_must_be_boolean(show(
            &engine,
            &serde_json::json!({ "spec": "pricing", "text": "yes" }),
        ));
    }

    #[test]
    fn check_text_lists_one_recommendation_per_line() {
        let source = "spec pricing 2026-01-01\n\"\"\"\nx\n\"\"\"\n\ndata qty: number\n  -> help \"Order quantity.\"\nrule total: qty\n";
        let mut engine = Engine::new();
        let source_type = SourceType::from_binding_label("pricing.lemma").expect("label");
        engine
            .load([(source_type, source.to_string())])
            .expect("load");
        let recommendations = engine.quality();
        assert!(!recommendations.is_empty());
        let expected: String = recommendations
            .iter()
            .map(|recommendation| format!("{recommendation}\n"))
            .collect();
        let text = check(&serde_json::json!({
            "sources": [["pricing.lemma", source]],
            "text": true
        }))
        .expect("check text");
        assert_eq!(text, expected);
        assert_text_must_be_boolean(check(&serde_json::json!({
            "sources": [["pricing.lemma", source]],
            "text": 0
        })));
    }

    #[test]
    fn check_text_without_recommendations_says_so() {
        let source = "spec pricing 2026-01-01\n\"\"\"\nBulk pricing.\n\"\"\"\n\ndata qty: number\n  -> minimum 0\n  -> maximum 1000000\n  -> help \"Order quantity.\"\n\nrule total: qty\n";
        let text = check(&serde_json::json!({
            "sources": [["pricing.lemma", source]],
            "text": true
        }))
        .expect("check text");
        assert_eq!(text, "No recommendations.\n");
    }
}
