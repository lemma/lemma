use indexmap::IndexMap;
use lemma::{
    DateGranularity, DateTimeValue, Engine, Error, EvaluateDataValue, ResourceLimits, Response,
    SourceType,
};
use std::path::PathBuf;
use std::sync::Arc;

pub fn path_source(file: &str) -> SourceType {
    SourceType::Path(Arc::new(PathBuf::from(file)))
}

pub fn now() -> DateTimeValue {
    date(2026, 10, 5)
}

pub fn date(year: i32, month: u32, day: u32) -> DateTimeValue {
    DateTimeValue {
        year,
        month,
        day,
        hour: 0,
        minute: 0,
        second: 0,
        microsecond: 0,
        timezone: None,
        granularity: DateGranularity::Full,
    }
}

pub fn lit(value: impl Into<String>) -> EvaluateDataValue {
    EvaluateDataValue::Literal(value.into())
}

pub fn boolean(value: bool) -> EvaluateDataValue {
    EvaluateDataValue::Boolean(value)
}

pub fn map(pairs: &[(&str, EvaluateDataValue)]) -> EvaluateDataValue {
    EvaluateDataValue::Map(
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.clone()))
            .collect(),
    )
}

pub fn list(values: Vec<EvaluateDataValue>) -> EvaluateDataValue {
    EvaluateDataValue::List(values)
}

pub fn data(pairs: &[(&str, EvaluateDataValue)]) -> IndexMap<String, EvaluateDataValue> {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.clone()))
        .collect()
}

pub fn load_ok(code: &str) -> Engine {
    load_ok_with_limits(code, ResourceLimits::default())
}

pub fn load_ok_with_limits(code: &str, limits: ResourceLimits) -> Engine {
    load_files_with_limits(&[("test.lemma", code)], limits)
}

pub fn load_files(files: &[(&str, &str)]) -> Engine {
    load_files_with_limits(files, ResourceLimits::default())
}

pub fn load_files_with_limits(files: &[(&str, &str)], limits: ResourceLimits) -> Engine {
    let mut engine = Engine::with_limits(limits);
    let sources = files
        .iter()
        .map(|(file, code)| (path_source(file), code.to_string()))
        .collect::<Vec<_>>();
    engine.load(sources).unwrap_or_else(|errs| {
        let joined = errs
            .iter()
            .map(|error| error.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        panic!("expected load to succeed, got: {joined}");
    });
    engine
}

/// Load `dependency` as repository `repository`, then the workspace `files`.
pub fn load_with_dependency(repository: &str, dependency: &str, files: &[(&str, &str)]) -> Engine {
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Dependency(repository.to_string()),
            dependency.to_string(),
        )])
        .unwrap_or_else(|errs| panic!("expected dependency to load, got: {errs:?}"));
    let sources = files
        .iter()
        .map(|(file, code)| (path_source(file), code.to_string()))
        .collect::<Vec<_>>();
    engine
        .load(sources)
        .unwrap_or_else(|errs| panic!("expected load to succeed, got: {errs:?}"));
    engine
}

pub fn load_err(code: &str) -> String {
    load_files_err(&[("test.lemma", code)])
}

pub fn load_planning_err(code: &str) -> String {
    load_files_planning_err(&[("test.lemma", code)])
}

pub fn load_parse_err(code: &str) -> String {
    let errors = load_errors(&[("test.lemma", code)]);
    let planning = errors
        .iter()
        .filter(|error| !matches!(error, Error::Parsing(_)))
        .map(|error| error.to_string())
        .collect::<Vec<_>>();
    assert!(
        planning.is_empty(),
        "expected parse Error, got: {}",
        planning.join("\n")
    );
    format_load_errors(&errors)
}

pub fn load_files_err(files: &[(&str, &str)]) -> String {
    format_load_errors(&load_errors(files))
}

pub fn load_files_planning_err(files: &[(&str, &str)]) -> String {
    let errors = load_errors(files);
    let parse = errors
        .iter()
        .filter(|error| matches!(error, Error::Parsing(_)))
        .map(|error| error.to_string())
        .collect::<Vec<_>>();
    assert!(
        parse.is_empty(),
        "expected planning Error, got parse Error: {}",
        parse.join("\n")
    );
    format_load_errors(&errors)
}

fn load_errors(files: &[(&str, &str)]) -> lemma::Errors {
    let mut engine = Engine::new();
    let sources = files
        .iter()
        .map(|(file, code)| (path_source(file), code.to_string()))
        .collect::<Vec<_>>();
    engine.load(sources).expect_err("expected load to fail")
}

fn format_load_errors(errors: &lemma::Errors) -> String {
    errors
        .iter()
        .map(|error| error.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn evaluate(
    engine: &Engine,
    spec: &str,
    inputs: IndexMap<String, EvaluateDataValue>,
) -> Response {
    evaluate_at(engine, spec, &now(), inputs, None, false)
}

pub fn evaluate_explain(
    engine: &Engine,
    spec: &str,
    inputs: IndexMap<String, EvaluateDataValue>,
) -> Response {
    evaluate_at(engine, spec, &now(), inputs, None, true)
}

pub fn evaluate_at(
    engine: &Engine,
    spec: &str,
    effective: &DateTimeValue,
    inputs: IndexMap<String, EvaluateDataValue>,
    rules: Option<&[String]>,
    explain: bool,
) -> Response {
    engine
        .evaluate(None, spec, Some(effective), inputs, rules, explain)
        .unwrap_or_else(|error| panic!("evaluate {spec}: {error}"))
}

pub fn evaluate_err(
    engine: &Engine,
    spec: &str,
    inputs: IndexMap<String, EvaluateDataValue>,
) -> Error {
    engine
        .evaluate(None, spec, Some(&now()), inputs, None, false)
        .expect_err("expected evaluate request Error")
}

pub fn value(response: &Response, rule: &str) -> String {
    let result = response
        .results
        .get(rule)
        .unwrap_or_else(|| panic!("rule '{rule}' missing"));
    if result.vetoed {
        panic!(
            "rule '{rule}' vetoed: {}",
            result.veto_reason.as_deref().unwrap_or("Vetoed")
        );
    }
    result
        .result()
        .unwrap_or_else(|| panic!("rule '{rule}' has no result"))
        .to_string()
}

pub fn veto_reason(response: &Response, rule: &str) -> String {
    let result = response
        .results
        .get(rule)
        .unwrap_or_else(|| panic!("rule '{rule}' missing"));
    assert!(result.vetoed, "rule '{rule}' was not vetoed");
    result
        .veto_reason
        .clone()
        .unwrap_or_else(|| panic!("rule '{rule}' missing veto reason"))
}

pub fn missing_data(response: &Response, rule: &str) -> Vec<String> {
    response
        .results
        .get(rule)
        .unwrap_or_else(|| panic!("rule '{rule}' missing"))
        .missing_data()
        .to_vec()
}

pub fn assert_contains(haystack: &str, needle: &str) {
    assert!(
        haystack.contains(needle),
        "expected `{needle}` in `{haystack}`"
    );
}
