//! A row written in a data list is a copy of the instance it names, a row appended
//! inside a rule takes that instance's values.

use crate::lists::{
    assert_contains, data, evaluate, evaluate_err, list, lit, load_ok, map, missing_data, value,
};

const STAFF: &str = r#"
spec employee
data name: text
data salary: number
rule double: salary * 2

spec b
uses employee
uses ada: employee -> with name: "Ada"
data staff: employee list -> with ada
rule pay: staff -> sum salary
rule ada_pay: ada.salary
"#;

#[test]
fn spec_row_salary_is_data_of_the_row() {
    let engine = load_ok(STAFF);
    let response = evaluate(&engine, "b", data(&[]));
    assert_eq!(missing_data(&response, "pay"), vec!["staff.1.salary"]);
    assert_eq!(missing_data(&response, "ada_pay"), vec!["ada.salary"]);
}

#[test]
fn spec_row_salary_does_not_fill_the_named_instance() {
    let engine = load_ok(STAFF);
    let response = evaluate(&engine, "b", data(&[("staff.1.salary", lit("100"))]));
    assert_eq!(value(&response, "pay"), "100");
    assert_eq!(missing_data(&response, "ada_pay"), vec!["ada.salary"]);
}

#[test]
fn named_instance_salary_does_not_fill_the_spec_row() {
    let engine = load_ok(STAFF);
    let response = evaluate(&engine, "b", data(&[("ada.salary", lit("100"))]));
    assert_eq!(value(&response, "ada_pay"), "100");
    assert_eq!(missing_data(&response, "pay"), vec!["staff.1.salary"]);
}

#[test]
fn spec_row_copied_binding_cannot_be_set() {
    let engine = load_ok(STAFF);
    let error = evaluate_err(&engine, "b", data(&[("staff.1.name", lit("\"Linus\""))]));
    assert_contains(
        &error.to_string(),
        "data 'staff.1.name' is sealed by `uses -> with` and cannot be supplied at evaluate",
    );
}

#[test]
fn whole_list_and_row_key_in_one_request_is_an_error() {
    let engine = load_ok(STAFF);
    let error = evaluate_err(
        &engine,
        "b",
        data(&[
            ("staff", list(vec![map(&[])])),
            ("staff.1.salary", lit("100")),
        ]),
    );
    assert_contains(
        &error.to_string(),
        "data 'staff': cannot supply both the whole list and dotted row keys such as 'staff.1.salary'",
    );
}

const ITEMS: &str = r#"
spec employee
data name: text
data salary: number

spec c
uses employee
uses kim: employee -> with name: "Kim"
data items: employee list
rule more_items: items -> with kim
rule total: more_items -> sum salary
"#;

#[test]
fn appended_instance_row_reads_that_instance_data() {
    let engine = load_ok(ITEMS);
    let response = evaluate(&engine, "c", data(&[]));
    assert_eq!(missing_data(&response, "total"), vec!["kim.salary"]);
    let response = evaluate(&engine, "c", data(&[("kim.salary", lit("100"))]));
    assert_eq!(value(&response, "total"), "100");
}

#[test]
fn appended_instance_binding_cannot_be_set() {
    let engine = load_ok(ITEMS);
    let error = evaluate_err(&engine, "c", data(&[("kim.name", lit("\"Linus\""))]));
    assert_contains(
        &error.to_string(),
        "data 'kim.name' is sealed by `uses -> with` and cannot be supplied at evaluate",
    );
}
