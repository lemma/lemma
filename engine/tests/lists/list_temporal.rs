//! Pinned vs unpinned row spec bodies; source rows follow the consumer.

use crate::lists::{
    assert_contains, data, date, evaluate_at, list, lit, load_files, load_files_planning_err, map,
    value,
};
use indexmap::IndexMap;

#[test]
fn pinned_row_spec_body_applies_to_source_and_evaluate_data_rows() {
    let engine = load_files(&[
        (
            "employee.lemma",
            r#"
spec employee 2024-01-01
data name: text
data salary: number
rule bonus:
  salary * 10%

spec employee 2025-01-01
data name: text
data salary: number
rule bonus:
  salary * 20%
"#,
        ),
        (
            "payroll.lemma",
            r#"
spec payroll
uses staff: employee 2024 list
  -> with name: "Ada", salary: 100
rule source_bonus:
  (staff -> first).bonus
rule evaluate_bonus:
  (staff -> last).bonus
"#,
        ),
    ]);
    let response = evaluate_at(
        &engine,
        "payroll",
        &date(2026, 1, 1),
        data(&[(
            "staff",
            list(vec![
                map(&[("name", lit("Ada")), ("salary", lit("100"))]),
                map(&[("name", lit("Grace")), ("salary", lit("100"))]),
            ]),
        )]),
        None,
        false,
    );
    assert_eq!(value(&response, "source_bonus"), "10");
    assert_eq!(value(&response, "evaluate_bonus"), "10");
}

#[test]
fn unpinned_row_spec_follows_consumer_slice() {
    let engine = load_files(&[
        (
            "employee.lemma",
            r#"
spec employee 2024-01-01
data name: text
data salary: number
rule bonus:
  salary * 10%

spec employee 2025-01-01
data name: text
data salary: number
rule bonus:
  salary * 20%
"#,
        ),
        (
            "payroll.lemma",
            r#"
spec payroll 2024-01-01
uses staff: employee list
  -> with name: "Ada", salary: 100
rule source_bonus:
  (staff -> first).bonus
"#,
        ),
    ]);
    let response = evaluate_at(
        &engine,
        "payroll",
        &date(2025, 6, 1),
        IndexMap::new(),
        None,
        false,
    );
    assert_eq!(value(&response, "source_bonus"), "20");
}

#[test]
fn source_rows_change_through_consumer_temporal_rows() {
    let engine = load_files(&[(
        "payroll.lemma",
        r#"
spec employee
data name: text

spec payroll 2024-01-01
uses staff: employee list
  -> with name: "Ada"
rule first_name:
  (staff -> first).name

spec payroll 2025-01-01
uses staff: employee list
  -> with name: "Grace"
rule first_name:
  (staff -> first).name
"#,
    )]);
    let in_2024 = evaluate_at(
        &engine,
        "payroll",
        &date(2024, 6, 1),
        IndexMap::new(),
        None,
        false,
    );
    let in_2025 = evaluate_at(
        &engine,
        "payroll",
        &date(2025, 6, 1),
        IndexMap::new(),
        None,
        false,
    );
    assert_eq!(value(&in_2024, "first_name"), "Ada");
    assert_eq!(value(&in_2025, "first_name"), "Grace");
}

#[test]
fn unpinned_consumer_rejects_row_spec_interface_change() {
    let err = load_files_planning_err(&[
        (
            "employee.lemma",
            r#"
spec employee 2024-01-01
data name: text
data salary: number

spec employee 2025-01-01
data name: text
data pay: number
"#,
        ),
        (
            "payroll.lemma",
            r#"
spec payroll
uses staff: employee list
  -> with name: "Ada", salary: 1
"#,
        ),
    ]);
    assert_contains(&err, "salary");
}
