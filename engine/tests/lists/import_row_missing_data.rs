//! Missing row fields read by a rule on a named `uses` template are reported in consumer keys.

use crate::lists::{data, evaluate, list, load_ok, map, missing_data};

#[test]
fn template_row_missing_field_under_uses_is_alias_qualified() {
    let engine = load_ok(
        r#"
spec employee
data name: text
spec payroll
uses employee
data staff: employee list
rule first_name: (staff -> first).name
spec report
uses p: payroll
rule names: p.first_name
"#,
    );
    let response = evaluate(
        &engine,
        "report",
        data(&[("p.staff", list(vec![map(&[])]))]),
    );
    assert_eq!(missing_data(&response, "names"), vec!["p.staff.1.name"]);
}

#[test]
fn spec_written_row_missing_field_is_named_in_each_reading_plan() {
    let engine = load_ok(
        r#"
spec employee
data name: text
data salary: number
spec payroll
uses employee
data staff: employee list
  -> with name: "ada"
rule pay: (staff -> first).salary
spec report
uses p: payroll
rule pay: p.pay
"#,
    );
    let own = evaluate(&engine, "payroll", data(&[]));
    assert_eq!(missing_data(&own, "pay"), vec!["staff.1.salary"]);
    let consumer = evaluate(&engine, "report", data(&[]));
    assert_eq!(missing_data(&consumer, "pay"), vec!["p.staff.1.salary"]);
}

#[test]
fn deferred_binding_on_unreached_uses_asks_the_writer_input() {
    let engine = load_ok(
        r#"
spec inner
data x: number
rule total: x

spec outer
data base: number
data mode: text
rule rate: base
uses i: inner
  -> with x: rate
rule result: 0
  unless mode is "use" then i.total
"#,
    );
    let response = evaluate(&engine, "outer", data(&[]));
    let missing = missing_data(&response, "result");
    assert!(missing.contains(&"mode".to_string()), "mode: {missing:?}");
    assert!(
        missing.contains(&"base".to_string()),
        "deferred target rate needs base: {missing:?}"
    );
    assert!(
        !missing.contains(&"i.x".to_string()),
        "sealed deferred field must not be asked: {missing:?}"
    );
}

#[test]
fn template_row_missing_field_two_hops_under_uses_is_consumer_qualified() {
    let engine = load_ok(
        r#"
spec employee
data name: text
spec mid
uses employee
data staff: employee list
rule first_name: (staff -> first).name
spec payroll
uses q: mid
rule first_name: q.first_name
spec report
uses p: payroll
rule names: p.first_name
"#,
    );
    let response = evaluate(
        &engine,
        "report",
        data(&[("p.q.staff", list(vec![map(&[])]))]),
    );
    assert_eq!(missing_data(&response, "names"), vec!["p.q.staff.1.name"]);
}
