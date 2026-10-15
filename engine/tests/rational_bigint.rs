use indexmap::IndexMap;
use lemma::{DateTimeValue, Engine, SourceType};
use rust_decimal::Decimal;

#[test]
fn decimal_max_multiply_vetoes_on_decimal_limit_not_numeric_overflow() {
    let max_decimal = Decimal::MAX.normalize().to_string();
    let code = format!(
        r#"
spec bigint_exact
data max_val: {max_decimal}
data two: 2
rule multiplied: max_val * two
"#
    );

    let mut engine = Engine::new();
    engine
        .load([(SourceType::Volatile, &code.to_string())])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .evaluate(
            None,
            "bigint_exact",
            Some(&now),
            IndexMap::new(),
            None,
            false,
        )
        .expect("evaluation must complete");

    let rule = response
        .results
        .get("multiplied")
        .expect("product rule must be present");

    assert!(rule.vetoed);
    assert_eq!(
        rule.veto_reason.as_deref(),
        Some("The calculated result exceeds the decimal value limit."),
        "rule_result_value_from_literal must veto magnitude overflow, not fail with numeric overflow"
    );
    assert_ne!(
        rule.veto_reason.as_deref(),
        Some("The numeric result overflowed."),
        "must not spuriously veto during exact rational evaluation before building RuleResultValue"
    );
}
