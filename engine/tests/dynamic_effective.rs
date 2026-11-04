//! `uses … -> effective <date>` selects a temporal dependency body from data.

use indexmap::IndexMap;
use lemma::{DateTimeValue, Engine, EvaluateDataValue};
use std::str::FromStr;

fn load(source: &str) -> Engine {
    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, source.to_string())])
        .expect("load");
    engine
}

fn load_err(source: &str) -> String {
    let mut engine = Engine::new();
    let err = engine
        .load([(lemma::SourceType::Volatile, source.to_string())])
        .expect_err("expected planning error");
    err.iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("; ")
}

fn evaluate(
    engine: &Engine,
    spec: &str,
    effective: &str,
    data: &[(&str, &str)],
) -> lemma::Response {
    let at = DateTimeValue::from_str(effective).expect("effective");
    let map: IndexMap<String, EvaluateDataValue> = data
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).into()))
        .collect();
    engine
        .evaluate(None, spec, Some(&at), map, None, false)
        .expect("evaluate")
}

fn rule_result(response: &lemma::Response, rule: &str) -> String {
    let entry = response.results.get(rule).expect(rule);
    if entry.vetoed {
        return format!("veto:{}", entry.veto_reason.as_deref().unwrap_or_default());
    }
    entry.result().unwrap_or_default().to_string()
}

#[test]
fn parse_effective_on_uses() {
    let source = r#"
spec alpha2 2020-01-01
data code: text
  -> option "NL"
rule name: "Netherlands"

spec intake
data birth_date: date
data birth_country: text
uses iso: alpha2
  -> effective birth_date
  -> with code: birth_country
rule birth_country_name: iso.name
"#;
    let engine = load(source);
    let response = evaluate(
        &engine,
        "intake",
        "2024-01-01",
        &[("birth_date", "2021-06-01"), ("birth_country", "NL")],
    );
    assert_eq!(rule_result(&response, "birth_country_name"), "Netherlands");
}

#[test]
fn dynamic_effective_selects_version_by_pin_date() {
    let source = r#"
spec alpha2 2020-01-01
data code: text
  -> option "NL"
  -> option "CS"
rule name:
  veto "unknown"
  unless code is "NL" then "Netherlands"
  unless code is "CS" then "Serbia and Montenegro"

spec alpha2 2023-01-01
data code: text
  -> option "NL"
  -> option "RS"
rule name:
  veto "unknown"
  unless code is "NL" then "Netherlands"
  unless code is "RS" then "Serbia"

spec intake
data birth_date: date
data birth_country: text
uses iso: alpha2
  -> effective birth_date
  -> with code: birth_country
rule birth_country_name: iso.name
"#;
    let engine = load(source);
    let early = evaluate(
        &engine,
        "intake",
        "2024-06-01",
        &[("birth_date", "2021-06-01"), ("birth_country", "CS")],
    );
    assert_eq!(
        rule_result(&early, "birth_country_name"),
        "Serbia and Montenegro"
    );

    let late = evaluate(
        &engine,
        "intake",
        "2024-06-01",
        &[("birth_date", "2023-06-01"), ("birth_country", "RS")],
    );
    assert_eq!(rule_result(&late, "birth_country_name"), "Serbia");

    let wrong_code = evaluate(
        &engine,
        "intake",
        "2024-06-01",
        &[("birth_date", "2023-06-01"), ("birth_country", "CS")],
    );
    assert!(
        rule_result(&wrong_code, "birth_country_name").starts_with("veto:"),
        "CS must veto against the 2023 options: {}",
        rule_result(&wrong_code, "birth_country_name")
    );

    let before = evaluate(
        &engine,
        "intake",
        "2024-06-01",
        &[("birth_date", "2019-01-01"), ("birth_country", "NL")],
    );
    assert!(
        rule_result(&before, "birth_country_name").starts_with("veto:"),
        "pin before first version must veto: {}",
        rule_result(&before, "birth_country_name")
    );
}

#[test]
fn reject_literal_pin_with_effective() {
    let err = load_err(
        r#"
spec alpha2 2020-01-01
data code: text

spec intake
data birth_date: date
uses iso: alpha2 2020-01-01
  -> effective birth_date
"#,
    );
    assert!(
        err.contains("Cannot combine") || err.contains("effective"),
        "{err}"
    );
}

#[test]
fn reject_two_effective_commands() {
    let err = load_err(
        r#"
spec alpha2 2020-01-01
data code: text

spec intake
data birth_date: date
data other_date: date
uses iso: alpha2
  -> effective birth_date
  -> effective other_date
"#,
    );
    assert!(
        err.contains("at most one") || err.contains("effective"),
        "{err}"
    );
}

#[test]
fn reject_effective_as_data_name() {
    let err = load_err(
        r#"
spec intake
data effective: date
"#,
    );
    assert!(
        err.contains("reserved") || err.contains("effective"),
        "{err}"
    );
}

#[test]
fn dynamic_effective_consumer_not_split_by_dependency_versions() {
    // Unpinned uses of alpha2 would plan separate slices at 2020 and 2023 and
    // reject option drift. With -> effective, the consumer stays one plan that
    // still works on both sides of the dependency boundary for the same pin.
    let source = r#"
spec alpha2 2020-01-01
data code: text
  -> option "NL"
  -> option "CS"
rule name:
  veto "unknown"
  unless code is "NL" then "Netherlands"
  unless code is "CS" then "Serbia and Montenegro"

spec alpha2 2023-01-01
data code: text
  -> option "NL"
  -> option "RS"
rule name:
  veto "unknown"
  unless code is "NL" then "Netherlands"
  unless code is "RS" then "Serbia"

spec intake
data birth_date: date
data birth_country: text
uses iso: alpha2
  -> effective birth_date
  -> with code: birth_country
rule birth_country_name: iso.name
"#;
    let engine = load(source);
    let data = [("birth_date", "2021-06-01"), ("birth_country", "CS")];
    let before_boundary = evaluate(&engine, "intake", "2021-06-01", &data);
    let after_boundary = evaluate(&engine, "intake", "2024-06-01", &data);
    assert_eq!(
        rule_result(&before_boundary, "birth_country_name"),
        "Serbia and Montenegro"
    );
    assert_eq!(
        rule_result(&after_boundary, "birth_country_name"),
        "Serbia and Montenegro"
    );
}

#[test]
fn option_only_drift_allowed_for_dynamic_effective() {
    let source = r#"
spec alpha2 2020-01-01
data code: text
  -> option "NL"
  -> help "country"
rule name: "old"

spec alpha2 2023-01-01
data code: text
  -> option "NL"
  -> option "RS"
  -> help "country"
rule name: "new"

spec intake
data birth_date: date
uses iso: alpha2
  -> effective birth_date
rule label: iso.name
"#;
    load(source);
}

#[test]
fn cycle_through_effective_pin_is_error() {
    let err = load_err(
        r#"
spec alpha2 2020-01-01
data code: text
data as_of: date

spec intake
data fallback: date
uses iso: alpha2
  -> effective chosen
  -> with code: "NL"
  -> with as_of: fallback
rule chosen: iso.as_of
"#,
    );
    assert!(
        err.contains("cycle") || err.contains("circular") || err.contains("Circular"),
        "{err}"
    );
}

#[test]
fn binding_slot_missing_on_one_version_is_error() {
    let err = load_err(
        r#"
spec alpha2 2020-01-01
data code: text
data label: text

spec alpha2 2023-01-01
data code: text

spec intake
data birth_date: date
uses iso: alpha2
  -> effective birth_date
  -> with label: "x"
"#,
    );
    assert!(
        err.contains("label") || err.contains("no data field") || err.contains("2023"),
        "{err}"
    );
}

#[test]
fn adding_dependency_version_rebuilds_dynamic_effective_consumer() {
    let mut engine = Engine::new();
    let dep = lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
        "alpha2.lemma",
    )));
    let consumer = lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
        "intake.lemma",
    )));
    engine
        .load([
            (
                dep.clone(),
                r#"
spec alpha2 2020-01-01
data code: text
  -> option "NL"
rule name: "Netherlands"
"#
                .to_string(),
            ),
            (
                consumer.clone(),
                r#"
spec intake
data birth_date: date
data birth_country: text
uses iso: alpha2
  -> effective birth_date
  -> with code: birth_country
rule birth_country_name: iso.name
"#
                .to_string(),
            ),
        ])
        .expect("load");

    engine
        .update(
            None,
            r#"
spec alpha2 2020-01-01
data code: text
  -> option "NL"
  -> option "CS"
rule name:
  veto "unknown"
  unless code is "NL" then "Netherlands"
  unless code is "CS" then "Serbia and Montenegro"

spec alpha2 2023-01-01
data code: text
  -> option "NL"
  -> option "RS"
rule name:
  veto "unknown"
  unless code is "NL" then "Netherlands"
  unless code is "RS" then "Serbia"
"#
            .to_string(),
            dep,
        )
        .expect("new dependency version must rebuild the dynamic-effective consumer");

    let late = evaluate(
        &engine,
        "intake",
        "2024-06-01",
        &[("birth_date", "2023-06-01"), ("birth_country", "RS")],
    );
    assert_eq!(rule_result(&late, "birth_country_name"), "Serbia");
}

#[test]
fn help_drift_rejected_for_dynamic_effective() {
    let err = load_err(
        r#"
spec alpha2 2020-01-01
data code: text
  -> help "a"
rule name: "old"

spec alpha2 2023-01-01
data code: text
  -> help "b"
rule name: "new"

spec intake
data birth_date: date
uses iso: alpha2
  -> effective birth_date
rule label: iso.code
"#,
    );
    assert!(
        err.contains("without pinning") || err.contains("changed") || err.contains("help"),
        "{err}"
    );
}

#[test]
fn dynamic_effective_type_path_unions_option_values() {
    let source = r#"
spec alpha2 2020-01-01
data code: text
  -> option "NL"
  -> option "CS"
rule name:
  veto "unknown"
  unless code is "NL" then "Netherlands"
  unless code is "CS" then "Serbia and Montenegro"

spec alpha2 2023-01-01
data code: text
  -> option "NL"
  -> option "RS"
rule name:
  veto "unknown"
  unless code is "NL" then "Netherlands"
  unless code is "RS" then "Serbia"

spec intake
data birth_date: date
uses iso: alpha2
  -> effective birth_date
  -> with code: birth_country
data birth_country: iso.code
rule birth_country_name: iso.name
"#;
    let engine = load(source);
    // Without birth_country, missing_data_options must list the union so either
    // era's codes are promptable before the pin selects a body.
    let missing = evaluate(
        &engine,
        "intake",
        "2024-06-01",
        &[("birth_date", "2021-06-01")],
    );
    let offered = missing
        .results
        .get("birth_country_name")
        .expect("birth_country_name")
        .missing_data_options()
        .get("birth_country")
        .expect("birth_country options");
    let rendered: Vec<String> = offered.iter().map(|value| format!("{value:?}")).collect();
    assert!(
        rendered.iter().any(|value| value.contains("CS"))
            && rendered.iter().any(|value| value.contains("RS"))
            && rendered.iter().any(|value| value.contains("NL")),
        "type path must expose the sorted option union, got {rendered:?}"
    );

    let early = evaluate(
        &engine,
        "intake",
        "2024-06-01",
        &[("birth_date", "2021-06-01"), ("birth_country", "CS")],
    );
    assert_eq!(
        rule_result(&early, "birth_country_name"),
        "Serbia and Montenegro"
    );

    let late = evaluate(
        &engine,
        "intake",
        "2024-06-01",
        &[("birth_date", "2023-06-01"), ("birth_country", "RS")],
    );
    assert_eq!(rule_result(&late, "birth_country_name"), "Serbia");

    let wrong_for_selected = evaluate(
        &engine,
        "intake",
        "2024-06-01",
        &[("birth_date", "2023-06-01"), ("birth_country", "CS")],
    );
    assert!(
        rule_result(&wrong_for_selected, "birth_country_name").starts_with("veto:"),
        "binding into the selected body must enforce that body's options: {}",
        rule_result(&wrong_for_selected, "birth_country_name")
    );
}

#[test]
fn dynamic_effective_type_path_rejects_divergent_options_rule() {
    let err = load_err(
        r#"
spec alpha2 2020-01-01
data codes: text list
rule offered:
  codes
data code: text
  -> options offered

spec alpha2 2023-01-01
data codes: text list
rule other_offered:
  codes
data code: text
  -> options other_offered

spec intake
data birth_date: date
uses iso: alpha2
  -> effective birth_date
data birth_country: iso.code
"#,
    );
    assert!(
        err.contains("options") || err.contains("changed") || err.contains("rule"),
        "{err}"
    );
}
