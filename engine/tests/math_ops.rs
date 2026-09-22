use lemma::DateTimeValue;
use lemma::Engine;
use lemma::ValueKind;
use rust_decimal::Decimal;
use std::collections::HashMap;

fn run(code: &str, rule: &str) -> Result<String, lemma::Errors> {
    let mut engine = Engine::new();
    engine.load([(lemma::SourceType::Volatile, code.to_string())])?;
    let now = DateTimeValue::now();
    let resp = engine
        .run(
            None,
            "test",
            Some(&now),
            HashMap::new(),
            Some(&[rule.to_string()]),
            false,
        )
        .expect("run should succeed after load");
    let v = resp
        .results
        .values()
        .find(|r| r.rule.name == rule)
        .and_then(|r| r.result().map(str::to_string))
        .expect("rule value");
    Ok(v.to_string())
}

fn run_decimal(code: &str, rule: &str) -> Result<Decimal, lemma::Errors> {
    let lit = run_literal(code, rule)?;
    match &lit.value {
        ValueKind::Number(d) => Ok(lemma::ValueKind::Number(d.clone())
            .as_decimal_magnitude()
            .unwrap()),
        other => panic!("expected stored Number(Decimal), got {:?}", other),
    }
}

fn run_literal(code: &str, rule: &str) -> Result<lemma::LiteralValue, lemma::Errors> {
    let mut engine = Engine::new();
    engine.load([(lemma::SourceType::Volatile, code.to_string())])?;
    let now = DateTimeValue::now();
    let resp = engine
        .run(
            None,
            "test",
            Some(&now),
            HashMap::new(),
            Some(&[rule.to_string()]),
            false,
        )
        .expect("run should succeed after load");
    let rule_result = resp.get(rule).unwrap_or_else(|_| panic!("rule {rule}"));
    if rule_result.vetoed {
        panic!("rule {rule} vetoed");
    }
    Ok(rule_result.to_literal())
}

fn run_authoritative_decimal(code: &str, rule: &str) -> Decimal {
    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("load");
    let now = DateTimeValue::now();
    let resp = engine
        .run(
            None,
            "test",
            Some(&now),
            HashMap::new(),
            Some(&[rule.to_string()]),
            false,
        )
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

fn assert_close_decimal(actual: Decimal, expected: Decimal, tol: Decimal) {
    let diff = if actual > expected {
        actual - expected
    } else {
        expected - actual
    };
    assert!(
        diff <= tol,
        "expected ~{} (±{}), got {} (diff {})",
        expected,
        tol,
        actual,
        diff
    );
}

fn tol(decimal_places: u32) -> Decimal {
    Decimal::new(1, decimal_places)
}

#[test]
fn test_exp_and_power() -> Result<(), lemma::Errors> {
    let code = r#"
    spec test
    rule a: exp 1
    rule b: 2 ^ 3
    "#;
    let a = run_decimal(code, "a")?;
    let b = run_decimal(code, "b")?;
    assert_close_decimal(a, Decimal::new(2718281828459045, 15), tol(9));
    assert_eq!(b, Decimal::from(8));
    Ok(())
}

#[test]
fn test_sqrt_nine() -> Result<(), lemma::Errors> {
    let code = r#"
    spec test
    rule a: sqrt 9
    "#;
    assert_eq!(run_decimal(code, "a")?, Decimal::from(3));
    Ok(())
}

#[test]
fn test_sqrt_perfect_square_stores_decimal() -> Result<(), lemma::Errors> {
    let code = r#"
    spec test
    rule four: sqrt 4
    rule three_halves: sqrt (9 / 4)
    "#;
    assert_eq!(run_decimal(code, "four")?, Decimal::from(2));
    assert_eq!(run_decimal(code, "three_halves")?, Decimal::new(15, 1));
    Ok(())
}

#[test]
fn test_sqrt_cross_rule_product_exact_two() {
    let code = r#"
    spec test
    rule sqrt_two: sqrt 2
    rule sqrt_product: sqrt_two * sqrt_two
    "#;
    assert_eq!(
        run_authoritative_decimal(code, "sqrt_product"),
        Decimal::from(2),
        "sqrt(2)*sqrt(2) across rules must normalize to exact 2"
    );
}

#[test]
fn test_log_cross_rule_exp_exact_one() {
    let code = r#"
    spec test
    rule exp_one: exp 1
    rule log_one: log exp_one
    rule log_zero: log 1
    "#;
    assert_eq!(
        run_authoritative_decimal(code, "log_one"),
        Decimal::ONE,
        "log(exp(1)) across rules must normalize to exact 1"
    );
    assert_eq!(run_authoritative_decimal(code, "log_zero"), Decimal::ZERO);
}

#[test]
fn test_power_with_irrational_exponent_plans_symbolically() -> Result<(), lemma::Errors> {
    let approx = run_decimal(
        r#"
    spec test
    rule e: 2 ^ 0.5
    "#,
        "e",
    )?;
    assert_close_decimal(approx, Decimal::new(1414213562373095, 15), tol(12));
    Ok(())
}

#[test]
fn test_trig_at_zero() -> Result<(), lemma::Errors> {
    let code = r#"
    spec test
    rule s: sin 0
    rule c: cos 0
    rule t: tan 0
    rule ars: asin 0
    rule ac: acos 1
    rule at: atan 0
    "#;
    assert_eq!(run(code, "s")?, "0");
    assert_eq!(run(code, "c")?, "1");
    assert_eq!(run(code, "t")?, "0");
    assert_eq!(run(code, "ars")?, "0");
    assert_eq!(run(code, "ac")?, "0");
    assert_eq!(run(code, "at")?, "0");
    Ok(())
}

#[test]
fn test_nested_math_ops() -> Result<(), lemma::Errors> {
    let code = r#"
    spec test
    rule a: round (abs -3.6)
    rule b: ceil (sqrt 2)
    rule c: floor (exp 1)
    "#;
    assert_eq!(run(code, "a")?, "4");
    assert_eq!(run(code, "b")?, "2");
    assert_eq!(run(code, "c")?, "2");
    Ok(())
}
