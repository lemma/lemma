//! Regression: stored rule results use Decimal in ValueKind and scalar JSON numbers.

use lemma::DateTimeValue;
use lemma::{Engine, LiteralValue, ValueKind};
use rust_decimal::Decimal;
use std::collections::HashMap;
fn rule_number(resp: &lemma::Response, rule: &str) -> Decimal {
    let rr = resp
        .results
        .get(rule)
        .unwrap_or_else(|| panic!("rule '{rule}' not found"));
    if rr.vetoed {
        panic!(
            "rule '{rule}' vetoed: {}",
            rr.veto_reason.as_deref().unwrap_or("Vetoed")
        );
    }
    let lit = rr
        .explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("value");
    match &lit.value {
        ValueKind::Number(d) => lemma::ValueKind::Number(d.clone())
            .as_decimal_magnitude()
            .unwrap(),
        other => panic!("rule '{rule}' expected Number, got {:?}", other),
    }
}

fn assert_json_number_scalar(lit: &LiteralValue) {
    let json =
        serde_json::to_value(lemma::api::LiteralValue::from(lit)).expect("LiteralValue serializes");
    let number = json
        .get("value")
        .and_then(|v| v.get("number"))
        .expect("number field in JSON");
    assert!(
        number.is_string(),
        "stored number must be JSON string, got {number}"
    );
    assert!(
        !number.is_array(),
        "stored number must not serialize as [n,d], got {number}"
    );
}

#[test]
fn exact_mul_stores_decimal() {
    let code = r#"
spec s
data x: 10
rule double: x * 2
"#;
    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("load");
    let now = DateTimeValue::now();
    let resp = engine
        .run(None, "s", Some(&now), HashMap::new(), None, true)
        .expect("run");
    assert_eq!(rule_number(&resp, "double"), Decimal::from(20));
    let lit = resp
        .results
        .get("double")
        .unwrap()
        .explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("value");
    assert_json_number_scalar(&lit);
}

#[test]
fn runtime_override_stores_decimal() {
    let code = r#"
spec s
data number_data: number
rule doubled: number_data * 2
"#;
    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("load");
    let now = DateTimeValue::now();
    let data = HashMap::from([("number_data".to_string(), "50".into())]);
    let resp = engine
        .run(None, "s", Some(&now), data, None, true)
        .expect("run");
    assert_eq!(rule_number(&resp, "doubled"), Decimal::from(100));
    let lit = resp
        .results
        .get("doubled")
        .unwrap()
        .explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("value");
    assert_json_number_scalar(&lit);
}

#[test]
fn sqrt_exact_commits_decimal() {
    let code = r#"
spec s
rule root: sqrt 9
"#;
    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("load");
    let now = DateTimeValue::now();
    let resp = engine
        .run(None, "s", Some(&now), HashMap::new(), None, true)
        .expect("run");
    assert_eq!(rule_number(&resp, "root"), Decimal::from(3));
}

#[test]
fn sqrt_irrational_stays_decimal() {
    let code = r#"
spec s
rule root: sqrt 2
"#;
    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("load");
    let now = DateTimeValue::now();
    let resp = engine
        .run(None, "s", Some(&now), HashMap::new(), None, true)
        .expect("run");
    let d = rule_number(&resp, "root");
    assert!(d > Decimal::from(1));
    assert!(d < Decimal::from(2));
    let lit = resp
        .results
        .get("root")
        .unwrap()
        .explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("value");
    assert!(matches!(lit.value, ValueKind::Number(_)));
    assert_json_number_scalar(&lit);
}

#[test]
fn sin_no_rational_relift() {
    let code = r#"
spec s
rule s: sin 1
"#;
    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("load");
    let now = DateTimeValue::now();
    let resp = engine
        .run(None, "s", Some(&now), HashMap::new(), None, true)
        .expect("run");
    let lit = resp
        .results
        .get("s")
        .unwrap()
        .explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("value");
    assert!(matches!(lit.value, ValueKind::Number(_)));
    assert_json_number_scalar(&lit);
}

#[test]
fn measure_result_magnitude_decimal() {
    let code = r#"
spec s
data money: measure
    -> unit eur: 1
    -> unit usd: 0.84
data amount: 100 usd
rule converted: amount as eur
"#;
    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("load");
    let now = DateTimeValue::now();
    let resp = engine
        .run(None, "s", Some(&now), HashMap::new(), None, true)
        .expect("run");
    let rr = resp.results.get("converted").unwrap();
    if rr.vetoed {
        panic!("veto: {}", rr.veto_reason.as_deref().unwrap_or("Vetoed"));
    }
    let lit = rr
        .explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("value");
    match &lit.value {
        ValueKind::Measure(magnitude) => {
            let measure = rr
                .result
                .as_ref()
                .and_then(|v| v.measure.as_ref())
                .expect("measure map");
            assert!(
                measure.contains_key("eur"),
                "expected eur in measure map, got {:?}",
                measure.keys().collect::<Vec<_>>()
            );
            assert_eq!(
                lemma::ValueKind::Number(magnitude.clone())
                    .as_decimal_magnitude()
                    .unwrap(),
                Decimal::from(84)
            );
            let json = serde_json::to_value(lemma::api::LiteralValue::from(&lit)).unwrap();
            let measure_json = json.get("value").and_then(|v| v.get("measure")).unwrap();
            assert!(
                !measure_json
                    .as_array()
                    .is_some_and(|a| a.len() == 2 && a[0].is_array()),
                "measure magnitude must not be rational array"
            );
        }
        other => panic!("expected Measure, got {:?}", other),
    }
}
