//! Integration smoke for transitive normalization (authoritative eval path).
//! Plan-shape assertions live in `engine/src/tests/transitive_normalization_plan_shape.rs`.

use lemma::DateTimeValue;
use lemma::Engine;
use rust_decimal::Decimal;
use std::collections::HashMap;

fn run_decimal(code: &str, spec: &str, rule: &str) -> Decimal {
    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("load");
    let now = DateTimeValue::now();
    let resp = engine
        .run(None, spec, Some(&now), HashMap::new(), None, false)
        .expect("run");
    let rule_result = resp.get(rule).unwrap_or_else(|_| {
        panic!(
            "rule '{rule}' missing from results; have {:?}",
            resp.results.keys().collect::<Vec<_>>()
        )
    });
    assert!(
        !rule_result.vetoed,
        "rule '{rule}' must not veto; reason={:?}",
        rule_result.veto_reason
    );
    let display = rule_result
        .result()
        .expect("authoritative display")
        .to_string();
    display.parse().unwrap_or_else(|_| {
        panic!("authoritative display for '{rule}' must be decimal, got '{display}'")
    })
}

#[test]
fn cross_rule_sqrt_product_is_exact_two() {
    let code = r#"
spec test
rule sqrt_two: sqrt 2
rule sqrt_product: sqrt_two * sqrt_two
"#;
    assert_eq!(
        run_decimal(code, "test", "sqrt_product"),
        Decimal::from(2),
        "sqrt(2)*sqrt(2) across rules must evaluate to exact 2"
    );
}

#[test]
fn cross_rule_log_exp_is_exact_one() {
    let code = r#"
spec t
rule a: exp 1
rule b: log a
"#;
    assert_eq!(run_decimal(code, "t", "b"), Decimal::ONE);
}

#[test]
fn rule_chain_quadruple_is_exact_two_hundred() {
    let code = r#"
spec t
data base_value: 50
rule doubled: base_value * 2
rule quadruple: doubled * 2
"#;
    assert_eq!(run_decimal(code, "t", "quadruple"), Decimal::from(200));
}
