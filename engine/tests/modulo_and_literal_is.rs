use indexmap::IndexMap;
use lemma::{format_source, DateTimeValue, Engine, SourceType};

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

fn load_error(code: &str) -> String {
    Engine::new()
        .load([(SourceType::Volatile, code.to_string())])
        .expect_err("load must fail")
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn mod_is_floored_remainder_result_takes_divisor_sign() {
    assert_eq!(run("spec test\nrule r: 7 mod 3\n", "r"), "1");
    assert_eq!(run("spec test\nrule r: 7 mod -3\n", "r"), "-2");
    assert_eq!(run("spec test\nrule r: -7 mod 3\n", "r"), "2");
    assert_eq!(run("spec test\nrule r: -7 mod -3\n", "r"), "-1");
}

#[test]
fn mod_on_exact_rationals_is_floored() {
    assert_eq!(
        run("spec test\nrule r: (-7 / 2) mod 1 is 0.5\n", "r"),
        "true"
    );
}

#[test]
fn number_literal_before_percent_is_percent_so_spaced_percent_modulo_is_rejected() {
    assert!(load_error("spec test\nrule r: 7 % 3\n").contains("mod"));
    assert!(load_error("spec test\nrule r: 50 % -10\n").contains("ratio"));
}

#[test]
fn percent_after_non_literal_operand_is_rejected_with_mod_hint() {
    let message = load_error("spec test\ndata value: number\nrule r: value % 7\n");
    assert!(message.contains("'mod'"), "{message}");
}

#[test]
fn parenthesized_left_operand_still_needs_mod() {
    assert_eq!(run("spec test\nrule r: (-7) mod 3\n", "r"), "2");
    assert!(load_error("spec test\nrule r: (7) % 3\n").contains("'mod'"));
}

#[test]
fn literal_is_and_is_veto_parse_as_comparisons() {
    assert_eq!(run("spec test\nrule r: 1 is 1\n", "r"), "true");
    assert_eq!(run("spec test\nrule r: 2 is veto\n", "r"), "false");
}

#[test]
fn attached_percent_stays_percent_suffix() {
    assert_eq!(
        run("spec test\ndata x: 10%\nrule r: 100% - x\n", "r"),
        "90%"
    );
}

#[test]
fn dangling_is_after_number_is_a_parse_error() {
    let code = "spec test\nrule r: 5 is\n";
    assert!(lemma::Engine::new()
        .load([(SourceType::Volatile, code.to_string())])
        .is_err());
}

#[test]
fn is_past_future_permille_mod_rejected_as_unit_names() {
    for name in ["is", "past", "future", "permille", "mod"] {
        let code = format!("spec test\ndata m: measure\n  -> unit {name}: 1\n");
        let result = Engine::new().load([(SourceType::Volatile, code)]);
        assert!(result.is_err(), "unit {name} must be rejected");
    }
}

#[test]
fn formatter_writes_mod_and_literal_is_loadable() {
    for (rule_expr, expected) in [
        ("(7) mod 3", "7 mod 3"),
        ("(1) is 1", "1 is 1"),
        ("(2) is veto", "2 is veto"),
    ] {
        let source = format!("spec test\nrule x: {rule_expr}\n");
        let formatted = format_source(&source, SourceType::Volatile).expect("format");
        assert!(
            formatted.contains(expected),
            "expected `{expected}` in formatted output:\n{formatted}"
        );
        Engine::new()
            .load([(SourceType::Volatile, formatted)])
            .expect("formatted output must load");
    }
}
