use indexmap::IndexMap;
use lemma::{DateTimeValue, Engine, SourceType};

fn run(code: &str, rule: &str) -> String {
    let mut engine = Engine::new();
    engine
        .load([(SourceType::Volatile, code.to_string())])
        .expect("load");
    let now = DateTimeValue::now();
    let resp = engine
        .evaluate(
            None,
            "test",
            Some(&now),
            IndexMap::new(),
            Some(&[rule.to_string()]),
            false,
        )
        .expect("evaluate");
    let rule_result = resp.get(rule).expect("rule result");
    assert!(!rule_result.vetoed, "rule {rule} vetoed");
    rule_result.result().expect("display").to_string()
}

#[test]
fn ceil_of_value_just_above_integer_is_exact() {
    assert_eq!(
        run("spec test\nrule r: ceil(3 + 1 / 10^29) is 4\n", "r"),
        "true"
    );
    assert_eq!(
        run("spec test\nrule r: ceil(3 + 1 / 10^29) is 3\n", "r"),
        "false"
    );
}

#[test]
fn floor_of_value_just_below_integer_is_exact() {
    assert_eq!(
        run("spec test\nrule r: floor(3 - 1 / 10^29) is 2\n", "r"),
        "true"
    );
}

#[test]
fn floor_and_ceil_of_negative_fractions() {
    assert_eq!(run("spec test\nrule r: floor(-5 / 2) is -3\n", "r"), "true");
    assert_eq!(run("spec test\nrule r: ceil(-5 / 2) is -2\n", "r"), "true");
    assert_eq!(run("spec test\nrule r: ceil(5 / 2) is 3\n", "r"), "true");
}

#[test]
fn abs_then_scale_round_trips_exactly() {
    assert_eq!(run("spec test\nrule r: abs(2 / 3) * 3 is 2\n", "r"), "true");
    assert_eq!(
        run("spec test\nrule r: abs(-2 / 3) * 3 is 2\n", "r"),
        "true"
    );
}

#[test]
fn floor_beyond_decimal_range_is_exact() {
    assert_eq!(
        run("spec test\nrule r: floor(2 ^ 100) is 2 ^ 100\n", "r"),
        "true"
    );
}

#[test]
fn round_half_goes_to_even_and_others_to_nearest() {
    assert_eq!(run("spec test\nrule r: round(5 / 2) is 2\n", "r"), "true");
    assert_eq!(run("spec test\nrule r: round(7 / 2) is 4\n", "r"), "true");
    assert_eq!(run("spec test\nrule r: round(-5 / 2) is -2\n", "r"), "true");
    assert_eq!(run("spec test\nrule r: round(13 / 5) is 3\n", "r"), "true");
    assert_eq!(
        run("spec test\nrule r: round(-13 / 5) is -3\n", "r"),
        "true"
    );
    assert_eq!(run("spec test\nrule r: round(12 / 5) is 2\n", "r"), "true");
}
