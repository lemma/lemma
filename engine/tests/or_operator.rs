use indexmap::IndexMap;
use lemma::{format_source, DateTimeValue, Engine, EvaluateDataValue, RuleResult, SourceType};
use serde_json::Value;

const SMALL_STACK: usize = 256 * 1024;

fn load(source: &str) -> Engine {
    let mut engine = Engine::new();
    engine
        .load([(SourceType::Volatile, source.to_string())])
        .expect("spec must load");
    engine
}

fn load_error(source: &str) -> String {
    let mut engine = Engine::new();
    engine
        .load([(SourceType::Volatile, source.to_string())])
        .expect_err("spec must fail to load")
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

fn evaluate(engine: &Engine, spec: &str, inputs: &[(&str, EvaluateDataValue)]) -> lemma::Response {
    evaluate_explained(engine, spec, inputs, false)
}

fn evaluate_explained(
    engine: &Engine,
    spec: &str,
    inputs: &[(&str, EvaluateDataValue)],
    explain: bool,
) -> lemma::Response {
    let data = inputs
        .iter()
        .map(|(name, value)| ((*name).to_string(), value.clone()))
        .collect::<IndexMap<_, _>>();
    engine
        .evaluate(None, spec, Some(&DateTimeValue::now()), data, None, explain)
        .expect("evaluation must succeed")
}

fn rule<'a>(response: &'a lemma::Response, name: &str) -> &'a RuleResult {
    response
        .results
        .get(name)
        .unwrap_or_else(|| panic!("rule '{name}' must be present"))
}

fn result<'a>(response: &'a lemma::Response, name: &str) -> &'a str {
    let rule = rule(response, name);
    assert!(!rule.vetoed, "{name} unexpectedly vetoed: {rule:?}");
    rule.result()
        .unwrap_or_else(|| panic!("rule '{name}' must have a result"))
}

#[test]
fn global_or_truth_veto_and_precedence_semantics() {
    let engine = load(
        r#"
spec logical_or
rule blocked: veto "blocked"
rule left_blocked: veto "left blocked"
rule right_blocked: veto "right blocked"
rule true_false: true or false
rule false_false: false or false
rule true_veto: true or blocked
rule false_veto: false or blocked
rule veto_true: blocked or true
rule veto_false: blocked or false
rule first_veto: left_blocked or right_blocked
rule and_binds_tighter: false or true and false
rule left_and_before_or: true and false or true
rule grouped_or: (false or true) and false
"#,
    );
    let response = evaluate(&engine, "logical_or", &[]);

    assert_eq!(result(&response, "true_false"), "true");
    assert_eq!(result(&response, "false_false"), "false");
    assert_eq!(result(&response, "true_veto"), "true");
    assert_eq!(result(&response, "veto_true"), "true");
    assert_eq!(result(&response, "and_binds_tighter"), "false");
    assert_eq!(result(&response, "left_and_before_or"), "true");
    assert_eq!(result(&response, "grouped_or"), "false");

    for name in ["false_veto", "veto_false"] {
        assert_eq!(
            rule(&response, name).veto_reason.as_deref(),
            Some("blocked"),
            "{name}"
        );
    }
    assert_eq!(
        rule(&response, "first_veto").veto_reason.as_deref(),
        Some("left blocked")
    );
}

#[test]
fn global_or_absorbs_or_propagates_missing_data_in_both_orders() {
    let engine = load(
        r#"
spec logical_or_missing
data missing_flag: boolean
data expensive: boolean
rule true_expensive: true or expensive
rule false_expensive: false or expensive
rule missing_true: missing_flag or true
rule missing_false: missing_flag or false
"#,
    );
    let response = evaluate(&engine, "logical_or_missing", &[]);

    assert_eq!(result(&response, "true_expensive"), "true");
    assert!(
        rule(&response, "true_expensive").missing_data().is_empty(),
        "absorbed operand must not remain live"
    );
    assert_eq!(result(&response, "missing_true"), "true");
    assert!(
        rule(&response, "missing_true").missing_data().is_empty(),
        "later true must absorb earlier missing data"
    );

    assert!(rule(&response, "false_expensive").is_missing_data());
    assert_eq!(
        rule(&response, "false_expensive").missing_data(),
        &["expensive"]
    );
    assert!(rule(&response, "missing_false").is_missing_data());
    assert_eq!(
        rule(&response, "missing_false").missing_data(),
        &["missing_flag"]
    );
}

#[test]
fn value_list_compound_predicates_use_global_or_and_and_precedence() {
    let engine = load(
        r#"
spec value_predicates
data fares: number list
  -> with 3, 4, 9, 12
data qualifying: number list
  -> with 3, 9, 12
rule selected:
  fares -> where(> 5 or is 3) -> sum
rule selected_then_chained:
  fares -> where(> 5 or is 3) -> where < 10 -> sum
rule nested:
  fares -> where((> 5 and < 10) or is 3) -> sum
rule all_qualifying:
  qualifying -> all(> 5 or is 3)
rule not_all_qualifying:
  fares -> all(> 5 or is 3)
rule any_qualifying:
  fares -> any(> 10 or is 3)
"#,
    );
    let response = evaluate(&engine, "value_predicates", &[]);

    assert_eq!(result(&response, "selected"), "24");
    assert_eq!(result(&response, "selected_then_chained"), "12");
    assert_eq!(result(&response, "nested"), "12");
    assert_eq!(result(&response, "all_qualifying"), "true");
    assert_eq!(result(&response, "not_all_qualifying"), "false");
    assert_eq!(result(&response, "any_qualifying"), "true");
}

#[test]
fn value_predicate_true_disjunct_absorbs_missing_sibling_for_one_row() {
    let engine = load(
        r#"
spec value_predicate_missing
data threshold: number
data fares: number list
  -> with 3
rule selected:
  fares -> where(> threshold or is 3) -> count
"#,
    );
    let response = evaluate(&engine, "value_predicate_missing", &[]);
    assert_eq!(result(&response, "selected"), "1");
    assert!(rule(&response, "selected").missing_data().is_empty());
}

#[test]
fn any_compound_predicate_visits_a_later_vetoed_row() {
    let engine = load(
        r#"
spec applicant
data name: text
data adult: boolean
rule eligible: adult
  unless name is "Blocked" then veto "blocked applicant"

spec applications
uses applicant
data applicants: applicant list
  -> with name: "Ready", adult: true
  -> with name: "Blocked", adult: false
rule accepted:
  applicants -> any(eligible or adult)
"#,
    );
    let response = evaluate(&engine, "applications", &[]);
    assert_eq!(
        rule(&response, "accepted").veto_reason.as_deref(),
        Some("blocked applicant")
    );
}

#[test]
fn instance_predicate_or_evaluates_each_row_once() {
    let engine = load(
        r#"
spec person
data name: text
data adult: boolean
data guardian: boolean

spec household
uses person
data people: person list
  -> with name: "Ada", adult: true, guardian: false
  -> with name: "Grace", adult: false, guardian: true
  -> with name: "Edsger", adult: false, guardian: false
rule admitted:
  people -> where(adult or guardian)
rule admitted_count:
  admitted -> count
"#,
    );
    let response = evaluate(&engine, "household", &[]);
    assert_eq!(result(&response, "admitted_count"), "2");
}

#[test]
fn compound_predicate_rejects_bare_and_comma_forms() {
    for (predicate, expected) in [
        ("where > 5 or is 3", "where(> 5 or is 3)"),
        ("where(> 5, is 3)", "found ','"),
        ("where(amount > 5, amount is 3)", "takes one argument"),
        ("where()", "expected an expression"),
    ] {
        let error = load_error(&format!(
            r#"
spec row
data amount: number
spec predicates
uses row
data fares: number list
  -> with 3, 9
data rows: row list
  -> with amount: 3
rule bad:
  {} -> {predicate}
"#,
            if predicate.contains("amount") {
                "rows"
            } else {
                "fares"
            }
        ));
        assert!(
            error.to_ascii_lowercase().contains(expected),
            "{predicate}: expected {expected:?} in error, got: {error}"
        );
    }
}

#[test]
fn value_list_rejects_instance_predicate_form() {
    for predicate in ["where(amount > 5)", "where(amount > 5 or amount is 3)"] {
        let error = load_error(&format!(
            r#"
spec predicates
data amount: number
data fares: number list
  -> with 3, 9
rule bad:
  fares -> {predicate}
"#
        ));
        assert!(
            error.contains("not a field expression"),
            "{predicate}: expected value-list form error, got: {error}"
        );
    }
}

#[test]
fn instance_list_rejects_value_predicate_form() {
    for predicate in ["where(> 5)", "where(> 5 or is 3)"] {
        let error = load_error(&format!(
            r#"
spec employee
data salary: number
spec payroll
uses employee
data staff: employee list
  -> with salary: 1
rule bad:
  staff -> {predicate}
"#
        ));
        assert!(
            error.contains("not a bare comparison"),
            "{predicate}: expected instance-list form error, got: {error}"
        );
    }
}

#[test]
fn nested_command_owns_its_commas_inside_instance_predicate() {
    let engine = load(
        r#"
spec rate
data weight: number
data volume: number
data price: number

spec parcel
data amount: number

spec shipping
uses rate
uses parcel
data rates: rate list
  -> key weight, volume
  -> with weight: 1, volume: 2, price: 5
data parcel_weight: number
  -> with 1
data parcel_volume: number
  -> with 2
data parcels: parcel list
  -> with amount: 3
rule kept:
  parcels -> where((rates -> find parcel_weight, parcel_volume).price > 0 or amount is 3)
rule kept_count:
  kept -> count
"#,
    );
    let response = evaluate(&engine, "shipping", &[]);
    assert_eq!(result(&response, "kept_count"), "1");
}

#[test]
fn formatter_preserves_logical_precedence_and_predicate_call_form() {
    let source = r#"
spec row
data amount: number
data adult: boolean
data guardian: boolean

spec formatting
uses row
data a: boolean
data b: boolean
data c: boolean
data rows: row list
rule grouped_or: a or b and c
rule forced_parens: (a or b) and c
rule not_call: not (a) or b
rule not_right: a or not b
rule selected: rows -> where((amount > 5 and amount < 10) or amount is 3)
"#;
    let formatted = format_source(source, SourceType::Volatile).expect("format");
    assert!(formatted.contains("rule grouped_or:"), "{formatted}");
    assert!(formatted.contains("a or b and c"), "{formatted}");
    assert!(formatted.contains("rule forced_parens:"), "{formatted}");
    assert!(formatted.contains("(a or b) and c"), "{formatted}");
    assert!(formatted.contains("not(a) or b"), "{formatted}");
    assert!(formatted.contains("a or not b"), "{formatted}");
    assert!(
        formatted.contains("rows -> where(amount > 5 and amount < 10 or amount is 3)"),
        "{formatted}"
    );
    assert_eq!(
        format_source(&formatted, SourceType::Volatile).expect("reformat"),
        formatted
    );
}

#[test]
fn or_chain_of_many_terms_plans_and_evaluates_on_a_small_stack() {
    std::thread::Builder::new()
        .stack_size(SMALL_STACK)
        .spawn(|| {
            let body = std::iter::repeat_n("false", 20_000)
                .collect::<Vec<_>>()
                .join(" or ");
            let engine = load(&format!("spec chain\nrule r: {body}\n"));
            let response = evaluate(&engine, "chain", &[]);
            assert_eq!(result(&response, "r"), "false");
        })
        .expect("spawn small-stack thread")
        .join()
        .expect("test body must not panic");
}

fn find_or_node(value: &Value) -> Option<&Value> {
    match value {
        Value::Object(object) => {
            if object.get("type").and_then(Value::as_str) == Some("or") {
                return Some(value);
            }
            object.values().find_map(find_or_node)
        }
        Value::Array(values) => values.iter().find_map(find_or_node),
        _ => None,
    }
}

#[test]
fn true_or_explanation_peels_to_the_deciding_disjunct() {
    let engine = load(
        r#"
spec explain_or
data a: boolean
data b: boolean
rule r: a or b
"#,
    );
    let response = evaluate_explained(
        &engine,
        "explain_or",
        &[("a", EvaluateDataValue::from(true))],
        true,
    );
    assert_eq!(result(&response, "r"), "true");
    let formatted = lemma::format_explanation(
        rule(&response, "r")
            .explanation
            .as_ref()
            .expect("explained"),
    );
    assert!(
        formatted.contains("a: true"),
        "true or must name the deciding disjunct, got {formatted}"
    );
    assert!(
        !formatted.contains("b:"),
        "true or must not require the unread sibling, got {formatted}"
    );
}

#[test]
fn false_or_explanation_lists_operands() {
    let engine = load(
        r#"
spec explain_or_false
data a: boolean
data b: boolean
rule r: a or b
"#,
    );
    let response = evaluate_explained(
        &engine,
        "explain_or_false",
        &[
            ("a", EvaluateDataValue::from(false)),
            ("b", EvaluateDataValue::from(false)),
        ],
        true,
    );
    assert_eq!(result(&response, "r"), "false");
    let formatted = lemma::format_explanation(
        rule(&response, "r")
            .explanation
            .as_ref()
            .expect("explained"),
    );
    assert!(
        formatted.contains("a") && formatted.contains("b"),
        "false or must list both operands, got {formatted}"
    );
}

#[test]
fn prefix_not_then_or_is_a_parse_error() {
    let error = load_error(
        r#"
spec prefix
data a: boolean
data b: boolean
rule r: not a or b
"#,
    );
    assert!(
        error.contains("found 'or'"),
        "expected prefix error naming 'or', got: {error}"
    );
}

#[test]
fn date_relative_suffix_then_or_parses() {
    let engine = load(
        r#"
spec dates
uses lemma units
data x: boolean
rule r: now in past 3 day or x
"#,
    );
    let response = evaluate(&engine, "dates", &[("x", EvaluateDataValue::from(true))]);
    assert_eq!(result(&response, "r"), "true");
}

#[test]
fn unless_condition_accepts_or() {
    let engine = load(
        r#"
spec grant
data a: boolean
data b: boolean
rule r: 0
  unless a or b then 1
"#,
    );
    let response = evaluate(
        &engine,
        "grant",
        &[
            ("a", EvaluateDataValue::from(false)),
            ("b", EvaluateDataValue::from(true)),
        ],
    );
    assert_eq!(result(&response, "r"), "1");
}

#[test]
fn unless_equality_or_still_evaluates_without_dispatch_peel() {
    let engine = load(
        r#"
spec grant
data x: number
data y: number
rule r: 0
  unless x is 1 or y is 2 then 1
"#,
    );
    let response = evaluate(
        &engine,
        "grant",
        &[
            ("x", EvaluateDataValue::from("0")),
            ("y", EvaluateDataValue::from("2")),
        ],
    );
    assert_eq!(result(&response, "r"), "1");
}

#[test]
fn or_is_reserved_in_any_case() {
    for name in ["or", "OR", "Or"] {
        let error = load_error(&format!(
            r#"
spec reserved
data {name}: boolean
"#
        ));
        assert!(
            error.to_ascii_lowercase().contains("or"),
            "{name}: expected reserved-keyword error, got: {error}"
        );
    }
}

#[test]
fn unless_or_false_does_not_drop_the_other_operand() {
    let engine = load(
        r#"
spec unless_or
data flag: boolean
rule r: 0
  unless flag or false then 1
"#,
    );
    let missing = evaluate(&engine, "unless_or", &[]);
    assert!(rule(&missing, "r").is_missing_data());
    assert_eq!(rule(&missing, "r").missing_data(), &["flag"]);
}

#[test]
fn unless_and_false_drops_the_arm() {
    let engine = load(
        r#"
spec unless_and
data flag: boolean
rule r: 0
  unless flag and false then 1
"#,
    );
    let missing = evaluate(&engine, "unless_and", &[]);
    assert_eq!(result(&missing, "r"), "0");
}

#[test]
fn show_wire_uses_or_with_nested_and_operands() {
    let engine = load(
        r#"
spec show_or
data a: boolean
data b: boolean
data c: boolean
rule decision: a or b and c
"#,
    );
    let show = engine
        .show(None, "show_or", Some(&DateTimeValue::now()))
        .expect("show");
    let json = serde_json::to_value(lemma::api::show_document(&engine, &show))
        .expect("serialize Show document");
    let or_node = find_or_node(&json).expect("Show must contain an or node");
    let operands = or_node["operands"].as_array().expect("or operands");
    assert_eq!(operands.len(), 2);
    assert_eq!(operands[1]["type"], "and");
}
