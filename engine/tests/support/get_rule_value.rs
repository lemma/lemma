use indexmap::IndexMap;
use lemma::{DateTimeValue, Engine, LiteralValue};

pub fn get_rule_value(engine: &Engine, spec_name: &str, rule_name: &str) -> LiteralValue {
    let now = DateTimeValue::now();
    let response = engine
        .evaluate(
            None,
            spec_name,
            Some(&now),
            IndexMap::new(),
            Some(&[rule_name.to_string()]),
            false,
        )
        .expect("evaluate");
    let rule_result = response
        .get(rule_name)
        .unwrap_or_else(|_| panic!("rule {rule_name}"));
    if rule_result.vetoed {
        panic!(
            "rule {rule_name} vetoed: {}",
            rule_result.veto_reason.as_deref().unwrap_or("Vetoed")
        );
    }
    rule_result.to_literal()
}
