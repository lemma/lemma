//! Aggregation and transform commands, precedence, empty/veto semantics, name clash.

use crate::lists::{
    assert_contains, data, evaluate, list, lit, load_ok, load_parse_err, load_planning_err, map,
    value, veto_reason,
};

fn fares_spec() -> &'static str {
    r#"
spec fares_card
data money: measure
  -> unit eur: 1.00
data fares: money list
  -> with 89 eur, 120 eur, 64 eur
"#
}

#[test]
fn value_list_commands() {
    let engine = load_ok(&format!(
        r#"
{fares}
rule total: fares -> sum
rule mean: fares -> avg
rule cheapest: fares -> min
rule dearest: fares -> max
rule n: fares -> count
rule over: fares -> any >= 100 eur
rule all_positive: fares -> all > 0 eur
rule high: fares -> where >= 100 eur
rule high_n: high -> count
rule ranked: fares -> sort desc
rule top_fare: ranked -> first
"#,
        fares = fares_spec()
    ));
    let response = evaluate(&engine, "fares_card", data(&[]));
    assert_eq!(value(&response, "total"), "273 eur");
    assert_eq!(value(&response, "mean"), "91 eur");
    assert_eq!(value(&response, "n"), "3");
    assert_eq!(value(&response, "over"), "true");
    assert_eq!(value(&response, "all_positive"), "true");
    assert_eq!(value(&response, "high_n"), "1");
    assert_eq!(value(&response, "top_fare"), "120 eur");
    assert_eq!(value(&response, "cheapest"), "64 eur");
    assert_eq!(value(&response, "dearest"), "120 eur");
}

#[test]
fn instance_list_commands() {
    let engine = load_ok(
        r#"
spec employee
data name: text
data salary: measure
  -> unit eur: 1.00
rule bonus:
  salary * 10%
spec payroll
uses staff: employee list
  -> with name: "Ada", salary: 9000 eur
  -> with name: "Grace", salary: 9500 eur
  -> with name: "Edsger", salary: 8000 eur
rule total_bonus:
  staff -> sum bonus
rule mean_salary:
  staff -> avg salary
rule n:
  staff -> count
rule any_rich:
  staff -> any salary > 9000 eur
rule all_paid:
  staff -> all salary > 0 eur
rule high:
  staff -> where salary >= 9000 eur
rule high_n:
  high -> count
rule top_bonus:
  (staff -> sort salary desc -> first).bonus
rule top_two:
  staff -> sort salary desc -> top 2
rule top_two_n:
  top_two -> count
"#,
    );
    let response = evaluate(&engine, "payroll", data(&[]));
    assert_eq!(value(&response, "total_bonus"), "2650 eur");
    assert_eq!(value(&response, "n"), "3");
    assert_eq!(value(&response, "any_rich"), "true");
    assert_eq!(value(&response, "all_paid"), "true");
    assert_eq!(value(&response, "high_n"), "2");
    assert_eq!(value(&response, "top_bonus"), "950 eur");
    assert_eq!(value(&response, "top_two_n"), "2");
}

#[test]
fn value_expression_on_value_list_is_error() {
    let err = load_planning_err(
        r#"
spec fares_card
data fares: number list
  -> with 1, 2
rule total:
  fares -> sum fares
"#,
    );
    assert_contains(&err, "sum");
}

#[test]
fn missing_expression_on_instance_list_is_error() {
    let err = load_planning_err(
        r#"
spec employee
data salary: number
spec payroll
uses staff: employee list
rule total:
  staff -> sum
"#,
    );
    assert_contains(&err, "sum");
}

#[test]
fn count_greater_than_zero_binds_at_keyword() {
    let engine = load_ok(
        r#"
spec items
data fares: number list
  -> with 1, 2
rule nonempty:
  fares -> count > 0
"#,
    );
    assert_eq!(
        value(&evaluate(&engine, "items", data(&[])), "nonempty"),
        "true"
    );
}

#[test]
fn sum_minus_is_binary_after_sum() {
    let engine = load_ok(&format!(
        r#"
{fares}
rule leftover:
  fares -> sum - 5 eur
"#,
        fares = fares_spec()
    ));
    assert_eq!(
        value(&evaluate(&engine, "fares_card", data(&[])), "leftover"),
        "268 eur"
    );
}

#[test]
fn sum_call_argument_ends_at_closing_paren() {
    let engine = load_ok(
        r#"
spec line
data qty: number
data price: measure
  -> unit eur: 1.00
spec order
uses lines: line list
  -> with qty: 2, price: 3 eur
  -> with qty: 1, price: 4 eur
rule total:
  lines -> sum(qty * price)
rule doubled:
  lines -> sum(qty * price) * 2
"#,
    );
    let response = evaluate(&engine, "order", data(&[]));
    assert_eq!(value(&response, "total"), "10 eur");
    assert_eq!(value(&response, "doubled"), "20 eur");
}

#[test]
fn sort_without_direction_is_asc() {
    let engine = load_ok(
        r#"
spec fares_card
data fares: number list
  -> with 3, 1, 2
rule ranked:
  fares -> sort
rule lowest:
  ranked -> first
rule highest:
  ranked -> last
"#,
    );
    let response = evaluate(&engine, "fares_card", data(&[]));
    assert_eq!(value(&response, "lowest"), "1");
    assert_eq!(value(&response, "highest"), "3");
}

#[test]
fn value_list_condition_is_not_and_in() {
    let engine = load_ok(
        r#"
spec codes
data codes: text list
  -> with "A1", "B2", "C3"
rule has_a:
  codes -> any is "A1"
rule no_z:
  codes -> all is not "Z9"
data scores: number list
  -> with 1, 5, 9
rule mid:
  scores -> where in 2...9
rule mid_n:
  mid -> count
"#,
    );
    let response = evaluate(&engine, "codes", data(&[]));
    assert_eq!(value(&response, "has_a"), "true");
    assert_eq!(value(&response, "no_z"), "true");
    assert_eq!(value(&response, "mid_n"), "1");
}

#[test]
fn value_list_and_or_not_in_condition_is_error() {
    for condition in [
        "where > 0 and < 10",
        "where(> 0 and < 10)",
        "where (> 0 and < 10)",
    ] {
        let err = load_parse_err(&format!(
            r#"
spec fares_card
data fares: number list
  -> with 1, 2
rule bad:
  fares -> {condition}
"#
        ));
        assert_contains(&err, "one comparison");
    }
}

#[test]
fn sum_avg_on_measure_converts_units() {
    let engine = load_ok(
        r#"
spec weights
data mass: measure
  -> unit kilogram: 1.00
  -> unit gram: 0.001
data samples: mass list
  -> with 1 kilogram, 500 gram
rule total:
  samples -> sum
"#,
    );
    assert_eq!(
        value(&evaluate(&engine, "weights", data(&[])), "total"),
        "1.5 kilogram"
    );
}

#[test]
fn product_on_measure_is_planning_error() {
    let err = load_planning_err(&format!(
        r#"
{fares}
rule times:
  fares -> product
"#,
        fares = fares_spec()
    ));
    assert_contains(&err, "product");
}

#[test]
fn min_max_sort_reject_text_boolean_range() {
    for ty in [
        "text list -> with \"a\", \"b\"",
        "boolean list -> with true, false",
    ] {
        let err = load_planning_err(&format!(
            r#"
spec sample
data values: {ty}
rule ranked:
  values -> sort asc
"#
        ));
        assert_contains(&err, "sort");
    }
    let err = load_planning_err(
        r#"
spec sample
data bands: number range list
  -> with 0...1, 1...2
rule smallest:
  bands -> min
"#,
    );
    assert_contains(&err, "min");
}

#[test]
fn non_boolean_condition_is_error() {
    let err = load_planning_err(
        r#"
spec employee
data salary: number
spec payroll
uses staff: employee list
rule bad:
  staff -> where salary
"#,
    );
    assert_contains(&err, "boolean");
}

#[test]
fn empty_list_identities_and_vetoes() {
    let engine = load_ok(
        r#"
spec sample
data money: measure
  -> unit eur: 1.00
data fares: money list
data rates: ratio list
rule total: fares -> sum
rule n: fares -> count
rule times: rates -> product
rule any_high: fares -> any > 0 eur
rule all_high: fares -> all > 0 eur
rule mean: fares -> avg
rule cheapest: fares -> min
rule dearest: fares -> max
"#,
    );
    let response = evaluate(&engine, "sample", data(&[]));
    assert_eq!(value(&response, "total"), "0 eur");
    assert_eq!(value(&response, "n"), "0");
    assert_eq!(value(&response, "any_high"), "false");
    assert_eq!(value(&response, "all_high"), "true");
    assert_contains(&veto_reason(&response, "mean"), "average");
    assert_contains(&veto_reason(&response, "times"), "product");
    assert_contains(&veto_reason(&response, "times"), "empty");
    assert_contains(&veto_reason(&response, "cheapest"), "empty");
    assert_contains(&veto_reason(&response, "dearest"), "empty");
}

#[test]
fn empty_product_guarded_by_count() {
    let engine = load_ok(
        r#"
spec growth
data factors: ratio list
rule compounded:
  factors -> product
  unless (factors -> count) is 0 then 100%
"#,
    );
    assert_eq!(
        value(&evaluate(&engine, "growth", data(&[])), "compounded"),
        "100%"
    );
    let response = evaluate(
        &engine,
        "growth",
        data(&[("factors", list(vec![lit("105%"), lit("110%")]))]),
    );
    assert_eq!(value(&response, "compounded"), "115.5%");
}

#[test]
fn reduction_vetoes_with_first_row_reason_no_short_circuit() {
    let engine = load_ok(
        r#"
spec employee
data name: text
data salary: measure
  -> unit eur: 1.00
rule bonus:
  salary * 10%
  unless name is "Ada" then veto "ada missing bonus"
  unless name is "Grace" then veto "grace missing bonus"
spec payroll
uses staff: employee list
  -> with name: "Ada", salary: 9000 eur
  -> with name: "Grace", salary: 9500 eur
rule total:
  staff -> sum bonus
rule any_paid:
  staff -> any bonus > 0 eur
"#,
    );
    let response = evaluate(&engine, "payroll", data(&[]));
    assert_contains(&veto_reason(&response, "total"), "ada missing bonus");
    assert_contains(&veto_reason(&response, "any_paid"), "ada missing bonus");
}

#[test]
fn where_no_match_then_sum_is_zero() {
    let engine = load_ok(&format!(
        r#"
{fares}
rule none:
  fares -> where >= 1000 eur
rule none_total:
  none -> sum
"#,
        fares = fares_spec()
    ));
    assert_eq!(
        value(&evaluate(&engine, "fares_card", data(&[])), "none_total"),
        "0 eur"
    );
}

#[test]
fn count_ignores_row_vetoes() {
    let engine = load_ok(
        r#"
spec employee
data name: text
data salary: measure
  -> unit eur: 1.00
rule bonus:
  veto "no bonus"
spec payroll
uses staff: employee list
  -> with name: "Ada", salary: 9000 eur
rule n:
  staff -> count
"#,
    );
    assert_eq!(value(&evaluate(&engine, "payroll", data(&[])), "n"), "1");
}

#[test]
fn top_literal_and_reference() {
    let engine = load_ok(
        r#"
spec fares_card
data money: measure
  -> unit eur: 1.00
data fares: money list
  -> with 89 eur, 120 eur, 64 eur
data take: number
  -> with 2
rule two:
  fares -> sort desc -> top 2
rule two_n:
  two -> count
rule by_ref:
  fares -> sort desc -> top take
rule by_ref_n:
  by_ref -> count
rule none:
  fares -> top 0
rule none_n:
  none -> count
rule more:
  fares -> top 10
rule more_n:
  more -> count
"#,
    );
    let response = evaluate(&engine, "fares_card", data(&[]));
    assert_eq!(value(&response, "two_n"), "2");
    assert_eq!(value(&response, "by_ref_n"), "2");
    assert_eq!(value(&response, "none_n"), "0");
    assert_eq!(value(&response, "more_n"), "3");
}

#[test]
fn top_expression_argument_is_planning_error() {
    let err = load_planning_err(
        r#"
spec fares_card
data fares: number list
  -> with 1
data take: number
rule bad:
  fares -> top(take + 1)
"#,
    );
    assert_contains(&err, "top");
}

#[test]
fn top_non_number_type_is_planning_error() {
    let err = load_planning_err(
        r#"
spec fares_card
data fares: number list
  -> with 1
data label: text
rule bad:
  fares -> top label
"#,
    );
    assert_contains(&err, "number");
}

#[test]
fn top_non_whole_literal_is_planning_error() {
    let err = load_planning_err(
        r#"
spec fares_card
data fares: number list
  -> with 1
rule bad:
  fares -> top 1.5
"#,
    );
    assert_contains(&err, "1.5");
}

#[test]
fn top_non_whole_reference_vetoes() {
    let engine = load_ok(
        r#"
spec fares_card
data fares: number list
  -> with 1, 2
data take: number
rule sliced:
  fares -> top take
"#,
    );
    let response = evaluate(&engine, "fares_card", data(&[("take", lit("1.5"))]));
    assert_contains(&veto_reason(&response, "sliced"), "1.5");
}

#[test]
fn sort_is_stable_and_last_sort_is_primary() {
    let engine = load_ok(
        r#"
spec employee
data name: text
data age: number
data salary: number
spec payroll
uses staff: employee list
  -> with name: "Ada", age: 40, salary: 9
  -> with name: "Grace", age: 30, salary: 9
  -> with name: "Edsger", age: 20, salary: 8
rule ordered:
  staff -> sort age asc -> sort salary desc
rule first_name:
  (ordered -> first).name
rule second_name:
  (ordered -> find 2).name
"#,
    );
    let response = evaluate(&engine, "payroll", data(&[]));
    assert_eq!(value(&response, "first_name"), "Grace");
    assert_eq!(value(&response, "second_name"), "Ada");
}

#[test]
fn sort_top_tie_keeps_earlier_row() {
    let engine = load_ok(
        r#"
spec employee
data name: text
data salary: number
spec payroll
uses staff: employee list
  -> with name: "Ada", salary: 9
  -> with name: "Grace", salary: 9
rule top_one:
  staff -> sort salary desc -> top 1
rule name:
  (top_one -> first).name
"#,
    );
    assert_eq!(
        value(&evaluate(&engine, "payroll", data(&[])), "name"),
        "Ada"
    );
}

#[test]
fn name_clash_requires_list_qualifier() {
    let err = load_planning_err(
        r#"
spec part
data money: measure
  -> unit eur: 1.00
uses components: part list
data quantity: number
data unit_cost: money
rule total:
  components -> sum(quantity * unit_cost)
"#,
    );
    assert_contains(&err, "components.quantity");
}

#[test]
fn qualified_row_side_and_bare_enclosing_name() {
    let engine = load_ok(
        r#"
spec line
data qty: number
data sku: text
spec order
data price_factor: number
  -> with 2
uses lines: line list
  -> with qty: 3, sku: "latte"
rule total:
  lines -> sum(qty * price_factor)
"#,
    );
    assert_eq!(value(&evaluate(&engine, "order", data(&[])), "total"), "6");
}

#[test]
fn list_name_inside_argument_is_current_row() {
    let engine = load_ok(
        r#"
spec employee
data bonus: number
spec payroll
uses staff: employee list
  -> with bonus: 10
  -> with bonus: 20
rule total:
  staff -> sum staff.bonus
"#,
    );
    assert_eq!(
        value(&evaluate(&engine, "payroll", data(&[])), "total"),
        "30"
    );
}

#[test]
fn command_on_vetoed_list_vetoes_with_that_reason() {
    let engine = load_ok(
        r#"
spec menu_item
data drink: text
  -> options "espresso", "latte"
data price: number
spec coffee_order
uses menu: menu_item list
  -> key drink
rule n:
  menu -> count
"#,
    );
    let response = evaluate(
        &engine,
        "coffee_order",
        data(&[(
            "menu",
            list(vec![
                map(&[("drink", lit("espresso")), ("price", lit("1"))]),
                map(&[("drink", lit("espresso")), ("price", lit("2"))]),
            ]),
        )]),
    );
    assert_contains(&veto_reason(&response, "n"), "espresso");
}

#[test]
fn command_results_are_deterministic() {
    let engine = load_ok(&format!(
        r#"
{fares}
rule ranked:
  fares -> sort desc -> first
"#,
        fares = fares_spec()
    ));
    let first = value(&evaluate(&engine, "fares_card", data(&[])), "ranked");
    let second = value(&evaluate(&engine, "fares_card", data(&[])), "ranked");
    assert_eq!(first, second);
    assert_eq!(first, "120 eur");
}
