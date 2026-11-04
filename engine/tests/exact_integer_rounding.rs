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
        run("spec test\nrule r: 3 + 1 / 10^29 -> ceil is 4\n", "r"),
        "true"
    );
    assert_eq!(
        run("spec test\nrule r: 3 + 1 / 10^29 -> ceil is 3\n", "r"),
        "false"
    );
}

#[test]
fn floor_of_value_just_below_integer_is_exact() {
    assert_eq!(
        run("spec test\nrule r: 3 - 1 / 10^29 -> floor is 2\n", "r"),
        "true"
    );
}

#[test]
fn floor_and_ceil_of_negative_fractions() {
    assert_eq!(
        run("spec test\nrule r: -5 / 2 -> floor is -3\n", "r"),
        "true"
    );
    assert_eq!(
        run("spec test\nrule r: -5 / 2 -> ceil is -2\n", "r"),
        "true"
    );
    assert_eq!(run("spec test\nrule r: 5 / 2 -> ceil is 3\n", "r"), "true");
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
        run("spec test\nrule r: 2 ^ 100 -> floor is 2 ^ 100\n", "r"),
        "true"
    );
}

#[test]
fn round_is_half_away_from_zero() {
    assert_eq!(run("spec test\nrule r: 5 / 2 -> round is 3\n", "r"), "true");
    assert_eq!(
        run("spec test\nrule r: -5 / 2 -> round is -3\n", "r"),
        "true"
    );
    assert_eq!(run("spec test\nrule r: 1 / 2 -> round is 1\n", "r"), "true");
    assert_eq!(
        run("spec test\nrule r: -1 / 2 -> round is -1\n", "r"),
        "true"
    );
    assert_eq!(run("spec test\nrule r: 7 / 2 -> round is 4\n", "r"), "true");
    assert_eq!(
        run("spec test\nrule r: -7 / 2 -> round is -4\n", "r"),
        "true"
    );
    assert_eq!(run("spec test\nrule r: 3 / 2 -> round is 2\n", "r"), "true");
    assert_eq!(
        run("spec test\nrule r: -3 / 2 -> round is -2\n", "r"),
        "true"
    );
    assert_eq!(
        run("spec test\nrule r: 13 / 5 -> round is 3\n", "r"),
        "true"
    );
    assert_eq!(
        run("spec test\nrule r: -13 / 5 -> round is -3\n", "r"),
        "true"
    );
    assert_eq!(
        run("spec test\nrule r: 12 / 5 -> round is 2\n", "r"),
        "true"
    );
    assert_eq!(
        run("spec test\nrule r: -12 / 5 -> round is -2\n", "r"),
        "true"
    );
}

#[test]
fn round_measure_magnitude_is_half_away_from_zero() {
    let code = r#"spec test
data money: measure
  -> unit eur: 1
rule positive_tie: 2.5 eur -> round(1 eur) is 3 eur
rule negative_tie: -2.5 eur -> round(1 eur) is -3 eur
rule nearer_down: 2.4 eur -> round(1 eur) is 2 eur
rule nearer_up: 2.6 eur -> round(1 eur) is 3 eur
"#;
    assert_eq!(run(code, "positive_tie"), "true");
    assert_eq!(run(code, "negative_tie"), "true");
    assert_eq!(run(code, "nearer_down"), "true");
    assert_eq!(run(code, "nearer_up"), "true");
}

#[test]
fn round_cent_magnitude_is_half_away_from_zero() {
    let code = r#"spec test
data money: measure
  -> unit eur: 1
  -> unit cent: 0.01
rule positive_tie: 2.5 cent -> round(1 cent) is 3 cent
rule negative_tie: -2.5 cent -> round(1 cent) is -3 cent
"#;
    assert_eq!(run(code, "positive_tie"), "true");
    assert_eq!(run(code, "negative_tie"), "true");
}

#[test]
fn round_percent_is_half_away_from_zero() {
    assert_eq!(
        run("spec test\nrule r: 0.5% -> round(1%) is 1%\n", "r"),
        "true"
    );
}

#[test]
fn round_percent_as_number_is_zero() {
    assert_eq!(
        run("spec test\nrule r: 0.5% as number -> round is 0\n", "r"),
        "true"
    );
}

#[test]
fn round_percent_other_magnitudes() {
    assert_eq!(
        run("spec test\nrule r: -0.5% -> round(1%) is -1%\n", "r"),
        "true"
    );
    assert_eq!(
        run("spec test\nrule r: 0.4% -> round(1%) is 0%\n", "r"),
        "true"
    );
    assert_eq!(
        run("spec test\nrule r: 0.6% -> round(1%) is 1%\n", "r"),
        "true"
    );
    assert_eq!(
        run("spec test\nrule r: 1.5% -> round(1%) is 2%\n", "r"),
        "true"
    );
}

#[test]
fn round_permille_is_half_away_from_zero() {
    assert_eq!(
        run("spec test\nrule r: 0.5%% -> round(1%%) is 1%%\n", "r"),
        "true"
    );
}

#[test]
fn round_basis_points_is_half_away_from_zero() {
    let code = r#"spec test
data spread: ratio
  -> unit basis_points: 10000
rule r: 0.5 basis_points -> round(1 basis_points) is 1 basis_points
"#;
    assert_eq!(run(code, "r"), "true");
}

#[test]
fn floor_ceil_abs_on_percent() {
    assert_eq!(
        run("spec test\nrule r: 0.5% -> floor(1%) is 0%\n", "r"),
        "true"
    );
    assert_eq!(
        run("spec test\nrule r: 0.5% -> ceil(1%) is 1%\n", "r"),
        "true"
    );
    assert_eq!(
        run("spec test\nrule r: -0.5% -> floor(1%) is -1%\n", "r"),
        "true"
    );
    assert_eq!(
        run("spec test\nrule r: -0.5% -> ceil(1%) is 0%\n", "r"),
        "true"
    );
    assert_eq!(run("spec test\nrule r: abs(-1.2%) is 1.2%\n", "r"), "true");
}
