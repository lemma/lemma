use lemma::DateTimeValue;
use lemma::Engine;
use std::collections::HashMap;

pub fn eval_rule_bool(
    engine: &Engine,
    spec_name: &str,
    rule: &str,
    effective: &DateTimeValue,
    data: HashMap<String, String>,
) -> bool {
    let response = engine
        .run(
            None,
            spec_name,
            Some(effective),
            data,
            Some(&[rule.to_string()]),
            false,
        )
        .expect("run");
    let rule_result = response.get(rule).unwrap_or_else(|_| panic!("rule {rule}"));
    if rule_result.vetoed {
        panic!(
            "rule {rule} vetoed: {}",
            rule_result.veto_reason.as_deref().unwrap_or("Vetoed")
        );
    }
    rule_result
        .result
        .as_ref()
        .expect("rule result value")
        .boolean
        .expect("boolean rule result")
}
