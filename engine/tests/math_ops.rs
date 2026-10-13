use indexmap::IndexMap;
use lemma::DateTimeValue;
use lemma::Engine;
use lemma::ValueKind;
use rust_decimal::Decimal;

fn run(code: &str, rule: &str) -> Result<String, lemma::Errors> {
    let mut engine = Engine::new();
    engine.load([(lemma::SourceType::Volatile, code.to_string())])?;
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
        .expect("evaluate should succeed after load");
    let v = resp
        .results
        .values()
        .find(|r| r.rule.name == rule)
        .and_then(|r| r.result().map(str::to_string))
        .expect("rule value");
    Ok(v.to_string())
}

fn evaluate_decimal(code: &str, rule: &str) -> Result<Decimal, lemma::Errors> {
    let lit = evaluate_literal(code, rule)?;
    match &lit.value {
        ValueKind::Number(d) => Ok(lemma::ValueKind::Number(d.clone())
            .as_decimal_magnitude()
            .unwrap()),
        other => panic!("expected stored Number(Decimal), got {:?}", other),
    }
}

fn evaluate_literal(code: &str, rule: &str) -> Result<lemma::LiteralValue, lemma::Errors> {
    let mut engine = Engine::new();
    engine.load([(lemma::SourceType::Volatile, code.to_string())])?;
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
        .expect("evaluate should succeed after load");
    let rule_result = resp.get(rule).unwrap_or_else(|_| panic!("rule {rule}"));
    if rule_result.vetoed {
        panic!("rule {rule} vetoed");
    }
    Ok(rule_result.to_literal())
}

fn evaluate_authoritative_decimal(code: &str, rule: &str) -> Decimal {
    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
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
    let a = evaluate_decimal(code, "a")?;
    let b = evaluate_decimal(code, "b")?;
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
    assert_eq!(evaluate_decimal(code, "a")?, Decimal::from(3));
    Ok(())
}

#[test]
fn test_sqrt_perfect_square_stores_decimal() -> Result<(), lemma::Errors> {
    let code = r#"
    spec test
    rule four: sqrt 4
    rule three_halves: sqrt (9 / 4)
    "#;
    assert_eq!(evaluate_decimal(code, "four")?, Decimal::from(2));
    assert_eq!(evaluate_decimal(code, "three_halves")?, Decimal::new(15, 1));
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
        evaluate_authoritative_decimal(code, "sqrt_product"),
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
        evaluate_authoritative_decimal(code, "log_one"),
        Decimal::ONE,
        "log(exp(1)) across rules must normalize to exact 1"
    );
    assert_eq!(
        evaluate_authoritative_decimal(code, "log_zero"),
        Decimal::ZERO
    );
}

#[test]
fn test_power_with_irrational_exponent_plans_symbolically() -> Result<(), lemma::Errors> {
    let approx = evaluate_decimal(
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
fn test_tan_half_matches_reference() -> Result<(), lemma::Errors> {
    let code = r#"
    spec test
    rule t: tan 0.5
    "#;
    let actual = evaluate_decimal(code, "t")?;
    let expected: Decimal = "0.5463024898437905132551794658"
        .parse()
        .expect("BUG: reference literal must parse");
    assert_close_decimal(actual, expected, tol(26));
    Ok(())
}

#[test]
fn test_inverse_trig_at_unit_points() -> Result<(), lemma::Errors> {
    let code = r#"
    spec test
    rule at1: atan 1
    rule atm1: atan(-1)
    rule as_root: asin 0.7071067811865475244008443621
    rule ac_root: acos 0.7071067811865475244008443621
    "#;
    let quarter_pi: Decimal = "0.7853981633974483096156608458"
        .parse()
        .expect("BUG: reference literal must parse");
    assert_close_decimal(evaluate_decimal(code, "at1")?, quarter_pi, tol(26));
    assert_close_decimal(evaluate_decimal(code, "atm1")?, -quarter_pi, tol(26));
    assert_close_decimal(evaluate_decimal(code, "as_root")?, quarter_pi, tol(26));
    assert_close_decimal(evaluate_decimal(code, "ac_root")?, quarter_pi, tol(26));
    Ok(())
}

#[test]
fn test_nested_math_ops() -> Result<(), lemma::Errors> {
    let code = r#"
    spec test
    rule a: round abs(-3.6)
    rule b: ceil sqrt 2
    rule c: floor (exp 1)
    "#;
    assert_eq!(run(code, "a")?, "4");
    assert_eq!(run(code, "b")?, "2");
    assert_eq!(run(code, "c")?, "2");
    Ok(())
}

#[test]
fn test_prefix_call_form_applies_operators_to_result() -> Result<(), lemma::Errors> {
    let code = r#"
    spec test
    data x: 1.2
    data express: boolean -> with false
    rule tight: floor(x) + 0.5
    rule wide: floor (x + 0.5)
    rule scaled: sqrt(16) * 2
    rule bare: floor x
    rule negated: not express
    rule negated_comparison: not(x > 3)
    "#;
    assert_eq!(run(code, "tight")?, "1.5");
    assert_eq!(run(code, "wide")?, "1");
    assert_eq!(run(code, "scaled")?, "8");
    assert_eq!(run(code, "bare")?, "1");
    assert_eq!(run(code, "negated")?, "true");
    assert_eq!(run(code, "negated_comparison")?, "true");
    Ok(())
}

#[test]
fn test_operator_after_bare_prefix_operand_is_parse_error() {
    for (body, suggestion) in [
        ("floor x + 1", "`floor(a) + b`"),
        ("sqrt 16 * 2", "`sqrt(a) * b`"),
        ("not x > 3", "`not(a > b)`"),
        ("not express and express", "`not(a) and b`"),
    ] {
        let code = format!("spec test\ndata x: 1.2\ndata express: boolean\nrule r: {body}");
        let mut engine = Engine::new();
        let errors = engine
            .load([(lemma::SourceType::Volatile, code)])
            .expect_err("bare prefix operand followed by an operator must not parse");
        let message = errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(message.contains(suggestion), "{body}: {message}");
    }
}
