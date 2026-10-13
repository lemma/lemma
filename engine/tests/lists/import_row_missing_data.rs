//! Missing row fields read by a rule on a named `uses` template are reported in consumer keys.

use crate::lists::{data, evaluate, list, load_ok, map, missing_data};

#[test]
fn template_row_missing_field_under_uses_is_alias_qualified() {
    let engine = load_ok(
        r#"
spec employee
data name: text
spec payroll
uses staff: employee list
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
fn template_row_missing_field_two_hops_under_uses_is_consumer_qualified() {
    let engine = load_ok(
        r#"
spec employee
data name: text
spec mid
uses staff: employee list
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
