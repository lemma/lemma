//! `lower` / `upper` read the ordered endpoints of a range.

use lemma::{DateTimeValue, Engine};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

fn source() -> lemma::SourceType {
    lemma::SourceType::Path(Arc::new(PathBuf::from("range_bounds.lemma")))
}

fn eval_rules(
    code: &str,
    rules: &[&str],
    data: HashMap<String, String>,
) -> HashMap<String, String> {
    let mut engine = Engine::new();
    engine
        .load([(source(), code.to_string())])
        .expect("Should parse and plan");
    let response = engine
        .run(
            None,
            "test",
            Some(&DateTimeValue::now()),
            data,
            Some(
                &rules
                    .iter()
                    .map(|rule| (*rule).to_string())
                    .collect::<Vec<_>>(),
            ),
            false,
        )
        .expect("Should evaluate");
    rules
        .iter()
        .map(|rule| {
            let result = response
                .results
                .get(*rule)
                .unwrap_or_else(|| panic!("rule '{rule}' missing"));
            assert!(
                !result.vetoed,
                "rule '{rule}' vetoed: {:?}",
                result.veto_reason
            );
            (
                (*rule).to_string(),
                result.result().expect("result").to_string(),
            )
        })
        .collect()
}

fn eval_one(code: &str, rule: &str) -> String {
    eval_rules(code, &[rule], HashMap::new())
        .remove(rule)
        .expect("rule")
}

fn expect_plan_error(code: &str, fragment: &str) {
    let mut engine = Engine::new();
    let error = engine
        .load([(source(), code.to_string())])
        .expect_err("expected a planning error");
    let combined = error
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ");
    assert!(
        combined.contains(fragment),
        "expected '{fragment}' in: {combined}"
    );
}

#[test]
fn number_endpoints_follow_written_order() {
    let code = r#"spec test
rule lo: lower (5...10)
rule hi: upper (5...10)"#;
    assert_eq!(eval_one(code, "lo"), "5");
    assert_eq!(eval_one(code, "hi"), "10");
}

#[test]
fn reversed_number_range_orders_endpoints() {
    let code = r#"spec test
rule lo: lower (10...5)
rule hi: upper (10...5)"#;
    assert_eq!(eval_one(code, "lo"), "5");
    assert_eq!(eval_one(code, "hi"), "10");
}

#[test]
fn date_endpoints() {
    let code = r#"spec test
rule lo: lower (2024-01-01...2024-06-15)
rule hi: upper (2024-01-01...2024-06-15)
rule start: 2024-01-01
rule end: 2024-06-15"#;
    let values = eval_rules(code, &["lo", "hi", "start", "end"], HashMap::new());
    assert_eq!(values["lo"], values["start"]);
    assert_eq!(values["hi"], values["end"]);
}

#[test]
fn time_endpoints() {
    let code = r#"spec test
rule lo: lower (09:00...17:00)
rule hi: upper (09:00...17:00)
rule start: 09:00
rule end: 17:00"#;
    let values = eval_rules(code, &["lo", "hi", "start", "end"], HashMap::new());
    assert_eq!(values["lo"], values["start"]);
    assert_eq!(values["hi"], values["end"]);
}

#[test]
fn measure_endpoint_keeps_written_unit() {
    let code = r#"spec test
uses lemma units
rule lo: lower (30 kilogram...35 kilogram)
rule hi: upper (30 kilogram...35 kilogram)
rule start: 30 kilogram
rule end: 35 kilogram"#;
    let values = eval_rules(code, &["lo", "hi", "start", "end"], HashMap::new());
    assert_eq!(values["lo"], values["start"]);
    assert_eq!(values["hi"], values["end"]);
}

#[test]
fn named_money_range_endpoint_is_money() {
    let code = r#"spec test
data money: measure
  -> unit eur: 1
data band: money range
rule start: lower band"#;
    let mut engine = Engine::new();
    engine
        .load([(source(), code.to_string())])
        .expect("Should parse and plan");
    let mut data = HashMap::new();
    data.insert("band".to_string(), "10 eur...40 eur".to_string());
    let response = engine
        .run(
            None,
            "test",
            Some(&DateTimeValue::now()),
            data,
            Some(&["start".to_string()]),
            false,
        )
        .expect("Should evaluate");
    let start = response.results.get("start").expect("start");
    assert!(!start.vetoed, "{:?}", start.veto_reason);
    assert_eq!(start.rule_type, "money");
    assert_eq!(start.result(), Some("10 eur"));
}

#[test]
fn ratio_endpoints() {
    let code = r#"spec test
rule lo: lower (10%...50%)
rule hi: upper (10%...50%)
rule start: 10%
rule end: 50%"#;
    let values = eval_rules(code, &["lo", "hi", "start", "end"], HashMap::new());
    assert_eq!(values["lo"], values["start"]);
    assert_eq!(values["hi"], values["end"]);
}

#[test]
fn calendar_endpoints() {
    let code = r#"spec test
uses lemma units
rule lo: lower (18 year...67 year)
rule hi: upper (18 year...67 year)
rule start: 18 year
rule end: 67 year"#;
    let values = eval_rules(code, &["lo", "hi", "start", "end"], HashMap::new());
    assert_eq!(values["lo"], values["start"]);
    assert_eq!(values["hi"], values["end"]);
}

#[test]
fn past_window_endpoints_match_now() {
    let code = r#"spec test
uses lemma units
rule start: lower (past 7 day)
rule from_now: now - 7 day
rule end: upper (past 7 day)
rule today: now"#;
    let values = eval_rules(code, &["start", "from_now", "end", "today"], HashMap::new());
    assert_eq!(values["start"], values["from_now"]);
    assert_eq!(values["end"], values["today"]);
}

#[test]
fn data_range_endpoints() {
    let code = r#"spec test
data tier: number range
rule lo: lower tier
rule hi: upper tier"#;
    let mut data = HashMap::new();
    data.insert("tier".to_string(), "3...9".to_string());
    let values = eval_rules(code, &["lo", "hi"], data);
    assert_eq!(values["lo"], "3");
    assert_eq!(values["hi"], "9");
}

#[test]
fn vetoed_range_propagates() {
    let code = r#"spec test
data closed: boolean
rule band:
  0...10
  unless closed then veto "closed"
rule start: lower band"#;
    let mut engine = Engine::new();
    engine
        .load([(source(), code.to_string())])
        .expect("Should parse and plan");
    let mut data = HashMap::new();
    data.insert("closed".to_string(), "true".to_string());
    let response = engine
        .run(
            None,
            "test",
            Some(&DateTimeValue::now()),
            data,
            Some(&["start".to_string()]),
            false,
        )
        .expect("Should evaluate");
    let start = response.results.get("start").expect("start");
    assert!(start.vetoed, "lower of a vetoed range must veto");
    let reason = start.veto_reason.as_deref().unwrap_or("");
    assert!(
        reason.contains("closed"),
        "expected the range veto message, got {reason}"
    );
}

#[test]
fn non_range_operand_is_a_planning_error() {
    expect_plan_error(
        r#"spec test
rule bad: lower 5"#,
        "requires a range",
    );
    expect_plan_error(
        r#"spec test
rule bad: upper true"#,
        "requires a range",
    );
}
