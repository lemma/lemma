//! Explanation text uses the source formatter's spelling: `items -> sum cost`,
//! not a second printer's `items -> sum(cost)`.

use indexmap::IndexMap;
use lemma::{format_explanation, format_source, DateTimeValue, Engine, Explanation, SourceType};

const LINES: &str = r#"
spec line
data qty: number
data cost: number
rule subtotal: qty * cost
"#;

const RATES: &str = r#"
spec freight_rate
data weight: measure
  -> unit kilogram: 1.00
data volume: measure
  -> unit liter: 1.00
data band: weight range
data volume_band: volume range
data price: measure
  -> unit eur: 1.00
"#;

fn order_spec(body: &str) -> String {
    format!(
        r#"{LINES}
spec order
uses items: line list
  -> with qty: 2, cost: 3
  -> with qty: 1, cost: 4
data n: 1
data flag: false
data fares: number list
  -> with 89, 120, 64
rule out:
  {body}
"#
    )
}

fn freight_spec(body: &str) -> String {
    format!(
        r#"{RATES}
spec order
uses rates: freight_rate list
  -> key band, volume_band
  -> with band: 0 kilogram...5 kilogram, volume_band: 0 liter...2 liter, price: 6.95 eur
data parcel_weight: rates.weight
data parcel_volume: rates.volume
rule out:
  {body}
"#
    )
}

/// The formatted `rule out:` body line and its `unless` conditions.
fn formatted_out(code: &str) -> (String, Vec<String>) {
    let formatted = format_source(code, SourceType::Volatile).expect("spec must format");
    let mut lines = formatted
        .lines()
        .skip_while(|line| line.trim() != "rule out:")
        .skip(1);
    let body = lines
        .next()
        .unwrap_or_else(|| panic!("no body after `rule out:` in:\n{formatted}"))
        .trim()
        .to_string();
    let conditions = lines
        .take_while(|line| line.starts_with("  unless "))
        .map(|line| {
            let clause = line.trim().strip_prefix("unless ").expect("unless line");
            let (condition, _) = clause
                .split_once(" then ")
                .unwrap_or_else(|| panic!("single-line unless clause expected: {line}"));
            condition.trim().to_string()
        })
        .collect();
    (body, conditions)
}

fn explain_out(code: &str, data: IndexMap<String, lemma::EvaluateDataValue>) -> Explanation {
    let mut engine = Engine::new();
    engine
        .load([(SourceType::Volatile, code.to_string())])
        .unwrap_or_else(|errs| panic!("spec must load: {errs:?}\n{code}"));
    let now = DateTimeValue::now();
    let response = engine
        .evaluate(
            None,
            "order",
            Some(&now),
            data,
            Some(&["out".to_string()]),
            true,
        )
        .expect("evaluation must succeed");
    response
        .results
        .get("out")
        .expect("out in response")
        .explanation
        .clone()
        .expect("out explanation")
}

fn assert_body_matches_format(code: &str, data: IndexMap<String, lemma::EvaluateDataValue>) {
    let (body, _) = formatted_out(code);
    let explanation = explain_out(code, data);
    assert_eq!(
        explanation.body, body,
        "explanation body must equal formatted source"
    );
}

#[test]
fn reductions_match_format() {
    for body in [
        "items -> sum cost",
        "items -> sum(cost * 2)",
        "items -> sum(cost) * 2",
        "2 * items -> sum cost",
        "items -> avg cost",
        "items -> min cost",
        "items -> max cost",
        "items -> product cost",
        "fares -> sum",
    ] {
        assert_body_matches_format(&order_spec(body), IndexMap::new());
    }
}

#[test]
fn reduction_bare_form_is_source_form() {
    let explanation = explain_out(&order_spec("items -> sum cost"), IndexMap::new());
    assert_eq!(explanation.body, "items -> sum cost");
    let formatted = format_explanation(&explanation);
    assert!(
        !formatted.contains("sum(cost)"),
        "explanation tree must not print call form:\n{formatted}"
    );
}

#[test]
fn row_rule_arguments_match_format() {
    for body in [
        "items -> sum subtotal",
        "items -> max subtotal",
        "items -> where subtotal > 3 -> count",
        "items -> sort subtotal desc -> count",
        "items -> sum(subtotal) * 2",
    ] {
        assert_body_matches_format(&order_spec(body), IndexMap::new());
    }
}

#[test]
fn sort_matches_format() {
    for body in [
        "(items -> sort cost desc -> first).qty",
        "items -> sort(cost * 2) desc -> count",
        "fares -> sort desc -> first",
    ] {
        assert_body_matches_format(&order_spec(body), IndexMap::new());
    }
}

#[test]
fn predicates_match_format() {
    for body in [
        "items -> any cost > 3",
        "items -> all cost > 0",
        "items -> where cost > 3 -> count",
        "items -> any(cost > 3 and qty > 1)",
        "items -> where(cost > 3 and qty > 1) -> count",
        "fares -> where >= 100 -> count",
        "fares -> any(>= n + 1)",
    ] {
        assert_body_matches_format(&order_spec(body), IndexMap::new());
    }
}

#[test]
fn top_and_find_match_format() {
    for body in [
        "items -> top 1 -> count",
        "items -> top n -> count",
        "fares -> find 2",
        "fares -> find(n + 1)",
    ] {
        assert_body_matches_format(&order_spec(body), IndexMap::new());
    }
}

#[test]
fn find_with_keys_matches_format() {
    let mut data = IndexMap::new();
    data.insert(
        "parcel_weight".to_string(),
        lemma::EvaluateDataValue::Literal("1 kilogram".to_string()),
    );
    data.insert(
        "parcel_volume".to_string(),
        lemma::EvaluateDataValue::Literal("1 liter".to_string()),
    );
    assert_body_matches_format(
        &freight_spec("(rates -> find parcel_weight, parcel_volume).price"),
        data,
    );
}

#[test]
fn math_prefix_matches_format() {
    for body in [
        "sqrt(n) * 2",
        "(sqrt n) * (sqrt n)",
        "2 * sqrt n",
        "sqrt(n + 1)",
    ] {
        assert_body_matches_format(&order_spec(body), IndexMap::new());
    }
}

#[test]
fn binary_operands_use_call_form_on_both_sides() {
    for (body, expected) in [
        ("(sqrt n) * (sqrt n)", "sqrt(n) * sqrt(n)"),
        ("2 * sqrt n", "2 * sqrt(n)"),
        ("2 * items -> sum cost", "2 * items -> sum(cost)"),
        ("items -> sum cost", "items -> sum cost"),
        ("sqrt n", "sqrt n"),
        ("n > 0 and not flag", "n > 0 and not flag"),
        ("(not flag) and n > 0", "not(flag) and n > 0"),
    ] {
        let code = order_spec(body);
        let (formatted, _) = formatted_out(&code);
        assert_eq!(formatted, expected, "format of `{body}`");
        assert_eq!(explain_out(&code, IndexMap::new()).body, expected);
    }
}

#[test]
fn unless_condition_matches_format() {
    let code = order_spec("0\n  unless items -> sum(cost) > 5 then 1");
    let (_, conditions) = formatted_out(&code);
    let [condition] = conditions.as_slice() else {
        panic!("one unless condition expected, got {conditions:?}");
    };
    let explanation = explain_out(&code, IndexMap::new());
    assert!(
        explanation
            .causes
            .iter()
            .any(|cause| &cause.condition == condition),
        "cause condition must equal formatted `{condition}`, got {:?}",
        explanation
            .causes
            .iter()
            .map(|cause| &cause.condition)
            .collect::<Vec<_>>()
    );
}
